// ── fjord-app · auth.rs ──────────────────────────────────────────────────────
//   do_login  authenticate, persist config, fetch home + series + system info, show main UI,
//             start WebSocket reconnect loop; the authenticate() HTTP client carries an
//             explicit 30s timeout (previously a bare reqwest::Client::new() with no
//             timeout — the one call in the app that could hang indefinitely against an
//             unreachable server)
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
                &login_http, &server_url, &user, &pass, &cfg.device_id,
            ).await?;
            info!("authenticated as {}", auth.user.name);
            cfg.server_url = server_url.to_string();
            cfg.user_id    = auth.user.id.clone();
            cfg.token      = auth.access_token.clone();
            save_config(&cfg);

            let client = Arc::new(JellyfinClient::new(
                server_url.clone(), auth.user.id, auth.access_token.clone(), cfg.device_id.clone(),
            )?);

            let (home_data, series_res, sysinfo_res) = tokio::join!(
                fetch_home_data(&client),
                client.get_all_series(),
                client.get_system_info(),
            );

            let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
            info!("loaded {} series", series.len());
            let (srv_name, srv_ver) = sysinfo_res
                .map(|i| (i.server_name, i.version))
                .unwrap_or_else(|e| { warn!("get_system_info: {:#}", e); (String::new(), String::new()) });
            {
                let mut s = state.lock().unwrap();
                s.config     = cfg;
                s.client     = Some(Arc::clone(&client));
                s.all_series = series.clone();
            }

            save_series_cache(&series);
            let sections        = home_data_sections(&home_data);
            let series2         = series.clone();
            let server_str      = server_url.to_string();
            let ww              = window_weak.clone();
            let ww_poster       = window_weak.clone();
            let ww_series       = window_weak.clone();
            let rt_handle_inner = rt_handle.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let g = AppState::get(&w);
                    g.set_server_url(ss(&server_str));
                    g.set_server_name(ss(&srv_name));
                    g.set_server_version(ss(&srv_ver));
                    push_home_data(&w, &home_data);
                    g.set_all_series(items_to_model(&series2));
                    g.set_show_login(false);
                    g.set_status(ss(""));
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
            spawn_poster_loading(client, sections, ww_poster, rt_handle_inner.clone());
            spawn_series_poster_loading(client2, series, ww_series, rt_handle_inner.clone());
            rt_handle_inner.spawn(async move {
                let map = fetch_movie_collections(&client3).await;
                state_coll.lock().unwrap().movie_collections = map;
            });
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
