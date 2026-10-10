// ── fjord-app · startup.rs ───────────────────────────────────────────────────
//   Startup / session-start flow (moved from main.rs, 0.5.0 step 3):
//   push_cached_data     push on-disk caches (home/movies/series/collections/artists/albums/
//                        playlists) into AppState/FjordState for instant display — only called
//                        after spawn_auto_login's probe confirms the server is reachable; takes
//                        the six screen-open caches (Phase 103) as an already-loaded parameter;
//                        also re-triggers discover::resync_jellyfin_watchlist_stars once
//                        all_movies/all_series are populated (2026-07-20 — the watchlist-driven
//                        resync commonly races ahead of this and finds 0 local matches; live-
//                        confirmed via cargo run before AND after this fix)
//                        (screen_caches.json can reach ~1.3MB after a prewarm — the caller reads
//                        + parses it via spawn_blocking before entering invoke_from_event_loop,
//                        since this function itself runs synchronously on the Slint UI thread)
//   spawn_screen_cache_refresh  post-login background refresh for the six screen-open caches
//                        (Phase 103): one batched get_items_by_ids_detailed call refreshes
//                        item_detail_cache; the 5 relationship caches (no batch endpoint) trickle
//                        out under a shared semaphore(2) so they never compete with foreground use.
//                        AMBIENT_REFRESH_LIMIT (40, Phase 104 fix) caps every cache to its most
//                        recent N keys (BoundedCache::recent_keys) regardless of actual cache
//                        size — without this, prewarm.rs raising a cache's cap to fit the whole
//                        library would make this *ambient, unprompted, every-login* sweep repeat
//                        the prewarm's full request volume every time instead of once;
//                        session_current() guarded (found in review, 2026-07-11) — this sweep
//                        writes per-user MediaItem data on every single login, not just when the
//                        opt-in prewarm button is pressed, so it had the same cross-session
//                        cache-contamination exposure prewarm.rs was already fixed for
//   wire_screen_cache_save_timer  60s repeating slint::Timer, flushes the six caches to
//                        screen_caches.json (Phase 103) — plus person_tmdb_id (2026-07-29,
//                        Deep Seerr integration; a 7th field on ScreenCachesFile, not a 7th
//                        cache in this timer's own six-cache framing, see config.rs). Reads
//                        `s.client`'s own user_id at save time, not `config.active()`
//                        (2026-08-16, code review — the latter falls back to profiles.first()
//                        whenever active_profile_id doesn't match, which sign-out deliberately
//                        clears; that mismatch previously wrote a signed-out session's cleared
//                        caches into an unrelated still-valid account's file); skips the save
//                        entirely when there's no live client at all.
//   spawn_auto_login     probe saved session (check_auth, 8s timeout) → best-effort display_name
//                        backfill (get_user_info, 2026-08-14 — the auto-login path never sees a
//                        login response, unlike do_login, so a blank profile name self-heals here
//                        instead of staying the raw user_id GUID forever) → push_cached_data (also
//                        pushes the sidebar's current-profile-tile now, previously only done by
//                        finish_session_setup and never on this, the ordinary launch path) then
//                        fetch_home_data/get_all_series/get_system_info + start_websocket +
//                        spawn_screen_cache_refresh, all now guarded by session_current() right
//                        after the join (2026-08-16, code review — a profile switch completing
//                        during this ~2.5s join could previously land the OUTGOING session's
//                        series/plugins/WS-abort-handle into the just-switched-to profile); 401: show-login; anything else (can't reach
//                        server at all): show-offline. Re-invoked by AppState.retry-connection.
//   spawn_jellyfin_admin_check  is the signed-in user a Jellyfin admin (Settings gating)
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_api::JellyfinClient;

use crate::MainWindow;
use crate::config::{FjordState, ScreenCachesFile};

// Bonfire Phase 6 (admin actions, 2026-09-04) — the Jellyfin-server-admin
// counterpart to spawn_seerr_settings_fetch above, same shape. Genuinely
// new capability: Fjord never modeled Jellyfin's own admin flag before
// this (see UserDto.policy's own doc comment). `FjordState` is rebuilt
// fresh on every process start — unlike a persisted Config field, there's
// no on-disk cache to fall back on — so this has to run unconditionally
// on EVERY session-establishment path (a fresh login, a switch, and an
// ordinary auto-login resume), not just when some other value happens to
// need backfilling; called from finish_session_setup (auth.rs) and
// spawn_auto_login (this file) directly, one call site each, rather than
// piggybacking on the existing get_user_info call a few lines up in
// spawn_auto_login (which is itself gated on `needs_name` and would
// silently skip this for the common already-named case).
pub(crate) fn spawn_jellyfin_admin_check(
    client: Arc<JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    rt.spawn(async move {
        let is_admin = match client.get_user_info().await {
            Ok(info) => info.policy.is_administrator,
            Err(e) => {
                warn!("get_user_info (server-admin check): {:#}", e);
                return;
            }
        };
        // Re-check via session_current (Arc::ptr_eq), matching every other
        // async-result race in this file — NOT a string comparison against
        // active_profile_id, which was a real bug found in code review:
        // signing out of an account and immediately re-logging into the
        // SAME account gives the new session the identical user_id string,
        // so a stale request from the torn-down OLD session would have
        // wrongly passed that check and overwritten this session's own
        // jellyfin_is_server_admin with a result computed for a client
        // that's no longer live.
        if !session_current(&state, &client) {
            return;
        }
        let mut s = state.lock().unwrap();
        s.jellyfin_is_server_admin = is_admin;
        drop(s);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_jellyfin_is_server_admin(is_admin);
            }
        });
    });
}

// ── startup connectivity gate ────────────────────────────────────────────────
// Push on-disk caches into AppState/FjordState for instant display. Only
// called once the saved-session auth probe has confirmed the server is
// reachable (see spawn_auto_login) — showing cached content before that was
// confirmed made a fully offline cold start look identical to normal quiet
// operation (nothing distinguished "stale but fine" from "can't reach the
// server at all").
pub(crate) fn push_cached_data(
    window: &MainWindow,
    client: &Arc<JellyfinClient>,
    state: &Arc<Mutex<FjordState>>,
    rt_handle: &tokio::runtime::Handle,
    screen_caches: Option<ScreenCachesFile>,
) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    let watchlist = state.lock().unwrap().jellyfin_watchlist_ids.clone();
    // client.user_id is the authoritative identity for this data (the
    // session it was actually fetched under), more direct than re-deriving
    // from state.config.active() and avoids an extra lock.
    let user_id = client.user_id.clone();
    crate::config::migrate_flat_caches_to_profile(&user_id);
    if let Some(cached_home) = load_home_cache(&user_id) {
        push_home_data(window, &cached_home, &watchlist);
        let sections = home_data_sections(&cached_home);
        spawn_poster_loading(
            Arc::clone(client),
            sections,
            window.as_weak(),
            rt_handle.clone(),
            Arc::clone(state),
        );
    }
    if let Some(cached_movies) = load_movies_cache(&user_id) {
        let model = items_to_model(&cached_movies, &watchlist);
        spawn_movies_poster_loading(
            Arc::clone(client),
            cached_movies.clone(),
            window.as_weak(),
            rt_handle.clone(),
        );
        // Display-only: do NOT set movies_fetched — the first grid open this
        // session must still do its network refresh (cache-staleness fix S1).
        state.lock().unwrap().all_movies = cached_movies;
        AppState::get(window).set_all_movies(model);
    }
    if let Some(cached_series) = load_series_cache(&user_id) {
        AppState::get(window).set_all_series(items_to_model(&cached_series, &watchlist));
        spawn_series_poster_loading(
            Arc::clone(client),
            cached_series.clone(),
            window.as_weak(),
            rt_handle.clone(),
            Arc::clone(state),
        );
        state.lock().unwrap().all_series = cached_series;
    }
    if let Some(cached_cols) = load_collections_cache(&user_id) {
        let model = items_to_model(&cached_cols, &watchlist);
        spawn_collections_poster_loading(
            Arc::clone(client),
            cached_cols.clone(),
            window.as_weak(),
            rt_handle.clone(),
        );
        state.lock().unwrap().all_collections = cached_cols;
        AppState::get(window).set_all_collections(model);
    }
    if let Some(cached_artists) = load_artists_cache(&user_id) {
        let model = items_to_model(&cached_artists, &watchlist);
        spawn_artists_poster_loading(
            Arc::clone(client),
            cached_artists.clone(),
            window.as_weak(),
            rt_handle.clone(),
        );
        state.lock().unwrap().all_artists = cached_artists;
        AppState::get(window).set_all_artists(model);
    }
    if let Some(cached_albums) = load_albums_cache(&user_id) {
        let model = items_to_model(&cached_albums, &watchlist);
        spawn_albums_poster_loading(
            Arc::clone(client),
            cached_albums.clone(),
            window.as_weak(),
            rt_handle.clone(),
        );
        state.lock().unwrap().all_albums = cached_albums;
        AppState::get(window).set_all_albums(model);
    }
    if let Some(cached_playlists) = load_playlists_cache(&user_id) {
        let model = items_to_model(&cached_playlists, &watchlist);
        spawn_playlists_poster_loading(
            Arc::clone(client),
            cached_playlists.clone(),
            window.as_weak(),
            rt_handle.clone(),
        );
        state.lock().unwrap().all_playlists = cached_playlists;
        AppState::get(window).set_all_playlists(model);
    }
    // Screen-open caches (Phase 103): loaded here too, so a reopened Detail/
    // Series/Season/Collection/Album/Artist/Person screen can skip the network
    // fetch and the loading spinner even on the very first open after a fresh
    // launch. Freshness is handled by the post-login background refresh
    // (spawn_auto_login) and WS-driven invalidation (ws.rs), not by discarding
    // this on load. The actual file read + JSON parse happens BEFORE this
    // function is called (see the call site) — this file can reach ~1.3MB
    // after a library prewarm, and this function runs synchronously on the
    // Slint UI/event-loop thread (invoked from invoke_from_event_loop), so
    // doing the blocking I/O here would stall rendering for its duration.
    if let Some(file) = screen_caches {
        let mut s = state.lock().unwrap();
        s.item_detail_cache = file.item_detail;
        s.similar_items_cache = file.similar_items;
        s.boxset_items_cache = file.boxset_items;
        s.artist_albums_cache = file.artist_albums;
        s.person_filmography_cache = file.person_filmography;
        s.container_tracks_cache = file.container_tracks;
        s.person_tmdb_id_cache = file.person_tmdb_id;
    }
    // Re-resolve the in-library watchlist star now that all_movies/all_series
    // have just been populated from disk cache (2026-07-20) — the very first
    // resync (triggered independently by the watchlist fetch itself) commonly
    // races ahead of this and finds nothing, since it isn't sequenced against
    // the cache load at all; this is a real, live-confirmed gap (a fresh
    // `cargo run` logged "resync_jellyfin_watchlist_stars -> 0 local
    // match(es)" despite 5 real watchlist ids and a populated movies.json).
    // No-op, cheap, if there's no live Seerr connection yet or nothing on the
    // watchlist matches anything cached.
    if state.lock().unwrap().seerr_client.is_some() {
        let state_wl = Arc::clone(state);
        let ww_wl = window.as_weak();
        rt_handle.spawn(async move {
            crate::discover::resync_jellyfin_watchlist_stars(state_wl, ww_wl).await;
        });
    }
}

/// Post-login background refresh for the six screen-open caches (Phase 103).
/// Called after the main startup burst (fetch_home_data/get_all_series/
/// get_system_info + WS) has already been fired — never blocks anything
/// visible, since the persisted cache (loaded in push_cached_data) is already
/// painting instantly the whole time this runs.
///
/// Fast step: item_detail_cache is refreshed in one batched
/// get_items_by_ids_detailed call. A requested id missing from the response
/// is a deleted item — removed from the cache rather than left as a ghost.
///
/// Slow step: the 5 relationship caches (similar items / boxset members /
/// artist albums / person filmography / album+playlist tracks) have no batch
/// endpoint — each cached key needs its own call. All of them share one
/// low-concurrency semaphore (2) so they trickle out gradually instead of
/// bursting, deliberately not competing with whatever the user is actually
/// doing on a slow connection. A 404 (item deleted) removes that entry; any
/// other error just leaves the stale entry in place for next time.
/// Cap on how many entries the *ambient* per-login sweep revalidates per
/// cache — independent of how large `BoundedCache.cap` has grown via the
/// opt-in prewarm sweep (`prewarm.rs`, Phase 104), which fills the cache with
/// the whole library. Without this, a prewarmed library would repeat the
/// prewarm's full request volume on every single login instead of once.
pub(crate) const AMBIENT_REFRESH_LIMIT: usize = 40;

pub(crate) fn spawn_screen_cache_refresh(
    client: Arc<JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    rt: tokio::runtime::Handle,
) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    rt.spawn(async move {
        let detail_keys = state
            .lock()
            .unwrap()
            .item_detail_cache
            .recent_keys(AMBIENT_REFRESH_LIMIT);
        if !detail_keys.is_empty() {
            match client.get_items_by_ids_detailed(&detail_keys).await {
                Ok(items) => {
                    if !session_current(&state, &client) {
                        return;
                    }
                    let returned: std::collections::HashSet<String> =
                        items.iter().map(|i| i.id.clone()).collect();
                    let mut s = state.lock().unwrap();
                    for item in items {
                        s.item_detail_cache.insert(item.id.clone(), item);
                    }
                    for key in &detail_keys {
                        if !returned.contains(key) {
                            s.item_detail_cache.remove(key);
                        }
                    }
                    debug!(
                        "screen cache refresh: item_detail_cache ({} keys)",
                        detail_keys.len()
                    );
                }
                Err(e) => warn!("screen cache refresh: get_items_by_ids_detailed: {:#}", e),
            }
        }

        let sem = Arc::new(tokio::sync::Semaphore::new(2));
        let mut set = tokio::task::JoinSet::new();

        for key in state
            .lock()
            .unwrap()
            .similar_items_cache
            .recent_keys(AMBIENT_REFRESH_LIMIT)
        {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) {
                    return;
                }
                match client.get_similar_items(&key).await {
                    Ok(v) => {
                        if session_current(&state, &client) {
                            state.lock().unwrap().similar_items_cache.insert(key, v);
                        }
                    }
                    Err(e) => {
                        if is_not_found(&e) {
                            state.lock().unwrap().similar_items_cache.remove(&key);
                        }
                    }
                }
            });
        }
        for key in state
            .lock()
            .unwrap()
            .boxset_items_cache
            .recent_keys(AMBIENT_REFRESH_LIMIT)
        {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) {
                    return;
                }
                match client.get_boxset_items(&key).await {
                    Ok(v) => {
                        if session_current(&state, &client) {
                            state.lock().unwrap().boxset_items_cache.insert(key, v);
                        }
                    }
                    Err(e) => {
                        if is_not_found(&e) {
                            state.lock().unwrap().boxset_items_cache.remove(&key);
                        }
                    }
                }
            });
        }
        for key in state
            .lock()
            .unwrap()
            .artist_albums_cache
            .recent_keys(AMBIENT_REFRESH_LIMIT)
        {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) {
                    return;
                }
                match client.get_artist_albums(&key).await {
                    Ok(v) => {
                        if session_current(&state, &client) {
                            state.lock().unwrap().artist_albums_cache.insert(key, v);
                        }
                    }
                    Err(e) => {
                        if is_not_found(&e) {
                            state.lock().unwrap().artist_albums_cache.remove(&key);
                        }
                    }
                }
            });
        }
        for key in state
            .lock()
            .unwrap()
            .person_filmography_cache
            .recent_keys(AMBIENT_REFRESH_LIMIT)
        {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) {
                    return;
                }
                match client.get_person_filmography(&key).await {
                    Ok(v) => {
                        if session_current(&state, &client) {
                            state
                                .lock()
                                .unwrap()
                                .person_filmography_cache
                                .insert(key, v);
                        }
                    }
                    Err(e) => {
                        if is_not_found(&e) {
                            state.lock().unwrap().person_filmography_cache.remove(&key);
                        }
                    }
                }
            });
        }
        for key in state
            .lock()
            .unwrap()
            .container_tracks_cache
            .recent_keys(AMBIENT_REFRESH_LIMIT)
        {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) {
                    return;
                }
                // container_tracks_cache holds both album and playlist ids with no
                // stored type marker; get_album_tracks is a ParentId-filtered query
                // so a playlist id just yields an empty (not error) result — try
                // that first, fall back to get_playlist_items on empty.
                match client.get_album_tracks(&key).await {
                    Ok(v) if !v.is_empty() => {
                        if session_current(&state, &client) {
                            state.lock().unwrap().container_tracks_cache.insert(key, v);
                        }
                    }
                    _ => match client.get_playlist_items(&key).await {
                        Ok(v) => {
                            if session_current(&state, &client) {
                                state.lock().unwrap().container_tracks_cache.insert(key, v);
                            }
                        }
                        Err(e) => {
                            if is_not_found(&e) {
                                state.lock().unwrap().container_tracks_cache.remove(&key);
                            }
                        }
                    },
                }
            });
        }

        while set.join_next().await.is_some() {}
        debug!("screen cache background refresh sweep complete");
    });
}

/// Periodically flushes the six screen-open caches to disk (Phase 103) —
/// mirrors `wire_nw_timer`'s repeating-`slint::Timer` shape. Always writes
/// unconditionally every tick rather than tracking a dirty flag: the file is
/// small and the write is cheap, so there's no real cost to a periodic write
/// that happened to find nothing new since the last one.
pub(crate) fn wire_screen_cache_save_timer(
    state: Arc<Mutex<FjordState>>,
    rt_handle: tokio::runtime::Handle,
) -> slint::Timer {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    let timer = slint::Timer::default();
    // Back to 60s, 2026-08-01 (was briefly widened to 300s the same day as a
    // mitigation, then reverted once the actual fix landed): save_screen_-
    // caches's clone of the six BoundedCaches used to be a genuine O(n)
    // HashMap+VecDeque copy under the global FjordState lock, real cost for
    // any session that's run the opt-in library prewarm — widening the
    // interval only reduced how OFTEN that cost was paid, not the cost
    // itself. `BoundedCache<V>` now wraps its storage in `Arc` with
    // `Arc::make_mut`-based copy-on-write (see its own doc comment), so the
    // clone this timer triggers is O(1) in the common case — no reason left
    // to save less often than before.
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(60),
        move || {
            let state2 = Arc::clone(&state);
            rt_handle.spawn(async move {
                // Read live at save time, unlike the fetch-tied call sites
                // elsewhere in this file — a periodic flush of whatever's
                // currently in FjordState has no async gap between "whose data
                // is this" and "whose file do I write it to": both come from
                // the same instant, inside save_screen_caches's own lock.
                //
                // Real bug, code-review 2026-08-16: this used to read
                // `config.active().user_id`, which can genuinely diverge from
                // what's actually loaded in FjordState — `Config::active()`
                // falls back to `profiles.first()` whenever `active_profile_id`
                // doesn't match any entry (exactly what sign-out does: it
                // clears active_profile_id, so if another account remains
                // known, this resolved to THAT unrelated account and wrote the
                // just-cleared caches into ITS screen_caches.json). Using the
                // live client's own user_id instead ties the save to the
                // session that's actually loaded — None (skip entirely) when
                // signed out or before any login completes, and correctly the
                // just-switched-to profile mid-switch, matching every other
                // fetch-tied call site in this file, which already keys off
                // `client.user_id` rather than `config.active()` for the exact
                // same reason.
                let user_id = state2
                    .lock()
                    .unwrap()
                    .client
                    .as_ref()
                    .map(|c| c.user_id.clone());
                if let Some(user_id) = user_id {
                    save_screen_caches(&state2, &user_id);
                }
            });
        },
    );
    timer
}

/// Probe the saved session, then either show cached content + refresh in the
/// background (reachable), redirect to login (definite 401), or show
/// OfflineScreen (anything else — DNS/connect/timeout: we genuinely can't
/// tell if the session is still valid). Re-invoked by the Retry button on
/// OfflineScreen with fresh clones, so this must not assume it only runs once.
pub(crate) fn spawn_auto_login(
    client: Arc<JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    let rt_handle2 = rt_handle.clone();
    rt_handle.spawn(async move {
        if let Err(e) = client.check_auth().await {
            if is_unauthorized(&e) {
                warn!("saved token is invalid (401) — showing login screen");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = window_weak.upgrade() {
                        let g = AppState::get(&w);
                        g.set_show_connecting(false);
                        g.set_show_login(true);
                        g.set_status(ss("Session expired — please log in again"));
                    }
                });
                return;
            }
            // Not a definite session failure — we genuinely can't reach the
            // server. Say so plainly instead of quietly proceeding into a
            // dashboard that would just fail a dozen ways in the background.
            warn!("auth probe failed (non-401): {e:#}");
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = window_weak.upgrade() {
                    let g = AppState::get(&w);
                    g.set_show_connecting(false);
                    g.set_show_offline(true);
                    g.set_offline_focused(0);
                    g.set_status(ss("Couldn't reach the server — check your connection."));
                }
            });
            return;
        }

        // Real bug fix, 2026-08-14, live-reported ("on an old login the
        // profilename is just random letters and numbers instead of the
        // profile name"): a profile whose display_name was never populated
        // — every pre-Bonfire migrated profile, and any profile that's only
        // ever gone through auto-login rather than a fresh password login
        // (do_login is the only other place that backfills this, from the
        // login response's own auth.user.name) — falls back to the raw
        // user_id GUID everywhere it's shown (the sidebar profile row, the
        // profile picker). This path never talks to the login endpoint at
        // all, so it never sees a real name unless fetched explicitly here.
        // Best-effort, only when actually needed (skips the extra request
        // for the overwhelmingly common already-named case).
        let needs_name = state
            .lock()
            .unwrap()
            .config
            .active()
            .display_name
            .is_empty();
        if needs_name {
            match client.get_user_info().await {
                Ok(info) if !info.name.is_empty() => {
                    let mut s = state.lock().unwrap();
                    // Re-check under the lock — a picker-driven switch could have
                    // changed the active profile while this request was in flight.
                    if s.config.active_profile_id == client.user_id
                        && s.config.active().display_name.is_empty()
                    {
                        s.config.active_mut().display_name = info.name;
                        let cfg_snapshot = s.config.clone();
                        drop(s);
                        save_config(&cfg_snapshot);
                    }
                }
                Ok(_) => {}
                Err(e) => warn!("get_user_info (display_name backfill): {:#}", e),
            }
        }

        // Reachable — load on-disk caches now for instant display, then
        // continue refreshing in the background exactly as before.
        // screen_caches.json can reach ~1.3MB after a library prewarm; read +
        // parse it here, off the UI thread (spawn_blocking, since this is
        // synchronous std::fs I/O running inside an async task) rather than
        // inside push_cached_data, which runs synchronously on the Slint
        // event-loop thread via invoke_from_event_loop below.
        let user_id_sc = client.user_id.clone();
        // Also covers screen_caches.json, ahead of the load below — run here
        // (not just inside push_cached_data's own call to this same fn) so
        // the very first post-upgrade launch gets its instant warm start for
        // ALL eight files, not seven of eight.
        crate::config::migrate_flat_caches_to_profile(&user_id_sc);
        let screen_caches = tokio::task::spawn_blocking(move || load_screen_caches(&user_id_sc))
            .await
            .ok()
            .flatten();
        let ww_cache = window_weak.clone();
        let client_cache = Arc::clone(&client);
        let state_cache = Arc::clone(&state);
        let rt_cache = rt_handle2.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_cache.upgrade() else { return };
            push_cached_data(&w, &client_cache, &state_cache, &rt_cache, screen_caches);
            let g = AppState::get(&w);
            g.set_show_connecting(false);
            g.set_show_offline(false);
            g.set_show_login(false);
            // Real gap fix, 2026-08-14: push_current_profile_tile was only
            // ever called from finish_session_setup (a fresh login/switch) —
            // auto-login, the path every ordinary launch actually takes,
            // never populated the sidebar's avatar/name row at all. Cheap,
            // local (Config only, no network), safe to call unconditionally.
            let cfg_snapshot = state_cache.lock().unwrap().config.clone();
            crate::profile::push_current_profile_tile(&g, &cfg_snapshot);
            w.invoke_grab_keyboard_focus();
        });

        info!("auto-login: fetching home data + series");
        let (home_data, series_res, sysinfo_res, plugins_res) = tokio::join!(
            fetch_home_data(&client, true),
            client.get_all_series(),
            client.get_system_info(),
            client.get_plugins(),
        );

        // Real bug, code-review 2026-08-16: this join is measured elsewhere
        // in this codebase at ~2.5s on a healthy server (longer on a slow
        // one) — and the UI is already fully interactive before it even
        // starts (push_cached_data/grab_keyboard_focus ran synchronously
        // just above). If a profile switch completes during that window
        // (the sidebar's Switch Profile/Switch Account and the picker are
        // fully reachable), every write below this point would otherwise
        // land the OUTGOING session's data (series list, plugin set, and —
        // worst of all — the WebSocket abort handle, silently overwriting
        // whichever of the two sessions' start_websocket calls lost the
        // race) into the now-current, possibly more-restricted profile.
        // session_current() is the same guard spawn_screen_cache_refresh
        // already uses for this exact class of race; bailing the whole
        // tail here (not just individual writes) is correct since nothing
        // past this point is meaningful once the session that requested it
        // is gone.
        if !session_current(&state, &client) {
            debug!("spawn_auto_login: session changed mid-flight, discarding stale results");
            return;
        }

        let series = series_res.unwrap_or_else(|e| {
            warn!("get_all_series: {:#}", e);
            vec![]
        });
        info!("loaded {} series", series.len());
        let (srv_name, srv_ver) = sysinfo_res
            .map(|i| (i.server_name, i.version))
            .unwrap_or_else(|e| {
                warn!("get_system_info: {:#}", e);
                (String::new(), String::new())
            });
        let plugins: std::collections::HashSet<String> = plugins_res
            .unwrap_or_else(|e| {
                warn!("get_plugins: {:#}", e);
                vec![]
            })
            .into_iter()
            .map(|p| p.name)
            .collect();
        {
            let mut s = state.lock().unwrap();
            s.all_series = series.clone();
            s.available_plugins = plugins;
        }
        // Real gap found 2026-08-11 from a live HTPC log showing zero
        // Bonfire-related activity across two separate launches despite the
        // user having Bonfire installed and genuinely running Fjord on that
        // machine: sync_bonfire_subprofiles (the ONLY thing that discovers
        // additional profiles and can ever make should_show_picker_at_startup
        // return true) was wired into do_login/finish_session_setup and
        // switch_to_profile, but never into THIS function — the ordinary
        // "resume an already-saved session" path every real launch uses
        // after the very first login. On any install using auto-login
        // (the norm — nobody re-types their password every launch), Bonfire
        // sub-profile discovery had genuinely never run again since whichever
        // session first signed in, regardless of server-side Bonfire state.
        // Same best-effort, always-attempted call finish_session_setup
        // already makes — get_plugins()/bonfire_list_profiles() both degrade
        // gracefully when the plugin isn't installed, so this costs nothing
        // extra for the overwhelming majority of servers that don't have it.
        crate::profile::sync_bonfire_subprofiles(
            Arc::clone(&client),
            Arc::clone(&state),
            rt_handle2.clone(),
            window_weak.clone(),
        );
        // Bonfire Phase 6 (2026-09-04) — same "must run on every session-
        // establishment path, not just when something else happens to need
        // it" reasoning as the sync_bonfire_subprofiles fix directly above:
        // FjordState.jellyfin_is_server_admin is never persisted, so an
        // ordinary auto-login resume (the overwhelming majority of real
        // launches) would otherwise leave it stuck at its default `false`
        // for the whole session, hiding the Bonfire Admin Settings row even
        // for a genuine server admin, on every launch after the first.
        crate::spawn_jellyfin_admin_check(
            Arc::clone(&client),
            Arc::clone(&state),
            window_weak.clone(),
            rt_handle2.clone(),
        );
        // Re-resolve the in-library watchlist star now that all_series holds
        // the fresh (not just cached) post-login list (2026-07-20) — one more
        // trigger point alongside push_cached_data's own, for the same
        // "the first resync commonly races ahead of the library data" reason.
        if state.lock().unwrap().seerr_client.is_some() {
            let state_wl = Arc::clone(&state);
            let ww_wl = window_weak.clone();
            rt_handle2.spawn(async move {
                crate::discover::resync_jellyfin_watchlist_stars(state_wl, ww_wl).await;
            });
        }

        save_home_cache(&client.user_id, &home_data);
        save_series_cache(&client.user_id, &series);
        let sections = home_data_sections(&home_data);
        let series2 = series.clone();
        let ww2 = window_weak.clone();
        let ww3 = window_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww2.upgrade() {
                let g = AppState::get(&w);
                g.set_server_name(ss(&srv_name));
                g.set_server_version(ss(&srv_ver));
                // Preserve already-decoded posters (from the cache push moments earlier)
                // across this unconditional startup refresh instead of flashing every
                // row blank — same rationale as ws.rs's delta-sync task (Phase 91/92).
                push_home_data_preserving_posters(&w, &home_data);
                g.set_all_series(refresh_row_preserving_posters(
                    &g.get_all_series(),
                    &series2,
                ));
                g.set_status(ss(""));
                w.invoke_grab_keyboard_focus();
            }
        });
        let client2 = Arc::clone(&client);
        let client3 = Arc::clone(&client);
        let client4 = Arc::clone(&client);
        let client5 = Arc::clone(&client);
        let state3 = Arc::clone(&state);
        let state4 = Arc::clone(&state);
        let state5 = Arc::clone(&state);
        let state6 = Arc::clone(&state);
        let state7 = Arc::clone(&state);
        let state8 = Arc::clone(&state);
        let ws_abort = ws::start_websocket(
            client4,
            Arc::clone(&state4),
            window_weak.clone(),
            rt_handle2.clone(),
        );
        state4.lock().unwrap().ws_abort = Some(ws_abort);
        spawn_poster_loading(client, sections, window_weak, rt_handle2.clone(), state7);
        spawn_series_poster_loading(client2, series, ww3, rt_handle2.clone(), state8);
        rt_handle2.spawn(async move {
            let map = fetch_movie_collections(&client3).await;
            state3.lock().unwrap().movie_collections = map;
        });
        rt_handle2.spawn(async move {
            let (
                movie_ids,
                series_ids,
                collection_ids,
                artist_ids,
                album_ids,
                playlist_ids,
                detail_ids,
            ) = {
                let s = state5.lock().unwrap();
                let m = s.all_movies.iter().map(|i| i.id.clone()).collect();
                let se = s.all_series.iter().map(|i| i.id.clone()).collect();
                let c = s.all_collections.iter().map(|i| i.id.clone()).collect();
                let a = s.all_artists.iter().map(|i| i.id.clone()).collect();
                let al = s.all_albums.iter().map(|i| i.id.clone()).collect();
                let pl = s.all_playlists.iter().map(|i| i.id.clone()).collect();
                // Cast-member portraits are cached in posters/ under PERSON ids,
                // which appear nowhere in the six flat library lists — only in
                // item_detail_cache (person detail entries are keyed by person
                // id, and every cached item's `people` credits reference more).
                // Without these, every 24h cleanup deleted the portraits the
                // image prewarm (Phase 104) and ordinary cast-row browsing had
                // cached — 8,346 files wiped in one observed run.
                let mut det: Vec<String> = Vec::new();
                for (k, item) in s.item_detail_cache.iter() {
                    for p in &item.people {
                        if !p.id.is_empty() {
                            det.push(p.id.clone());
                        }
                    }
                    det.push(k.to_string());
                }
                (m, se, c, a, al, pl, det)
            };
            run_poster_cache_cleanup(
                movie_ids,
                series_ids,
                collection_ids,
                artist_ids,
                album_ids,
                playlist_ids,
                detail_ids,
            )
            .await;
        });
        spawn_screen_cache_refresh(client5, state6, rt_handle2.clone());
    });
}

// ── entry point ───────────────────────────────────────────────────────────────
