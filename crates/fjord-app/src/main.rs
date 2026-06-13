// ── fjord-app · main.rs ──────────────────────────────────────────────────────
//   model helpers        item_to_card_item, items_to_model, push_section_model
//   settings helpers     apply_settings_to_window ↔ read_settings_from_window
//   main                 entry point; wires all AppState global callbacks
//     apply saved cfg    cold-start vs warm-start, check_auth
//     auto-login         warm-start path: fetch home + series data
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
mod controls;
mod detail;
mod home;
mod movies;
mod playback;
mod poster;
mod series;
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
use home::{load_home_cache, save_home_cache, fetch_home_data, push_home_data, home_data_sections, wire_nw_timer};
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
    h.title          = SharedString::from(i.display_name().as_str());
    h.year           = i.production_year.unwrap_or(0) as i32;
    h.has_played     = i.user_data.played;
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
    g.set_settings_audio_spdif(s.audio_spdif);
    g.set_settings_hwdec(ss(&s.hwdec));
    g.set_settings_hwdec_image_format(ss(&s.hwdec_image_format));
    g.set_settings_vf(ss(&s.vf));
    g.set_settings_gpu_api(ss(&s.gpu_api));
    g.set_settings_video_sync(ss(&s.video_sync));
    g.set_settings_opengl_early_flush(s.opengl_early_flush);
    g.set_settings_video_latency_hacks(s.video_latency_hacks);
    g.set_settings_interpolation(s.interpolation);
    g.set_settings_tscale(ss(&s.tscale));
    g.set_settings_tone_mapping(ss(&s.tone_mapping));
    g.set_settings_target_colorspace_hint(s.target_colorspace_hint);
    g.set_settings_deinterlace(s.deinterlace);
    g.set_settings_cache_mb(s.cache_size_mb as i32);
    g.set_settings_video_behind(s.video_behind);
    g.set_settings_launch_fullscreen(s.launch_fullscreen);
}

fn read_settings_from_window(w: &MainWindow, s: &mut FjordState) {
    let g = AppState::get(w);
    s.audio_spdif            = g.get_settings_audio_spdif();
    s.hwdec                  = g.get_settings_hwdec().to_string();
    s.hwdec_image_format     = g.get_settings_hwdec_image_format().to_string();
    s.vf                     = g.get_settings_vf().to_string();
    s.gpu_api                = g.get_settings_gpu_api().to_string();
    s.video_sync             = g.get_settings_video_sync().to_string();
    s.opengl_early_flush     = g.get_settings_opengl_early_flush();
    s.video_latency_hacks    = g.get_settings_video_latency_hacks();
    s.interpolation          = g.get_settings_interpolation();
    s.tscale                 = g.get_settings_tscale().to_string();
    s.tone_mapping           = g.get_settings_tone_mapping().to_string();
    s.target_colorspace_hint = g.get_settings_target_colorspace_hint();
    s.deinterlace            = g.get_settings_deinterlace();
    s.cache_size_mb          = g.get_settings_cache_mb().max(0) as u32;
    s.video_behind           = g.get_settings_video_behind();
    s.launch_fullscreen      = g.get_settings_launch_fullscreen();
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
        {
            let mut s = state.lock().unwrap();
            s.apply_from_config(&cfg);
        }
        apply_settings_to_window(&window, &state.lock().unwrap());
        if cfg.launch_fullscreen {
            window.window().set_fullscreen(true);
        }

        if let Ok(server_url) = Url::parse(&cfg.server_url) {
            let client = Arc::new(JellyfinClient::new(server_url.clone(), cfg.user_id, cfg.token, cfg.device_id.clone()));
            state.lock().unwrap().client = Some(Arc::clone(&client));
            AppState::get(&window).set_server_url(ss(cfg.server_url.as_str()));

            if let Some(cached_home) = load_home_cache() {
                push_home_data(&window, &cached_home);
            }

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
                let (home_data, series_res) = tokio::join!(
                    fetch_home_data(&client),
                    client.get_all_series(),
                );

                let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
                info!("loaded {} series", series.len());
                state2.lock().unwrap().all_series = series.clone();

                save_home_cache(&home_data);
                let sections = home_data_sections(&home_data);
                let ww2 = window_weak.clone();
                let ww3 = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        push_home_data(&w, &home_data);
                        AppState::get(&w).set_show_login(false);
                        AppState::get(&w).set_status(ss(""));
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
                        let config = state2.lock().unwrap().player_config();
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
            let title    = item_id.clone();
            let video3b  = Arc::clone(&video3);
            let ww3      = window_weak.clone();
            let rth3     = rt_handle.clone();
            rt_handle.spawn(async move {
                let detail    = client.get_item_detail(&item_id).await.ok();
                let item_type = detail.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
                let series_id = detail.as_ref().and_then(|i| i.series_id.clone());
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
            // Movies (nav == 1): lazy-fetch only if not already loaded.
            if !s.all_movies.is_empty() { return; }
            drop(s);
            let state_ol2 = Arc::clone(&state_ol);
            let ww2  = ww_ol.clone();
            let ww3  = ww_ol.clone();
            let rth3 = rth_ol.clone();
            rth_ol.spawn(async move {
                match client.get_all_movies().await {
                    Ok(movies) => {
                        state_ol2.lock().unwrap().all_movies = movies.clone();
                        let movies2 = movies.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_all_movies(items_to_model(&movies2));
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
        AppState::get(&window).on_open_detail(move |id| {
            detail::open_detail(id.to_string(), Arc::clone(&state2), ww.clone(), rt_handle.clone());
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

    // ── settings changed ──────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_settings_changed(move || {
            let Some(w) = window_weak.upgrade() else { return; };
            { let mut s = state.lock().unwrap(); read_settings_from_window(&w, &mut s); }
            if let Some(mut cfg) = load_config() {
                let s = state.lock().unwrap();
                cfg.audio_spdif            = s.audio_spdif;
                cfg.hwdec                  = s.hwdec.clone();
                cfg.hwdec_image_format     = s.hwdec_image_format.clone();
                cfg.vf                     = s.vf.clone();
                cfg.gpu_api                = s.gpu_api.clone();
                cfg.video_sync             = s.video_sync.clone();
                cfg.opengl_early_flush     = s.opengl_early_flush;
                cfg.video_latency_hacks    = s.video_latency_hacks;
                cfg.interpolation          = s.interpolation;
                cfg.tscale                 = s.tscale.clone();
                cfg.tone_mapping           = s.tone_mapping.clone();
                cfg.target_colorspace_hint = s.target_colorspace_hint;
                cfg.deinterlace            = s.deinterlace;
                cfg.cache_size_mb          = s.cache_size_mb;
                cfg.video_behind           = s.video_behind;
                cfg.launch_fullscreen      = s.launch_fullscreen;
                let launch_fs = s.launch_fullscreen;
                drop(s);
                save_config(&cfg);
                w.window().set_fullscreen(launch_fs);
                info!("settings saved");
            }
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
            }
        });
    }

    AppState::get(&window).on_quit(|| { slint::quit_event_loop().ok(); });

    window.invoke_grab_keyboard_focus();
    window.run()?;
    Ok(())
}
