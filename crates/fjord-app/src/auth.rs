use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::JellyfinClient;
use slint::SharedString;
use tracing::{error, info, warn};
use url::Url;

use crate::config::{AppState, load_config, save_config, ensure_device_id, save_item_cache};
use crate::home::{fetch_home_data, home_data_sections, push_home_data};
use crate::movies::spawn_movies_poster_loading;
use crate::poster::{spawn_poster_loading, spawn_series_poster_loading};
use crate::MainWindow;

fn ss(s: &str) -> SharedString { SharedString::from(s) }

pub(crate) fn do_login(
    server:      String,
    user:        String,
    pass:        String,
    state:       Arc<Mutex<AppState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    if let Some(w) = window_weak.upgrade() { w.set_status(ss("Connecting…")); }

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
                server_url.clone(), auth.user.id, auth.access_token.clone(), cfg.device_id,
            ));

            let ww_p = window_weak.clone();
            let (items_result, home_data, series_res) = tokio::join!(
                client.get_all_items(move |n| {
                    let ww = ww_p.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() { w.set_status(ss(&format!("Loading… {n}"))); }
                    });
                }),
                fetch_home_data(&client),
                client.get_all_series(),
            );

            let items = items_result?;
            info!("loaded {} items", items.len());
            save_item_cache(&items);
            let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
            info!("loaded {} series", series.len());
            let mut s = state.lock().unwrap();
            s.client     = Some(Arc::clone(&client));
            s.all_movies = items.iter().filter(|i| i.item_type == "Movie").cloned().collect();
            s.media_raw  = items;
            s.all_series = series.clone();
            s.apply_filter("");
            let names             = crate::display_names(&s.filtered_items);
            let movies            = s.all_movies.clone();
            let movies_for_poster = movies.clone();
            drop(s);

            let sections        = home_data_sections(&home_data);
            let server_str      = server_url.to_string();
            let ww              = window_weak.clone();
            let ww_poster       = window_weak.clone();
            let ww_series       = window_weak.clone();
            let rt_handle_inner = rt_handle.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    w.set_server_url(ss(&server_str));
                    w.set_media_items(crate::to_slint_model(names));
                    w.set_all_movies(crate::items_to_model(&movies));
                    push_home_data(&w, &home_data);
                    w.set_show_login(false);
                    w.set_status(ss(""));
                }
            });
            let client2   = Arc::clone(&client);
            let client3   = Arc::clone(&client);
            let ww_movies = window_weak.clone();
            spawn_poster_loading(client, sections, ww_poster, rt_handle_inner.clone());
            spawn_series_poster_loading(client2, series, ww_series, rt_handle_inner.clone());
            spawn_movies_poster_loading(client3, movies_for_poster, ww_movies, rt_handle_inner);
            Ok(())
        }.await;

        if let Err(e) = result {
            error!("login failed: {:#}", e);
            let msg = format!("{:#}", e);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = window_weak.upgrade() { w.set_status(ss(&msg)); }
            });
        }
    });
}
