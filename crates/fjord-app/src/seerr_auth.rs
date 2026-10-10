// ── fjord-app · seerr_auth.rs ────────────────────────────────────────────────
//   build_seerr_client   Config.seerr_* → SeerrClient when enabled and a cookie/key is present
//   connected_label      "Connected via X" for the Settings → Integrations row
//   push_seerr_status    seerr-connected / -connected-label / -unencrypted from a Config snapshot
//                        (every successful connect calls auth::note_if_http_fallback first)
//   spawn_refresh_seerr_version  GET /status → AppState.seerr-version (startup + every connect)
//   resolve_seerr_url    HTTPS-then-HTTP for a possibly-schemeless URL via the get_status probe
//                        (only on auth::is_connectivity_failure); its StatusInfo supplies the
//                        version too
//   existing_connect_seerr_zones  ConnectSeerrScreen's D-pad zones (-1 ✕, 0 URL, 1 tabs, 2+ by
//                        method/polling), dispatched inline in keys.rs's show_connect_seerr tier
//   clear_connection / commit_connection  connection-scoped resets (Calendar/filter caches,
//                        person caches, the 3 watchlist models); commit also starts
//                        ensure_discover_watchlist
//   wire_connect_seerr   ConnectSeerrScreen: the 4 auth methods (API key, Jellyfin login, Quick
//                        Connect, local account) + open/disconnect; every URL goes through
//                        resolve_seerr_url; open resets Quick Connect state, the zone and the
//                        on-screen keyboard; the Quick Connect poll runs one probe at a time and
//                        stops with an error after repeated resolve failures
//   spawn_seerr_settings_fetch  the Integrations dropdowns' options and current values (regions,
//                        languages, discover region) + seerr user id / admin / blocklist bits,
//                        in one round
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_seerr::{SeerrAuth, SeerrClient, StatusInfo};
use slint::{ComponentHandle, Global, Weak};
use url::Url;

use crate::config::{FjordState, save_config};
use crate::{AppState, MainWindow, show_toast};

pub(crate) fn build_seerr_client(c: &crate::config::ProfileSettings) -> Option<Arc<SeerrClient>> {
    if !c.seerr_enabled || c.seerr_url.is_empty() {
        return None;
    }
    let base_url = Url::parse(&c.seerr_url).ok()?;
    let auth = match c.seerr_auth_method.as_str() {
        "apikey" if !c.seerr_api_key.is_empty() => SeerrAuth::ApiKey(c.seerr_api_key.clone()),
        "jellyfin" | "quickconnect" | "local" if !c.seerr_session_cookie.is_empty() => {
            SeerrAuth::Session(c.seerr_session_cookie.clone())
        }
        _ => return None,
    };
    SeerrClient::new(base_url, auth).ok().map(Arc::new)
}

/// Fetches Seerr's own version (GET /status, unauthenticated) and pushes it
/// to `AppState.seerr-version`. Called after every successful connect
/// (inline with that auth flow, see `commit_connection` call sites below)
/// and once at startup if a saved connection already exists — mirrors how
/// `server-name`/`server-version` are fetched fresh each session rather than
/// persisted, since it's cheap and this way it can never go stale.
pub(crate) fn spawn_refresh_seerr_version(
    base_url: Url,
    ww: Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
) {
    rt.spawn(async move {
        let Ok(status) = SeerrClient::get_status(&base_url).await else {
            return;
        };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_seerr_version(status.version.as_str().into());
            }
        });
    });
}

/// Resolves a possibly-schemeless Seerr URL like Login does for Jellyfin
/// (auth::candidate_server_urls): tries HTTPS then HTTP with the cheap unauthenticated
/// get_status probe, moving on only on a connectivity failure — a candidate that answers
/// (even non-2xx) is final. The StatusInfo it returns doubles as the version string the
/// connect closures need afterwards.
async fn resolve_seerr_url(url: &str) -> anyhow::Result<(Url, StatusInfo)> {
    let candidates = crate::auth::candidate_server_urls(url);
    let mut last_err: Option<anyhow::Error> = None;
    for (i, candidate) in candidates.iter().enumerate() {
        let base_url = Url::parse(candidate)?;
        match SeerrClient::get_status(&base_url).await {
            Ok(status) => return Ok((base_url, status)),
            Err(e) => {
                // auth::is_connectivity_failure, not `status().is_none()` (which also matches a
                // JSON decode failure on a reachable server).
                let is_connectivity = crate::auth::is_connectivity_failure(&e);
                let is_last = i + 1 == candidates.len();
                if is_connectivity && !is_last {
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no server address given")))
}

/// The ordered zone list for the current ConnectSeerrScreen tab/polling state (gaps allowed,
/// recomputed live — like profile_edit::existing_profile_edit_zones). -1 = close ✕ (Up from
/// zone 0), 0 = url-input, 1 = tab row, 2+ depend on connect-seerr-method; a polling Quick
/// Connect tab has nothing focusable below the URL. Order must match the visual top-to-bottom
/// order (next_zone/prev_zone are positional).
pub(crate) fn existing_connect_seerr_zones(g: &AppState) -> Vec<i32> {
    let mut zones = vec![-1, 0, 1];
    match g.get_connect_seerr_method() {
        0 => zones.extend([2, 3]),    // API key: key-input, submit
        1 => zones.extend([2, 3, 4]), // Jellyfin: username, password, submit
        2 => {
            if !g.get_connect_seerr_qc_polling() {
                zones.push(2);
            } // "Get Code" — nothing while polling
        }
        3 => zones.extend([2, 3, 4]), // Local account: email, password, submit
        _ => {}
    }
    zones
}

pub(crate) fn connected_label(method: &str) -> &'static str {
    match method {
        "apikey" => "Connected via API key",
        "jellyfin" => "Connected via Jellyfin login",
        "quickconnect" => "Connected via Jellyfin Quick Connect",
        "local" => "Connected via local account",
        _ => "Not connected",
    }
}

pub(crate) fn push_seerr_status(g: &AppState<'_>, c: &crate::config::ProfileSettings) {
    let connected = c.seerr_enabled
        && !c.seerr_url.is_empty()
        && (!c.seerr_api_key.is_empty() || !c.seerr_session_cookie.is_empty());
    g.set_seerr_connected(connected);
    g.set_seerr_unencrypted(
        connected
            && c.seerr_url
                .trim()
                .to_ascii_lowercase()
                .starts_with("http://"),
    );
    g.set_seerr_connected_label(
        if connected {
            connected_label(&c.seerr_auth_method)
        } else {
            "Not connected"
        }
        .into(),
    );
}

/// Clears the connection (session-auth 401, or explicit Disconnect) and
/// persists it — does NOT touch `seerr_enabled` (see the app_state.slint doc
/// comment: enabled and connected are independent). `pub(crate)` so
/// discover.rs's 401 handling can reuse it rather than re-deriving the same
/// clear-and-persist steps.
pub(crate) fn clear_connection(state: &Arc<Mutex<FjordState>>, ww: &Weak<MainWindow>) {
    let mut s = state.lock().unwrap();
    {
        let p = s.config.active_mut();
        p.seerr_auth_method.clear();
        p.seerr_api_key.clear();
        p.seerr_session_cookie.clear();
    }
    s.seerr_client = None;
    s.discover_landing_fetched = false;
    s.discover_filter_options_fetched = false;
    s.discover_known_requests.clear();
    s.discover_watchlist_ids.clear();
    s.jellyfin_watchlist_ids.clear();
    s.discover_watchlist_fetched = false;
    s.discover_calendar_entries.clear();
    s.seerr_discover_region = None;
    s.seerr_genres_movie.clear();
    s.seerr_genres_tv.clear();
    s.seerr_providers_movie.clear();
    s.seerr_providers_tv.clear();
    s.seerr_streaming_region = None;
    s.seerr_regions.clear();
    s.seerr_user_id = None;
    s.seerr_is_admin = false;
    s.seerr_can_manage_blocklist = false;
    s.seerr_admin_last_refresh = None;
    // Request/watchlist-patched results from the old connection would show stale pill state.
    s.person_tmdb_id_cache.clear();
    s.person_other_work_cache.clear();
    let cfg = s.config.clone();
    let profile = cfg.active().clone();
    drop(s);
    save_config(&cfg);
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        push_seerr_status(&g, &profile);
        g.set_seerr_is_admin(false);
        g.set_seerr_can_manage_blocklist(false);
        // Clear the 3 Slint-side watchlist models too, or they keep the old connection's content.
        g.set_discover_watchlist_mixed(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_discover_watchlist_movies(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_discover_watchlist_tv(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        // Dashboard Coming Up rows (2026-08-02) — same reasoning, same 3
        // Slint-side models.
        g.set_discover_coming_up_mixed(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_discover_coming_up_movies(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_discover_coming_up_tv(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
    }
}

fn commit_connection(
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    base_url: &Url,
    method: &'static str,
    auth: SeerrAuth,
    version: Option<String>,
    rt: &tokio::runtime::Handle,
) {
    let mut s = state.lock().unwrap();
    {
        let p = s.config.active_mut();
        p.seerr_url = base_url.to_string();
        p.seerr_auth_method = method.into();
        match &auth {
            SeerrAuth::ApiKey(k) => {
                p.seerr_api_key = k.clone();
                p.seerr_session_cookie.clear();
            }
            SeerrAuth::Session(c) => {
                p.seerr_session_cookie = c.clone();
                p.seerr_api_key.clear();
            }
        }
    }
    let Ok(client) = SeerrClient::new(base_url.clone(), auth) else {
        drop(s);
        return;
    };
    let client = Arc::new(client);
    s.seerr_client = Some(Arc::clone(&client));
    s.discover_landing_fetched = false; // a (re)connect may point at a different server/catalog
    s.discover_filter_options_fetched = false;
    s.discover_known_requests.clear();
    s.discover_watchlist_ids.clear();
    s.jellyfin_watchlist_ids.clear();
    s.discover_watchlist_fetched = false;
    s.discover_calendar_entries.clear();
    s.seerr_discover_region = None;
    s.seerr_genres_movie.clear();
    s.seerr_genres_tv.clear();
    s.seerr_providers_movie.clear();
    s.seerr_providers_tv.clear();
    s.seerr_streaming_region = None;
    s.seerr_regions.clear();
    s.seerr_user_id = None; // re-resolved by spawn_seerr_settings_fetch below
    s.seerr_is_admin = false;
    s.seerr_can_manage_blocklist = false; // re-resolved by spawn_seerr_settings_fetch below
    s.seerr_admin_last_refresh = None;
    // Deep Seerr integration (2026-07-29) — a (re)connect may point at a
    // different server/catalog, same reasoning as the other clears above.
    s.person_tmdb_id_cache.clear();
    s.person_other_work_cache.clear();
    let cfg = s.config.clone();
    let profile = cfg.active().clone();
    drop(s);
    save_config(&cfg);
    crate::spawn_seerr_settings_fetch(client, Arc::clone(state), ww.clone(), rt.clone());
    // Home/Movies/TV dashboard Watchlist rows (2026-07-20) — the guard
    // reset above (discover_watchlist_fetched = false) means this actually
    // re-fetches on a fresh connect/reconnect, not just a no-op call.
    crate::discover::ensure_discover_watchlist(Arc::clone(state), ww.clone(), rt.clone());
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        push_seerr_status(&g, &profile);
        if let Some(v) = version {
            g.set_seerr_version(v.as_str().into());
        }
        // A fresh connect may be another server: clear the old watchlist content now rather than
        // when ensure_discover_watchlist's fetch (above) lands.
        g.set_discover_watchlist_mixed(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_discover_watchlist_movies(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_discover_watchlist_tv(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        // Dashboard Coming Up rows (2026-08-02) — same reasoning, same 3
        // Slint-side models.
        g.set_discover_coming_up_mixed(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_discover_coming_up_movies(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_discover_coming_up_tv(crate::items_to_model(
            &[],
            &std::collections::HashSet::new(),
        ));
        g.set_show_connect_seerr(false);
        // ConnectSeerrScreen's LineEdits hold real Slint keyboard focus while
        // typing — closing the screen doesn't return it to the app's own
        // global FocusScope on its own, which silently dead-ends ALL keyboard
        // navigation afterward (same class of bug as the post-login
        // grab-keyboard-focus calls elsewhere in main.rs; found live after
        // signing in to Seerr left Settings' keyboard nav completely dead).
        w.invoke_grab_keyboard_focus();
    }
}

pub(crate) fn wire_connect_seerr(
    window: &MainWindow,
    state: Arc<Mutex<FjordState>>,
    rt: tokio::runtime::Handle,
) {
    let g = AppState::get(window);

    g.on_open_connect_seerr({
        let ww = window.as_weak();
        move || {
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_connect_seerr_error(slint::SharedString::new());
                g.set_connect_seerr_busy(false);
                // Every open starts clean — reopening mid-Quick-Connect used to show the stale
                // "waiting for approval" view against an expired secret.
                g.set_connect_seerr_qc_polling(false);
                g.set_connect_seerr_qc_code(slint::SharedString::new());
                g.set_connect_seerr_qc_secret(slint::SharedString::new());
                // Reset the zone and keyboard state too: a stale zone value never re-fires the
                // zone→focus trackers (`changed` needs a real transition), which left the D-pad
                // dead.
                g.set_connect_seerr_zone(0);
                g.set_show_onscreen_keyboard(false);
                g.set_onscreen_keyboard_target(slint::SharedString::new());
                g.set_onscreen_keyboard_cursor(0);
                g.set_show_connect_seerr(true);
            }
        }
    });

    g.on_seerr_disconnect({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let state = Arc::clone(&state);
            let ww = ww.clone();
            let client = state.lock().unwrap().seerr_client.clone();
            rt.spawn(async move {
                // Local state is cleared either way — a failed server-side
                // logout shouldn't leave the user stuck "connected" in the UI
                // to a session they've already asked to drop.
                let logout_err = if let Some(c) = client {
                    c.logout().await.err().map(|e| e.to_string())
                } else {
                    None
                };
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    clear_connection(&state, &ww2);
                    if let Some(e) = logout_err {
                        show_toast(ww2, format!("Seerr sign-out on the server failed ({e}), disconnected locally anyway"));
                    }
                });
            });
        }
    });

    // ── API key ──────────────────────────────────────────────────────────
    g.on_connect_seerr_api_key({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |url, key| {
            let state = Arc::clone(&state);
            let ww2 = ww.clone();
            let key = key.to_string();
            set_busy(&ww, true);
            rt.spawn(async move {
                let (base_url, version) = match resolve_seerr_url(&url).await {
                    Ok((u, status)) => (u, Some(status.version)),
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            set_error(&ww2, &format!("Couldn't reach that server: {e}"));
                        });
                        return;
                    }
                };
                // No dedicated "verify this key" endpoint — a bad key fails on
                // first authenticated use, so probe with a cheap search call.
                let client = SeerrClient::new(base_url.clone(), SeerrAuth::ApiKey(key.clone()));
                let result = match client {
                    Ok(c) => c.search("test", 1).await.map(|_| ()),
                    Err(e) => Err(e),
                };
                let rt_inner = tokio::runtime::Handle::current();
                let _ = slint::invoke_from_event_loop(move || {
                    set_busy(&ww2, false);
                    match result {
                        Ok(()) => {
                            crate::auth::note_if_http_fallback(&ww2, "Seerr", &url, &base_url);
                            commit_connection(
                                &state,
                                &ww2,
                                &base_url,
                                "apikey",
                                SeerrAuth::ApiKey(key),
                                version,
                                &rt_inner,
                            )
                        }
                        Err(e) => set_error(&ww2, &format!("Couldn't verify that key: {e}")),
                    }
                });
            });
        }
    });

    // ── Jellyfin username/password ──────────────────────────────────────────
    g.on_connect_seerr_jellyfin({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |url, username, password| {
            let state = Arc::clone(&state);
            let ww2 = ww.clone();
            let (username, password) = (username.to_string(), password.to_string());
            set_busy(&ww, true);
            rt.spawn(async move {
                let (base_url, version) = match resolve_seerr_url(&url).await {
                    Ok((u, status)) => (u, Some(status.version)),
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            set_error(&ww2, &format!("Couldn't reach that server: {e}"));
                        });
                        return;
                    }
                };
                let result = SeerrClient::sign_in_jellyfin(&base_url, &username, &password).await;
                let rt_inner = tokio::runtime::Handle::current();
                let _ = slint::invoke_from_event_loop(move || {
                    set_busy(&ww2, false);
                    match result {
                        Ok((auth, _user)) => {
                            crate::auth::note_if_http_fallback(&ww2, "Seerr", &url, &base_url);
                            commit_connection(
                                &state, &ww2, &base_url, "jellyfin", auth, version, &rt_inner,
                            )
                        }
                        Err(e) => set_error(&ww2, &format!("Sign-in failed: {e}")),
                    }
                });
            });
        }
    });

    // ── Local Seerr account ──────────────────────────────────────────────
    g.on_connect_seerr_local({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |url, email, password| {
            let state = Arc::clone(&state);
            let ww2 = ww.clone();
            let (email, password) = (email.to_string(), password.to_string());
            set_busy(&ww, true);
            rt.spawn(async move {
                let (base_url, version) = match resolve_seerr_url(&url).await {
                    Ok((u, status)) => (u, Some(status.version)),
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            set_error(&ww2, &format!("Couldn't reach that server: {e}"));
                        });
                        return;
                    }
                };
                let result = SeerrClient::sign_in_local(&base_url, &email, &password).await;
                let rt_inner = tokio::runtime::Handle::current();
                let _ = slint::invoke_from_event_loop(move || {
                    set_busy(&ww2, false);
                    match result {
                        Ok((auth, _user)) => {
                            crate::auth::note_if_http_fallback(&ww2, "Seerr", &url, &base_url);
                            commit_connection(
                                &state, &ww2, &base_url, "local", auth, version, &rt_inner,
                            )
                        }
                        Err(e) => set_error(&ww2, &format!("Sign-in failed: {e}")),
                    }
                });
            });
        }
    });

    // ── Jellyfin Quick Connect ───────────────────────────────────────────
    g.on_connect_seerr_quickconnect_start({
        let ww = window.as_weak();
        let rt = rt.clone();
        move |url| {
            let ww2 = ww.clone();
            set_busy(&ww, true);
            rt.spawn(async move {
                let base_url = match resolve_seerr_url(&url).await {
                    // The version isn't needed here — Quick Connect only ever
                    // commits from the `poll` closure below, once actually
                    // authenticated, not from this initiate step.
                    Ok((u, _status)) => u,
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            set_error(&ww2, &format!("Couldn't reach that server: {e}"));
                        });
                        return;
                    }
                };
                let result = SeerrClient::quick_connect_initiate(&base_url).await;
                let _ = slint::invoke_from_event_loop(move || {
                    set_busy(&ww2, false);
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        match result {
                            Ok(qc) => {
                                g.set_connect_seerr_qc_code(qc.code.into());
                                g.set_connect_seerr_qc_secret(qc.secret.into());
                                g.set_connect_seerr_qc_polling(true);
                            }
                            Err(e) => {
                                set_error(&ww2, &format!("Couldn't start Quick Connect: {e}"))
                            }
                        }
                    }
                });
            });
        }
    });

    g.on_connect_seerr_quickconnect_poll({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        // Captured once (the callback is registered once). resolve_seerr_url's fallback can outlast
        // the 2 s poll Timer against a hung server: one probe at a time (poll_in_flight), and a
        // resolve failure ends polling with an error instead of waiting forever.
        let poll_in_flight = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let resolve_failures = Arc::new(std::sync::atomic::AtomicU32::new(0));
        move |url, secret| {
            use std::sync::atomic::Ordering;
            if poll_in_flight.swap(true, Ordering::SeqCst) {
                // A previous tick's probe (or the check/authenticate call
                // after it) is still in flight — skip this tick rather than
                // starting a second, overlapping resolve_seerr_url against
                // the same candidate.
                return;
            }
            let state = Arc::clone(&state);
            let ww2 = ww.clone();
            let secret = secret.to_string();
            let poll_in_flight = Arc::clone(&poll_in_flight);
            let resolve_failures = Arc::clone(&resolve_failures);
            rt.spawn(async move {
                let (base_url, version) = match resolve_seerr_url(&url).await {
                    Ok((u, status)) => {
                        resolve_failures.store(0, Ordering::Relaxed);
                        (u, Some(status.version))
                    }
                    Err(e) => {
                        // Bounded, not swallowed forever: give up and
                        // surface a real error after enough consecutive
                        // failures that this genuinely looks like a
                        // sustained outage rather than one transient blip
                        // (~20s at the 2s poll interval), rather than
                        // spinning silently for as long as the screen
                        // stays open.
                        const MAX_CONSECUTIVE_RESOLVE_FAILURES: u32 = 10;
                        let failures = resolve_failures.fetch_add(1, Ordering::Relaxed) + 1;
                        if failures >= MAX_CONSECUTIVE_RESOLVE_FAILURES {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww2.upgrade() {
                                    AppState::get(&w).set_connect_seerr_qc_polling(false);
                                }
                                set_error(
                                    &ww2,
                                    &format!("Lost the connection while waiting for approval: {e}"),
                                );
                            });
                        }
                        poll_in_flight.store(false, Ordering::SeqCst);
                        return;
                    }
                };
                match SeerrClient::quick_connect_check(&base_url, &secret).await {
                    Ok(true) => {
                        let auth_result =
                            SeerrClient::quick_connect_authenticate(&base_url, &secret).await;
                        let rt_inner = tokio::runtime::Handle::current();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_connect_seerr_qc_polling(false);
                            }
                            match auth_result {
                                Ok((auth, _user)) => {
                                    crate::auth::note_if_http_fallback(
                                        &ww2, "Seerr", &url, &base_url,
                                    );
                                    commit_connection(
                                        &state,
                                        &ww2,
                                        &base_url,
                                        "quickconnect",
                                        auth,
                                        version,
                                        &rt_inner,
                                    )
                                }
                                Err(e) => set_error(&ww2, &format!("Quick Connect failed: {e}")),
                            }
                        });
                    }
                    Ok(false) => {} // still waiting — caller polls again on a timer
                    Err(e) => {
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_connect_seerr_qc_polling(false);
                            }
                            set_error(&ww2, &format!("{e} — try again"));
                        });
                    }
                }
                poll_in_flight.store(false, Ordering::SeqCst);
            });
        }
    });
}

fn set_busy(ww: &Weak<MainWindow>, busy: bool) {
    if let Some(w) = ww.upgrade() {
        AppState::get(&w).set_connect_seerr_busy(busy);
    }
}

// Errors here are setup-time and stay on-screen (ConnectSeerrScreen's own
// error text), not a toast — matches how LoginScreen surfaces auth failures.
fn set_error(ww: &Weak<MainWindow>, msg: &str) {
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_connect_seerr_busy(false);
        g.set_connect_seerr_error(msg.into());
    }
}

// ── Seerr user-settings discovery (region + display language + discover language) ──
// Like fetch_audio_devices/fetch_system_fonts, but from Seerr, once a connection exists — at
// startup (saved connection) and after a fresh connect (commit_connection), like
// spawn_refresh_seerr_version. Fills the Integrations dropdowns (streaming region, display
// language, discover language, discover region) in one round: regions/languages in
// parallel, one get_current_user + get_user_settings for all the current values.
pub(crate) fn spawn_seerr_settings_fetch(
    client: Arc<fjord_seerr::SeerrClient>,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    rt.spawn(async move {
        let (regions_res, languages_res) =
            tokio::join!(client.get_watch_provider_regions(), client.get_languages());
        let mut region_pairs: Vec<(String, String)> = regions_res
            .unwrap_or_default()
            .iter()
            .map(|r| (r.iso_3166_1.clone(), format!("{} ({})", r.english_name, r.iso_3166_1)))
            .collect();
        region_pairs.sort_by(|a, b| a.1.cmp(&b.1));
        let mut language_pairs: Vec<(String, String)> = languages_res
            .unwrap_or_default()
            .iter()
            .map(|l| (l.iso_639_1.clone(), format!("{} ({})", l.english_name, l.iso_639_1)))
            .collect();
        language_pairs.sort_by(|a, b| a.1.cmp(&b.1));

        // Mirrors resolve_streaming_region's own read path (discover.rs).
        // Also captures the connected account's own id + MANAGE_REQUESTS
        // permission bit here (piggybacking on this same /auth/me call,
        // rather than a second one) — the Discover context menu's
        // Edit/Cancel ownership check and Approve/Decline gate.
        let current_user = client.get_current_user().await.ok();
        let (user_id, is_admin) =
            current_user.as_ref().map(|u| (Some(u.id), u.can_manage_requests())).unwrap_or((None, false));
        // MANAGE_BLOCKLIST is its own permission bit (see can_manage_blocklist) — read from the
        // same user, no extra request.
        let can_manage_blocklist = current_user.as_ref().is_some_and(|u| u.can_manage_blocklist());
        debug!(
            "seerr: current user id={user_id:?} permissions={:?} can_manage_requests={is_admin} can_manage_blocklist={can_manage_blocklist}",
            current_user.as_ref().map(|u| u.permissions),
        );
        let settings = async {
            let user = current_user?;
            client.get_user_settings(user.id).await.ok()
        }
        .await;

        let current_region_code = settings
            .as_ref()
            .and_then(|s| s.streaming_region.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "US".to_string());
        let current_region_desc = region_pairs
            .iter()
            .find(|(code, _)| code == &current_region_code)
            .map(|(_, desc)| desc.clone())
            .unwrap_or_else(|| current_region_code.clone());

        // Discover Region — a different setting from the streaming region (see
        // resolve_discover_region), resolved from the same region list.
        let current_discover_region_code = settings
            .as_ref()
            .and_then(|s| s.discover_region.clone())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "US".to_string());
        let current_discover_region_desc = region_pairs
            .iter()
            .find(|(code, _)| code == &current_discover_region_code)
            .map(|(_, desc)| desc.clone())
            .unwrap_or_else(|| current_discover_region_code.clone());

        // "" = "Default (English)" (Seerr's admin-configured fallback, see UserGeneralSettings); a
        // fresh account's GET can omit locale.
        let current_locale_code = settings.as_ref().and_then(|s| s.locale.clone()).unwrap_or_default();
        let current_locale_desc = if current_locale_code.is_empty() {
            "Default (English)".to_string()
        } else {
            language_pairs
                .iter()
                .find(|(code, _)| code == &current_locale_code)
                .map(|(_, desc)| desc.clone())
                .unwrap_or_else(|| current_locale_code.clone())
        };

        // "all" (literal) = "Default (All Languages)": an empty string would fall through to the
        // admin's originalLanguage (discover.ts createTmdbWithRegionLanguage), not "no filter".
        let current_lang_code = settings
            .as_ref()
            .and_then(|s| s.original_language.clone())
            .filter(|s| s != "all" && !s.is_empty())
            .unwrap_or_else(|| "all".to_string());
        let current_lang_desc = if current_lang_code == "all" {
            "Default (All Languages)".to_string()
        } else {
            language_pairs
                .iter()
                .find(|(code, _)| code == &current_lang_code)
                .map(|(_, desc)| desc.clone())
                .unwrap_or_else(|| current_lang_code.clone())
        };

        {
            let mut s = state.lock().unwrap();
            s.seerr_regions = region_pairs.clone();
            s.seerr_streaming_region = Some(current_region_code);
            s.seerr_discover_region = Some(current_discover_region_code);
            s.seerr_languages = language_pairs.clone();
            s.seerr_locale = Some(current_locale_code);
            s.seerr_original_language = Some(current_lang_code);
            s.seerr_user_id = user_id;
            s.seerr_is_admin = is_admin;
            s.seerr_can_manage_blocklist = can_manage_blocklist;
        }

        let region_display: Vec<slint::SharedString> =
            region_pairs.iter().map(|(_, d)| slint::SharedString::from(d.as_str())).collect();
        let mut language_display: Vec<slint::SharedString> =
            language_pairs.iter().map(|(_, d)| slint::SharedString::from(d.as_str())).collect();
        // Each language dropdown gets its own "Default" row — the labels differ, as in Seerr's web
        // UI.
        let mut discover_lang_display = language_display.clone();
        language_display.insert(0, slint::SharedString::from("Default (English)"));
        discover_lang_display.insert(0, slint::SharedString::from("Default (All Languages)"));

        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_settings_streaming_region_display(slint::ModelRc::new(slint::VecModel::from(region_display)));
                g.set_settings_streaming_region_desc(slint::SharedString::from(current_region_desc.as_str()));
                g.set_settings_discover_region_desc(slint::SharedString::from(current_discover_region_desc.as_str()));
                g.set_settings_display_language_display(slint::ModelRc::new(slint::VecModel::from(language_display)));
                g.set_settings_display_language_desc(slint::SharedString::from(current_locale_desc.as_str()));
                g.set_settings_discover_language_display(slint::ModelRc::new(slint::VecModel::from(discover_lang_display)));
                g.set_settings_discover_language_desc(slint::SharedString::from(current_lang_desc.as_str()));
                g.set_seerr_is_admin(is_admin);
                g.set_seerr_can_manage_blocklist(can_manage_blocklist);
            }
        });
    });
}
