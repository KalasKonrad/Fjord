// ── fjord-app · auth.rs ──────────────────────────────────────────────────────
//   do_login  authenticate, persist config, then finish_session_setup; the authenticate()
//             HTTP client carries an explicit 30s timeout (previously a bare
//             reqwest::Client::new() with no timeout — the one call in the app that could
//             hang indefinitely against an unreachable server)
//   finish_session_setup  shared tail of every "we have a valid client, now make it the
//             active session" flow (Bonfire Phase 1, step 6, 2026-08-09) — fetch home
//             data/series/system info/plugins, persist cfg, update FjordState/AppState,
//             start WebSocket, spawn poster loading + movie-collections fetch, refresh
//             Settings → Profiles → Default Profile's dropdown (step 7). Reused verbatim
//             by profile.rs's switch_to_profile so the two flows can't drift.
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::JellyfinClient;
use slint::SharedString;
use tracing::{error, info, warn};
use url::Url;

use slint::Global;
use crate::AppState;
use crate::config::{FjordState, save_config, ensure_device_id};
use crate::home::{fetch_home_data, fetch_movie_collections, home_data_sections, push_home_data, save_series_cache};
use crate::{items_to_model, ws};
use crate::poster::{spawn_poster_loading, spawn_series_poster_loading};
use crate::MainWindow;

fn ss(s: &str) -> SharedString { SharedString::from(s) }

pub(crate) fn do_login(
    server:      String,
    user:        String,
    pass:        String,
    append:      bool,
    state:       Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    if let Some(w) = window_weak.upgrade() { AppState::get(&w).set_status(ss("Connecting…")); }

    let rt_handle_sp = rt_handle.clone();
    rt_handle.spawn(async move {
        let rt_handle = rt_handle_sp;
        let result: Result<()> = async {
            let server_url = Url::parse(&server)?;
            // Clone existing config so player/app settings survive sign-out + re-login.
            // Only auth fields are overwritten below.
            let mut cfg = state.lock().unwrap().config.clone();
            ensure_device_id(&mut cfg);
            // Matches JellyfinClient's own timeout — this call previously used a
            // bare default reqwest::Client (no timeout at all), the one place in
            // the app a black-holed connection could hang indefinitely.
            let login_http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?;
            let auth = fjord_api::authenticate(
                &login_http, &server_url, &user, &pass, &cfg.device.device_id,
            ).await?;
            info!("authenticated as {}", auth.user.name);
            // `append` (Bonfire Phase 1, step 6, 2026-08-09 — the picker's own
            // "+ Add Account" tile) means "keep every existing profile intact,
            // add this one alongside them" rather than the normal sign-in
            // behavior of overwriting whichever profile is currently active
            // (correct for "sign back into the same slot after sign-out", wrong
            // for genuinely adding a second account). If this exact user_id is
            // already known locally (re-authenticating a profile whose stored
            // token had gone stale), update that entry in place instead of
            // creating a duplicate.
            if append {
                if let Some(p) = cfg.profiles.iter_mut().find(|p| p.user_id == auth.user.id) {
                    p.server_url = server_url.to_string();
                    p.token      = auth.access_token.clone();
                } else {
                    cfg.profiles.push(crate::config::ProfileSettings {
                        server_url: server_url.to_string(),
                        user_id:    auth.user.id.clone(),
                        token:      auth.access_token.clone(),
                        ..Default::default()
                    });
                }
            } else {
                let p = cfg.active_mut();
                p.server_url = server_url.to_string();
                p.user_id    = auth.user.id.clone();
                p.token      = auth.access_token.clone();
            }
            // active_profile_id doubles as the active profile's own user_id
            // (Config::active()'s lookup key) — keep it in sync with the
            // identity just written above so it still names a real entry.
            cfg.active_profile_id = auth.user.id.clone();
            let user_id = cfg.active_profile_id.clone();
            save_config(&cfg);

            let client = Arc::new(JellyfinClient::new(
                server_url.clone(), auth.user.id, auth.access_token.clone(), cfg.device.device_id.clone(),
            )?);

            finish_session_setup(client, cfg, user_id, server_url, state, window_weak.clone(), rt_handle).await;
            Ok(())
        }.await;

        if let Err(e) = result {
            error!("login failed: {:#}", e);
            let msg = format!("{:#}", e);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = window_weak.upgrade() { AppState::get(&w).set_status(ss(&msg)); }
            });
        }
    });
}

// ── finish_session_setup ─────────────────────────────────────────────────────
// Shared tail of every "we already have a valid client for this profile, now
// make it the active session and show its content" flow — extracted from
// do_login (Bonfire Phase 1, step 6, 2026-08-09) so profile.rs's own
// switch_to_profile can reuse it verbatim instead of re-deriving the same
// ~10-field setup and risking drift, the same reasoning reset_session_state
// was extracted for. Fetches home data/series/system info/plugins in
// parallel, persists cfg (already fully mutated by the caller — this fn
// only reads it), updates FjordState + AppState, starts the WebSocket,
// spawns poster loading + the movie-collections fetch.
//
// Callers differ only in how `client` was obtained (a fresh password
// sign-in here; a Bonfire-minted or already-stored token in profile.rs) and
// in what `cfg` looks like going in (do_login mutates cfg.active_mut()
// in place; a profile switch finds-or-creates a different profiles[] entry
// instead) — everything from here on is identical either way.
pub(crate) async fn finish_session_setup(
    client:      Arc<JellyfinClient>,
    cfg:         crate::config::Config,
    user_id:     String,
    server_url:  Url,
    state:       Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    let (home_data, series_res, sysinfo_res, plugins_res) = tokio::join!(
        fetch_home_data(&client),
        client.get_all_series(),
        client.get_system_info(),
        client.get_plugins(),
    );

    let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
    info!("loaded {} series", series.len());
    let (srv_name, srv_ver) = sysinfo_res
        .map(|i| (i.server_name, i.version))
        .unwrap_or_else(|e| { warn!("get_system_info: {:#}", e); (String::new(), String::new()) });
    let plugins: std::collections::HashSet<String> = plugins_res
        .unwrap_or_else(|e| { warn!("get_plugins: {:#}", e); vec![] })
        .into_iter().map(|p| p.name).collect();
    {
        let mut s = state.lock().unwrap();
        s.config     = cfg;
        s.client     = Some(Arc::clone(&client));
        s.available_plugins = plugins;
        s.all_series = series.clone();
    }

    // Bonfire Phase 1, step 6 (2026-08-09): best-effort, always attempted —
    // get_plugins()/bonfire_list_profiles() both already degrade gracefully
    // when the plugin isn't installed, so this costs nothing extra for the
    // overwhelming majority of servers that don't have it.
    crate::profile::sync_bonfire_subprofiles(Arc::clone(&client), Arc::clone(&state), rt_handle.clone());

    save_series_cache(&user_id, &series);
    let sections        = home_data_sections(&home_data);
    let series2         = series.clone();
    let server_str      = server_url.to_string();
    let ww              = window_weak.clone();
    let ww_poster       = window_weak.clone();
    let ww_series       = window_weak.clone();
    let rt_handle_inner = rt_handle.clone();
    // Fresh session — no prior CardItem rows for these to carry an existing
    // on_watchlist forward from, so the persisted set has to be read
    // explicitly here (2026-07-20, see FjordState.jellyfin_watchlist_ids'
    // own doc comment). cfg_snapshot is read in the same lock, purely so
    // Settings → Profiles → Default Profile's dropdown reflects whatever
    // sync_bonfire_subprofiles/the Add-Account path may have just changed
    // in Config.profiles, without needing a restart first (step 7,
    // 2026-08-09) — apply_settings_to_window is the only other site that
    // pushes this, and it only ever runs once, at startup.
    let (watchlist, cfg_snapshot) = {
        let s = state.lock().unwrap();
        (s.jellyfin_watchlist_ids.clone(), s.config.clone())
    };
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            let g = AppState::get(&w);
            g.set_server_url(ss(&server_str));
            g.set_server_name(ss(&srv_name));
            g.set_server_version(ss(&srv_ver));
            push_home_data(&w, &home_data, &watchlist);
            g.set_all_series(items_to_model(&series2, &watchlist));
            g.set_show_login(false);
            g.set_show_profile_picker(false);
            g.set_status(ss(""));
            crate::profile::refresh_profile_settings_dropdown(&g, &cfg_snapshot);
            w.invoke_grab_keyboard_focus();
        }
    });
    let client2      = Arc::clone(&client);
    let client3      = Arc::clone(&client);
    let client4      = Arc::clone(&client);
    let state_coll   = state.clone();
    let state_ws     = state.clone();
    let ws_abort = ws::start_websocket(client4, Arc::clone(&state_ws), window_weak.clone(), rt_handle_inner.clone());
    state_ws.lock().unwrap().ws_abort = Some(ws_abort);
    spawn_poster_loading(client, sections, ww_poster, rt_handle_inner.clone(), Arc::clone(&state));
    spawn_series_poster_loading(client2, series, ww_series, rt_handle_inner.clone(), Arc::clone(&state));
    rt_handle_inner.spawn(async move {
        let map = fetch_movie_collections(&client3).await;
        state_coll.lock().unwrap().movie_collections = map;
    });
}
