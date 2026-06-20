// ── fjord-app · home.rs ──────────────────────────────────────────────────────
//   HomeData        continue-watching, next-up, recently-added rows (all tabs)
//   cache_path      XDG_CACHE_HOME resolver: ~/.cache/fjord/<filename>
//   load_cache<T>   read + deserialize a JSON cache file
//   save_cache<T>   serialize + write a JSON cache file
//   home cache      load_home_cache, save_home_cache (JSON at ~/.cache/fjord/home.json)
//   library caches  load/save_movies_cache, load/save_series_cache (movies.json, series.json)
//   fetch_home_data async: fetch all home rows from Jellyfin in parallel
//   push_home_data  write HomeData into AppState global (called from UI thread)
//   home_data_sections  split HomeData into poster-loading sections array
//   wire_nw_timer   30 s timer: refresh Not Watched rows when idle + tab visible
// ─────────────────────────────────────────────────────────────────────────────
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fjord_api::{models::MediaItem, JellyfinClient};
use serde::{Deserialize, Serialize};
use tracing::warn;

use slint::Global;
use crate::config::FjordState;
use crate::AppState;
use crate::playback::VideoState;
use crate::MainWindow;

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct HomeData {
    pub continue_watching:     Vec<MediaItem>,
    pub next_up:               Vec<MediaItem>,
    pub recently_added:        Vec<MediaItem>,
    pub recently_added_movies: Vec<MediaItem>,
    pub recently_added_tv:     Vec<MediaItem>,
    pub not_watched_movies:    Vec<MediaItem>,
    pub not_watched_tv:        Vec<MediaItem>,
}

fn cache_path(filename: &str) -> PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            PathBuf::from(home).join(".cache")
        });
    base.join("fjord").join(filename)
}

fn load_cache<T: serde::de::DeserializeOwned>(path: PathBuf) -> Option<T> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cache<T: serde::Serialize + ?Sized>(path: PathBuf, data: &T) {
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string(data) { let _ = std::fs::write(&path, json); }
}

pub(crate) fn home_cache_path() -> PathBuf { cache_path("home.json") }
pub(crate) fn load_home_cache()            -> Option<HomeData>     { load_cache(home_cache_path()) }
pub(crate) fn save_home_cache(hd: &HomeData)                       { save_cache(home_cache_path(), hd) }

// ── Library list caches (movies.json / series.json) ───────────────────────────

fn movies_cache_path() -> PathBuf { cache_path("movies.json") }
fn series_cache_path() -> PathBuf { cache_path("series.json") }

pub(crate) fn load_movies_cache()                     -> Option<Vec<MediaItem>> { load_cache(movies_cache_path()) }
pub(crate) fn save_movies_cache(items: &[MediaItem])                            { save_cache(movies_cache_path(), items) }
pub(crate) fn load_series_cache()                     -> Option<Vec<MediaItem>> { load_cache(series_cache_path()) }
pub(crate) fn save_series_cache(items: &[MediaItem])                            { save_cache(series_cache_path(), items) }

pub(crate) async fn fetch_home_data(client: &JellyfinClient) -> HomeData {
    let (cw, nu, ra, ram, nwm, nwt) = tokio::join!(
        client.get_continue_watching(),
        client.get_next_up(),
        client.get_recently_added(Some("Series")),
        client.get_recently_added(Some("Movie")),
        client.get_unwatched(Some("Movie")),
        client.get_unwatched(Some("Series")),
    );
    // recently_added and recently_added_tv are the same query (CR2-6).
    let recently_added = ra.unwrap_or_else(|e| { warn!("recently_added: {:#}", e); vec![] });
    HomeData {
        continue_watching:     cw.unwrap_or_else(|e|  { warn!("continue_watching: {:#}", e);     vec![] }),
        next_up:               nu.unwrap_or_else(|e|  { warn!("next_up: {:#}", e);               vec![] }),
        recently_added_tv:     recently_added.clone(),
        recently_added,
        recently_added_movies: ram.unwrap_or_else(|e| { warn!("recently_added_movies: {:#}", e); vec![] }),
        not_watched_movies:    nwm.unwrap_or_else(|e| { warn!("not_watched_movies: {:#}", e);    vec![] }),
        not_watched_tv:        nwt.unwrap_or_else(|e| { warn!("not_watched_tv: {:#}", e);        vec![] }),
    }
}

pub(crate) fn push_home_data(window: &MainWindow, hd: &HomeData) {
    let cw_movies: Vec<_> = hd.continue_watching.iter().filter(|i| i.item_type == "Movie").cloned().collect();
    let cw_tv:     Vec<_> = hd.continue_watching.iter().filter(|i| i.item_type == "Episode").cloned().collect();
    let g = AppState::get(window);
    g.set_continue_watching(crate::items_to_model(&hd.continue_watching));
    g.set_next_up(crate::items_to_model(&hd.next_up));
    g.set_recently_added(crate::items_to_model(&hd.recently_added));
    g.set_continue_watching_movies(crate::items_to_model(&cw_movies));
    g.set_recently_added_movies(crate::items_to_model(&hd.recently_added_movies));
    g.set_not_watched_movies(crate::items_to_model(&hd.not_watched_movies));
    g.set_continue_watching_tv(crate::items_to_model(&cw_tv));
    g.set_recently_added_tv(crate::items_to_model(&hd.recently_added_tv));
    g.set_not_watched_tv(crate::items_to_model(&hd.not_watched_tv));
}

pub(crate) fn home_data_sections(hd: &HomeData) -> [Vec<MediaItem>; 9] {
    let cw_movies = hd.continue_watching.iter().filter(|i| i.item_type == "Movie").cloned().collect();
    let cw_tv     = hd.continue_watching.iter().filter(|i| i.item_type == "Episode").cloned().collect();
    [
        hd.continue_watching.clone(),
        hd.next_up.clone(),
        hd.recently_added.clone(),
        cw_movies,
        hd.recently_added_movies.clone(),
        hd.not_watched_movies.clone(),
        cw_tv,
        hd.recently_added_tv.clone(),
        hd.not_watched_tv.clone(),
    ]
}

pub(crate) fn wire_nw_timer(
    window_weak: slint::Weak<MainWindow>,
    video:       Arc<Mutex<VideoState>>,
    state:       Arc<Mutex<FjordState>>,
    rt_handle:   tokio::runtime::Handle,
) -> slint::Timer {
    let timer_nw = slint::Timer::default();
    timer_nw.start(slint::TimerMode::Repeated, Duration::from_secs(30), move || {
        if video.lock().unwrap().player.is_some() { return; }
        let Some(w) = window_weak.upgrade() else { return };
        let nav = AppState::get(&w).get_active_nav();
        if nav != 1 && nav != 2 { return; }

        let (due_movies, due_tv) = {
            let s = state.lock().unwrap();
            (
                nav == 1 && s.last_nw_mov_refresh.map_or(true,    |t| t.elapsed() >= Duration::from_secs(600)),
                nav == 2 && s.last_nw_tv_refresh.map_or(true, |t| t.elapsed() >= Duration::from_secs(600)),
            )
        };
        if !due_movies && !due_tv { return; }

        let client = state.lock().unwrap().client.as_ref().map(Arc::clone);
        let Some(client) = client else { return };

        let ww     = window_weak.clone();
        let rt2    = rt_handle.clone();
        let state2 = Arc::clone(&state);
        rt_handle.spawn(async move {
            // Stamp the cooldown only after a successful fetch (CR-9):
            // stamping before means a network error silently resets the 10-min cooldown.
            if due_movies {
                match client.get_unwatched(Some("Movie")).await {
                    Err(e) if crate::is_unauthorized(&e) => {
                        warn!("get_unwatched (movies) 401 — session expired");
                        let ww2 = ww.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_show_login(true);
                                AppState::get(&w).set_status("Session expired — please log in again".into());
                            }
                        });
                        return;
                    }
                    Err(_) => return,
                    Ok(items) => {
                        state2.lock().unwrap().last_nw_mov_refresh = Some(Instant::now());
                        let ww2    = ww.clone();
                        let items2 = items.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() { AppState::get(&w).set_not_watched_movies(crate::items_to_model(&items2)); }
                        });
                        let mut sections: [Vec<MediaItem>; 9] = Default::default();
                        sections[5] = items;
                        crate::spawn_poster_loading(Arc::clone(&client), sections, ww.clone(), rt2.clone());
                    }
                }
            }
            if due_tv {
                match client.get_unwatched(Some("Series")).await {
                    Err(e) if crate::is_unauthorized(&e) => {
                        warn!("get_unwatched (tv) 401 — session expired");
                        let ww2 = ww.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_show_login(true);
                                AppState::get(&w).set_status("Session expired — please log in again".into());
                            }
                        });
                        return;
                    }
                    Err(_) => return,
                    Ok(items) => {
                        state2.lock().unwrap().last_nw_tv_refresh = Some(Instant::now());
                        let ww2    = ww.clone();
                        let items2 = items.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() { AppState::get(&w).set_not_watched_tv(crate::items_to_model(&items2)); }
                        });
                        let mut sections: [Vec<MediaItem>; 9] = Default::default();
                        sections[8] = items;
                        crate::spawn_poster_loading(client, sections, ww, rt2);
                    }
                }
            }
        });
    });
    timer_nw
}
