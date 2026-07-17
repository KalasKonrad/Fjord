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
//   wire_connect_seerr   registers all ConnectSeerrScreen callbacks: the 4
//                        auth methods (API key, Jellyfin login, Quick Connect,
//                        local account) plus open/disconnect
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_seerr::{SeerrAuth, SeerrClient};
use slint::{ComponentHandle, Global, Weak};
use url::Url;

use crate::config::{save_config, FjordState};
use crate::{show_toast, AppState, MainWindow};

pub(crate) fn build_seerr_client(c: &crate::config::Config) -> Option<Arc<SeerrClient>> {
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

pub(crate) fn connected_label(method: &str) -> &'static str {
    match method {
        "apikey" => "Connected via API key",
        "jellyfin" => "Connected via Jellyfin login",
        "quickconnect" => "Connected via Jellyfin Quick Connect",
        "local" => "Connected via local account",
        _ => "Not connected",
    }
}

pub(crate) fn push_seerr_status(g: &AppState<'_>, c: &crate::config::Config) {
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
    s.config.seerr_auth_method.clear();
    s.config.seerr_api_key.clear();
    s.config.seerr_session_cookie.clear();
    s.seerr_client = None;
    s.discover_landing_fetched = false;
    s.seerr_streaming_region = None;
    s.seerr_regions.clear();
    save_config(&s.config);
    let cfg = s.config.clone();
    drop(s);
    if let Some(w) = ww.upgrade() {
        push_seerr_status(&AppState::get(&w), &cfg);
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
    s.config.seerr_url = base_url.to_string();
    s.config.seerr_auth_method = method.into();
    match &auth {
        SeerrAuth::ApiKey(k) => {
            s.config.seerr_api_key = k.clone();
            s.config.seerr_session_cookie.clear();
        }
        SeerrAuth::Session(c) => {
            s.config.seerr_session_cookie = c.clone();
            s.config.seerr_api_key.clear();
        }
    }
    let Ok(client) = SeerrClient::new(base_url.clone(), auth) else {
        drop(s);
        return;
    };
    let client = Arc::new(client);
    s.seerr_client = Some(Arc::clone(&client));
    s.discover_landing_fetched = false; // a (re)connect may point at a different server/catalog
    s.seerr_streaming_region = None;
    s.seerr_regions.clear();
    save_config(&s.config);
    let cfg = s.config.clone();
    drop(s);
    crate::spawn_seerr_settings_fetch(client, Arc::clone(state), ww.clone(), rt.clone());
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        push_seerr_status(&g, &cfg);
        if let Some(v) = version {
            g.set_seerr_version(v.as_str().into());
        }
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
            let Ok(base_url) = Url::parse(&url) else {
                set_error(&ww, "That doesn't look like a valid URL");
                return;
            };
            let state = Arc::clone(&state);
            let ww2 = ww.clone();
            let key = key.to_string();
            set_busy(&ww, true);
            rt.spawn(async move {
                // No dedicated "verify this key" endpoint — a bad key fails on
                // first authenticated use, so probe with a cheap search call.
                let client = SeerrClient::new(base_url.clone(), SeerrAuth::ApiKey(key.clone()));
                let result = match client {
                    Ok(c) => c.search("test", 1).await.map(|_| ()),
                    Err(e) => Err(e),
                };
                let version = SeerrClient::get_status(&base_url).await.ok().map(|s| s.version);
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
            let Ok(base_url) = Url::parse(&url) else {
                set_error(&ww, "That doesn't look like a valid URL");
                return;
            };
            let state = Arc::clone(&state);
            let ww2 = ww.clone();
            let (username, password) = (username.to_string(), password.to_string());
            set_busy(&ww, true);
            rt.spawn(async move {
                let result = SeerrClient::sign_in_jellyfin(&base_url, &username, &password).await;
                let version = SeerrClient::get_status(&base_url).await.ok().map(|s| s.version);
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
            let Ok(base_url) = Url::parse(&url) else {
                set_error(&ww, "That doesn't look like a valid URL");
                return;
            };
            let state = Arc::clone(&state);
            let ww2 = ww.clone();
            let (email, password) = (email.to_string(), password.to_string());
            set_busy(&ww, true);
            rt.spawn(async move {
                let result = SeerrClient::sign_in_local(&base_url, &email, &password).await;
                let version = SeerrClient::get_status(&base_url).await.ok().map(|s| s.version);
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
            let Ok(base_url) = Url::parse(&url) else {
                set_error(&ww, "That doesn't look like a valid URL");
                return;
            };
            let ww2 = ww.clone();
            set_busy(&ww, true);
            rt.spawn(async move {
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
        move |url, secret| {
            let Ok(base_url) = Url::parse(&url) else { return };
            let state = Arc::clone(&state);
            let ww2 = ww.clone();
            let secret = secret.to_string();
            rt.spawn(async move {
                match SeerrClient::quick_connect_check(&base_url, &secret).await {
                    Ok(true) => {
                        let auth_result =
                            SeerrClient::quick_connect_authenticate(&base_url, &secret).await;
                        let version = SeerrClient::get_status(&base_url).await.ok().map(|s| s.version);
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
