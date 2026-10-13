// ── fjord-app · home.rs ──────────────────────────────────────────────────────
//   HomeSection     named enum for the 17 poster-loading sections
//   HomeData        continue-watching, next-up, recently-added, collections + music rows +
//                   favorites
//   profile_cache_dir/cache_path  ~/.cache/fjord/profiles/<user_id>/<filename>; None (nothing
//                   read/written) unless user_id passes config::safe_cache_name; user_id is always
//                   passed in, never resolved here
//   load_cache<T> / save_cache<T>  JSON cache file read / write
//   home cache      load_home_cache, save_home_cache (home.json)
//   library caches  load/save_movies_cache, _series_cache, _collections_cache, _artists_cache,
//                   _albums_cache, _playlists_cache
//   fetch_home_data async: every home row in parallel (Recently Added via /Items/Latest — grouped,
//                   played included, like the web home); include_not_watched = false skips the slow
//                   Not Watched rows (only finish_session_setup's login join; auth.rs's
//                   spawn_not_watched_rows fetches them afterwards)
//   push_home_data  HomeData → AppState (UI thread); takes the watchlist id snapshot (a fresh build
//                   has no old row to carry on_watchlist from)
//   push_home_data_preserving_posters / refresh_row_preserving_posters  merge fresh items against
//                   the old model (posters and on_watchlist kept by id), applied through
//                   apply_cards_preserving_identity — used by ws.rs's delta sync, the auto-login
//                   refresh and spawn_library_fetch
//   home_data_sections  HomeData → [(HomeSection, Vec<MediaItem>); 17]
//   refresh_favorites   re-fetch the favorites rows after a toggle (posters preserved)
//   wire_nw_timer   30 s timer: refresh the Not Watched rows when idle + tab visible; failures are
//                   logged, and a movies failure doesn't skip the TV fetch
//   fetch_movie_collections  background: movie_id → (boxset_id, boxset_name) map
//   run_poster_cache_cleanup  delete orphaned posters/ + backdrops/ files (24 h guard; known ids =
//                   the six library lists + detail cache keys + credited person ids)
//   wire_item_play  callbacks moved from main() (0.5.0 step 3): play from home / library rows
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fjord_api::{JellyfinClient, models::MediaItem};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::AppState;
use crate::config::{FjordState, backdrop_cache_dir, poster_cache_dir, xdg_cache_base};
use crate::playback::VideoState;
use crate::{CardItem, MainWindow};
use slint::{Global, Model, ModelRc};

// Named enum for the 17 dashboard poster-loading sections.
// The discriminant equals the array index used in spawn_poster_loading.
#[repr(usize)]
#[derive(Copy, Clone)]
pub(crate) enum HomeSection {
    ContinueWatching = 0,
    NextUp = 1,
    RecentlyAdded = 2,
    ContinueWatchingMovies = 3,
    RecentlyAddedMovies = 4,
    NotWatchedMovies = 5,
    ContinueWatchingTv = 6,
    RecentlyAddedTv = 7,
    NotWatchedTv = 8,
    RecentlyAddedCollections = 9,
    UnwatchedCollections = 10,
    RecentlyAddedAlbums = 11,
    RecentlyPlayedAlbums = 12,
    FavoriteMovies = 13,
    FavoriteSeries = 14,
    FavoriteAlbums = 15,
    Playlists = 16,
}

impl HomeSection {
    pub(crate) fn empty_array() -> [(HomeSection, Vec<MediaItem>); 17] {
        [
            (HomeSection::ContinueWatching, vec![]),
            (HomeSection::NextUp, vec![]),
            (HomeSection::RecentlyAdded, vec![]),
            (HomeSection::ContinueWatchingMovies, vec![]),
            (HomeSection::RecentlyAddedMovies, vec![]),
            (HomeSection::NotWatchedMovies, vec![]),
            (HomeSection::ContinueWatchingTv, vec![]),
            (HomeSection::RecentlyAddedTv, vec![]),
            (HomeSection::NotWatchedTv, vec![]),
            (HomeSection::RecentlyAddedCollections, vec![]),
            (HomeSection::UnwatchedCollections, vec![]),
            (HomeSection::RecentlyAddedAlbums, vec![]),
            (HomeSection::RecentlyPlayedAlbums, vec![]),
            (HomeSection::FavoriteMovies, vec![]),
            (HomeSection::FavoriteSeries, vec![]),
            (HomeSection::FavoriteAlbums, vec![]),
            (HomeSection::Playlists, vec![]),
        ]
    }
}

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct HomeData {
    pub continue_watching: Vec<MediaItem>,
    pub next_up: Vec<MediaItem>,
    pub recently_added_movies: Vec<MediaItem>,
    pub recently_added_tv: Vec<MediaItem>,
    pub not_watched_movies: Vec<MediaItem>,
    pub not_watched_tv: Vec<MediaItem>,
    pub recently_added_collections: Vec<MediaItem>,
    pub unwatched_collections: Vec<MediaItem>,
    pub recently_added_albums: Vec<MediaItem>,
    pub recently_played_albums: Vec<MediaItem>,
    pub favorite_movies: Vec<MediaItem>,
    pub favorite_series: Vec<MediaItem>,
    pub favorite_albums: Vec<MediaItem>,
    // Added in Phase 57 — default keeps pre-playlist home.json caches loadable.
    #[serde(default)]
    pub playlists: Vec<MediaItem>,
}

// Every on-disk cache below is scoped under the OWNING profile's Jellyfin user_id, so a
// second profile never sees the first one's library/watch state at warm start (posters and
// backdrops stay shared). `user_id` is always passed in, captured with the client/fetch —
// never re-derived at save time (a profile switch can happen in between). None when user_id
// isn't a valid cache name (config::safe_cache_name): nothing is read or written.
fn profile_cache_dir(user_id: &str) -> Option<PathBuf> {
    Some(
        xdg_cache_base()
            .join("fjord")
            .join("profiles")
            .join(crate::config::safe_cache_name(user_id)?),
    )
}

fn cache_path(user_id: &str, filename: &str) -> Option<PathBuf> {
    Some(profile_cache_dir(user_id)?.join(filename))
}

fn load_cache<T: serde::de::DeserializeOwned>(path: Option<PathBuf>) -> Option<T> {
    let data = std::fs::read_to_string(path?).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_cache<T: serde::Serialize + ?Sized>(path: Option<PathBuf>, data: &T) {
    let Some(path) = path else {
        warn!("cache: not a valid user id for a cache folder — not saved");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string(data) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

pub(crate) fn home_cache_path(user_id: &str) -> Option<PathBuf> {
    cache_path(user_id, "home.json")
}
pub(crate) fn load_home_cache(user_id: &str) -> Option<HomeData> {
    load_cache(home_cache_path(user_id))
}
pub(crate) fn save_home_cache(user_id: &str, hd: &HomeData) {
    save_cache(home_cache_path(user_id), hd)
}

// ── Library list caches (movies.json / series.json / collections.json) ───────

fn movies_cache_path(user_id: &str) -> Option<PathBuf> {
    cache_path(user_id, "movies.json")
}
fn series_cache_path(user_id: &str) -> Option<PathBuf> {
    cache_path(user_id, "series.json")
}
fn collections_cache_path(user_id: &str) -> Option<PathBuf> {
    cache_path(user_id, "collections.json")
}
fn artists_cache_path(user_id: &str) -> Option<PathBuf> {
    cache_path(user_id, "artists.json")
}
fn albums_cache_path(user_id: &str) -> Option<PathBuf> {
    cache_path(user_id, "albums.json")
}
fn playlists_cache_path(user_id: &str) -> Option<PathBuf> {
    cache_path(user_id, "playlists.json")
}

pub(crate) fn load_movies_cache(user_id: &str) -> Option<Vec<MediaItem>> {
    load_cache(movies_cache_path(user_id))
}
pub(crate) fn save_movies_cache(user_id: &str, items: &[MediaItem]) {
    save_cache(movies_cache_path(user_id), items)
}
pub(crate) fn load_series_cache(user_id: &str) -> Option<Vec<MediaItem>> {
    load_cache(series_cache_path(user_id))
}
pub(crate) fn save_series_cache(user_id: &str, items: &[MediaItem]) {
    save_cache(series_cache_path(user_id), items)
}
pub(crate) fn load_collections_cache(user_id: &str) -> Option<Vec<MediaItem>> {
    load_cache(collections_cache_path(user_id))
}
pub(crate) fn save_collections_cache(user_id: &str, items: &[MediaItem]) {
    save_cache(collections_cache_path(user_id), items)
}
pub(crate) fn load_artists_cache(user_id: &str) -> Option<Vec<MediaItem>> {
    load_cache(artists_cache_path(user_id))
}
pub(crate) fn save_artists_cache(user_id: &str, items: &[MediaItem]) {
    save_cache(artists_cache_path(user_id), items)
}
pub(crate) fn load_albums_cache(user_id: &str) -> Option<Vec<MediaItem>> {
    load_cache(albums_cache_path(user_id))
}
pub(crate) fn save_albums_cache(user_id: &str, items: &[MediaItem]) {
    save_cache(albums_cache_path(user_id), items)
}
pub(crate) fn load_playlists_cache(user_id: &str) -> Option<Vec<MediaItem>> {
    load_cache(playlists_cache_path(user_id))
}
pub(crate) fn save_playlists_cache(user_id: &str, items: &[MediaItem]) {
    save_cache(playlists_cache_path(user_id), items)
}

/// `include_not_watched`: the Not Watched Movies/TV rows are the slowest queries (Jellyfin's
/// SortBy=Random + recursive unplayed filter for series takes seconds server-side) and only
/// appear on the Movies/TV dashboards, so login doesn't wait for them: `false` only for
/// finish_session_setup's login join (auth.rs fetches them afterwards). With `true` (WS delta,
/// playback stop, auto-login refresh — all background) the pair runs as its own join right
/// after the main 12-branch one.
pub(crate) async fn fetch_home_data(
    client: &JellyfinClient,
    include_not_watched: bool,
) -> HomeData {
    let (cw, nu, ra, ram, rac, uwc, raa, rpa, fam, fas, fal, pls) = tokio::join!(
        crate::timed("continue_watching", client.get_continue_watching()),
        crate::timed("next_up", client.get_next_up()),
        crate::timed("recently_added_tv", client.get_latest("Episode")),
        crate::timed("recently_added_movies", client.get_latest("Movie")),
        crate::timed(
            "recently_added_collections",
            client.get_recently_added_collections()
        ),
        crate::timed("unwatched_collections", client.get_unwatched_collections()),
        crate::timed("recently_added_albums", client.get_latest_music()),
        crate::timed(
            "recently_played_albums",
            client.get_recently_played_albums()
        ),
        crate::timed("favorite_movies", client.get_favorites("Movie")),
        crate::timed("favorite_series", client.get_favorites("Series")),
        crate::timed("favorite_albums", client.get_favorites("MusicAlbum")),
        crate::timed("playlists", client.get_all_playlists()),
    );
    let (nwm, nwt) = if include_not_watched {
        tokio::join!(
            crate::timed("not_watched_movies", client.get_unwatched(Some("Movie"))),
            crate::timed("not_watched_tv", client.get_unwatched(Some("Series"))),
        )
    } else {
        (Ok(vec![]), Ok(vec![]))
    };
    HomeData {
        continue_watching: cw.unwrap_or_else(|e| {
            warn!("continue_watching: {:#}", e);
            vec![]
        }),
        next_up: nu.unwrap_or_else(|e| {
            warn!("next_up: {:#}", e);
            vec![]
        }),
        recently_added_tv: ra.unwrap_or_else(|e| {
            warn!("recently_added_tv: {:#}", e);
            vec![]
        }),
        recently_added_movies: ram.unwrap_or_else(|e| {
            warn!("recently_added_movies: {:#}", e);
            vec![]
        }),
        not_watched_movies: nwm.unwrap_or_else(|e| {
            warn!("not_watched_movies: {:#}", e);
            vec![]
        }),
        not_watched_tv: nwt.unwrap_or_else(|e| {
            warn!("not_watched_tv: {:#}", e);
            vec![]
        }),
        recently_added_collections: rac.unwrap_or_else(|e| {
            warn!("recently_added_collections: {:#}", e);
            vec![]
        }),
        unwatched_collections: uwc.unwrap_or_else(|e| {
            warn!("unwatched_collections: {:#}", e);
            vec![]
        }),
        recently_added_albums: raa.unwrap_or_else(|e| {
            warn!("recently_added_albums: {:#}", e);
            vec![]
        }),
        recently_played_albums: rpa.unwrap_or_else(|e| {
            warn!("recently_played_albums: {:#}", e);
            vec![]
        }),
        favorite_movies: fam.unwrap_or_else(|e| {
            warn!("favorite_movies: {:#}", e);
            vec![]
        }),
        favorite_series: fas.unwrap_or_else(|e| {
            warn!("favorite_series: {:#}", e);
            vec![]
        }),
        favorite_albums: fal.unwrap_or_else(|e| {
            warn!("favorite_albums: {:#}", e);
            vec![]
        }),
        playlists: pls.unwrap_or_else(|e| {
            warn!("playlists: {:#}", e);
            vec![]
        }),
    }
}

/// `watchlist` (FjordState.jellyfin_watchlist_ids snapshot): a fresh build has no previous
/// row to carry on_watchlist from (unlike refresh_row_preserving_posters).
pub(crate) fn push_home_data(
    window: &MainWindow,
    hd: &HomeData,
    watchlist: &std::collections::HashSet<String>,
) {
    let cw_movies: Vec<_> = hd
        .continue_watching
        .iter()
        .filter(|i| i.item_type == "Movie")
        .cloned()
        .collect();
    let cw_tv: Vec<_> = hd
        .continue_watching
        .iter()
        .filter(|i| i.item_type == "Episode")
        .cloned()
        .collect();
    let g = AppState::get(window);
    g.set_continue_watching(crate::items_to_model(&hd.continue_watching, watchlist));
    g.set_next_up(crate::items_to_model(&hd.next_up, watchlist));
    g.set_recently_added(crate::items_to_model(&hd.recently_added_tv, watchlist));
    g.set_continue_watching_movies(crate::items_to_model(&cw_movies, watchlist));
    g.set_recently_added_movies(crate::items_to_model(&hd.recently_added_movies, watchlist));
    g.set_not_watched_movies(crate::items_to_model(&hd.not_watched_movies, watchlist));
    g.set_continue_watching_tv(crate::items_to_model(&cw_tv, watchlist));
    g.set_recently_added_tv(crate::items_to_model(&hd.recently_added_tv, watchlist));
    g.set_not_watched_tv(crate::items_to_model(&hd.not_watched_tv, watchlist));
    g.set_recently_added_collections(crate::items_to_model(
        &hd.recently_added_collections,
        watchlist,
    ));
    g.set_unwatched_collections(crate::items_to_model(&hd.unwatched_collections, watchlist));
    g.set_recently_added_albums(crate::items_to_model(&hd.recently_added_albums, watchlist));
    g.set_recently_played_albums(crate::items_to_model(&hd.recently_played_albums, watchlist));
    g.set_favorite_movies(crate::items_to_model(&hd.favorite_movies, watchlist));
    g.set_favorite_series(crate::items_to_model(&hd.favorite_series, watchlist));
    g.set_favorite_albums(crate::items_to_model(&hd.favorite_albums, watchlist));
    g.set_music_playlists(crate::items_to_model(&hd.playlists, watchlist));
}

// Merge fresh server data into a home-row model, keeping already-decoded posters for rows
// that persist (by id). Rebuilt strictly from `fresh`, so removed rows disappear (an upsert
// would never remove). The apply itself is apply_cards_preserving_identity.
pub(crate) fn refresh_row_preserving_posters(
    old: &ModelRc<CardItem>,
    fresh: &[MediaItem],
) -> ModelRc<CardItem> {
    let old_by_id: HashMap<String, CardItem> = (0..old.row_count())
        .filter_map(|i| old.row_data(i))
        .map(|c| (c.id.to_string(), c))
        .collect();
    let rows: Vec<CardItem> = fresh
        .iter()
        .map(|item| {
            // No state here, so item_to_card_item's watchlist lookup misses; on_watchlist is
            // carried over from `old` like the poster (see FjordState.jellyfin_watchlist_ids).
            let mut card = crate::item_to_card_item(item, &std::collections::HashSet::new());
            if let Some(existing) = old_by_id.get(&item.id) {
                if existing.has_poster {
                    card.poster = existing.poster.clone();
                    card.has_poster = true;
                }
                card.on_watchlist = existing.on_watchlist;
            }
            card
        })
        .collect();
    crate::apply_cards_preserving_identity(old, rows)
}

/// Same field-by-field writes as push_home_data, but preserves already-decoded posters
/// for rows that persist across the refresh (see refresh_row_preserving_posters) instead
/// of wholesale-replacing every row with poster-less cards. Used by the WS delta-sync
/// debounced refresh (ws.rs) so a single favorite/resume toggle doesn't flash every
/// poster in the affected row while spawn_poster_loading re-decodes them from scratch —
/// push_home_data itself is left untouched for cold-start/login/retry call sites, where
/// there's no prior on-screen poster state worth preserving anyway.
pub(crate) fn push_home_data_preserving_posters(window: &MainWindow, hd: &HomeData) {
    let cw_movies: Vec<_> = hd
        .continue_watching
        .iter()
        .filter(|i| i.item_type == "Movie")
        .cloned()
        .collect();
    let cw_tv: Vec<_> = hd
        .continue_watching
        .iter()
        .filter(|i| i.item_type == "Episode")
        .cloned()
        .collect();
    let g = AppState::get(window);
    g.set_continue_watching(refresh_row_preserving_posters(
        &g.get_continue_watching(),
        &hd.continue_watching,
    ));
    g.set_next_up(refresh_row_preserving_posters(
        &g.get_next_up(),
        &hd.next_up,
    ));
    g.set_recently_added(refresh_row_preserving_posters(
        &g.get_recently_added(),
        &hd.recently_added_tv,
    ));
    g.set_continue_watching_movies(refresh_row_preserving_posters(
        &g.get_continue_watching_movies(),
        &cw_movies,
    ));
    g.set_recently_added_movies(refresh_row_preserving_posters(
        &g.get_recently_added_movies(),
        &hd.recently_added_movies,
    ));
    g.set_not_watched_movies(refresh_row_preserving_posters(
        &g.get_not_watched_movies(),
        &hd.not_watched_movies,
    ));
    g.set_continue_watching_tv(refresh_row_preserving_posters(
        &g.get_continue_watching_tv(),
        &cw_tv,
    ));
    g.set_recently_added_tv(refresh_row_preserving_posters(
        &g.get_recently_added_tv(),
        &hd.recently_added_tv,
    ));
    g.set_not_watched_tv(refresh_row_preserving_posters(
        &g.get_not_watched_tv(),
        &hd.not_watched_tv,
    ));
    g.set_recently_added_collections(refresh_row_preserving_posters(
        &g.get_recently_added_collections(),
        &hd.recently_added_collections,
    ));
    g.set_unwatched_collections(refresh_row_preserving_posters(
        &g.get_unwatched_collections(),
        &hd.unwatched_collections,
    ));
    g.set_recently_added_albums(refresh_row_preserving_posters(
        &g.get_recently_added_albums(),
        &hd.recently_added_albums,
    ));
    g.set_recently_played_albums(refresh_row_preserving_posters(
        &g.get_recently_played_albums(),
        &hd.recently_played_albums,
    ));
    g.set_favorite_movies(refresh_row_preserving_posters(
        &g.get_favorite_movies(),
        &hd.favorite_movies,
    ));
    g.set_favorite_series(refresh_row_preserving_posters(
        &g.get_favorite_series(),
        &hd.favorite_series,
    ));
    g.set_favorite_albums(refresh_row_preserving_posters(
        &g.get_favorite_albums(),
        &hd.favorite_albums,
    ));
    g.set_music_playlists(refresh_row_preserving_posters(
        &g.get_music_playlists(),
        &hd.playlists,
    ));
}

pub(crate) fn home_data_sections(hd: &HomeData) -> [(HomeSection, Vec<MediaItem>); 17] {
    let cw_movies = hd
        .continue_watching
        .iter()
        .filter(|i| i.item_type == "Movie")
        .cloned()
        .collect();
    let cw_tv = hd
        .continue_watching
        .iter()
        .filter(|i| i.item_type == "Episode")
        .cloned()
        .collect();
    [
        (HomeSection::ContinueWatching, hd.continue_watching.clone()),
        (HomeSection::NextUp, hd.next_up.clone()),
        (HomeSection::RecentlyAdded, hd.recently_added_tv.clone()),
        (HomeSection::ContinueWatchingMovies, cw_movies),
        (
            HomeSection::RecentlyAddedMovies,
            hd.recently_added_movies.clone(),
        ),
        (HomeSection::NotWatchedMovies, hd.not_watched_movies.clone()),
        (HomeSection::ContinueWatchingTv, cw_tv),
        (HomeSection::RecentlyAddedTv, hd.recently_added_tv.clone()),
        (HomeSection::NotWatchedTv, hd.not_watched_tv.clone()),
        (
            HomeSection::RecentlyAddedCollections,
            hd.recently_added_collections.clone(),
        ),
        (
            HomeSection::UnwatchedCollections,
            hd.unwatched_collections.clone(),
        ),
        (
            HomeSection::RecentlyAddedAlbums,
            hd.recently_added_albums.clone(),
        ),
        (
            HomeSection::RecentlyPlayedAlbums,
            hd.recently_played_albums.clone(),
        ),
        (HomeSection::FavoriteMovies, hd.favorite_movies.clone()),
        (HomeSection::FavoriteSeries, hd.favorite_series.clone()),
        (HomeSection::FavoriteAlbums, hd.favorite_albums.clone()),
        (HomeSection::Playlists, hd.playlists.clone()),
    ]
}

/// Re-fetch Movie/Series/MusicAlbum favorites and update the three favorites rows — after
/// every favourite toggle, so they update at once. Posters preserved
/// (refresh_row_preserving_posters).
pub(crate) fn refresh_favorites(
    client: Arc<JellyfinClient>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    state: Arc<Mutex<FjordState>>,
) {
    rt.spawn(async move {
        let (fam, fas, fal) = tokio::join!(
            client.get_favorites("Movie"),
            client.get_favorites("Series"),
            client.get_favorites("MusicAlbum"),
        );
        let fam = fam.unwrap_or_else(|e| {
            warn!("refresh favorite_movies: {:#}", e);
            vec![]
        });
        let fas = fas.unwrap_or_else(|e| {
            warn!("refresh favorite_series: {:#}", e);
            vec![]
        });
        let fal = fal.unwrap_or_else(|e| {
            warn!("refresh favorite_albums: {:#}", e);
            vec![]
        });

        // Push metadata immediately (no posters yet; poster loading fills them in below)
        let ww2 = ww.clone();
        let fam2 = fam.clone();
        let fas2 = fas.clone();
        let fal2 = fal.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww2.upgrade() {
                let g = AppState::get(&w);
                g.set_favorite_movies(refresh_row_preserving_posters(
                    &g.get_favorite_movies(),
                    &fam2,
                ));
                g.set_favorite_series(refresh_row_preserving_posters(
                    &g.get_favorite_series(),
                    &fas2,
                ));
                g.set_favorite_albums(refresh_row_preserving_posters(
                    &g.get_favorite_albums(),
                    &fal2,
                ));
            }
        });

        let mut sections = HomeSection::empty_array();
        sections[13].1 = fam;
        sections[14].1 = fas;
        sections[15].1 = fal;
        let rt2 = tokio::runtime::Handle::current();
        crate::poster::spawn_poster_loading(client, sections, ww, rt2, state);
    });
}

pub(crate) fn wire_nw_timer(
    window_weak: slint::Weak<MainWindow>,
    video: Arc<Mutex<VideoState>>,
    state: Arc<Mutex<FjordState>>,
    rt_handle: tokio::runtime::Handle,
) -> slint::Timer {
    let timer_nw = slint::Timer::default();
    timer_nw.start(
        slint::TimerMode::Repeated,
        Duration::from_secs(30),
        move || {
            if video.lock().unwrap().player.is_some() {
                return;
            }
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            let nav = AppState::get(&w).get_active_nav();
            if nav != 2 && nav != 1 {
                return;
            } // nav=2=Movies, nav=1=TV

            let (due_movies, due_tv) = {
                let s = state.lock().unwrap();
                (
                    nav == 2
                        && s.last_nw_mov_refresh
                            .is_none_or(|t| t.elapsed() >= Duration::from_secs(600)),
                    nav == 1
                        && s.last_nw_tv_refresh
                            .is_none_or(|t| t.elapsed() >= Duration::from_secs(600)),
                )
            };
            if !due_movies && !due_tv {
                return;
            }

            let client = state.lock().unwrap().client.as_ref().map(Arc::clone);
            let Some(client) = client else { return };

            let ww = window_weak.clone();
            let rt2 = rt_handle.clone();
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
                                    AppState::get(&w)
                                        .set_status("Session expired — please log in again".into());
                                }
                            });
                            return;
                        }
                        Err(e) => {
                            // Log and fall through — no early return, which would also skip the
                            // independent TV fetch below (this runs every 30 s).
                            warn!("get_unwatched (movies) failed: {e:#}");
                        }
                        Ok(items) => {
                            state2.lock().unwrap().last_nw_mov_refresh = Some(Instant::now());
                            let ww2 = ww.clone();
                            let items2 = items.clone();
                            let watchlist = state2.lock().unwrap().jellyfin_watchlist_ids.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww2.upgrade() {
                                    AppState::get(&w).set_not_watched_movies(
                                        crate::items_to_model(&items2, &watchlist),
                                    );
                                }
                            });
                            let mut sections = HomeSection::empty_array();
                            sections[HomeSection::NotWatchedMovies as usize].1 = items;
                            crate::spawn_poster_loading(
                                Arc::clone(&client),
                                sections,
                                ww.clone(),
                                rt2.clone(),
                                Arc::clone(&state2),
                            );
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
                                    AppState::get(&w)
                                        .set_status("Session expired — please log in again".into());
                                }
                            });
                        }
                        Err(e) => {
                            // See the identical fix on the movies branch above.
                            warn!("get_unwatched (tv) failed: {e:#}");
                        }
                        Ok(items) => {
                            state2.lock().unwrap().last_nw_tv_refresh = Some(Instant::now());
                            let ww2 = ww.clone();
                            let items2 = items.clone();
                            let watchlist = state2.lock().unwrap().jellyfin_watchlist_ids.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww2.upgrade() {
                                    AppState::get(&w).set_not_watched_tv(crate::items_to_model(
                                        &items2, &watchlist,
                                    ));
                                }
                            });
                            let mut sections = HomeSection::empty_array();
                            sections[HomeSection::NotWatchedTv as usize].1 = items;
                            crate::spawn_poster_loading(client, sections, ww, rt2, state2);
                        }
                    }
                }
            });
        },
    );
    timer_nw
}

// ── Collection map ────────────────────────────────────────────────────────────

/// Fetch all BoxSets and their member IDs, building a reverse map from movie_id to
/// (boxset_id, boxset_name). Called once in the background after login; stored in FjordState.
pub(crate) async fn fetch_movie_collections(
    client: &JellyfinClient,
) -> HashMap<String, (String, String)> {
    let boxsets = match client.get_all_boxsets().await {
        Ok(b) => b,
        Err(e) => {
            warn!("get_all_boxsets: {:#}", e);
            return HashMap::new();
        }
    };

    let sem = Arc::new(tokio::sync::Semaphore::new(4));
    let mut tasks: tokio::task::JoinSet<Vec<(String, String, String)>> =
        tokio::task::JoinSet::new();
    for bs in boxsets {
        let client_c = client.clone();
        let sem_c = sem.clone();
        let bs_id = bs.id.clone();
        let bs_name = bs.name.clone();
        tasks.spawn(async move {
            let _permit = sem_c.acquire_owned().await.ok();
            let items = client_c.get_boxset_items(&bs_id).await.unwrap_or_default();
            items
                .into_iter()
                .map(|i| (i.id, bs_id.clone(), bs_name.clone()))
                .collect()
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
    std::fs::read_to_string(last_cleanup_path())
        .ok()?
        .trim()
        .parse()
        .ok()
}

/// Delete orphaned files from the `posters/` and `backdrops/` caches (name = item id not in
/// the library). `detail_ids` = item_detail_cache keys + every cached item's credited person
/// ids — cast portraits are cached under person ids, which the flat library lists never
/// contain. Skipped when the library id set is empty (network error / first run;
/// `detail_ids` alone doesn't count — it can come from the persisted screen cache) or when it
/// ran within 24 h.
pub(crate) async fn run_poster_cache_cleanup(
    movie_ids: Vec<String>,
    series_ids: Vec<String>,
    collection_ids: Vec<String>,
    artist_ids: Vec<String>,
    album_ids: Vec<String>,
    playlist_ids: Vec<String>,
    detail_ids: Vec<String>,
) {
    use std::collections::HashSet;
    use std::time::{SystemTime, UNIX_EPOCH};

    if movie_ids.is_empty()
        && series_ids.is_empty()
        && collection_ids.is_empty()
        && artist_ids.is_empty()
        && album_ids.is_empty()
        && playlist_ids.is_empty()
    {
        return;
    }

    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    if let Some(last) = read_last_cleanup()
        && now_secs.saturating_sub(last) < 86_400
    {
        return;
    }

    let known: HashSet<String> = movie_ids
        .into_iter()
        .chain(series_ids)
        .chain(collection_ids)
        .chain(artist_ids)
        .chain(album_ids)
        .chain(playlist_ids)
        .chain(detail_ids)
        .collect();

    let mut deleted = 0u32;
    for dir in [poster_cache_dir(), backdrop_cache_dir()] {
        let Ok(mut entries) = tokio::fs::read_dir(&dir).await else {
            continue;
        };
        while let Ok(Some(entry)) = entries.next_entry().await {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            // .tag sidecars (artwork revalidation) and .tmp leftovers belong to
            // their base image id — judge them by that id, not the full name.
            let base = name
                .strip_suffix(".tag.tmp")
                .or_else(|| name.strip_suffix(".tag"))
                .or_else(|| name.strip_suffix(".tmp"))
                .unwrap_or(&name);
            if !known.contains(base) && tokio::fs::remove_file(entry.path()).await.is_ok() {
                deleted += 1;
            }
        }
    }

    let ts = now_secs.to_string();
    let p = last_cleanup_path();
    if let Some(parent) = p.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&p, ts.as_bytes()).await;

    if deleted > 0 {
        tracing::info!("poster cache cleanup: deleted {deleted} orphaned file(s)");
    }
}

// ── wire_item_play (moved from main(), 0.5.0 step 3) ─────────────────────
/// Wires play from home / library rows: item_play.
pub(crate) fn wire_item_play(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    video: &std::sync::Arc<std::sync::Mutex<crate::playback::VideoState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    let video = std::sync::Arc::clone(video);
    // ── play from home / library rows ─────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let video3 = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();

        AppState::get(&window).on_item_play(move |item_id| {
            let item_id = item_id.to_string();
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else {
                return;
            };

            // BoxSet (collection) — open collection screen instead of playing.
            // all_collections is only populated when the library grid is first opened; fall
            // back to the always-present dashboard models when it hasn't been opened yet.
            let boxset_info = s
                .all_collections
                .iter()
                .find(|i| i.id == item_id)
                .map(|bs| (bs.id.clone(), bs.name.clone()))
                .or_else(|| {
                    let w = window_weak.upgrade()?;
                    let g = AppState::get(&w);
                    let find_boxset = |model: ModelRc<CardItem>| -> Option<(String, String)> {
                        for idx in 0..model.row_count() {
                            if let Some(c) = model.row_data(idx)
                                && c.id.as_str() == item_id
                                && c.item_type.as_str() == "BoxSet"
                            {
                                return Some((c.id.to_string(), c.title.to_string()));
                            }
                        }
                        None
                    };
                    find_boxset(g.get_recently_added_collections())
                        .or_else(|| find_boxset(g.get_unwatched_collections()))
                });
            if let Some((bs_id, bs_name)) = boxset_info {
                let ww2 = window_weak.clone();
                let state2 = state.clone();
                let rt_handle2 = rt_handle.clone();
                drop(s);
                collection::open_collection_screen(bs_id, bs_name, state2, ww2, rt_handle2);
                return;
            }

            if s.all_series.iter().any(|i| i.id == item_id) {
                let state2 = state.clone();
                let ww2 = window_weak.clone();
                let rt_handle2 = rt_handle.clone();
                let video4 = Arc::clone(&video3);
                drop(s);
                rt_handle.spawn(async move {
                    let next = client.get_next_up_for_series(&item_id).await.ok().flatten();
                    if let Some(next) = next {
                        let mut config = state2.lock().unwrap().player_config();
                        config.start_position_secs = next.resume_position_secs();
                        let cli2 = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                        let Some(cli2) = cli2 else {
                            let _ = slint::invoke_from_event_loop(move || {
                                open_series_screen(item_id, state2, ww2, rt_handle2);
                            });
                            return;
                        };
                        let url = cli2.direct_play_url(&next.id);
                        let title = next.display_name();
                        let ep_id = next.id.clone();
                        let series_id = next.series_id.clone();
                        let video_info = next.video_stream_info();
                        let _ = slint::invoke_from_event_loop(move || {
                            start_playback(
                                url,
                                ep_id,
                                "Episode",
                                title,
                                config,
                                cli2,
                                series_id,
                                None,
                                &video4,
                                &ww2,
                                &rt_handle2,
                                &state2,
                                video_info,
                            );
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
            let state_album = Arc::clone(&state);
            let state_purge = Arc::clone(&state);
            let state_play = Arc::clone(&state);
            drop(s);
            let play_url = client.direct_play_url(&item_id);
            let video3b = Arc::clone(&video3);
            let ww3 = window_weak.clone();
            let rth3 = rt_handle.clone();
            rt_handle.spawn(async move {
                let detail = match client.get_item_detail(&item_id).await {
                    Ok(d) => Some(d),
                    Err(e) if crate::is_not_found(&e) => {
                        // Ghost card: the item is gone server-side — clean up
                        // instead of handing mpv a dead stream URL (S4).
                        purge_deleted_item(&state_purge, &ww3, &item_id);
                        return;
                    }
                    Err(e) => {
                        warn!("item_play get_item_detail({item_id}): {e:#}");
                        None
                    }
                };
                let item_type = detail
                    .as_ref()
                    .map(|i| i.item_type.clone())
                    .unwrap_or_default();
                let series_id = detail.as_ref().and_then(|i| i.series_id.clone());
                let title = detail
                    .as_ref()
                    .map(|i| i.display_name())
                    .unwrap_or_else(|| item_id.clone());
                let video_info = detail.as_ref().and_then(|i| i.video_stream_info());
                config.start_position_secs = detail.and_then(|i| i.resume_position_secs());

                if item_type == "MusicAlbum" {
                    let _ = slint::invoke_from_event_loop(move || {
                        album::open_album_screen(item_id, title, state_album, ww3, rth3);
                    });
                    return;
                }
                if item_type == "Playlist" {
                    let _ = slint::invoke_from_event_loop(move || {
                        album::open_playlist_screen(item_id, title, state_album, ww3, rth3);
                    });
                    return;
                }
                if item_type == "MusicArtist" {
                    // Same class of bug as albums: an artist id has no stream.
                    let _ = slint::invoke_from_event_loop(move || {
                        artist::open_artist_screen(item_id, title, state_album, ww3, rth3);
                    });
                    return;
                }

                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(
                        play_url,
                        item_id,
                        &item_type,
                        title,
                        config,
                        client,
                        series_id,
                        None,
                        &video3b,
                        &ww3,
                        &rth3,
                        &state_play,
                        video_info,
                    );
                });
            });
        });
    }
}
