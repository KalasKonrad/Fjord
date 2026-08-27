// ── fjord-app · seerr_auth.rs ────────────────────────────────────────────────
//   build_seerr_client   Config.seerr_* -> SeerrClient, if enabled + a valid
//                        cookie/key is present (used at startup and after
//                        every successful ConnectSeerrScreen flow)
//   connected_label      Config.seerr_auth_method -> human-readable "Connected
//                        via X" string for the Settings → Integrations row
//   push_seerr_status    pushes seerr-connected / seerr-connected-label to
//                        AppState from a Config snapshot
//   spawn_refresh_seerr_version  GET /status (unauthenticated) -> AppState.seerr-version;
//                        called after every successful connect and once at startup
//   resolve_seerr_url    HTTPS-then-HTTP scheme-fallback for a raw, possibly-schemeless
//                        server URL (2026-08-23, mirroring auth.rs::authenticate_with_fallback
//                        for Jellyfin's own Login screen) — reuses the cheap, unauthenticated
//                        get_status probe as both the reachability check AND the version-
//                        string fetch every auth closure below already needs, so 4 of the 5
//                        no longer call get_status a second time; classifies a connectivity
//                        failure via the shared auth::is_connectivity_failure (2026-08-26,
//                        code review — was a bare status().is_none() check, which also
//                        matched a JSON-decode failure on a genuinely reachable server)
//   Quick Connect poll   on_connect_seerr_quickconnect_poll gained an in-flight AtomicBool
//                        guard + a bounded consecutive-resolve-failure AtomicU32 counter
//                        (2026-08-26, code review) — the original `Err(_) => return` on a
//                        resolve failure silently swallowed a mid-poll outage forever (no
//                        error, qc-polling never reset) while also piling up an overlapping
//                        probe every 2s tick against a server that was never going to answer
//   existing_connect_seerr_zones  ConnectSeerrScreen's D-pad zone list, recomputed live off
//                        connect-seerr-method/-qc-polling (2026-08-23 — this screen had zero
//                        keyboard nav before), dispatched inline in keys.rs's show_connect_seerr
//                        tier (mirrors login-zone's inline shape, not ProfileEditScreen's
//                        delegate-to-a-separate-function one)
//   wire_connect_seerr   registers all ConnectSeerrScreen callbacks: the 4
//                        auth methods (API key, Jellyfin login, Quick Connect,
//                        local account) plus open/disconnect; on_open_connect_seerr
//                        also resets Quick Connect's polling/code/secret on every
//                        open (real bug fixed 2026-07-18: these were only ever
//                        cleared by the poll callback's own success/error arms, so
//                        closing the screen mid-flow and reopening re-showed a
//                        stale "waiting for approval" view against an expired secret)
//                        and, since 2026-08-26 (code review), also connect-seerr-zone
//                        + the on-screen keyboard's own 3 properties — a stale
//                        non-zero zone surviving a close/reopen left the D-pad
//                        completely dead on the next open, since Slint's `changed`
//                        never re-fires when the value didn't actually change;
//                        all 5 auth closures resolve their typed server URL via
//                        resolve_seerr_url instead of a bare Url::parse (2026-08-23)
//   clear_connection     also resets the 3 discover-watchlist-mixed/movies/tv AppState
//                        models to empty (2026-07-20, Watchlist row) — same connection-
//                        scoped cache cleanup this function already does for the
//                        Calendar/filter caches; also clears person_tmdb_id_cache/
//                        person_other_work_cache (2026-07-29, Deep Seerr integration)
//   commit_connection    also calls discover::ensure_discover_watchlist right after a
//                        fresh connect (2026-07-20, same site spawn_seerr_settings_fetch
//                        is already called from) and resets the same 3 watchlist models
//                        before the new connection's own fetch populates them; also
//                        clears person_tmdb_id_cache/person_other_work_cache (2026-07-29,
//                        Deep Seerr integration) — a (re)connect may point at a different
//                        server/catalog, same reasoning as the caches above
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_seerr::{SeerrAuth, SeerrClient, StatusInfo};
use slint::{ComponentHandle, Global, Weak};
use url::Url;

use crate::config::{save_config, FjordState};
use crate::{show_toast, AppState, MainWindow};

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
pub(crate) fn spawn_refresh_seerr_version(base_url: Url, ww: Weak<MainWindow>, rt: &tokio::runtime::Handle) {
    rt.spawn(async move {
        let Ok(status) = SeerrClient::get_status(&base_url).await else { return };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_seerr_version(status.version.as_str().into());
            }
        });
    });
}

/// Resolves a raw, possibly-schemeless Seerr server URL the same way
/// Login's own `auth::candidate_server_urls`/`authenticate_with_fallback`
/// already do for Jellyfin (2026-08-23 — the identical bare-host-fails-
/// outright gap existed here too: every one of `wire_connect_seerr`'s 5
/// closures did a plain `Url::parse(&url)` with no fallback at all). Tries
/// each HTTPS-then-HTTP candidate via the cheap, unauthenticated
/// `get_status` probe, moving to the next candidate only on a genuine
/// connectivity failure (DNS/connect/TLS/timeout — never got a real HTTP
/// response back); a candidate that reaches the server (even a non-2xx
/// status) is the final answer, same reasoning as Login's own fallback —
/// retrying under a different scheme can't fix a real server-side error.
/// `get_status` doubles as the version-string fetch every one of
/// `wire_connect_seerr`'s auth closures already needs after a successful
/// attempt, so this resolves AND supplies that value in one step — 4 of
/// the 5 closures that used to call `get_status` a second time afterward
/// now just reuse the already-fetched `StatusInfo` instead (Quick
/// Connect's own `start` closure never called `get_status` before this,
/// so for it specifically this genuinely adds one new, but cheap, network
/// call it didn't previously make).
async fn resolve_seerr_url(url: &str) -> anyhow::Result<(Url, StatusInfo)> {
    let candidates = crate::auth::candidate_server_urls(url);
    let mut last_err: Option<anyhow::Error> = None;
    for (i, candidate) in candidates.iter().enumerate() {
        let base_url = Url::parse(candidate)?;
        match SeerrClient::get_status(&base_url).await {
            Ok(status) => return Ok((base_url, status)),
            Err(e) => {
                // Code review, 2026-08-26: was `re.status().is_none()`,
                // which also matches a JSON-decode failure on a genuinely
                // reachable HTTPS server — see
                // `auth::is_connectivity_failure`'s own doc comment for the
                // full "why," shared verbatim rather than re-derived here.
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

/// The current, valid ordered zone list for whichever ConnectSeerrScreen
/// tab/polling-state combination is active (2026-08-23, full D-pad
/// rollout) — same "gaps are fine, recomputed live off current state, not
/// cached" shape as `profile_edit::existing_profile_edit_zones`. Zone -1 is
/// the close-✕ button (reached via Up from zone 0); zone 0 is the tab row
/// (always present); zone 1 is the shared `url-input` (always present
/// regardless of tab); zones 2+ vary by `connect-seerr-method` and, for
/// Quick Connect specifically, `connect-seerr-qc-polling` — a polling Quick
/// Connect tab has nothing interactive below the URL field at all (its body
/// swaps to a code display + a 2s-Timer-driven poll, no button to focus).
pub(crate) fn existing_connect_seerr_zones(g: &AppState) -> Vec<i32> {
    let mut zones = vec![-1, 0, 1];
    match g.get_connect_seerr_method() {
        0 => zones.extend([2, 3]),       // API key: key-input, submit
        1 => zones.extend([2, 3, 4]),    // Jellyfin: username, password, submit
        2 => {
            if !g.get_connect_seerr_qc_polling() { zones.push(2); } // "Get Code" — nothing while polling
        }
        3 => zones.extend([2, 3, 4]),    // Local account: email, password, submit
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
    g.set_seerr_connected_label(
        if connected { connected_label(&c.seerr_auth_method) } else { "Not connected" }.into(),
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
    // Deep Seerr integration (2026-07-29) — same precedented gap this
    // function's own doc comment already warns about below: these two hold
    // request/watchlist-patched results that would otherwise show stale
    // pill state from the just-cleared connection.
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
        // Dashboard Watchlist rows (2026-07-20) — real bug class this
        // project has already been bitten by once (discover_watchlist_ids/
        // discover_calendar_entries/seerr_discover_region were originally
        // missing from this same reset): a disconnect must clear the 3
        // Slint-side watchlist models too, or they'd show stale content
        // from the just-cleared connection.
        g.set_discover_watchlist_mixed(crate::items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_watchlist_movies(crate::items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_watchlist_tv(crate::items_to_model(&[], &std::collections::HashSet::new()));
        // Dashboard Coming Up rows (2026-08-02) — same reasoning, same 3
        // Slint-side models.
        g.set_discover_coming_up_mixed(crate::items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_coming_up_movies(crate::items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_coming_up_tv(crate::items_to_model(&[], &std::collections::HashSet::new()));
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
        // A fresh connect may point at a different server/catalog — clear
        // any watchlist content still showing from the previous connection
        // rather than leaving it visible until ensure_discover_watchlist's
        // own fetch (above) lands (2026-07-20, same reset-completeness gap
        // this doc already documents having been bitten by once for
        // discover_watchlist_ids/discover_calendar_entries/seerr_discover_region).
        g.set_discover_watchlist_mixed(crate::items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_watchlist_movies(crate::items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_watchlist_tv(crate::items_to_model(&[], &std::collections::HashSet::new()));
        // Dashboard Coming Up rows (2026-08-02) — same reasoning, same 3
        // Slint-side models.
        g.set_discover_coming_up_mixed(crate::items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_coming_up_movies(crate::items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_coming_up_tv(crate::items_to_model(&[], &std::collections::HashSet::new()));
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
                // Real bug fixed 2026-07-18: these three were never reset on
                // open, only by the poll callback's own success/error arms —
                // closing the screen mid-Quick-Connect (before approval or
                // expiry) and reopening re-showed the stale "waiting for
                // approval" view against an old, likely-expired secret, with
                // no visible way back to the method picker short of closing
                // the whole screen again (which didn't fix it either, since
                // nothing here cleared it). Every open now starts clean.
                g.set_connect_seerr_qc_polling(false);
                g.set_connect_seerr_qc_code(slint::SharedString::new());
                g.set_connect_seerr_qc_secret(slint::SharedString::new());
                // Real bug, code review 2026-08-26: connect-seerr-zone (and
                // the on-screen keyboard's own state) were never reset here,
                // unlike every other transient field above. Closing the
                // screen while a text-field zone was focused (e.g. zone 2)
                // and reopening left that same zone value in place — since
                // Slint's `changed` only fires on a genuine value
                // transition, the zone→focus mirror trackers never re-fire,
                // so no field gets native focus, and (per keys.rs's own
                // dispatchable check) zone 2 isn't reachable there either —
                // the screen looked interactive but the D-pad was
                // completely dead until the user reached for the mouse.
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
                        Ok(()) => commit_connection(&state, &ww2, &base_url, "apikey", SeerrAuth::ApiKey(key), version, &rt_inner),
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
                        Ok((auth, _user)) => commit_connection(&state, &ww2, &base_url, "jellyfin", auth, version, &rt_inner),
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
                        Ok((auth, _user)) => commit_connection(&state, &ww2, &base_url, "local", auth, version, &rt_inner),
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
                            Err(e) => set_error(&ww2, &format!("Couldn't start Quick Connect: {e}")),
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
        // Both captured once, for the lifetime of this closure registration
        // (this callback is registered exactly once in wire_connect_seerr,
        // not re-registered per poll) — code review, 2026-08-26, real bug:
        // `resolve_seerr_url`'s own HTTPS-then-HTTP fallback can genuinely
        // take longer than the 2s Timer interval against a hung (not
        // refused) connection, and the ORIGINAL `Err(_) => return` silently
        // swallowed every resolve failure forever with qc-polling never
        // reset — a server that goes unreachable mid-poll left the screen
        // stuck on "waiting for approval," with no error and no way out
        // short of Escape (abandoning the whole attempt), for as long as
        // the user left it open, while also piling up a fresh overlapping
        // probe every 2 seconds against a server that was never going to
        // answer any of them.
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
                                set_error(&ww2, &format!("Lost the connection while waiting for approval: {e}"));
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
                                    commit_connection(&state, &ww2, &base_url, "quickconnect", auth, version, &rt_inner)
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
