// ── fjord-app · discover/search.rs ───────────────────────────────────────────
//   spawn_discover_search      debounced (300ms) + generation-guarded search dispatch (page 1);
//                              text-only cards pushed immediately, posters patched in
//                              as they arrive (bounded concurrency, TMDB CDN, own disk cache);
//                              records page/total_pages in FjordState for spawn_discover_search_more
//   spawn_discover_search_more  fetches+appends the next results page — triggered by
//                              handle_key's Down-at-last-row via the discover-load-more
//                              callback (Seerr/TMDB search commonly has far more pages than
//                              the single page v1 ever fetched, capping results well below
//                              what Seerr's own web UI shows for the same query); no-ops
//                              quietly with no next page / a fetch already in flight
//   fetch_and_patch_posters    bounded-concurrency TMDB poster fetch + in-place model patch,
//                              shared by both search functions above (idx is pre-offset by
//                              the caller for the append case)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Search ───────────────────────────────────────────────────────────────────

/// Rough target row count for "the grid looks full without scrolling" —
/// deliberately a fixed estimate, not a pixel-exact viewport-height
/// computation (that would need a new geometry property pushed from
/// `MainWindow::sync_layout()`, mirroring `dash-cw`/`dash-ch`/`library-cols`,
/// for comparatively little payoff over a conservative constant). Real UX
/// gap, live-reported: "search should fill the screen so you don't need to
/// go to the end of a row to get new items" — a single TMDB search page
/// (~20 raw results, fewer once `person` is filtered out) often doesn't
/// fill even a modest window, so the user hit the Down-triggered load-more
/// on almost every search before this existed.
const DISCOVER_AUTOFILL_ROWS: i32 = 6;

/// Called from both search commit closures (page 1 and each appended page)
/// on the UI thread, right after `discover-results` is set. If the grid
/// still has fewer rows than `DISCOVER_AUTOFILL_ROWS` would need, reuses the
/// exact same `discover-load-more` callback the Down-at-last-row keyboard
/// path already fires — `spawn_discover_search_more`'s own guards (no next
/// page / already loading) make this safe to call unconditionally here;
/// each successful page's own commit re-checks and chains again, so this
/// naturally stops once the grid is full or the search runs out of pages.
pub(crate) fn maybe_autofill_grid(g: &AppState) {
    let cols = g.get_library_cols().max(1);
    let target = cols * DISCOVER_AUTOFILL_ROWS;
    if (g.get_discover_results().row_count() as i32) < target {
        g.invoke_discover_load_more();
    }
}

/// Bounded-concurrency TMDB poster fetch + in-place patch into
/// `discover-results`, shared by `spawn_discover_search` (page 1, `idx`
/// is the row's own position) and `spawn_discover_search_more` (page N,
/// `idx` is already offset by the row count at the time of the fetch —
/// see that function's own comment for why that offset has to be captured
/// synchronously before the fetch starts rather than recomputed here).
pub(crate) async fn fetch_and_patch_posters(
    ww: Weak<MainWindow>,
    generation: Arc<AtomicU64>,
    my_gen: u64,
    poster_jobs: Vec<(usize, String, String, String)>,
) {
    if poster_jobs.is_empty() {
        return;
    }
    let Ok(http) = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
    else {
        return;
    };
    let sem = Arc::new(tokio::sync::Semaphore::new(8));
    let mut set = tokio::task::JoinSet::new();
    for (idx, item_type, tmdb_id, poster_path) in poster_jobs {
        let http = http.clone();
        let sem = Arc::clone(&sem);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let cache_key = format!(
                "{}-{}",
                if item_type == "DiscoverMovie" {
                    "movie"
                } else {
                    "tv"
                },
                tmdb_id
            );
            let bytes = fetch_tmdb_image(&http, TMDB_POSTER_BASE, &poster_path, &cache_key).await?;
            let buf = decode_poster_buffer(&bytes)?;
            Some((idx, item_type, tmdb_id, buf))
        });
    }
    // Live-reported 2026-08-21 ("all the posters flash every time it loads
    // one item") — investigated as a commit-frequency problem first (a
    // short-lived batching window landed here, then a wider one was
    // attempted) before the user's own follow-ups ("why do we ned to flash
    // every poster when we trickle in data?", "its not good if the user
    // need to wait log for a big search") made the real shape of the ask
    // clear: keep the steady per-item trickle — don't delay or batch
    // commits at all, the first-found result should show the instant it's
    // ready — and instead stop animating each arrival at all. The actual
    // "flash" was never commit frequency; `set_row_data(idx, ...)` is
    // already a genuine single-row patch, confirmed by re-reading it, not
    // a model rebuild that could explain unrelated cards re-animating. It
    // was `MediaCard`'s own poster `FadeInTrigger` (widgets.slint) firing
    // on every has-poster transition — correct, deliberate motion in
    // isolation, but with ~20 cards each independently popping through
    // that same fade at a slightly different moment as their own fetch
    // completes, the accumulated effect across the whole grid reads as
    // continuous flashing rather than a calm progressive fill. Removed the
    // fade there instead (see widgets.slint) — a poster now simply appears
    // the instant its own row is patched, with no motion to draw the eye.
    // That's what makes committing per-arrival, with no batching window,
    // safe again: nothing here is trying to reduce how often a card
    // "flashes" any more, so there is no longer a size/timing dial to get
    // right — every completed fetch just lands as soon as it's done.
    while let Some(res) = set.join_next().await {
        let Ok(Some((idx, item_type, tmdb_id, buf))) = res else {
            continue;
        };
        if generation.load(Ordering::SeqCst) != my_gen {
            break; // a newer search superseded this one
        }
        let ww2 = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_discover_results();
            let Some(mut card) = model.row_data(idx) else {
                return;
            };
            // Defensive: confirm the row at this index is still the same
            // item before patching, matching the id-match guard used
            // elsewhere in this codebase for in-place model patches.
            if card.id.as_str() != tmdb_id || card.item_type.as_str() != item_type {
                return;
            }
            card.poster = slint::Image::from_rgba8(buf);
            card.has_poster = true;
            model.set_row_data(idx, card);
        });
    }
}

pub(crate) fn spawn_discover_search(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    query: String,
    generation: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = generation.fetch_add(1, Ordering::SeqCst) + 1;

    if query.trim().is_empty() {
        {
            let mut s = state.lock().unwrap();
            s.discover_search_page = 0;
            s.discover_search_total_pages = 0;
            s.discover_search_loading_more = false;
        }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_discover_results(ModelRc::new(VecModel::from(Vec::<CardItem>::new())));
                g.set_discover_searching(false);
            }
        });
        return;
    }

    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        // Silent no-op here used to look identical to "found nothing" from
        // the user's side — no error, no spinner, no log line. Real bug,
        // found live: a search typed while (for whatever reason)
        // `seerr_client` was `None` produced literally no feedback at all.
        warn!("seerr: search dispatched with no seerr_client set — not connected?");
        show_toast(
            ww,
            "Not connected to Seerr — check Settings → Integrations".into(),
        );
        return;
    };
    let is_session_auth = client.is_session_auth();

    // Real bug, live-reported with a video (2026-08-21) — the "No results
    // for X" empty-state text (discover.slint) is correctly gated on
    // `!discover-searching`, but this used to only flip searching=true
    // AFTER the 300ms debounce sleep below finished — leaving the whole
    // debounce window itself (every keystroke, not just the first) with
    // searching=false and discover-results still holding whatever the
    // PREVIOUS query left behind (empty, for the very first search from
    // the landing rows). The video showed exactly this: the full Trending/
    // Popular grid disappearing straight to a blank "No results" screen
    // the instant a character was typed, well before any search had
    // actually run. Fixed by setting it synchronously, right here, before
    // the debounce delay even starts — a stale (superseded) task's own
    // early-return below never touches this flag, so it stays true for the
    // whole gap and only the WINNING (non-superseded) task's own commit
    // closure or error branch ever clears it back to false.
    let ww_searching = ww.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww_searching.upgrade() {
            AppState::get(&w).set_discover_searching(true);
        }
    });

    rt.spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if generation.load(Ordering::SeqCst) != my_gen {
            return; // superseded by a newer keystroke before the debounce elapsed
        }

        debug!("seerr: searching for {query:?}");
        let response = match client.search(&query, 1).await {
            Ok(r) => r,
            Err(e) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        AppState::get(&w).set_discover_searching(false);
                    }
                });
                handle_seerr_error(&state, &ww, is_session_auth, "Seerr search failed", &e);
                return;
            }
        };
        if generation.load(Ordering::SeqCst) != my_gen {
            return; // a newer search already superseded this response
        }

        // Search commonly has far more than one page's worth of results
        // (a common word can run into the hundreds) — page 1 alone is what
        // used to cap Fjord's result count well below what Seerr's own web
        // UI shows for the same query, real bug, live-reported. This state
        // is what `spawn_discover_search_more` (below) reads to fetch
        // subsequent pages, triggered as the user's keyboard nav reaches
        // the last row of the grid.
        let results = response.results;
        // Real bug, live-reported 2026-08-12 ("Gran Hermano... have the
        // Mentalis's poster image" / "Law & order missing poster..."): metas
        // and poster_jobs used to be built from two INDEPENDENTLY filtered
        // views of `results` — `search_result_to_meta` also drops
        // blocklisted items, not just non-movie/tv ones, while the old
        // zip's own filter only checked media_type — so a blocklisted item
        // anywhere in the results silently shifted every poster-job pairing
        // after it by one position (same root cause as the identical bug in
        // ensure_discover_landing, see that function's own fix comment).
        // Fixed the same way: metas and poster_jobs are built together in
        // one single-pass filter, so they can't drift apart. patch_known_
        // request_state/patch_watchlist_state still run afterward, under
        // the lock — they mutate metas in place without changing its
        // length or order, so doing that second doesn't reopen the bug.
        let mut metas: Vec<DiscoverCardMeta> = Vec::with_capacity(results.len());
        let mut poster_jobs: Vec<(usize, String, String, String)> = Vec::new();
        for r in &results {
            let Some(m) = search_result_to_meta(r) else {
                continue;
            };
            if let Some(p) = r.poster_path.clone() {
                poster_jobs.push((metas.len(), m.item_type.to_string(), m.id.clone(), p));
            }
            metas.push(m);
        }
        debug!(
            "seerr: search {query:?} page 1/{} -> {} raw result(s), {} movie/tv card(s)",
            response.total_pages,
            results.len(),
            metas.len()
        );
        {
            let mut s = state.lock().unwrap();
            s.discover_search_page = 1;
            s.discover_search_total_pages = response.total_pages;
            s.discover_search_loading_more = false;
            // Real bug fixed 2026-07-18 — see FjordState.discover_known_requests'
            // own doc comment: search results never carried real request
            // state at all, so an already-requested item's context menu
            // offered "Request" instead of "Edit/Cancel/View Request".
            for m in &mut metas {
                patch_known_request_state(m, &s.discover_known_requests);
                patch_watchlist_state(m, &s.discover_watchlist_ids);
            }
            // Full raw fetch history for this query — apply_search_filters'
            // only source of genre_ids/vote_average, which never make it
            // onto CardItem (never displayed). Overwritten (not extended)
            // here since this is page 1 of a fresh query.
            s.discover_search_metas = metas.clone();
        }

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_commit.upgrade() {
                let g = AppState::get(&w);
                // A fresh query always replaces the whole result set (a
                // different query has no reason to keep the same ids in the
                // same order, so apply_cards_preserving_identity's own
                // same-shape check would never fire here) — but rapid
                // keystrokes commonly land on overlapping results ("the
                // bour" -> "the bourn"), and blanking + re-fetching every
                // poster on each one is exactly the flash the user reported
                // while typing. Carry forward already-decoded posters by
                // (id, item_type) across the swap, same pattern
                // apply_search_filters already uses for its own re-filter.
                let old = g.get_discover_results();
                let old_posters: std::collections::HashMap<(String, String), (slint::Image, bool)> =
                    (0..old.row_count())
                        .filter_map(|i| old.row_data(i))
                        .map(|c| {
                            (
                                (c.id.to_string(), c.item_type.to_string()),
                                (c.poster.clone(), c.has_poster),
                            )
                        })
                        .collect();
                let cards: Vec<CardItem> = metas
                    .into_iter()
                    .map(|m| {
                        let key = (m.id.clone(), m.item_type.to_string());
                        let mut card = m.into_card_item();
                        if let Some((poster, has_poster)) = old_posters.get(&key) {
                            card.poster = poster.clone();
                            card.has_poster = *has_poster;
                        }
                        card
                    })
                    .collect();
                // Real bug, live-reported 2026-08-17 ("still flash every
                // item"): carrying posters forward (above) fixed the poster
                // BLANKING, but this still built a brand-new ModelRc every
                // commit — the exact Phase 96 class of bug (a fresh model
                // instance makes Slint destroy/recreate every delegate
                // element regardless of whether the underlying data
                // changed, re-triggering each card's own FadeInTrigger
                // fade-in). The doc comment this replaced argued a fresh
                // query "has no reason to keep the same ids in the same
                // order," which is true in general but not for the actual
                // reported case — overlapping keystrokes ("the bour" ->
                // "the bourn") very often DO return the same top results in
                // the same relative order (TMDB's own popularity sort is
                // stable across a narrowing query), so the same-shape check
                // routinely succeeds and was simply never being attempted.
                g.set_discover_results(crate::apply_cards_preserving_identity(&old, cards));
                g.set_discover_searching(false);
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
                maybe_autofill_grid(&g);
            }
        });

        fetch_and_patch_posters(ww.clone(), Arc::clone(&generation), my_gen, poster_jobs).await;
        // Re-narrow to whatever filters are already set — a no-op when
        // none are (search_filters_active's own early return), so this is
        // safe to call unconditionally after every commit. Must run AFTER
        // the poster patch above, not before — see apply_search_filters'
        // own doc comment for why.
        if generation.load(Ordering::SeqCst) == my_gen {
            apply_search_filters(&state, &ww);
        }
    });
}

/// Fetches the next page of the *current* search and appends it to
/// `discover-results` (Seerr/TMDB search commonly has far more pages than
/// Fjord originally ever fetched — see `spawn_discover_search`'s own
/// comment). Triggered by `discover::handle_key`'s Down-at-last-row branch
/// via the `discover-load-more` AppState callback: `keys.rs::handle_key`
/// doesn't hold `state`/`rt` in the per-mode match arms, so the fetch has
/// to be dispatched from wherever this callback is registered instead (see
/// `wire_discover`) — the same reason several other keyboard-triggered
/// async actions in this codebase (e.g. context menu's queue mutations) go
/// through an `AppState` callback rather than being threaded through
/// `keys.rs` directly. No-ops quietly (not an error, no toast) when
/// there's no next page, a fetch is already in flight, or no search has
/// landed yet — every one of those is an entirely normal state to be in on
/// any given Down press, not a failure.
pub(crate) fn spawn_discover_search_more(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    query: String,
    generation: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = generation.load(Ordering::SeqCst);
    let (client, next_page) = {
        let mut s = state.lock().unwrap();
        if s.discover_search_loading_more {
            return;
        }
        if s.discover_search_page == 0 || s.discover_search_page >= s.discover_search_total_pages {
            return;
        }
        let Some(client) = s.seerr_client.clone() else {
            return;
        };
        s.discover_search_loading_more = true;
        (client, s.discover_search_page + 1)
    };
    let is_session_auth = client.is_session_auth();

    // Append offset: the row count *right now*, read synchronously on the
    // calling (UI event loop) thread — `discover-results` can only be
    // touched from there. Safe against a race with a fresh search landing
    // first: that path bumps `generation` synchronously before its own debounce
    // sleep even starts, so this fetch's `generation` check below (after the
    // network round trip) will already see the mismatch and bail before
    // ever using this offset.
    let offset = ww
        .upgrade()
        .map(|w| AppState::get(&w).get_discover_results().row_count())
        .unwrap_or(0);

    let state2 = Arc::clone(&state);
    rt.spawn(async move {
        debug!("seerr: loading more results for {query:?}, page {next_page}");
        let response = match client.search(&query, next_page).await {
            Ok(r) => r,
            Err(e) => {
                state2.lock().unwrap().discover_search_loading_more = false;
                handle_seerr_error(&state2, &ww, is_session_auth, "Seerr search failed", &e);
                return;
            }
        };
        if generation.load(Ordering::SeqCst) != my_gen {
            state2.lock().unwrap().discover_search_loading_more = false;
            return; // a newer search superseded this one before it landed
        }
        let results = response.results;
        // Same bug + fix as spawn_discover_search — see that function's own
        // fix comment. metas and poster_jobs built together in one pass so
        // they can't drift apart on a blocklisted item.
        let mut metas: Vec<DiscoverCardMeta> = Vec::with_capacity(results.len());
        let mut poster_jobs: Vec<(usize, String, String, String)> = Vec::new();
        for r in &results {
            let Some(m) = search_result_to_meta(r) else {
                continue;
            };
            if let Some(p) = r.poster_path.clone() {
                poster_jobs.push((
                    offset + metas.len(),
                    m.item_type.to_string(),
                    m.id.clone(),
                    p,
                ));
            }
            metas.push(m);
        }
        debug!(
            "seerr: search {query:?} page {next_page}/{} -> {} raw result(s), {} card(s)",
            response.total_pages,
            results.len(),
            metas.len()
        );
        {
            let mut s = state2.lock().unwrap();
            s.discover_search_page = next_page;
            s.discover_search_total_pages = response.total_pages;
            s.discover_search_loading_more = false;
            // See spawn_discover_search's identical patch — real bug fixed
            // 2026-07-18.
            for m in &mut metas {
                patch_known_request_state(m, &s.discover_known_requests);
                patch_watchlist_state(m, &s.discover_watchlist_ids);
            }
            s.discover_search_metas.extend(metas.clone());
        }

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_commit.upgrade() {
                let g = AppState::get(&w);
                let existing = g.get_discover_results();
                let new_cards: Vec<CardItem> = metas
                    .into_iter()
                    .map(DiscoverCardMeta::into_card_item)
                    .collect();
                // True incremental append (2026-08-02, real bug live-reported
                // as "the grid flash several times" while searching): a page
                // 2/3/4 auto-load only ever ADDS rows to what's already on
                // screen, but swapping in a brand-new ModelRc — even one
                // built from the exact same existing rows plus the new ones
                // — makes Slint destroy and reconstruct every already-shown
                // card element (this file's own established "Phase 96 flash
                // bug"), discarding their already-decoded poster Images and
                // re-running each one's poster FadeInTrigger for no reason.
                // discover-results is always constructed as a VecModel
                // elsewhere in this file, so downcasting back to it and
                // calling extend() (one row_added notification for the
                // whole batch) appends onto the SAME live model instance —
                // existing rows are never touched. Falls back to a full
                // rebuild only if that assumption somehow doesn't hold.
                if let Some(vm) = existing.as_any().downcast_ref::<VecModel<CardItem>>() {
                    vm.extend(new_cards);
                } else {
                    let mut all: Vec<CardItem> = (0..existing.row_count())
                        .filter_map(|i| existing.row_data(i))
                        .collect();
                    all.extend(new_cards);
                    g.set_discover_results(ModelRc::new(VecModel::from(all)));
                }
                maybe_autofill_grid(&g);
            }
        });

        fetch_and_patch_posters(ww.clone(), Arc::clone(&generation), my_gen, poster_jobs).await;
        if generation.load(Ordering::SeqCst) == my_gen {
            apply_search_filters(&state2, &ww);
        }
    });
}
