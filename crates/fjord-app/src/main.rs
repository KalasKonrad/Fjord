// ── fjord-app · main.rs ──────────────────────────────────────────────────────
//   model helpers        item_to_card_item, items_to_model, push_section_model
//   settings helpers     apply_settings_to_window ↔ read_settings_from_window
//   main                 entry point; wires all AppState global callbacks
//     apply saved cfg    cold-start vs warm-start, check_auth; load movies+series cache instantly
//     auto-login         warm-start path: fetch + save home/series; push series model early
//     login              on_do_login → auth::do_login
//     browse play        on_play_item (server-side search results)
//     home / library     on_item_play, on_open_library (lazy movie fetch)
//     detail             on_play_detail, on_resume_detail, on_close_detail
//     series             on_open_series, on_series_select_season, on_play_series_episode
//     auto-advance       on_cancel_auto_advance
//     player controls    wire_controls
//     settings           on_settings_changed
//     fullscreen         on_toggle_fullscreen, launch-fullscreen setting
//     sign-out           on_sign_out
// ─────────────────────────────────────────────────────────────────────────────
slint::include_modules!();

mod auth;
mod browse;
mod config;
mod context_menu;
mod controls;
mod detail;
mod home;
mod keys;
mod movies;
mod playback;
mod poster;
mod series;
mod settings;
mod stats;

use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, ModelRc, SharedString, StandardListViewItem, VecModel};
use tracing::{debug, info, warn};
use url::Url;

use config::{
    FjordState,
    config_path,
    load_config, save_config, ensure_device_id,
};
use home::{
    load_home_cache, save_home_cache, fetch_home_data, push_home_data, home_data_sections, wire_nw_timer,
    load_movies_cache, save_movies_cache, load_series_cache, save_series_cache,
};
use movies::spawn_movies_poster_loading;
use playback::{VideoState, start_playback, wire_rendering_notifier, wire_mpv_timer};
use poster::{spawn_poster_loading, spawn_series_poster_loading};
use series::{EpisodeRaw, make_episode_raw, raw_to_entry, spawn_episode_thumb_loading, open_series_screen};

fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .map(|s| s.as_u16() == 401)
        .unwrap_or(false)
}

// ── model helpers ─────────────────────────────────────────────────────────────

pub(crate) fn item_to_card_item(i: &MediaItem) -> CardItem {
    let mut h = CardItem::default();
    h.id             = SharedString::from(i.id.as_str());
    h.item_type      = SharedString::from(i.item_type.as_str());
    h.title          = SharedString::from(i.display_name().as_str());
    h.year           = i.production_year.unwrap_or(0) as i32;
    h.has_played     = i.user_data.played;
    h.is_favorite    = i.user_data.is_favorite;
    h.resume_pct     = i.resume_pct();
    h.unplayed_count = i.user_data.unplayed_item_count;
    h
}

pub(crate) fn items_to_model(items: &[MediaItem]) -> ModelRc<CardItem> {
    ModelRc::new(VecModel::from(items.iter().map(item_to_card_item).collect::<Vec<_>>()))
}

pub(crate) fn push_section_model(window: &MainWindow, sec: usize, model: ModelRc<CardItem>) {
    let g = AppState::get(window);
    match sec {
        0 => g.set_continue_watching(model),
        1 => g.set_next_up(model),
        2 => g.set_recently_added(model),
        3 => g.set_continue_watching_movies(model),
        4 => g.set_recently_added_movies(model),
        5 => g.set_not_watched_movies(model),
        6 => g.set_continue_watching_tv(model),
        7 => g.set_recently_added_tv(model),
        8 => g.set_not_watched_tv(model),
        9 => g.set_all_series(model),
        _ => {}
    }
}

pub(crate) fn to_slint_model(names: Vec<String>) -> ModelRc<StandardListViewItem> {
    let items: Vec<StandardListViewItem> = names.into_iter().map(|name| {
        let mut e = StandardListViewItem::default();
        e.text = SharedString::from(name.as_str());
        e
    }).collect();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn display_names(items: &[MediaItem]) -> Vec<String> {
    items.iter().map(|i| i.display_name()).collect()
}

fn ss(s: &str) -> SharedString { SharedString::from(s) }

fn apply_settings_to_window(w: &MainWindow, s: &FjordState) {
    let g = AppState::get(w);
    let c = &s.config;
    g.set_settings_audio_spdif(c.audio_spdif);
    g.set_settings_hwdec(ss(&c.hwdec));
    g.set_settings_vf(ss(&c.vf));
    g.set_settings_gpu_api(ss(&c.gpu_api));
    g.set_settings_video_sync(ss(&c.video_sync));
    g.set_settings_opengl_early_flush(c.opengl_early_flush);
    g.set_settings_video_latency_hacks(c.video_latency_hacks);
    g.set_settings_interpolation(c.interpolation);
    g.set_settings_tscale(ss(&c.tscale));
    g.set_settings_tone_mapping(ss(&c.tone_mapping));
    g.set_settings_target_colorspace_hint(c.target_colorspace_hint);
    g.set_settings_deinterlace(c.deinterlace);
    g.set_settings_cache_mb(c.cache_size_mb as i32);
    g.set_settings_video_behind(c.video_behind);
    g.set_settings_launch_fullscreen(c.launch_fullscreen);
}

fn read_settings_from_window(w: &MainWindow, s: &mut FjordState) {
    let g = AppState::get(w);
    let c = &mut s.config;
    c.audio_spdif            = g.get_settings_audio_spdif();
    c.hwdec                  = g.get_settings_hwdec().to_string();
    c.vf                     = g.get_settings_vf().to_string();
    c.gpu_api                = g.get_settings_gpu_api().to_string();
    c.video_sync             = g.get_settings_video_sync().to_string();
    c.opengl_early_flush     = g.get_settings_opengl_early_flush();
    c.video_latency_hacks    = g.get_settings_video_latency_hacks();
    c.interpolation          = g.get_settings_interpolation();
    c.tscale                 = g.get_settings_tscale().to_string();
    c.tone_mapping           = g.get_settings_tone_mapping().to_string();
    c.target_colorspace_hint = g.get_settings_target_colorspace_hint();
    c.deinterlace            = g.get_settings_deinterlace();
    c.cache_size_mb          = g.get_settings_cache_mb().max(0) as u32;
    c.video_behind           = g.get_settings_video_behind();
    c.launch_fullscreen      = g.get_settings_launch_fullscreen();
}

// ── entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    let log_dir = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
        })
        .join("fjord");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::never(&log_dir, "fjord.log");
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,fjord_app=debug,fjord_player=debug,fjord_api=debug")
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())
        .with(tracing_subscriber::fmt::layer().with_writer(file_writer))
        .init();
    info!("log file: {}", log_dir.join("fjord.log").display());

    let rt     = tokio::runtime::Runtime::new()?;
    let window = MainWindow::new()?;
    let state  = Arc::new(Mutex::new(FjordState::new()));
    let video  = Arc::new(Mutex::new(VideoState::default()));

    wire_rendering_notifier(&window, Arc::clone(&video));
    let mpv_timer = wire_mpv_timer(window.as_weak(), Arc::clone(&video), Arc::clone(&state), rt.handle().clone());
    std::mem::forget(mpv_timer);

    let nw_timer = wire_nw_timer(window.as_weak(), Arc::clone(&video), Arc::clone(&state), rt.handle().clone());
    std::mem::forget(nw_timer);

    // ── apply saved config ────────────────────────────────────────────────────
    if let Some(mut cfg) = load_config() {
        ensure_device_id(&mut cfg);
        state.lock().unwrap().config = cfg;
        apply_settings_to_window(&window, &state.lock().unwrap());
        let s = state.lock().unwrap();
        let launch_fs      = s.config.launch_fullscreen;
        let server_url_str = s.config.server_url.clone();
        let user_id        = s.config.user_id.clone();
        let token          = s.config.token.clone();
        let device_id      = s.config.device_id.clone();
        drop(s);
        if launch_fs { window.window().set_fullscreen(true); }

        if let Ok(server_url) = Url::parse(&server_url_str) {
            let client = Arc::new(JellyfinClient::new(server_url.clone(), user_id, token, device_id));
            state.lock().unwrap().client = Some(Arc::clone(&client));
            AppState::get(&window).set_server_url(ss(&server_url_str));

            if let Some(cached_home) = load_home_cache() {
                push_home_data(&window, &cached_home);
                let sections = home_data_sections(&cached_home);
                spawn_poster_loading(Arc::clone(&client), sections, window.as_weak(), rt.handle().clone());
            }
            if let Some(cached_movies) = load_movies_cache() {
                let model = items_to_model(&cached_movies);
                spawn_movies_poster_loading(Arc::clone(&client), cached_movies.clone(), window.as_weak(), rt.handle().clone());
                let mut s = state.lock().unwrap();
                s.all_movies     = cached_movies;
                s.movies_fetched = true;
                drop(s);
                AppState::get(&window).set_all_movies(model);
            }
            if let Some(cached_series) = load_series_cache() {
                AppState::get(&window).set_all_series(items_to_model(&cached_series));
                spawn_series_poster_loading(Arc::clone(&client), cached_series.clone(), window.as_weak(), rt.handle().clone());
                state.lock().unwrap().all_series = cached_series;
            }
            AppState::get(&window).set_show_login(false);
            window.invoke_grab_keyboard_focus();

            let window_weak = window.as_weak();
            let state2      = Arc::clone(&state);
            let rt_handle2  = rt.handle().clone();
            rt.spawn(async move {
                if let Err(e) = client.check_auth().await {
                    if is_unauthorized(&e) {
                        warn!("saved token is invalid (401) — showing login screen");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = window_weak.upgrade() {
                                AppState::get(&w).set_show_login(true);
                                AppState::get(&w).set_status(ss("Session expired — please log in again"));
                            }
                        });
                        return;
                    }
                    warn!("auth probe failed (non-401): {e:#}");
                }

                info!("auto-login: fetching home data + series");
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
                state2.lock().unwrap().all_series = series.clone();

                save_home_cache(&home_data);
                save_series_cache(&series);
                let sections = home_data_sections(&home_data);
                let series2  = series.clone();
                let ww2 = window_weak.clone();
                let ww3 = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        g.set_server_name(ss(&srv_name));
                        g.set_server_version(ss(&srv_ver));
                        push_home_data(&w, &home_data);
                        g.set_all_series(items_to_model(&series2));
                        g.set_show_login(false);
                        g.set_status(ss(""));
                        w.invoke_grab_keyboard_focus();
                    }
                });
                let client2 = Arc::clone(&client);
                spawn_poster_loading(client, sections, window_weak, rt_handle2.clone());
                spawn_series_poster_loading(client2, series, ww3, rt_handle2);
            });
        }
    }

    // ── login ─────────────────────────────────────────────────────────────────
    {
        let state       = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle   = rt.handle().clone();
        AppState::get(&window).on_do_login(move |server, user, pass| {
            auth::do_login(server.to_string(), user.to_string(), pass.to_string(),
                           Arc::clone(&state), window_weak.clone(), rt_handle.clone());
        });
    }

    // ── filter / library search / nav ─────────────────────────────────────────
    browse::wire_browse(&window, Arc::clone(&state), rt.handle().clone());

    // ── play from browse list ─────────────────────────────────────────────────
    {
        let state        = Arc::clone(&state);
        let video2       = Arc::clone(&video);
        let window_weak  = window.as_weak();
        let rt_handle    = rt.handle().clone();

        AppState::get(&window).on_play_item(move |idx| {
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return; };
            let Some(item)   = s.filtered_items.get(idx as usize) else { return; };
            let item_id    = item.id.clone();
            let item_title = item.display_name();
            if item.item_type == "Series" {
                let state2     = state.clone();
                let ww2        = window_weak.clone();
                let rt_handle2 = rt_handle.clone();
                drop(s);
                open_series_screen(item_id, state2, ww2, rt_handle2);
                return;
            }
            let play_url  = client.direct_play_url(&item_id);
            let mut config = s.player_config();
            let item_type  = item.item_type.clone();
            let series_id  = item.series_id.clone();
            drop(s);
            let video2b   = Arc::clone(&video2);
            let ww2       = window_weak.clone();
            let rth2      = rt_handle.clone();
            rt_handle.spawn(async move {
                let pos = client.get_item_detail(&item_id).await
                    .ok().and_then(|i| i.resume_position_secs());
                config.start_position_secs = pos;
                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(play_url, item_id, &item_type, item_title, config, client,
                                   &video2b, &ww2, &rth2);
                    video2b.lock().unwrap().playing_series_id = series_id;
                });
            });
        });
    }

    // ── play from home / library rows ─────────────────────────────────────────
    {
        let state       = Arc::clone(&state);
        let video3      = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle   = rt.handle().clone();

        AppState::get(&window).on_item_play(move |item_id| {
            let item_id = item_id.to_string();
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return; };

            if s.all_series.iter().any(|i| i.id == item_id) {
                let state2     = state.clone();
                let ww2        = window_weak.clone();
                let rt_handle2 = rt_handle.clone();
                let video4     = Arc::clone(&video3);
                drop(s);
                rt_handle.spawn(async move {
                    let next = client.get_next_up_for_series(&item_id).await.ok().flatten();
                    if let Some(next) = next {
                        let mut config = state2.lock().unwrap().player_config();
                        config.start_position_secs = next.resume_position_secs();
                        let cli2   = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                        let Some(cli2) = cli2 else {
                            let _ = slint::invoke_from_event_loop(move || {
                                open_series_screen(item_id, state2, ww2, rt_handle2);
                            });
                            return;
                        };
                        let url       = cli2.direct_play_url(&next.id);
                        let title     = next.display_name();
                        let ep_id     = next.id.clone();
                        let series_id = next.series_id.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            start_playback(url, ep_id, "Episode", title, config, cli2,
                                           &video4, &ww2, &rt_handle2);
                            video4.lock().unwrap().playing_series_id = series_id;
                        });
                    } else {
                        let _ = slint::invoke_from_event_loop(move || {
                            open_series_screen(item_id, state2, ww2, rt_handle2);
                        });
                    }
                });
                return;
            }

            let mut config = s.player_config();
            drop(s);
            let play_url = client.direct_play_url(&item_id);
            let video3b  = Arc::clone(&video3);
            let ww3      = window_weak.clone();
            let rth3     = rt_handle.clone();
            rt_handle.spawn(async move {
                let detail    = client.get_item_detail(&item_id).await.ok();
                let item_type = detail.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
                let series_id = detail.as_ref().and_then(|i| i.series_id.clone());
                let title     = detail.as_ref().map(|i| i.display_name()).unwrap_or_else(|| item_id.clone());
                config.start_position_secs = detail.and_then(|i| i.resume_position_secs());
                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(play_url, item_id, &item_type, title, config, client,
                                   &video3b, &ww3, &rth3);
                    video3b.lock().unwrap().playing_series_id = series_id;
                });
            });
        });
    }

    // ── lazy library grid ─────────────────────────────────────────────────────
    {
        let state_ol  = Arc::clone(&state);
        let ww_ol     = window.as_weak();
        let rth_ol    = rt.handle().clone();
        AppState::get(&window).on_open_library(move |nav| {
            let s = state_ol.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            if nav == 2 {
                // TV: all_series already loaded at startup; poster loading runs then too.
                let series = s.all_series.clone();
                drop(s);
                let ww2  = ww_ol.clone();
                let rth2 = rth_ol.clone();
                if !series.is_empty() {
                    spawn_series_poster_loading(client, series, ww2, rth2);
                }
                return;
            }
            // Movies (nav == 1): lazy-fetch from network once; cache pre-populates on warm start.
            if s.movies_fetched { return; }
            drop(s);
            let state_ol2 = Arc::clone(&state_ol);
            let ww2  = ww_ol.clone();
            let ww3  = ww_ol.clone();
            let rth3 = rth_ol.clone();
            rth_ol.spawn(async move {
                match client.get_all_movies().await {
                    Ok(movies) => {
                        {
                            let mut s = state_ol2.lock().unwrap();
                            s.all_movies     = movies.clone();
                            s.movies_fetched = true;
                        }
                        save_movies_cache(&movies);
                        let movies2 = movies.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                let model = items_to_model(&movies2);
                                let g = AppState::get(&w);
                                g.set_all_movies(model.clone());
                                // Refresh library-display if the grid is still open with no search
                                if g.get_show_library() && g.get_library_query().is_empty() {
                                    g.set_library_display(model);
                                }
                            }
                        });
                        spawn_movies_poster_loading(client, movies, ww3, rth3);
                    }
                    Err(e) => warn!("open_library movies: {:#}", e),
                }
            });
        });
    }

    // ── detail page ───────────────────────────────────────────────────────────
    {
        let state2    = Arc::clone(&state);
        let ww        = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_detail(move |id, item_type| {
            detail::open_detail(id.to_string(), item_type.to_string(), Arc::clone(&state2), ww.clone(), rt_handle.clone());
        });
    }
    {
        let state_pd  = Arc::clone(&state);
        let ww        = window.as_weak();
        let video_pd  = Arc::clone(&video);
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_play_detail(move || {
            let Some(w) = ww.upgrade() else { return };
            let id = AppState::get(&w).get_detail_id().to_string();
            if id.is_empty() { return }
            let g = AppState::get(&w);
            g.set_detail_scroll(0.0);
            g.set_show_detail(false);
            drop(g);
            let s = state_pd.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            config.start_position_secs = None;
            let title = AppState::get(&w).get_detail_title().to_string();
            drop(s);
            let play_url  = client.direct_play_url(&id);
            let video_pd2 = Arc::clone(&video_pd);
            let ww2       = ww.clone();
            let rth2      = rt_handle.clone();
            info!("play_detail: {}", id);
            rt_handle.spawn(async move {
                let detail    = client.get_item_detail(&id).await.ok();
                let item_type = detail.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
                let series_id = detail.and_then(|i| i.series_id);
                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(play_url, id, &item_type, title, config, client, &video_pd2, &ww2, &rth2);
                    video_pd2.lock().unwrap().playing_series_id = series_id;
                });
            });
        });
    }
    {
        let state_rd  = Arc::clone(&state);
        let ww        = window.as_weak();
        let video_rd  = Arc::clone(&video);
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_resume_detail(move || {
            let Some(w) = ww.upgrade() else { return };
            let id = AppState::get(&w).get_detail_id().to_string();
            if id.is_empty() { return }
            let g = AppState::get(&w);
            g.set_detail_scroll(0.0);
            g.set_show_detail(false);
            drop(g);
            let s = state_rd.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            let title = AppState::get(&w).get_detail_title().to_string();
            drop(s);
            let play_url  = client.direct_play_url(&id);
            let video_rd2 = Arc::clone(&video_rd);
            let ww2       = ww.clone();
            let rth2      = rt_handle.clone();
            rt_handle.spawn(async move {
                let detail    = client.get_item_detail(&id).await.ok();
                let item_type = detail.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
                let series_id = detail.as_ref().and_then(|i| i.series_id.clone());
                config.start_position_secs = detail.and_then(|i| i.resume_position_secs());
                info!("resume_detail: {} from {:?}s", id, config.start_position_secs);
                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(play_url, id, &item_type, title, config, client, &video_rd2, &ww2, &rth2);
                    video_rd2.lock().unwrap().playing_series_id = series_id;
                });
            });
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(&window).on_close_detail(move || {
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_show_detail(false);
                AppState::get(&w).set_detail_id("".into());
            }
        });
    }

    // ── series drill-down ─────────────────────────────────────────────────────
    {
        let state_os = Arc::clone(&state);
        let ww_os    = window.as_weak();
        let rth_os   = rt.handle().clone();
        AppState::get(&window).on_open_series(move |id| {
            open_series_screen(id.to_string(), state_os.clone(), ww_os.clone(), rth_os.clone());
        });
    }
    {
        let state_ss = Arc::clone(&state);
        let ww_ss    = window.as_weak();
        let rth_ss   = rt.handle().clone();
        AppState::get(&window).on_series_select_season(move |idx| {
            let idx = idx as usize;
            let s   = state_ss.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let series_id = s.series_open_id.clone();
            let Some(season_id) = s.series_season_ids.get(idx).cloned() else { return };
            drop(s);
            if let Some(w) = ww_ss.upgrade() {
                let g = AppState::get(&w);
                g.set_series_loading(true);
                g.set_series_episodes(ModelRc::new(VecModel::<EpisodeEntry>::default()));
                g.set_series_focused_ep(0);
            }
            let state_ss2 = state_ss.clone();
            let ww_ss2    = ww_ss.clone();
            let ww_ss3    = ww_ss.clone();
            let rth_ss2   = rth_ss.clone();
            let sid2      = series_id.clone();
            rth_ss.spawn(async move {
                let eps = client.get_season_episodes(&sid2, &season_id).await.unwrap_or_else(|e| {
                    warn!("get_season_episodes {} {}: {:#}", sid2, season_id, e);
                    vec![]
                });
                debug!("series {} season {} — {} episode(s)", sid2, season_id, eps.len());
                { state_ss2.lock().unwrap().series_episode_items = eps.clone(); }
                let raws: Vec<EpisodeRaw> = eps.iter().map(make_episode_raw).collect();
                let sid3 = sid2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww_ss2.upgrade() else { return };
                    if AppState::get(&w).get_series_id().as_str() != sid3 { return; }
                    let entries: Vec<EpisodeEntry> = raws.into_iter().map(raw_to_entry).collect();
                    AppState::get(&w).set_series_episodes(ModelRc::new(VecModel::from(entries)));
                    AppState::get(&w).set_series_loading(false);
                });
                spawn_episode_thumb_loading(client, eps, sid2, ww_ss3, rth_ss2);
            });
        });
    }
    {
        let state_pe = Arc::clone(&state);
        let video_pe = Arc::clone(&video);
        let ww_pe    = window.as_weak();
        let rth_pe   = rt.handle().clone();
        AppState::get(&window).on_play_series_episode(move |id| {
            let id = id.to_string();
            let s  = state_pe.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let ep_item = s.series_episode_items.iter().find(|i| i.id == id).cloned();
            let mut config = s.player_config();
            let series_id = ep_item.as_ref().and_then(|i| i.series_id.clone());
            drop(s);
            if let Some(w) = ww_pe.upgrade() { AppState::get(&w).set_show_series(false); }
            let play_url  = client.direct_play_url(&id);
            let title     = ep_item.map(|i| i.display_name()).unwrap_or_else(|| id.clone());
            let video_pe2 = Arc::clone(&video_pe);
            let ww_pe2    = ww_pe.clone();
            let rth_pe2   = rth_pe.clone();
            info!("play_series_episode: {}", id);
            rth_pe.spawn(async move {
                let pos = client.get_item_detail(&id).await
                    .ok().and_then(|i| i.resume_position_secs());
                config.start_position_secs = pos;
                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(play_url, id, "Episode", title, config, client, &video_pe2, &ww_pe2, &rth_pe2);
                    video_pe2.lock().unwrap().playing_series_id = series_id;
                });
            });
        });
    }
    {
        let state_cs = Arc::clone(&state);
        let ww_cs    = window.as_weak();
        AppState::get(&window).on_close_series(move || {
            debug!("close_series");
            if let Some(w) = ww_cs.upgrade() {
                AppState::get(&w).set_show_series(false);
                AppState::get(&w).set_series_id("".into());
            }
            let mut s = state_cs.lock().unwrap();
            s.series_open_id.clear();
            s.series_season_ids.clear();
            s.series_episode_items.clear();
        });
    }

    // ── auto-advance cancel ───────────────────────────────────────────────────
    {
        let state_ca = Arc::clone(&state);
        let ww_ca    = window.as_weak();
        AppState::get(&window).on_cancel_auto_advance(move || {
            state_ca.lock().unwrap().next_ep_pending = None;
            if let Some(w) = ww_ca.upgrade() {
                AppState::get(&w).set_show_next_ep_banner(false);
            }
        });
    }

    // ── player controls ───────────────────────────────────────────────────────
    controls::wire_controls(&window, Arc::clone(&video));

    // ── context menu ──────────────────────────────────────────────────────────
    context_menu::wire_context_menu(&window, Arc::clone(&state), Arc::clone(&video), rt.handle().clone());

    // ── settings changed ──────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_settings_changed(move || {
            let Some(w) = window_weak.upgrade() else { return; };
            let mut s = state.lock().unwrap();
            read_settings_from_window(&w, &mut s);
            let launch_fs = s.config.launch_fullscreen;
            save_config(&s.config);
            drop(s);
            w.window().set_fullscreen(launch_fs);
            info!("settings saved");
        });
    }

    // ── fullscreen toggle ────────────────────────────────────────────────────
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_toggle_fullscreen(move || {
            if let Some(w) = window_weak.upgrade() {
                let fs = w.window().is_fullscreen();
                w.window().set_fullscreen(!fs);
            }
        });
    }

    // ── sign-out ──────────────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_sign_out(move || {
            let _ = std::fs::remove_file(config_path());
            let mut s = state.lock().unwrap();
            s.client = None;
            s.all_movies.clear();
            s.all_series.clear();
            s.filtered_items.clear();
            s.series_open_id.clear();
            s.series_season_ids.clear();
            s.series_episode_items.clear();
            drop(s);
            if let Some(w) = window_weak.upgrade() {
                let g = AppState::get(&w);
                g.set_show_login(true);
                g.set_active_nav(0);
                g.set_show_browse(false);
                g.set_server_url(ss(""));
                g.set_server_name(ss(""));
                g.set_server_version(ss(""));
            }
        });
    }

    AppState::get(&window).on_quit(|| { slint::quit_event_loop().ok(); });

    // ── keyboard dispatch ────────────────────────────────────────────────────
    {
        let state2 = Arc::clone(&state);
        let ww     = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_handle_key(move |key, shift, ctrl, repeat| {
            let Some(w) = ww.upgrade() else { return false; };
            keys::handle_key(key.as_str(), shift, ctrl, repeat, &state2, &w, &rt2)
        });
    }

    // ── keybinding reset ─────────────────────────────────────────────────────
    {
        let state2 = Arc::clone(&state);
        let ww     = window.as_weak();
        AppState::get(&window).on_keybinding_reset_defaults(move || {
            let Some(w) = ww.upgrade() else { return; };
            {
                let mut st = state2.lock().unwrap();
                st.keybindings = keys::default_keybindings();
                config::save_keybindings(&st.keybindings);
            }
            keys::push_keybinding_rows(&w, &state2);
        });
    }

    keys::push_keybinding_rows(&window, &state);

    window.invoke_grab_keyboard_focus();
    window.run()?;
    Ok(())
}
