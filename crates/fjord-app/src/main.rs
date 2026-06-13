slint::include_modules!();

mod auth;
mod browse;
mod config;
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
use slint::{Model, ModelRc, SharedString, StandardListViewItem, VecModel};
use tracing::{debug, info, warn};
use url::Url;

use config::{
    AppState,
    config_path, item_cache_path,
    load_config, save_config, ensure_device_id,
    load_item_cache, save_item_cache, is_item_cache_fresh,
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
    match sec {
        0 => window.set_continue_watching(model),
        1 => window.set_next_up(model),
        2 => window.set_recently_added(model),
        3 => window.set_continue_watching_movies(model),
        4 => window.set_recently_added_movies(model),
        5 => window.set_not_watched_movies(model),
        6 => window.set_continue_watching_tv(model),
        7 => window.set_recently_added_tv(model),
        8 => window.set_not_watched_tv(model),
        9 => window.set_all_series(model),
        _ => {}
    }
}

fn to_slint_model(names: Vec<String>) -> ModelRc<StandardListViewItem> {
    let items: Vec<StandardListViewItem> = names.into_iter().map(|name| {
        let mut e = StandardListViewItem::default();
        e.text = SharedString::from(name.as_str());
        e
    }).collect();
    ModelRc::new(VecModel::from(items))
}

fn display_names(items: &[MediaItem]) -> Vec<String> {
    items.iter().map(|i| i.display_name()).collect()
}

fn ss(s: &str) -> SharedString { SharedString::from(s) }

fn apply_settings_to_window(w: &MainWindow, s: &AppState) {
    w.set_settings_audio_spdif(s.audio_spdif);
    w.set_settings_hwdec(ss(&s.hwdec));
    w.set_settings_hwdec_image_format(ss(&s.hwdec_image_format));
    w.set_settings_vf(ss(&s.vf));
    w.set_settings_gpu_api(ss(&s.gpu_api));
    w.set_settings_video_sync(ss(&s.video_sync));
    w.set_settings_opengl_early_flush(s.opengl_early_flush);
    w.set_settings_video_latency_hacks(s.video_latency_hacks);
    w.set_settings_interpolation(s.interpolation);
    w.set_settings_tscale(ss(&s.tscale));
    w.set_settings_tone_mapping(ss(&s.tone_mapping));
    w.set_settings_target_colorspace_hint(s.target_colorspace_hint);
    w.set_settings_deinterlace(s.deinterlace);
    w.set_settings_cache_mb(s.cache_size_mb as i32);
    w.set_settings_video_behind(s.video_behind);
    w.set_settings_launch_fullscreen(s.launch_fullscreen);
}

fn read_settings_from_window(w: &MainWindow, s: &mut AppState) {
    s.audio_spdif            = w.get_settings_audio_spdif();
    s.hwdec                  = w.get_settings_hwdec().to_string();
    s.hwdec_image_format     = w.get_settings_hwdec_image_format().to_string();
    s.vf                     = w.get_settings_vf().to_string();
    s.gpu_api                = w.get_settings_gpu_api().to_string();
    s.video_sync             = w.get_settings_video_sync().to_string();
    s.opengl_early_flush     = w.get_settings_opengl_early_flush();
    s.video_latency_hacks    = w.get_settings_video_latency_hacks();
    s.interpolation          = w.get_settings_interpolation();
    s.tscale                 = w.get_settings_tscale().to_string();
    s.tone_mapping           = w.get_settings_tone_mapping().to_string();
    s.target_colorspace_hint = w.get_settings_target_colorspace_hint();
    s.deinterlace            = w.get_settings_deinterlace();
    s.cache_size_mb          = w.get_settings_cache_mb().max(0) as u32;
    s.video_behind           = w.get_settings_video_behind();
    s.launch_fullscreen      = w.get_settings_launch_fullscreen();
}

// ── entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    // Log to both stderr and ~/.cache/fjord/fjord.log for HTPC debugging.
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
    // External crates (winit, sctk, reqwest, …) flood the log at DEBUG.
    // Default to WARN for everything; our own crates stay at DEBUG.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,fjord_app=debug,fjord_player=debug,fjord_api=debug")
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())                          // stderr
        .with(tracing_subscriber::fmt::layer().with_writer(file_writer)) // file
        .init();
    info!("log file: {}", log_dir.join("fjord.log").display());

    let rt     = tokio::runtime::Runtime::new()?;
    let window = MainWindow::new()?;
    let state  = Arc::new(Mutex::new(AppState::new()));
    let video  = Arc::new(Mutex::new(VideoState::default()));

    // ── rendering notifier + mpv event-poll timer ────────────────────────────
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
            window.set_server_url(ss(cfg.server_url.as_str()));

            if let Some(cached) = load_item_cache() {
                info!("item cache: {} items loaded instantly", cached.len());
                let mut s = state.lock().unwrap();
                s.all_movies = cached.iter().filter(|i| i.item_type == "Movie").cloned().collect();
                s.media_raw  = cached;
                s.apply_filter("");
                let names       = display_names(&s.filtered_items);
                let movie_model = items_to_model(&s.all_movies);
                drop(s);
                // Still on the main thread before window.run() — set directly,
                // no invoke_from_event_loop needed (avoids a one-frame login flash).
                window.set_media_items(to_slint_model(names));
                window.set_all_movies(movie_model);
                window.set_show_login(false);
                window.set_status(ss(""));
            }

            // Show cached home data immediately so no "Loading library…" flash.
            if let Some(cached_home) = load_home_cache() {
                push_home_data(&window, &cached_home);
            }

            let window_weak = window.as_weak();
            let state2      = Arc::clone(&state);
            let rt_handle2  = rt.handle().clone();
            rt.spawn(async move {
                // Probe auth before heavy refresh — show login screen cleanly on 401.
                if let Err(e) = client.check_auth().await {
                    if is_unauthorized(&e) {
                        warn!("saved token is invalid (401) — showing login screen");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = window_weak.upgrade() {
                                w.set_show_login(true);
                                w.set_status(ss("Session expired — please log in again"));
                            }
                        });
                        return;
                    }
                    warn!("auth probe failed (non-401): {e:#}");
                }

                // Skip the expensive full-library refresh when the cache is recent.
                // Home data (continue watching, next up, etc.) always refreshes.
                let (maybe_new_items, home_data, series_res) = if is_item_cache_fresh() {
                    info!("auto-login: item cache fresh — refreshing home data only");
                    let (hd, sr) = tokio::join!(fetch_home_data(&client), client.get_all_series());
                    (None::<Vec<MediaItem>>, hd, sr)
                } else {
                    info!("auto-login: refreshing library + home data (background)");
                    let (items_res, hd, sr) = tokio::join!(
                        client.get_all_items(|_| {}),
                        fetch_home_data(&client),
                        client.get_all_series(),
                    );
                    match items_res {
                        Ok(items) => (Some(items), hd, sr),
                        Err(e)    => { warn!("library refresh failed: {:#}", e); (None, hd, sr) }
                    }
                };

                if let Some(items) = maybe_new_items {
                    save_item_cache(&items);
                    let mut s = state2.lock().unwrap();
                    s.all_movies = items.iter().filter(|i| i.item_type == "Movie").cloned().collect();
                    s.media_raw  = items;
                    s.apply_filter("");
                    let names   = display_names(&s.filtered_items);
                    let movies  = s.all_movies.clone();  // Vec<MediaItem> is Send
                    drop(s);
                    let ww = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() {
                            w.set_media_items(to_slint_model(names));
                            w.set_all_movies(items_to_model(&movies));
                        }
                    });
                }

                let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
                info!("loaded {} series", series.len());
                {
                    let mut s = state2.lock().unwrap();
                    s.all_series = series.clone();
                    s.refilter();
                    // browse list now includes series — push updated names to UI below
                }

                let browse_names = {
                    let s = state2.lock().unwrap();
                    display_names(&s.filtered_items)
                };
                save_home_cache(&home_data);
                let sections = home_data_sections(&home_data);
                let ww2 = window_weak.clone();
                let ww3 = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        push_home_data(&w, &home_data);
                        w.set_media_items(to_slint_model(browse_names));
                        w.set_show_login(false);
                        w.set_status(ss(""));
                    }
                });
                let movies_for_poster = state2.lock().unwrap().all_movies.clone();
                let client2 = Arc::clone(&client);
                let client3 = Arc::clone(&client);
                let ww4 = window_weak.clone();
                spawn_poster_loading(client, sections, window_weak, rt_handle2.clone());
                spawn_series_poster_loading(client2, series, ww3, rt_handle2.clone());
                spawn_movies_poster_loading(client3, movies_for_poster, ww4, rt_handle2);
            });
        }
    }

    // ── login ─────────────────────────────────────────────────────────────────
    {
        let state       = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle   = rt.handle().clone();
        window.on_do_login(move |server, user, pass| {
            auth::do_login(server.to_string(), user.to_string(), pass.to_string(),
                           Arc::clone(&state), window_weak.clone(), rt_handle.clone());
        });
    }

    // ── filter / library search / nav ─────────────────────────────────────────
    browse::wire_browse(&window, Arc::clone(&state));

    // ── play from browse list ─────────────────────────────────────────────────
    {
        let state        = Arc::clone(&state);
        let video2       = Arc::clone(&video);
        let window_weak  = window.as_weak();
        let rt_handle    = rt.handle().clone();

        window.on_play_item(move |idx| {
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return; };
            let Some(item)   = s.filtered_items.get(idx as usize) else { return; };
            let item_id    = item.id.clone();
            let item_title = item.display_name();
            // Series items in browse results route to the drill-down screen
            if item.item_type == "Series" {
                let state2     = state.clone();
                let ww2        = window_weak.clone();
                let rt_handle2 = rt_handle.clone();
                drop(s);
                open_series_screen(item_id, state2, ww2, rt_handle2);
                return;
            }
            let play_url   = client.direct_play_url(&item_id);
            let mut config = s.player_config();
            config.start_position_secs = item.resume_position_secs();
            let item_type  = item.item_type.clone();
            let series_id  = item.series_id.clone();
            drop(s);

            start_playback(play_url, item_id, &item_type, item_title, config, client,
                           &video2, &window_weak, &rt_handle);
            video2.lock().unwrap().playing_series_id = series_id;
        });
    }

    // ── play from home / library rows ─────────────────────────────────────────
    {
        let state       = Arc::clone(&state);
        let video3      = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle   = rt.handle().clone();

        window.on_item_play(move |item_id| {
            let item_id = item_id.to_string();
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return; };

            // Series: play next unwatched episode; fall back to series screen if
            // there is no next episode (fully watched) or the API call fails.
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
            let found = s.media_raw.iter().find(|i| i.id == item_id).cloned();
            config.start_position_secs = found.as_ref().and_then(|i| i.resume_position_secs());
            let item_type = found.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
            let series_id = found.and_then(|i| i.series_id);
            drop(s);
            let play_url = client.direct_play_url(&item_id);
            let title    = item_id.clone();

            start_playback(play_url, item_id, &item_type, title, config, client,
                           &video3, &window_weak, &rt_handle);
            video3.lock().unwrap().playing_series_id = series_id;
        });
    }

    // ── detail page ───────────────────────────────────────────────────────────
    {
        let state2    = Arc::clone(&state);
        let ww        = window.as_weak();
        let rt_handle = rt.handle().clone();
        window.on_open_detail(move |id| {
            detail::open_detail(id.to_string(), Arc::clone(&state2), ww.clone(), rt_handle.clone());
        });
    }
    {
        let state_pd  = Arc::clone(&state);
        let ww        = window.as_weak();
        let video_pd  = Arc::clone(&video);
        let rt_handle = rt.handle().clone();
        window.on_play_detail(move || {
            let Some(w) = ww.upgrade() else { return };
            let id = w.get_detail_id().to_string();
            if id.is_empty() { return }
            w.set_show_detail(false);
            let s = state_pd.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            // Don't resume — play from start
            config.start_position_secs = None;
            let found_pd  = s.media_raw.iter().find(|i| i.id == id).cloned();
            let item_type = found_pd.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
            let series_id = found_pd.and_then(|i| i.series_id);
            let title = w.get_detail_title().to_string();
            drop(s);
            let play_url = client.direct_play_url(&id);
            info!("play_detail: {}", id);
            start_playback(play_url, id, &item_type, title, config, client, &video_pd, &ww, &rt_handle);
            video_pd.lock().unwrap().playing_series_id = series_id;
        });
    }
    {
        let state_rd  = Arc::clone(&state);
        let ww        = window.as_weak();
        let video_rd  = Arc::clone(&video);
        let rt_handle = rt.handle().clone();
        window.on_resume_detail(move || {
            let Some(w) = ww.upgrade() else { return };
            let id = w.get_detail_id().to_string();
            if id.is_empty() { return }
            w.set_show_detail(false);
            let s = state_rd.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            let found = s.media_raw.iter().find(|i| i.id == id).cloned();
            config.start_position_secs = found.as_ref().and_then(|i| i.resume_position_secs());
            let item_type = found.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
            let series_id = found.and_then(|i| i.series_id);
            let title = w.get_detail_title().to_string();
            drop(s);
            let play_url = client.direct_play_url(&id);
            info!("resume_detail: {} from {:?}s", id, config.start_position_secs);
            start_playback(play_url, id, &item_type, title, config, client, &video_rd, &ww, &rt_handle);
            video_rd.lock().unwrap().playing_series_id = series_id;
        });
    }
    {
        let ww = window.as_weak();
        window.on_close_detail(move || {
            if let Some(w) = ww.upgrade() {
                w.set_show_detail(false);
                w.set_detail_id("".into());
            }
        });
    }

    // ── series drill-down ─────────────────────────────────────────────────────
    {
        let state_os = Arc::clone(&state);
        let ww_os    = window.as_weak();
        let rth_os   = rt.handle().clone();
        window.on_open_series(move |id| {
            open_series_screen(id.to_string(), state_os.clone(), ww_os.clone(), rth_os.clone());
        });
    }
    {
        let state_ss = Arc::clone(&state);
        let ww_ss    = window.as_weak();
        let rth_ss   = rt.handle().clone();
        window.on_series_select_season(move |idx| {
            let idx = idx as usize;
            let s   = state_ss.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let series_id = s.series_open_id.clone();
            let Some(season_id) = s.series_season_ids.get(idx).cloned() else { return };
            drop(s);
            if let Some(w) = ww_ss.upgrade() {
                w.set_series_loading(true);
                w.set_series_episodes(ModelRc::new(VecModel::<EpisodeEntry>::default()));
                w.set_series_focused_ep(0);
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
                    if w.get_series_id().as_str() != sid3 { return; }
                    let entries: Vec<EpisodeEntry> = raws.into_iter().map(raw_to_entry).collect();
                    w.set_series_episodes(ModelRc::new(VecModel::from(entries)));
                    w.set_series_loading(false);
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
        window.on_play_series_episode(move |id| {
            let id = id.to_string();
            let s  = state_pe.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let ep_item = s.series_episode_items.iter().find(|i| i.id == id).cloned();
            let mut config = s.player_config();
            config.start_position_secs = ep_item.as_ref().and_then(|i| i.resume_position_secs());
            let series_id = ep_item.as_ref().and_then(|i| i.series_id.clone());
            drop(s);
            if let Some(w) = ww_pe.upgrade() { w.set_show_series(false); }
            let play_url = client.direct_play_url(&id);
            let title    = ep_item.map(|i| i.display_name()).unwrap_or_else(|| id.clone());
            info!("play_series_episode: {}", id);
            start_playback(play_url, id, "Episode", title, config, client, &video_pe, &ww_pe, &rth_pe);
            video_pe.lock().unwrap().playing_series_id = series_id;
        });
    }
    {
        let state_cs = Arc::clone(&state);
        let ww_cs    = window.as_weak();
        window.on_close_series(move || {
            debug!("close_series");
            if let Some(w) = ww_cs.upgrade() {
                w.set_show_series(false);
                w.set_series_id("".into());
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
        window.on_cancel_auto_advance(move || {
            state_ca.lock().unwrap().next_ep_pending = None;
            if let Some(w) = ww_ca.upgrade() {
                w.set_show_next_ep_banner(false);
            }
        });
    }

    // ── player controls ───────────────────────────────────────────────────────
    {
        let video5 = Arc::clone(&video);
        let ww     = window.as_weak();
        window.on_pause_play_toggle(move || {
            let vs = video5.lock().unwrap();
            if let Some(p) = vs.player.as_ref() { p.toggle_pause(); }
            drop(vs);
            if let Some(w) = ww.upgrade() {
                let now_paused = !w.get_is_paused();
                debug!("pause_play_toggle → {}", if now_paused { "paused" } else { "playing" });
                w.set_is_paused(now_paused);
            }
        });
    }
    {
        let video6 = Arc::clone(&video);
        window.on_seek_backward(move || {
            if let Some(p) = video6.lock().unwrap().player.as_ref() {
                debug!("seek_backward 10s");
                p.seek_backward(10.0);
            }
        });
    }
    {
        let video7 = Arc::clone(&video);
        window.on_seek_forward(move || {
            if let Some(p) = video7.lock().unwrap().player.as_ref() {
                debug!("seek_forward 10s");
                p.seek_forward(10.0);
            }
        });
    }
    {
        let video_sbl = Arc::clone(&video);
        window.on_seek_backward_long(move || {
            if let Some(p) = video_sbl.lock().unwrap().player.as_ref() {
                debug!("seek_backward 30s");
                p.seek_backward(30.0);
            }
        });
    }
    {
        let video_sfl = Arc::clone(&video);
        window.on_seek_forward_long(move || {
            if let Some(p) = video_sfl.lock().unwrap().player.as_ref() {
                debug!("seek_forward 30s");
                p.seek_forward(30.0);
            }
        });
    }
    {
        let video8 = Arc::clone(&video);
        window.on_stop_playback(move || {
            info!("stop_playback requested");
            let mut vs = video8.lock().unwrap();
            vs.user_stopped = true;
            if let Some(p) = vs.player.as_ref() { p.stop(); }
        });
    }
    {
        let video_seek = Arc::clone(&video);
        window.on_seek_to(move |ratio| {
            let vs = video_seek.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                let dur = p.get_duration();
                if dur > 0.0 {
                    let secs = ratio as f64 * dur;
                    debug!("seek_to: ratio={:.3} → {:.1}s / {:.1}s", ratio, secs, dur);
                    p.seek_to(secs);
                }
            }
        });
    }
    {
        let video_si = Arc::clone(&video);
        window.on_skip_intro(move || {
            let vs = video_si.lock().unwrap();
            if let (Some(ref ts), Some(p)) = (vs.intro_timestamps.as_ref(), vs.player.as_ref()) {
                info!("skip intro: seeking to {:.1}s", ts.intro_end);
                p.seek_to(ts.intro_end);
            }
        });
    }
    {
        let video_sub = Arc::clone(&video);
        window.on_select_sub(move |id| {
            if let Some(p) = video_sub.lock().unwrap().player.as_ref() {
                debug!("select subtitle track id={}", id);
                p.set_sub_track(id as i64);
            }
        });
    }
    {
        let video_aud = Arc::clone(&video);
        window.on_select_audio(move |id| {
            if let Some(p) = video_aud.lock().unwrap().player.as_ref() {
                debug!("select audio track id={}", id);
                p.set_audio_track(id as i64);
            }
        });
    }
    {
        let video_cps = Arc::clone(&video);
        let ww = window.as_weak();
        window.on_commit_panel_selection(move || {
            let Some(w) = ww.upgrade() else { return };
            let panel  = w.get_player_open_panel();
            let cursor = w.get_player_panel_cursor() as usize;
            let vs = video_cps.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                match panel {
                    1 => {
                        // Sub panel: cursor 0 = Off, 1+ = sub-tracks[cursor-1]
                        let id = if cursor == 0 {
                            0i32
                        } else {
                            w.get_sub_tracks().row_data(cursor - 1).map(|t| t.id).unwrap_or(0)
                        };
                        debug!("commit sub: cursor={} → id={}", cursor, id);
                        p.set_sub_track(id as i64);
                        w.set_current_sub_id(id);
                    }
                    2 => {
                        let id = w.get_audio_tracks().row_data(cursor).map(|t| t.id).unwrap_or(1);
                        debug!("commit audio: cursor={} → id={}", cursor, id);
                        p.set_audio_track(id as i64);
                        w.set_current_audio_id(id);
                    }
                    3 => {
                        let id = w.get_video_tracks().row_data(cursor).map(|t| t.id).unwrap_or(1);
                        debug!("commit video: cursor={} → id={}", cursor, id);
                        p.set_video_track(id as i64);
                        w.set_current_video_id(id);
                    }
                    _ => {}
                }
            }
        });
    }
    {
        let video_vol_up = Arc::clone(&video);
        window.on_volume_up(move || {
            if let Some(p) = video_vol_up.lock().unwrap().player.as_ref() { p.adjust_volume(5.0); }
        });
    }
    {
        let video_vol_dn = Arc::clone(&video);
        window.on_volume_down(move || {
            if let Some(p) = video_vol_dn.lock().unwrap().player.as_ref() { p.adjust_volume(-5.0); }
        });
    }
    {
        let video_sv = Arc::clone(&video);
        let ww = window.as_weak();
        window.on_show_controls(move || {
            if let Some(w) = ww.upgrade() { w.set_controls_visible(true); }
            video_sv.lock().unwrap().controls_idle_ticks = 0;
        });
    }
    {
        let video_vid = Arc::clone(&video);
        window.on_select_video(move |id| {
            if let Some(p) = video_vid.lock().unwrap().player.as_ref() {
                debug!("select video track id={}", id);
                p.set_video_track(id as i64);
            }
        });
    }
    {
        let ww = window.as_weak();
        window.on_resume_player(move || {
            let Some(w) = ww.upgrade() else { return };
            if w.get_has_background_player() {
                info!("resuming player to fullscreen");
                w.set_is_playing(true);
                w.set_has_background_player(false);
                w.set_video_behind_ui(false);
                w.set_controls_visible(true);
            }
        });
    }
    {
        let video_mute = Arc::clone(&video);
        window.on_mute_toggle(move || {
            if let Some(p) = video_mute.lock().unwrap().player.as_ref() {
                p.toggle_mute();
            }
        });
    }
    {
        let ww = window.as_weak();
        window.on_toggle_stats(move || {
            let Some(w) = ww.upgrade() else { return; };
            w.set_stats_visible(!w.get_stats_visible());
        });
    }
    {
        let ww = window.as_weak();
        window.on_minimize_player(move || {
            let Some(w) = ww.upgrade() else { return; };
            let behind = w.get_settings_video_behind();
            w.set_is_playing(false);
            w.set_has_background_player(true);
            w.set_video_behind_ui(behind);
            w.set_stats_visible(false);
        });
    }
    // ── settings changed ──────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        window.on_settings_changed(move || {
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

    // ── fullscreen toggle (F key / F11) ──────────────────────────────────────
    {
        let window_weak = window.as_weak();
        window.on_toggle_fullscreen(move || {
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
        window.on_sign_out(move || {
            let _ = std::fs::remove_file(config_path());
            let _ = std::fs::remove_file(item_cache_path());
            let mut s = state.lock().unwrap();
            s.client = None;
            s.media_raw.clear();
            s.all_movies.clear();
            s.all_series.clear();
            s.filtered_items.clear();
            s.series_open_id.clear();
            s.series_season_ids.clear();
            s.series_episode_items.clear();
            drop(s);
            if let Some(w) = window_weak.upgrade() {
                w.set_show_login(true);
                w.set_active_nav(0);
                w.set_show_browse(false);
                w.set_server_url(ss(""));
            }
        });
    }

    window.on_quit(|| { slint::quit_event_loop().ok(); });

    window.invoke_grab_keyboard_focus();
    window.run()?;
    Ok(())
}
