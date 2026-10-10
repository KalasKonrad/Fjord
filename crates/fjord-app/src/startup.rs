// ── fjord-app · startup.rs ───────────────────────────────────────────────────
//   Startup / session-start flow (moved from main.rs, 0.5.0 step 3):
//   spawn_jellyfin_admin_check  is the signed-in user a Jellyfin admin (Settings gating); runs
//                        on every session start (session-guarded)
//   push_cached_data     on-disk caches (home/movies/series/collections/artists/albums/playlists
//                        + the six screen-open caches, already parsed off-thread by the caller)
//                        → AppState/FjordState; only after the auth probe found the server;
//                        re-resolves the watchlist stars
//   spawn_screen_cache_refresh  post-login refresh of the screen-open caches: one batched call
//                        for item details, the relationship caches one call per key behind a
//                        semaphore of 2, at most AMBIENT_REFRESH_LIMIT recent keys per cache;
//                        session-guarded
//   wire_screen_cache_save_timer  60 s flush of the screen-open caches (+ person_tmdb_id) to the
//                        live client's user's screen_caches.json; no client → no save
//   spawn_auto_login     probe the saved session (check_auth, 8 s) → display_name backfill →
//                        push_cached_data + profile tile → home data/series/system info,
//                        WebSocket, screen-cache refresh, Bonfire sync, admin check (all
//                        session-guarded after the join), 24 h poster-cache cleanup; 401 →
//                        Login, unreachable → Offline.
//                        Re-invoked by AppState.retry-connection
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_api::JellyfinClient;

use crate::MainWindow;
use crate::config::{FjordState, ScreenCachesFile};

// Is the signed-in user a Jellyfin server admin (gates the Bonfire Admin Settings row)?
// Not persisted, so it runs on every session start: finish_session_setup (auth.rs) and
// spawn_auto_login (this file).
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
        // session_current (Arc::ptr_eq), not a user_id comparison: signing out and back into the
        // same account gives the same user_id, so a stale result would pass that check.
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
// Push on-disk caches into AppState/FjordState for instant display. Only called once
// spawn_auto_login's probe has confirmed the server is reachable, so an offline start
// doesn't look like a normal quiet one.
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
    // Screen-open caches: a reopened screen skips the network fetch even on the first open
    // after launch. Freshness comes from the post-login refresh (spawn_screen_cache_refresh) and
    // WebSocket invalidation (ws.rs). The caller reads and parses the file (up to ~1.3 MB after
    // a prewarm) off-thread — this runs on the UI thread.
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
    // Re-resolve the in-library watchlist star now that all_movies/all_series are filled from
    // cache — the watchlist fetch's own resync usually runs before them and matches nothing.
    // Cheap no-op without a Seerr connection.
    if state.lock().unwrap().seerr_client.is_some() {
        let state_wl = Arc::clone(state);
        let ww_wl = window.as_weak();
        rt_handle.spawn(async move {
            crate::discover::resync_jellyfin_watchlist_stars(state_wl, ww_wl).await;
        });
    }
}

/// Cap on how many entries the ambient per-login sweep revalidates per cache, however large
/// the opt-in prewarm (prewarm.rs) has grown the cache — otherwise every login would repeat
/// the prewarm's full request volume.
pub(crate) const AMBIENT_REFRESH_LIMIT: usize = 40;

/// Post-login background refresh of the six screen-open caches; never blocks anything visible
/// (the persisted caches are already showing). item_detail_cache: one batched
/// get_items_by_ids_detailed call. The 5 relationship caches have no batch endpoint: one call
/// per key, sharing a semaphore of 2 so they trickle out. A 404 (item deleted) removes the
/// entry; any other error keeps the stale one. Session-guarded.
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

/// Flushes the six screen-open caches to disk every 60 s. Writes unconditionally (small,
/// cheap file) instead of tracking a dirty flag.
pub(crate) fn wire_screen_cache_save_timer(
    state: Arc<Mutex<FjordState>>,
    rt_handle: tokio::runtime::Handle,
) -> slint::Timer {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    let timer = slint::Timer::default();
    // 60 s: the cache clone is O(1) copy-on-write (BoundedCache wraps its storage in Arc).
    timer.start(
        slint::TimerMode::Repeated,
        std::time::Duration::from_secs(60),
        move || {
            let state2 = Arc::clone(&state);
            rt_handle.spawn(async move {
                // The live client's user_id, not config.active(): active() falls back to
                // profiles.first() when active_profile_id matches nothing (sign-out clears it),
                // which wrote one session's caches into another account's file. No client → no
                // save.
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

        // Backfill a missing display_name (otherwise the raw user_id is shown): auto-login never
        // sees a login response, which is where do_login gets the name. Only when needed.
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
            // The sidebar profile tile (finish_session_setup does this for login/switch;
            // auto-login is the ordinary launch path). Local, cheap.
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

        // The UI is interactive during the ~2.5 s join above: if a profile switch completed
        // meanwhile, everything below (series, plugins, the WebSocket abort handle) would land in
        // the new session. Bail out entirely.
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
        // Bonfire sub-profile discovery on the ordinary auto-login path too (it is the only thing
        // that can make the startup picker appear). Degrades to a no-op without the plugin.
        crate::profile::sync_bonfire_subprofiles(
            Arc::clone(&client),
            Arc::clone(&state),
            rt_handle2.clone(),
            window_weak.clone(),
        );
        // Admin flag on every session start (not persisted) — see spawn_jellyfin_admin_check.
        crate::spawn_jellyfin_admin_check(
            Arc::clone(&client),
            Arc::clone(&state),
            window_weak.clone(),
            rt_handle2.clone(),
        );
        // Re-resolve the watchlist star now that all_series holds the fresh list (see
        // push_cached_data).
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
                // Cast-member portraits are cached in posters/ under PERSON ids, which only
                // appear in item_detail_cache (person entries and every item's `people`).
                // Without these the 24 h cleanup deleted every cached portrait.
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
