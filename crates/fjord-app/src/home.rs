// ── fjord-app · home.rs ──────────────────────────────────────────────────────
//   HomeSection     named enum for the 17 poster-loading sections (replaces raw usize)
//   HomeData        continue-watching, next-up, recently-added, collections + music rows + favorites
//   cache_path      XDG_CACHE_HOME resolver: ~/.cache/fjord/<filename>
//   load_cache<T>   read + deserialize a JSON cache file
//   save_cache<T>   serialize + write a JSON cache file
//   home cache      load_home_cache, save_home_cache (JSON at ~/.cache/fjord/home.json)
//   library caches  load/save_movies_cache, load/save_series_cache, load/save_collections_cache, load/save_artists_cache, load/save_albums_cache, load/save_playlists_cache
//   fetch_home_data async: fetch all home rows in parallel; Recently Added rows use
//                   /Items/Latest (grouped, played incl.) — same as the Jellyfin web home
//   push_home_data  write HomeData into AppState global (called from UI thread)
//   home_data_sections  split HomeData into [(HomeSection, Vec<MediaItem>); 17]
//   refresh_favorites   re-fetch Movie/Series/MusicAlbum favorites and update AppState + posters
//   wire_nw_timer   30 s timer: refresh Not Watched rows when idle + tab visible
//   fetch_movie_collections  background: build movie_id → (boxset_id, boxset_name) map
//   run_poster_cache_cleanup  delete orphaned files from posters/ + backdrops/ (24 h guard)
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fjord_api::{models::MediaItem, JellyfinClient};
use serde::{Deserialize, Serialize};
use tracing::warn;

use slint::Global;
use crate::config::{FjordState, xdg_cache_base, poster_cache_dir, backdrop_cache_dir};
use crate::AppState;
use crate::playback::VideoState;
use crate::MainWindow;

// Named enum for the 17 dashboard poster-loading sections.
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
    RecentlyAddedAlbums      = 11,
    RecentlyPlayedAlbums     = 12,
    FavoriteMovies           = 13,
    FavoriteSeries           = 14,
    FavoriteAlbums           = 15,
    Playlists                = 16,
}

impl HomeSection {
    pub(crate) fn empty_array() -> [(HomeSection, Vec<MediaItem>); 17] {
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
            (HomeSection::RecentlyAddedAlbums,      vec![]),
            (HomeSection::RecentlyPlayedAlbums,     vec![]),
            (HomeSection::FavoriteMovies,           vec![]),
            (HomeSection::FavoriteSeries,           vec![]),
            (HomeSection::FavoriteAlbums,           vec![]),
            (HomeSection::Playlists,                vec![]),
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
    pub recently_added_albums:      Vec<MediaItem>,
    pub recently_played_albums:     Vec<MediaItem>,
    pub favorite_movies:            Vec<MediaItem>,
    pub favorite_series:            Vec<MediaItem>,
    pub favorite_albums:            Vec<MediaItem>,
    // Added in Phase 57 — default keeps pre-playlist home.json caches loadable.
    #[serde(default)]
    pub playlists:                  Vec<MediaItem>,
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

// ── Library list caches (movies.json / series.json / collections.json) ───────

fn movies_cache_path()      -> PathBuf { cache_path("movies.json") }
fn series_cache_path()      -> PathBuf { cache_path("series.json") }
fn collections_cache_path() -> PathBuf { cache_path("collections.json") }
fn artists_cache_path()     -> PathBuf { cache_path("artists.json") }
fn albums_cache_path()      -> PathBuf { cache_path("albums.json") }
fn playlists_cache_path()   -> PathBuf { cache_path("playlists.json") }

pub(crate) fn load_movies_cache()                     -> Option<Vec<MediaItem>> { load_cache(movies_cache_path()) }
pub(crate) fn save_movies_cache(items: &[MediaItem])                            { save_cache(movies_cache_path(), items) }
pub(crate) fn load_series_cache()                     -> Option<Vec<MediaItem>> { load_cache(series_cache_path()) }
pub(crate) fn save_series_cache(items: &[MediaItem])                            { save_cache(series_cache_path(), items) }
pub(crate) fn load_collections_cache()                -> Option<Vec<MediaItem>> { load_cache(collections_cache_path()) }
pub(crate) fn save_collections_cache(items: &[MediaItem])                       { save_cache(collections_cache_path(), items) }
pub(crate) fn load_artists_cache()                    -> Option<Vec<MediaItem>> { load_cache(artists_cache_path()) }
pub(crate) fn save_artists_cache(items: &[MediaItem])                           { save_cache(artists_cache_path(), items) }
pub(crate) fn load_albums_cache()                     -> Option<Vec<MediaItem>> { load_cache(albums_cache_path()) }
pub(crate) fn save_albums_cache(items: &[MediaItem])                            { save_cache(albums_cache_path(), items) }
pub(crate) fn load_playlists_cache()                  -> Option<Vec<MediaItem>> { load_cache(playlists_cache_path()) }
pub(crate) fn save_playlists_cache(items: &[MediaItem])                         { save_cache(playlists_cache_path(), items) }

pub(crate) async fn fetch_home_data(client: &JellyfinClient) -> HomeData {
    let (cw, nu, ra, ram, nwm, nwt, rac, uwc, raa, rpa, fam, fas, fal, pls) = tokio::join!(
        client.get_continue_watching(),
        client.get_next_up(),
        client.get_latest("Episode"),
        client.get_latest("Movie"),
        client.get_unwatched(Some("Movie")),
        client.get_unwatched(Some("Series")),
        client.get_recently_added_collections(),
        client.get_unwatched_collections(),
        client.get_latest("Audio"),
        client.get_recently_played_albums(),
        client.get_favorites("Movie"),
        client.get_favorites("Series"),
        client.get_favorites("MusicAlbum"),
        client.get_all_playlists(),
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
        recently_added_albums:      raa.unwrap_or_else(|e| { warn!("recently_added_albums: {:#}", e);      vec![] }),
        recently_played_albums:     rpa.unwrap_or_else(|e| { warn!("recently_played_albums: {:#}", e);     vec![] }),
        favorite_movies:            fam.unwrap_or_else(|e| { warn!("favorite_movies: {:#}", e);            vec![] }),
        favorite_series:            fas.unwrap_or_else(|e| { warn!("favorite_series: {:#}", e);            vec![] }),
        favorite_albums:            fal.unwrap_or_else(|e| { warn!("favorite_albums: {:#}", e);            vec![] }),
        playlists:                  pls.unwrap_or_else(|e| { warn!("playlists: {:#}", e);                  vec![] }),
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
    g.set_recently_added_albums(crate::items_to_model(&hd.recently_added_albums));
    g.set_recently_played_albums(crate::items_to_model(&hd.recently_played_albums));
    g.set_favorite_movies(crate::items_to_model(&hd.favorite_movies));
    g.set_favorite_series(crate::items_to_model(&hd.favorite_series));
    g.set_favorite_albums(crate::items_to_model(&hd.favorite_albums));
    g.set_music_playlists(crate::items_to_model(&hd.playlists));
}

pub(crate) fn home_data_sections(hd: &HomeData) -> [(HomeSection, Vec<MediaItem>); 17] {
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
        (HomeSection::RecentlyAddedAlbums,      hd.recently_added_albums.clone()),
        (HomeSection::RecentlyPlayedAlbums,     hd.recently_played_albums.clone()),
        (HomeSection::FavoriteMovies,           hd.favorite_movies.clone()),
        (HomeSection::FavoriteSeries,           hd.favorite_series.clone()),
        (HomeSection::FavoriteAlbums,           hd.favorite_albums.clone()),
        (HomeSection::Playlists,                hd.playlists.clone()),
    ]
}

/// Re-fetch Movie/Series/MusicAlbum favorites from the server and update the three favorites rows
/// in AppState. Called after any successful favourite toggle so the rows update immediately
/// without waiting for the next full home refresh or app restart.
pub(crate) fn refresh_favorites(
    client: Arc<JellyfinClient>,
    ww:     slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
) {
    rt.spawn(async move {
        let (fam, fas, fal) = tokio::join!(
            client.get_favorites("Movie"),
            client.get_favorites("Series"),
            client.get_favorites("MusicAlbum"),
        );
        let fam = fam.unwrap_or_else(|e| { warn!("refresh favorite_movies: {:#}", e); vec![] });
        let fas = fas.unwrap_or_else(|e| { warn!("refresh favorite_series: {:#}", e); vec![] });
        let fal = fal.unwrap_or_else(|e| { warn!("refresh favorite_albums: {:#}", e); vec![] });

        // Push metadata immediately (no posters yet; poster loading fills them in below)
        let ww2  = ww.clone();
        let fam2 = fam.clone();
        let fas2 = fas.clone();
        let fal2 = fal.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww2.upgrade() {
                let g = AppState::get(&w);
                g.set_favorite_movies(crate::items_to_model(&fam2));
                g.set_favorite_series(crate::items_to_model(&fas2));
                g.set_favorite_albums(crate::items_to_model(&fal2));
            }
        });

        let mut sections = HomeSection::empty_array();
        sections[13].1 = fam;
        sections[14].1 = fas;
        sections[15].1 = fal;
        let rt2 = tokio::runtime::Handle::current();
        crate::poster::spawn_poster_loading(client, sections, ww, rt2);
    });
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

// ── Poster / backdrop cache cleanup ──────────────────────────────────────────

fn last_cleanup_path() -> std::path::PathBuf {
    xdg_cache_base().join("fjord").join("last_cleanup")
}

fn read_last_cleanup() -> Option<u64> {
    std::fs::read_to_string(last_cleanup_path()).ok()?.trim().parse().ok()
}

/// Delete orphaned files from `posters/` and `backdrops/` cache directories.
/// A file is orphaned when its name (= item ID) is not in the current library.
/// Skips the run if the combined ID set is empty (network error / first run)
/// or if cleanup ran within the last 24 h.
pub(crate) async fn run_poster_cache_cleanup(
    movie_ids:      Vec<String>,
    series_ids:     Vec<String>,
    collection_ids: Vec<String>,
    artist_ids:     Vec<String>,
    album_ids:      Vec<String>,
    playlist_ids:   Vec<String>,
) {
    use std::collections::HashSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    if movie_ids.is_empty() && series_ids.is_empty() && collection_ids.is_empty() && artist_ids.is_empty() && album_ids.is_empty() && playlist_ids.is_empty() { return; }

    let now_secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    if let Some(last) = read_last_cleanup() {
        if now_secs.saturating_sub(last) < 86_400 { return; }
    }

    let known: HashSet<String> = movie_ids.into_iter()
        .chain(series_ids)
        .chain(collection_ids)
        .chain(artist_ids)
        .chain(album_ids)
        .chain(playlist_ids)
        .collect();

    let mut deleted = 0u32;
    for dir in [poster_cache_dir(), backdrop_cache_dir()] {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else { continue };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // .tag sidecars (artwork revalidation) and .tmp leftovers belong to
            // their base image id — judge them by that id, not the full name.
            let base = name.strip_suffix(".tag.tmp")
                .or_else(|| name.strip_suffix(".tag"))
                .or_else(|| name.strip_suffix(".tmp"))
                .unwrap_or(&name);
            if !known.contains(base) {
                if tokio::fs::remove_file(entry.path()).await.is_ok() { deleted += 1; }
            }
        }
    }

    let ts = now_secs.to_string();
    let p  = last_cleanup_path();
    if let Some(parent) = p.parent() { let _ = tokio::fs::create_dir_all(parent).await; }
    let _ = tokio::fs::write(&p, ts.as_bytes()).await;

    if deleted > 0 {
        tracing::info!("poster cache cleanup: deleted {deleted} orphaned file(s)");
    }
}
