// ── fjord-app · discover/watchlist.rs ────────────────────────────────────────
//   discover_toggle_watchlist   POST/DELETE /watchlist, updates discover_watchlist_ids,
//                              patches every model the card might be visible in
//                              (patch_watchlist_on_all_models) + request-detail-on-watchlist
//                              if that item's detail page is open, toasts, calls refresh_watchlist
//                              (which rebuilds the calendar too); debug!-logged at entry/success
//                              (2026-07-19, live report of "no confirmation" with no evidence in
//                              the log of the call ever happening — added to get direct proof of
//                              where it breaks on the next attempt instead of guessing again);
//                              the context-menu callsite also warn!s if context-menu-item-id
//                              fails to parse as a tmdb id (its one silent-early-return path);
//                              its success handler also resolves this one tmdb_id -> Jellyfin id
//                              via find_local_item and patches the in-library star in place
//                              (context_menu::patch_watchlist_on_jellyfin_models, 2026-07-20)
//   ensure_discover_watchlist/refresh_watchlist/fetch_and_store_watchlist  fetch-once-per-
//                              session (paginated, 200-item safety cap) + refresh-after-toggle
//                              pair mirroring ensure_discover_landing/refresh_requested_row;
//                              both funnel through the shared fetch_and_store_watchlist, which
//                              also triggers build_calendar_entries on every fetch, and — since
//                              2026-07-20 — independently spawns populate_watchlist_rows (Discover/
//                              dashboard Watchlist rows) and resync_jellyfin_watchlist_stars
//                              (in-library star bulk resync) on every fetch too, so all four
//                              consumers share the one already-fetched discover_watchlist_ids set
//   ── Watchlist row (2026-07-20, user request — "add a row for the watchlist as in
//      seerr... culd also add it to the home dashbord, and movies dashbord... and
//      series dashbord... status indicator to the posters like we do for everything
//      else") — a genuine "everything on the watchlist" row, distinct from Coming
//      Up's date-filtered subset; not deduped against Coming Up or any other row ──
//   watchlist_movie_to_meta/watchlist_tv_to_meta  DiscoverCardMeta builders for a plain
//                              watchlist item — movie_details_to_meta/tv_details_to_meta
//                              are NOT reusable (require a real &RequestEntry); availability
//                              comes straight from d.media_info (search_result_to_meta's own
//                              single-tier approach), on_watchlist: true set directly; callers
//                              call patch_known_request_state afterward for Edit/Cancel rows
//   WATCHLIST_ROW_CAP            20, matching fetch_requested_row's/build_calendar_entries's
//                              own cap — not a fresh judgment call, reusing the number this
//                              exact cost tradeoff was already reasoned about for
//   populate_watchlist_rows/push_watchlist_rows/fetch_watchlist_posters  detail-fetch (bounded
//                              Semaphore+JoinSet) up to WATCHLIST_ROW_CAP watchlist items, split
//                              client-side by item_type into mixed/movies/tv (mirrors home.rs's
//                              own Continue-Watching cw_movies/cw_tv split) feeding BOTH the
//                              Discover Watchlist row and the Home/Movies/TV dashboard rows —
//                              one fetch, four consumers. Two-phase threading (build
//                              DiscoverCardMeta off-thread, touch AppState/CardItem only inside
//                              invoke_from_event_loop) is mandatory here — see push_coming_up_row's
//                              own doc comment for the real bug this discipline exists to prevent.
//                              Posters patched afterward by id+item_type match across all 3
//                              models (not by index — the same tmdb id sits at a different row
//                              index in the mixed list vs. its own type-specific list).
//                              push_watchlist_rows routes all 3 models through
//                              apply_cards_preserving_identity (2026-07-22, code review finding)
//                              rather than a bare ModelRc::new swap — this function runs on every
//                              watchlist refresh, i.e. after every single toggle anywhere in the
//                              app, and a bare swap would re-fade every OTHER already-visible card
//                              (Phase 96's documented class of bug) just because one item changed
//   resync_jellyfin_watchlist_stars  in-library watchlist star (2026-07-20, user request — "if
//                              its in library it shuld also show there") — resolves each
//                              currently-watchlisted tmdb id (state.discover_watchlist_ids, read
//                              fresh here — no Seerr fetch, pure local re-check) to a local
//                              Jellyfin item via find_local_item; writes the resolved set into
//                              FjordState.jellyfin_watchlist_ids (the persistent source of truth
//                              item_to_card_item/items_to_model consult — real bug fix, a live
//                              model patch alone gets silently wiped by the next screen rebuild,
//                              see that field's own doc comment) AND patches
//                              context_menu.rs::patch_watchlist_on_jellyfin_models for each
//                              match (immediate feedback on whatever's on screen right now);
//                              genuinely not add-only — ids present in the old set but missing
//                              from the fresh one are explicitly patched back to false too.
//                              pub(crate), takes no ids param (reads state itself) so it can be
//                              called from anywhere as a cheap local re-check, not just after a
//                              fresh Seerr fetch — real gap found by LIVE-TESTING this exact fix:
//                              the resync triggered by fetch_and_store_watchlist's own trigger
//                              points (session start + every toggle refresh) reliably races
//                              AHEAD of all_movies/all_series being populated and finds 0 local
//                              matches on that first pass (confirmed via cargo run — "watchlist
//                              -> 5 id(s)" then "resync_jellyfin_watchlist_stars -> 0 local
//                              match(es)"); also re-triggered from main.rs's push_cached_data
//                              (once cache-loaded movies/series land), the auto-login fresh-
//                              series landing point, and spawn_movies_list_fetch's own
//                              completion — confirmed via a second cargo run that one of these
//                              later triggers finds the real matches ("-> 4 local match(es)").
//                              Generation-guarded (2026-07-22, code review finding: with 4
//                              independent trigger points and no ordering between them, an older
//                              call finishing AFTER a newer one had already written a more-
//                              complete result could silently clobber it, un-starring genuinely-
//                              still-watchlisted cards via its own stale diff) — see
//                              FjordState.jellyfin_watchlist_resync_seq's own doc comment
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// Watchlist row's own meta builders (2026-07-20) — `movie_details_to_meta`/
/// `tv_details_to_meta` above are NOT reusable here: both require a
/// `&RequestEntry` built from a real `MediaRequest`, which a plain
/// watchlist item may not have at all. `availability` is instead derived
/// straight from `d.media_info`, the SAME single-tier approach
/// `search_result_to_meta` already uses for Trending/Popular/Upcoming/New
/// in Theaters (not the Requested row's own dual-tier logic — a watchlist
/// item's primary concern is list membership, not request-tier status).
/// `on_watchlist: true` is set directly rather than read from
/// `d.on_user_watchlist` since membership is true by definition for every
/// candidate this function is ever called on. Callers are expected to call
/// `patch_known_request_state` afterward so an item that's ALSO requested
/// still gets its Edit/Cancel context-menu rows (not the visual pill,
/// which already comes from `media_info` above — `KnownRequest` doesn't
/// carry availability/is4k, only request_id/pending/mine).
///
/// Both return `None` for a Blocklisted item (2026-08-06, same "don't show
/// this in Discover" rule `search_result_to_meta` filters by — see its own
/// doc comment): blocklisting never removes the title from the actual Seerr
/// Watchlist (the two are independent Seerr entities, confirmed from
/// `Blocklist.addToBlocklist`'s own source, which only ever touches
/// `Media.status`/`status4k`), so without this filter a blocklisted-but-
/// still-watchlisted item would keep resurfacing here on every watchlist
/// refresh regardless of `remove_card_from_all_models` having pulled it off
/// screen a moment earlier.
fn watchlist_movie_to_meta(
    tmdb_id: i64,
    d: &MovieDetails,
) -> Option<(DiscoverCardMeta, Option<String>)> {
    let availability = availability_tag(d.media_info.as_ref().and_then(|mi| mi.status()));
    if availability == "blocklisted" {
        return None;
    }
    let year = d
        .release_date
        .as_deref()
        .filter(|s| s.len() >= 4)
        .map(|s| &s[..4])
        .unwrap_or("");
    let meta = DiscoverCardMeta {
        id: tmdb_id.to_string(),
        item_type: "DiscoverMovie",
        title: d.title.clone(),
        subtitle: year.to_string(),
        year: year.parse().unwrap_or(0),
        availability,
        requested_4k: false,
        other_tier_available: false,
        other_tier_requested: false,
        request_id: String::new(),
        request_pending: false,
        request_mine: false,
        genre_ids: Vec::new(),
        vote_average: 0.0,
        popularity: 0.0,
        on_watchlist: true,
    };
    Some((meta, d.poster_path.clone()))
}

fn watchlist_tv_to_meta(tmdb_id: i64, d: &TvDetails) -> Option<(DiscoverCardMeta, Option<String>)> {
    let availability = availability_tag(d.media_info.as_ref().and_then(|mi| mi.status()));
    if availability == "blocklisted" {
        return None;
    }
    let year = d
        .first_air_date
        .as_deref()
        .filter(|s| s.len() >= 4)
        .map(|s| &s[..4])
        .unwrap_or("");
    let meta = DiscoverCardMeta {
        id: tmdb_id.to_string(),
        item_type: "DiscoverTv",
        title: d.name.clone(),
        subtitle: year.to_string(),
        year: year.parse().unwrap_or(0),
        availability,
        requested_4k: false,
        other_tier_available: false,
        other_tier_requested: false,
        request_id: String::new(),
        request_pending: false,
        request_mine: false,
        genre_ids: Vec::new(),
        vote_average: 0.0,
        popularity: 0.0,
        on_watchlist: true,
    };
    Some((meta, d.poster_path.clone()))
}

/// Fetches every page of the connected user's Watchlist (`GET
/// /discover/watchlist`) into a plain `(item_type, tmdb_id)` id set — once
/// per session, guarded by `FjordState.discover_watchlist_fetched`, same
/// shape as `discover_landing_fetched`. Deliberately fetches ALL pages, not
/// just a capped prefix like `fetch_requested_row`'s 20-item cap: unlike
/// that cap (which bounds a much more expensive per-item DETAIL fetch),
/// this is plain id/title rows with no per-item network call, so even a
/// few hundred watchlist entries is a handful of cheap list fetches — safety-
/// capped at 10 pages (200 items) so a pathological watchlist can't loop
/// forever. Best-effort: a failed page just stops pagination early rather
/// than erroring the whole fetch. Watchlist + Release Calendar, 2026-07-18.
pub(crate) fn ensure_discover_watchlist(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let client = {
        let mut s = state.lock().unwrap();
        if s.discover_watchlist_fetched {
            return;
        }
        let Some(client) = s.seerr_client.clone() else {
            return;
        };
        s.discover_watchlist_fetched = true;
        client
    };
    rt.spawn(async move {
        fetch_and_store_watchlist(&client, &state, &ww).await;
    });
}

/// Shared by `ensure_discover_watchlist` and `refresh_watchlist` — fetches
/// every page, stores the resulting id set, and triggers a calendar rebuild
/// (the watchlist is one of the two sets `build_calendar_entries` unions).
async fn fetch_and_store_watchlist(
    client: &fjord_seerr::SeerrClient,
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
) {
    let mut ids: std::collections::HashSet<(&'static str, String)> =
        std::collections::HashSet::new();
    let mut page = 1;
    loop {
        let resp = match client.get_watchlist(page).await {
            Ok(r) => r,
            Err(e) => {
                debug!("seerr: get_watchlist page {page} failed: {e:#}");
                break;
            }
        };
        for item in &resp.results {
            let item_type = if item.media_type == "movie" {
                "DiscoverMovie"
            } else {
                "DiscoverTv"
            };
            ids.insert((item_type, item.tmdb_id.to_string()));
        }
        if page >= resp.total_pages || page >= 10 {
            break;
        }
        page += 1;
    }
    debug!("seerr: watchlist -> {} id(s)", ids.len());
    state.lock().unwrap().discover_watchlist_ids = ids.clone();
    build_calendar_entries(Arc::clone(state), ww.clone()).await;

    // Detail-fetch a preview of the watchlist (title+poster, unlike the
    // plain id set above) for the Discover Watchlist row + the Home/Movies/TV
    // dashboard rows — all 4 consumers share this ONE fetch (2026-07-20).
    // Spawned independently (tokio::spawn, not awaited inline) so these ~20
    // extra per-item detail fetches don't delay build_calendar_entries's own
    // commit above, mirroring how that function is itself spawned
    // independently from ensure_discover_landing for the identical reason.
    let client2 = client.clone();
    let state2 = Arc::clone(state);
    let ww2 = ww.clone();
    let ids2 = ids.clone();
    tokio::spawn(async move {
        populate_watchlist_rows(client2, state2, ww2, ids2).await;
    });

    // In-library watchlist star resync (2026-07-20) — same trigger points
    // as the row population above (session start + every toggle refresh),
    // spawned independently so it can't delay either of the other two.
    let state3 = Arc::clone(state);
    let ww3 = ww.clone();
    tokio::spawn(async move {
        resync_jellyfin_watchlist_stars(state3, ww3).await;
    });
}

/// Re-resolves EVERY currently-known-watchlisted tmdb id
/// (`FjordState.discover_watchlist_ids`, read fresh here — no Seerr network
/// call, this is a pure local re-check) to a local Jellyfin item (if owned)
/// via `find_local_item`, then (1) writes the resolved id set into
/// `FjordState.jellyfin_watchlist_ids` — the persistent source of truth
/// `item_to_card_item`/`items_to_model`/the various carry-forward merges
/// consult at CardItem-construction time (real bug fixed 2026-07-20: a
/// live-model patch alone, step 2 below, gets silently wiped by the next
/// screen rebuild — see `FjordState.jellyfin_watchlist_ids`'s own doc
/// comment) — and (2) patches the watchlist star onto every already-
/// rendered native Jellyfin `CardItem` model that item might be visible in
/// (`context_menu.rs::patch_watchlist_on_jellyfin_models`) for IMMEDIATE
/// feedback on whatever's on screen right now, without waiting for a
/// rebuild. This is the reactive, "patch on watchlist/request changes
/// only" population strategy (user's explicit choice over an eager
/// full-library scan on login): bounded by watchlist size in the LOOKUP
/// direction, not library size in the SCAN direction. `find_local_item`
/// itself does the actual `all_movies`/`all_series` scan, so this is a
/// genuine lookup per candidate, not a scan over the whole library.
/// Genuinely not add-only: ids present in the OLD set but missing from the
/// freshly-resolved one (removed from the watchlist, or no longer locally
/// owned) are explicitly patched back to `false` too, so a star can't get
/// stuck on stale. Two-phase pattern: `find_local_item` only reads
/// `FjordState` (safe from any thread — plain mutex lock, no Slint touch);
/// the resolved ids (plain Send-safe data) are collected first, then moved
/// into ONE `invoke_from_event_loop` closure to do the actual `CardItem`
/// patching — the same discipline `push_coming_up_row`'s real bug
/// (found+fixed earlier this session) established as mandatory.
///
/// `pub(crate)` and callable with no fresh Seerr fetch (unlike
/// `ensure_discover_watchlist`/`fetch_and_store_watchlist`) specifically so
/// it can ALSO be re-run from `main.rs` right after `all_movies`/`all_series`
/// get freshly populated (cache load, post-login fetch) — real gap found by
/// live-testing THIS exact fix: the very first resync (triggered by the
/// watchlist fetch itself, early in startup) reliably runs BEFORE the
/// library lists are populated, so `find_local_item` finds 0 matches on
/// that pass and the star never appears without a second, later resolve.
pub(crate) async fn resync_jellyfin_watchlist_stars(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
) {
    // Generation guard (2026-07-22, code review finding) — see
    // FjordState.jellyfin_watchlist_resync_seq's own doc comment for the
    // race this prevents. Captured BEFORE the scan so any call that starts
    // after us is guaranteed a higher number.
    let my_seq = {
        let mut s = state.lock().unwrap();
        s.jellyfin_watchlist_resync_seq += 1;
        s.jellyfin_watchlist_resync_seq
    };
    let ids = state.lock().unwrap().discover_watchlist_ids.clone();
    let resolved: std::collections::HashSet<String> = ids
        .iter()
        .filter_map(|(item_type, tmdb_id)| {
            let media_type = if *item_type == "DiscoverMovie" {
                "movie"
            } else {
                "tv"
            };
            find_local_item(&state, media_type, tmdb_id).map(|(jellyfin_id, _)| jellyfin_id)
        })
        .collect();
    let previous = {
        let mut s = state.lock().unwrap();
        if s.jellyfin_watchlist_resync_seq != my_seq {
            // A newer resync already started while we were scanning — it
            // will (or already did) write a fresher result, so writing ours
            // now would risk clobbering it with staler data. Skip outright
            // rather than racing: only the most-recently-started call ever
            // writes.
            debug!(
                "seerr: resync_jellyfin_watchlist_stars -> stale (newer resync started), discarding"
            );
            return;
        }
        std::mem::replace(&mut s.jellyfin_watchlist_ids, resolved.clone())
    };
    debug!(
        "seerr: resync_jellyfin_watchlist_stars -> {} local match(es)",
        resolved.len()
    );
    if resolved.is_empty() && previous.is_empty() {
        return;
    }
    let removed: Vec<String> = previous.difference(&resolved).cloned().collect();
    let added: Vec<String> = resolved.into_iter().collect();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = ww.upgrade() else { return };
        let g = AppState::get(&w);
        for jellyfin_id in added {
            crate::context_menu::patch_watchlist_on_jellyfin_models(&g, &jellyfin_id, true);
        }
        for jellyfin_id in removed {
            crate::context_menu::patch_watchlist_on_jellyfin_models(&g, &jellyfin_id, false);
        }
    });
}

/// Capped at 20, matching `fetch_requested_row`'s own `.truncate(20)` and
/// `build_calendar_entries`'s own candidate cap — this exact "a real
/// watchlist can be large, per-item detail fetches are the expensive part"
/// tradeoff was already reasoned about once for this feature and settled
/// on 20; reusing that number rather than picking a fresh one.
const WATCHLIST_ROW_CAP: usize = 20;

async fn populate_watchlist_rows(
    client: fjord_seerr::SeerrClient,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    ids: std::collections::HashSet<(&'static str, String)>,
) {
    let candidates: Vec<(&'static str, String)> = ids.into_iter().take(WATCHLIST_ROW_CAP).collect();
    if candidates.is_empty() {
        push_watchlist_rows(&ww, &state, Vec::new());
        return;
    }
    let n = candidates.len();
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set: tokio::task::JoinSet<(usize, Option<RequestedRowItem>)> =
        tokio::task::JoinSet::new();
    for (idx, (item_type, tmdb_id_str)) in candidates.into_iter().enumerate() {
        let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else {
            continue;
        };
        let client = client.clone();
        let sem = Arc::clone(&sem);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let item = if item_type == "DiscoverMovie" {
                client
                    .get_movie(tmdb_id)
                    .await
                    .ok()
                    .and_then(|d| watchlist_movie_to_meta(tmdb_id, &d))
            } else {
                client
                    .get_tv(tmdb_id)
                    .await
                    .ok()
                    .and_then(|d| watchlist_tv_to_meta(tmdb_id, &d))
            };
            (idx, item)
        });
    }
    let mut out: Vec<Option<RequestedRowItem>> = (0..n).map(|_| None).collect();
    while let Some(res) = set.join_next().await {
        let Ok((idx, item)) = res else { continue };
        out[idx] = item;
    }
    let mut items: Vec<RequestedRowItem> = out.into_iter().flatten().collect();
    // Populates request_id/pending/mine (Edit/Cancel context-menu rows) for
    // an item that's ALSO requested — see watchlist_movie_to_meta's own doc
    // comment for why this doesn't touch the visual availability pill,
    // which is already set from media_info directly.
    let known = state.lock().unwrap().discover_known_requests.clone();
    for (meta, _) in &mut items {
        patch_known_request_state(meta, &known);
    }
    debug!("seerr: watchlist row items -> {} card(s)", items.len());
    push_watchlist_rows(&ww, &state, items.clone());
    fetch_watchlist_posters(ww, &items).await;
}

/// Splits by item_type into mixed/movies/tv (mirrors home.rs's own
/// Continue-Watching cw_movies/cw_tv 3-way split — one source, filtered
/// client-side, no extra network calls) and commits all 3 AppState models
/// inside ONE `invoke_from_event_loop` closure. Mandatory two-phase
/// pattern: `items` is plain Send-safe `DiscoverCardMeta` data built
/// off-thread; `CardItem` (always `!Send` — carries a `slint::Image` field
/// regardless of whether it's populated) is only ever constructed here,
/// and every `AppState` touch happens inside this one closure — the exact
/// discipline `push_coming_up_row`'s own real bug (found+fixed earlier this
/// session: called directly from a Tokio-thread `async fn`, silently never
/// set anything because `slint::Weak::upgrade()` returns `None` off the UI
/// thread, no panic, no error) established as mandatory for this file.
///
/// Routes all 3 models through `apply_cards_preserving_identity` (2026-07-22,
/// code review finding) instead of unconditionally building a fresh
/// `ModelRc` — this function runs on EVERY watchlist refresh, which per
/// `refresh_watchlist`'s own doc comment fires after every single Add/Remove
/// Watchlist toggle anywhere in the app. A bare `ModelRc::new(...)` swap, per
/// CLAUDE.md's own documented Phase 96 finding, makes Slint destroy and
/// recreate every delegate element even when nothing in the row actually
/// changed — re-fading every OTHER already-visible card's poster and
/// discarding its already-decoded `Image` handle just because one unrelated
/// item was toggled.
fn push_watchlist_rows(
    ww: &Weak<MainWindow>,
    state: &Arc<Mutex<FjordState>>,
    items: Vec<RequestedRowItem>,
) {
    let ww = ww.clone();
    // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09) — this
    // function had no staleness guard at all. Not the same Arc::ptr_eq
    // shape as seerr_session_current: `client` arrives here as an owned,
    // value-cloned `SeerrClient` (Clone-by-value, see fjord-seerr's own
    // impl) rather than a threaded-through `Arc<SeerrClient>` — its
    // original Arc identity was already lost several calls up this chain
    // (fetch_and_store_watchlist takes `&SeerrClient`, deref-coerced from
    // the Arc it started as), so a true identity check would mean
    // re-plumbing that whole chain's client type. A coarser but still
    // real check instead: if Seerr has been disconnected entirely (the
    // common case for both sign-out and a profile switch to an account
    // with no Seerr connection configured — Bonfire sub-profiles very
    // plausibly don't each have their own), bail rather than commit stale
    // rows. Does not catch switching to a DIFFERENT account that also has
    // Seerr connected — a narrower residual gap, left open rather than
    // risking a deeper refactor of an otherwise-working fetch chain.
    let state = Arc::clone(state);
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = ww.upgrade() else { return };
        if state.lock().unwrap().seerr_client.is_none() {
            return;
        }
        let g = AppState::get(&w);
        let mixed: Vec<CardItem> = items
            .iter()
            .map(|(m, _)| m.clone().into_card_item())
            .collect();
        let movies: Vec<CardItem> = items
            .iter()
            .filter(|(m, _)| m.item_type == "DiscoverMovie")
            .map(|(m, _)| m.clone().into_card_item())
            .collect();
        let tv: Vec<CardItem> = items
            .iter()
            .filter(|(m, _)| m.item_type == "DiscoverTv")
            .map(|(m, _)| m.clone().into_card_item())
            .collect();
        debug!(
            "seerr: push_watchlist_rows -> mixed={} movies={} tv={}",
            mixed.len(),
            movies.len(),
            tv.len()
        );
        g.set_discover_watchlist_mixed(crate::apply_cards_preserving_identity(
            &g.get_discover_watchlist_mixed(),
            mixed,
        ));
        g.set_discover_watchlist_movies(crate::apply_cards_preserving_identity(
            &g.get_discover_watchlist_movies(),
            movies,
        ));
        g.set_discover_watchlist_tv(crate::apply_cards_preserving_identity(
            &g.get_discover_watchlist_tv(),
            tv,
        ));
    });
}

/// Patches posters onto all 3 watchlist models by id+item_type match (not
/// by row index — the same tmdb id can sit at a DIFFERENT row index in
/// `discover-watchlist-mixed` vs. its own type-specific list, unlike the
/// Coming Up row's single-model index-based patch). Same bounded-
/// concurrency fetch-then-patch shape as `fetch_coming_up_posters`.
async fn fetch_watchlist_posters(ww: Weak<MainWindow>, items: &[RequestedRowItem]) {
    let jobs: Vec<(String, String, String)> = items
        .iter()
        .filter_map(|(m, p)| {
            p.clone()
                .map(|p| (m.item_type.to_string(), m.id.clone(), p))
        })
        .collect();
    if jobs.is_empty() {
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
    for (item_type, tmdb_id, poster_path) in jobs {
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
            Some((item_type, tmdb_id, buf))
        });
    }
    while let Some(res) = set.join_next().await {
        let Ok(Some((item_type, tmdb_id, buf))) = res else {
            continue;
        };
        let ww2 = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            let g = AppState::get(&w);
            for model in [
                g.get_discover_watchlist_mixed(),
                g.get_discover_watchlist_movies(),
                g.get_discover_watchlist_tv(),
            ] {
                for i in 0..model.row_count() {
                    if let Some(mut card) = model.row_data(i)
                        && card.id.as_str() == tmdb_id
                        && card.item_type.as_str() == item_type
                    {
                        card.poster = slint::Image::from_rgba8(buf.clone());
                        card.has_poster = true;
                        model.set_row_data(i, card);
                    }
                }
            }
        });
    }
}

/// Re-fetches the full watchlist id set and rebuilds the calendar —
/// called right after `discover_toggle_watchlist` succeeds, mirroring
/// `refresh_requested_row`'s "cheap enough for an infrequent action" shape.
pub(crate) fn refresh_watchlist(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        return;
    };
    rt.spawn(async move {
        fetch_and_store_watchlist(&client, &state, &ww).await;
    });
}

/// Add/remove Watchlist — wired from the Discover context menu's Watchlist
/// row and RequestDetailScreen's Watchlist button. POST/DELETE, then
/// patches every visible card + updates the id cache + rebuilds the
/// calendar (a watchlist change is one of the two things that can change
/// what's on it). Watchlist + Release Calendar, 2026-07-18.
/// `success_toast` replaces the usual "Added to/Removed from Watchlist"
/// toast — used by a successful request, which auto-adds to the watchlist
/// and shows one combined toast instead of two back to back (2026-10-04).
#[allow(clippy::too_many_arguments)]
pub(crate) fn discover_toggle_watchlist(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    tmdb_id: i64,
    media_type: String,
    title: String,
    adding: bool,
    success_toast: Option<&'static str>,
) {
    debug!(
        "seerr: discover_toggle_watchlist tmdb={tmdb_id} media_type={media_type} adding={adding}"
    );
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    let item_type: &'static str = if media_type == "movie" {
        "DiscoverMovie"
    } else {
        "DiscoverTv"
    };
    let rt2 = rt.clone();

    rt.spawn(async move {
        let result = if adding {
            client.add_watchlist(tmdb_id, &media_type, &title).await
        } else {
            client.remove_watchlist(tmdb_id, &media_type).await
        };
        match result {
            Ok(()) => {
                debug!("seerr: discover_toggle_watchlist succeeded tmdb={tmdb_id} adding={adding}");
                {
                    let mut s = state.lock().unwrap();
                    let key = (item_type, tmdb_id.to_string());
                    if adding {
                        s.discover_watchlist_ids.insert(key);
                    } else {
                        s.discover_watchlist_ids.remove(&key);
                    }
                }
                // Resolved once, reused below for both the in-library star
                // patch (add or remove) and, on add only, the played-state
                // reset — a plain (Jellyfin id, Jellyfin item_type) lookup,
                // no network call.
                let local_item = crate::discover::find_local_item(&state, &media_type, &tmdb_id.to_string());

                // Adding an already-watched item back to the watchlist reads
                // as "I want to watch this again," not left watched
                // (2026-08-02, user request, asked directly rather than
                // guessed — the alternative was blocking the add outright).
                // Only applies to items already in the local Jellyfin
                // library; a Discover-only item has no played state to
                // reset. Real Jellyfin API call (mark_unplayed), not just a
                // local flag flip — Jellyfin echoes it back through
                // UserDataChanged the same way every other played-state
                // change in this app does, so every other visible model
                // (Not Watched rows, etc.) still converges via the existing
                // WS path; the two writes below are only for INSTANT
                // feedback on whatever's already on screen right now.
                let mut reset_played = false;
                if adding && let Some((jellyfin_id, _)) = &local_item {
                    let was_played = {
                        let s = state.lock().unwrap();
                        let list: &[fjord_api::models::MediaItem] =
                            if media_type == "movie" { &s.all_movies } else { &s.all_series };
                        list.iter().find(|m| &m.id == jellyfin_id).is_some_and(|m| m.user_data.played)
                    };
                    if was_played {
                        let jf_client = state.lock().unwrap().client.as_ref().map(Arc::clone);
                        if let Some(jf_client) = jf_client {
                            match jf_client.mark_unplayed(jellyfin_id).await {
                                Ok(()) => {
                                    state.lock().unwrap().update_item_user_state(jellyfin_id, Some(false), None);
                                    reset_played = true;
                                }
                                Err(e) => warn!("discover_toggle_watchlist: mark_unplayed({jellyfin_id}) failed: {e:#}"),
                            }
                        }
                    }
                }

                let ww2 = ww.clone();
                let state3 = Arc::clone(&state);
                let local_item2 = local_item.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        patch_watchlist_on_all_models(&g, item_type, tmdb_id, adding);
                        if g.get_show_request_detail()
                            && g.get_request_detail_media_type().as_str() == media_type
                            && g.get_request_detail_tmdb_id() == tmdb_id as i32
                        {
                            g.set_request_detail_on_watchlist(adding);
                        }
                        // In-library star (2026-07-20) — the toggled item
                        // might ALSO be a native Jellyfin card somewhere
                        // (Continue Watching, the library grid, etc); patch
                        // that too, same shape as patch_watchlist_on_all_models
                        // above but by Jellyfin id instead of tmdb id. Also
                        // keep the persisted jellyfin_watchlist_ids set in
                        // sync incrementally (not just resync's own wholesale
                        // replace) so a screen rebuilt between now and the
                        // next resync still gets the right value at
                        // construction time, not just this live patch.
                        if let Some((jellyfin_id, _)) = &local_item2 {
                            crate::context_menu::patch_watchlist_on_jellyfin_models(&g, jellyfin_id, adding);
                            let mut s = state3.lock().unwrap();
                            if adding { s.jellyfin_watchlist_ids.insert(jellyfin_id.clone()); }
                            else      { s.jellyfin_watchlist_ids.remove(jellyfin_id); }
                            drop(s);
                            if reset_played {
                                crate::context_menu::update_card_in_all_models(&w, jellyfin_id, Some(false), None);
                            }
                        }
                    }
                });
                show_toast(
                    ww.clone(),
                    success_toast.unwrap_or(if adding {
                        if reset_played { "Added to Watchlist — marked unwatched" } else { "Added to Watchlist" }
                    } else {
                        "Removed from Watchlist"
                    }).into(),
                );
                refresh_watchlist(Arc::clone(&state), ww, rt2);
            }
            Err(e) => handle_seerr_error(&state, &ww, is_session_auth, "Couldn't update watchlist", &e),
        }
    });
}
