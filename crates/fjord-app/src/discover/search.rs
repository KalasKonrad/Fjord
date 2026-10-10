// ── fjord-app · discover/search.rs ──────────────────────────────────────────
//   DISCOVER_AUTOFILL_ROWS / maybe_autofill_grid  load more pages until the grid looks full
//   spawn_discover_search      debounced (300 ms), generation-guarded search (page 1): text cards at
//                              once (posters carried over from the previous query by id), posters
//                              patched as they arrive; records page/total_pages
//   spawn_discover_search_more next page, appended onto the live VecModel — via discover-load-more
//                              (Down at the grid's last row); no-op without a next page / while busy
//   fetch_and_patch_posters    bounded-concurrency TMDB poster fetch + per-row patch, shared by both
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Search ───────────────────────────────────────────────────────────────────

/// Rough row count for "the grid looks full" — a fixed estimate, not a viewport
/// computation (that needs a new geometry property from `MainWindow::sync_layout()`).
/// One TMDB page (~20 results, fewer without people) often doesn't fill the window,
/// so without autofill nearly every search needed a load-more.
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
    // Commit each poster the moment it arrives (no batching): the "flashing" while posters
    // trickle in came from MediaCard's per-poster fade (removed in widgets.slint), not from
    // the commits — `set_row_data(idx, …)` is a single-row patch.
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
        // A search with no client looked exactly like "no results" — log it.
        warn!("seerr: search dispatched with no seerr_client set — not connected?");
        show_toast(
            ww,
            "Not connected to Seerr — check Settings → Integrations".into(),
        );
        return;
    };
    let is_session_auth = client.is_session_auth();

    // Set searching=true NOW, before the 300 ms debounce: otherwise the "No results for
    // X" empty state (gated on !discover-searching) showed during the debounce, replacing
    // the landing rows on the first keystroke. A superseded task never touches the flag;
    // only the winning task's commit or error clears it.
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

        // Paging state for `spawn_discover_search_more` (later pages load as keyboard nav
        // reaches the grid's last row) — page 1 alone capped results well below Seerr's own UI.
        let results = response.results;
        // metas and poster_jobs are built in ONE filter pass (search_result_to_meta also
        // drops blocklisted items; two separate filters shifted every later poster by one —
        // same as ensure_discover_landing). The request/watchlist patches below change
        // nothing in length or order.
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
            // Request state from the known-requests cache, so an already-requested result
            // offers Edit/Cancel/View Request.
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
                // Carry already-decoded posters forward by (id, item_type): consecutive keystrokes
                // ("the bour" → "the bourn") mostly return overlapping results, and re-fetching their
                // posters each time made the grid flash while typing (like apply_search_filters).
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
                // apply_cards_preserving_identity, not a fresh ModelRc: overlapping keystrokes often
                // return the same top results in the same order (TMDB's popularity sort is stable),
                // and a new model re-creates every card (each poster re-fades).
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
                // Append page 2+ onto the SAME VecModel (extend: one row_added for the batch) — a new
                // ModelRc would re-create every card already on screen. Full rebuild only as a fallback.
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
