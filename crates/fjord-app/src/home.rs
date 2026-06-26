// ── fjord-app · home.rs ──────────────────────────────────────────────────────
//   HomeSection     named enum for the 11 poster-loading sections (replaces raw usize)
//   HomeData        continue-watching, next-up, recently-added, collections rows
//   cache_path      XDG_CACHE_HOME resolver: ~/.cache/fjord/<filename>
//   load_cache<T>   read + deserialize a JSON cache file
//   save_cache<T>   serialize + write a JSON cache file
//   home cache      load_home_cache, save_home_cache (JSON at ~/.cache/fjord/home.json)
//   library caches  load/save_movies_cache, load/save_series_cache (movies.json, series.json)
//   fetch_home_data async: fetch all home rows from Jellyfin in parallel
//   push_home_data  write HomeData into AppState global (called from UI thread)
//   home_data_sections  split HomeData into [(HomeSection, Vec<MediaItem>); 11]
//   wire_nw_timer   30 s timer: refresh Not Watched rows when idle + tab visible
//   fetch_movie_collections  background: build movie_id → (boxset_id, boxset_name) map
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fjord_api::{models::MediaItem, JellyfinClient};
use serde::{Deserialize, Serialize};
use tracing::warn;

use slint::Global;
use crate::config::{FjordState, xdg_cache_base};
use crate::AppState;
use crate::playback::VideoState;
use crate::MainWindow;

// Named enum for the 11 dashboard poster-loading sections.
// The discriminant equals the array index used in spawn_poster_loading.
#[repr(usize)]
#[derive(Copy, Clone)]
pub(crate) enum HomeSection {
    ContinueWatching         = 0,
    NextUp                   = 1,
    RecentlyAdded            = 2,
    ContinueWatchingMovies   = 3,
    RecentlyAddedMovies      = 4,
    NotWatchedMovies         = 5,
    ContinueWatchingTv       = 6,
    RecentlyAddedTv          = 7,
    NotWatchedTv             = 8,
    RecentlyAddedCollections = 9,
    UnwatchedCollections     = 10,
}

impl HomeSection {
    pub(crate) fn empty_array() -> [(HomeSection, Vec<MediaItem>); 11] {
        [
            (HomeSection::ContinueWatching,         vec![]),
            (HomeSection::NextUp,                   vec![]),
            (HomeSection::RecentlyAdded,            vec![]),
            (HomeSection::ContinueWatchingMovies,   vec![]),
            (HomeSection::RecentlyAddedMovies,      vec![]),
            (HomeSection::NotWatchedMovies,         vec![]),
            (HomeSection::ContinueWatchingTv,       vec![]),
            (HomeSection::RecentlyAddedTv,          vec![]),
            (HomeSection::NotWatchedTv,             vec![]),
            (HomeSection::RecentlyAddedCollections, vec![]),
            (HomeSection::UnwatchedCollections,     vec![]),
        ]
    }
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct HomeData {
    pub continue_watching:          Vec<MediaItem>,
    pub next_up:                    Vec<MediaItem>,
    pub recently_added_movies:      Vec<MediaItem>,
    pub recently_added_tv:          Vec<MediaItem>,
    pub not_watched_movies:         Vec<MediaItem>,
    pub not_watched_tv:             Vec<MediaItem>,
    pub recently_added_collections: Vec<MediaItem>,
    pub unwatched_collections:      Vec<MediaItem>,
}

fn cache_path(filename: &str) -> PathBuf {
    xdg_cache_base().join("fjord").join(filename)
}

fn load_cache<T: serde::de::DeserializeOwned>(path: PathBuf) -> Option<T> {
    let data = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cache<T: serde::Serialize + ?Sized>(path: PathBuf, data: &T) {
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string(data) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
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
    let (cw, nu, ra, ram, nwm, nwt, rac, uwc) = tokio::join!(
        client.get_continue_watching(),
        client.get_next_up(),
        client.get_recently_added(Some("Series")),
        client.get_recently_added(Some("Movie")),
        client.get_unwatched(Some("Movie")),
        client.get_unwatched(Some("Series")),
        client.get_recently_added_collections(),
        client.get_unwatched_collections(),
    );
    HomeData {
        continue_watching:          cw.unwrap_or_else(|e|  { warn!("continue_watching: {:#}", e);          vec![] }),
        next_up:                    nu.unwrap_or_else(|e|  { warn!("next_up: {:#}", e);                    vec![] }),
        recently_added_tv:          ra.unwrap_or_else(|e|  { warn!("recently_added_tv: {:#}", e);          vec![] }),
        recently_added_movies:      ram.unwrap_or_else(|e| { warn!("recently_added_movies: {:#}", e);      vec![] }),
        not_watched_movies:         nwm.unwrap_or_else(|e| { warn!("not_watched_movies: {:#}", e);         vec![] }),
        not_watched_tv:             nwt.unwrap_or_else(|e| { warn!("not_watched_tv: {:#}", e);             vec![] }),
        recently_added_collections: rac.unwrap_or_else(|e| { warn!("recently_added_collections: {:#}", e); vec![] }),
        unwatched_collections:      uwc.unwrap_or_else(|e| { warn!("unwatched_collections: {:#}", e);      vec![] }),
    }
}

pub(crate) fn push_home_data(window: &MainWindow, hd: &HomeData) {
    let cw_movies: Vec<_> = hd.continue_watching.iter().filter(|i| i.item_type == "Movie").cloned().collect();
    let cw_tv:     Vec<_> = hd.continue_watching.iter().filter(|i| i.item_type == "Episode").cloned().collect();
    let g = AppState::get(window);
    g.set_continue_watching(crate::items_to_model(&hd.continue_watching));
    g.set_next_up(crate::items_to_model(&hd.next_up));
    g.set_recently_added(crate::items_to_model(&hd.recently_added_tv));
    g.set_continue_watching_movies(crate::items_to_model(&cw_movies));
    g.set_recently_added_movies(crate::items_to_model(&hd.recently_added_movies));
    g.set_not_watched_movies(crate::items_to_model(&hd.not_watched_movies));
    g.set_continue_watching_tv(crate::items_to_model(&cw_tv));
    g.set_recently_added_tv(crate::items_to_model(&hd.recently_added_tv));
    g.set_not_watched_tv(crate::items_to_model(&hd.not_watched_tv));
    g.set_recently_added_collections(crate::items_to_model(&hd.recently_added_collections));
    g.set_unwatched_collections(crate::items_to_model(&hd.unwatched_collections));
}

pub(crate) fn home_data_sections(hd: &HomeData) -> [(HomeSection, Vec<MediaItem>); 11] {
    let cw_movies = hd.continue_watching.iter().filter(|i| i.item_type == "Movie").cloned().collect();
    let cw_tv     = hd.continue_watching.iter().filter(|i| i.item_type == "Episode").cloned().collect();
    [
        (HomeSection::ContinueWatching,         hd.continue_watching.clone()),
        (HomeSection::NextUp,                   hd.next_up.clone()),
        (HomeSection::RecentlyAdded,            hd.recently_added_tv.clone()),
        (HomeSection::ContinueWatchingMovies,   cw_movies),
        (HomeSection::RecentlyAddedMovies,      hd.recently_added_movies.clone()),
        (HomeSection::NotWatchedMovies,         hd.not_watched_movies.clone()),
        (HomeSection::ContinueWatchingTv,       cw_tv),
        (HomeSection::RecentlyAddedTv,          hd.recently_added_tv.clone()),
        (HomeSection::NotWatchedTv,             hd.not_watched_tv.clone()),
        (HomeSection::RecentlyAddedCollections, hd.recently_added_collections.clone()),
        (HomeSection::UnwatchedCollections,     hd.unwatched_collections.clone()),
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
        if nav != 2 && nav != 1 { return; }  // nav=2=Movies, nav=1=TV

        let (due_movies, due_tv) = {
            let s = state.lock().unwrap();
            (
                nav == 2 && s.last_nw_mov_refresh.map_or(true, |t| t.elapsed() >= Duration::from_secs(600)),
                nav == 1 && s.last_nw_tv_refresh.map_or(true,  |t| t.elapsed() >= Duration::from_secs(600)),
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
                        let mut sections = HomeSection::empty_array();
                        sections[HomeSection::NotWatchedMovies as usize].1 = items;
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
                        let mut sections = HomeSection::empty_array();
                        sections[HomeSection::NotWatchedTv as usize].1 = items;
                        crate::spawn_poster_loading(client, sections, ww, rt2);
                    }
                }
            }
        });
    });
    timer_nw
}

// ── Collection map ────────────────────────────────────────────────────────────

/// Fetch all BoxSets and their member IDs, building a reverse map from movie_id to
/// (boxset_id, boxset_name). Called once in the background after login; stored in FjordState.
pub(crate) async fn fetch_movie_collections(
    client: &JellyfinClient,
) -> HashMap<String, (String, String)> {
    let boxsets = match client.get_all_boxsets().await {
        Ok(b)  => b,
        Err(e) => { warn!("get_all_boxsets: {:#}", e); return HashMap::new(); }
    };

    let sem = Arc::new(tokio::sync::Semaphore::new(4));
    let mut tasks: tokio::task::JoinSet<Vec<(String, String, String)>> = tokio::task::JoinSet::new();
    for bs in boxsets {
        let client_c = client.clone();
        let sem_c    = sem.clone();
        let bs_id    = bs.id.clone();
        let bs_name  = bs.name.clone();
        tasks.spawn(async move {
            let _permit = sem_c.acquire_owned().await.ok();
            let items = client_c.get_boxset_items(&bs_id).await.unwrap_or_default();
            items.into_iter().map(|i| (i.id, bs_id.clone(), bs_name.clone())).collect()
        });
    }

    let mut map = HashMap::new();
    while let Some(res) = tasks.join_next().await {
        if let Ok(entries) = res {
            for (movie_id, bs_id, bs_name) in entries {
                map.insert(movie_id, (bs_id, bs_name));
            }
        }
    }
    map
}
