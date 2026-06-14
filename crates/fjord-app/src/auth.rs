// ── fjord-app · auth.rs ──────────────────────────────────────────────────────
//   do_login  authenticate, persist config, fetch home + series, show main UI
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::JellyfinClient;
use slint::SharedString;
use tracing::{error, info, warn};
use url::Url;

use slint::Global;
use crate::AppState;
use crate::config::{FjordState, load_config, save_config, ensure_device_id};
use crate::home::{fetch_home_data, home_data_sections, push_home_data, save_series_cache};
use crate::{items_to_model};
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
            let mut cfg = load_config().unwrap_or_default();
            ensure_device_id(&mut cfg);
            let auth = fjord_api::authenticate(
                &reqwest::Client::new(), &server_url, &user, &pass, &cfg.device_id,
            ).await?;
            info!("authenticated as {}", auth.user.name);
            cfg.server_url = server_url.to_string();
            cfg.user_id    = auth.user.id.clone();
            cfg.token      = auth.access_token.clone();
            save_config(&cfg);

            let client = Arc::new(JellyfinClient::new(
                server_url.clone(), auth.user.id, auth.access_token.clone(), cfg.device_id.clone(),
            ));

            let (home_data, series_res) = tokio::join!(
                fetch_home_data(&client),
                client.get_all_series(),
            );

            let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
            info!("loaded {} series", series.len());
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
                    push_home_data(&w, &home_data);
                    g.set_all_series(items_to_model(&series2));
                    g.set_show_login(false);
                    g.set_status(ss(""));
                    w.invoke_grab_keyboard_focus();
                }
            });
            let client2 = Arc::clone(&client);
            spawn_poster_loading(client, sections, ww_poster, rt_handle_inner.clone());
            spawn_series_poster_loading(client2, series, ww_series, rt_handle_inner);
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
