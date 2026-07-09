// ── fjord-app · main.rs ──────────────────────────────────────────────────────
//   model helpers        item_to_card_item, items_to_model, apply_cards_preserving_identity (mutate an
//                        existing model in place when ids/order match, so unrelated cards' poster Image
//                        elements survive a refresh instead of re-fading, Phase 96), push_section_model
//                        (takes HomeSection), show_toast (any-thread toast helper)
//   settings helpers     apply_settings_to_window ↔ read_settings_from_window
//   push_cached_data     push on-disk caches (home/movies/series/collections/artists/albums/
//                        playlists) into AppState/FjordState for instant display — only called
//                        after spawn_auto_login's probe confirms the server is reachable
//   spawn_auto_login     probe saved session (check_auth, 8s timeout) → reachable: push_cached_data
//                        then fetch_home_data/get_all_series/get_system_info + start_websocket
//                        (unchanged from before); 401: show-login; anything else (can't reach
//                        server at all): show-offline. Re-invoked by AppState.retry-connection.
//   main                 entry point; log rotation (fjord.log → .old each start) + per-layer
//                        filters (console + file both use Config.log_level, read directly off
//                        disk before the subscriber exists; RUST_LOG still overrides either —
//                        Settings→General row, applies on next launch); panic hook (writes to
//                        fjord.log); logs "fjord version: {FJORD_BUILD_ID}"; wires all AppState
//                        global callbacks
//     apply saved cfg    cold-start vs warm-start; sets show-connecting, calls spawn_auto_login
//                        — cached content is no longer shown before connectivity is confirmed
//                        (a full outage used to look identical to normal quiet operation)
//     login              on_do_login → auth::do_login (also starts websocket)
//     browse play        on_play_item (server-side search results)
//     home / library     on_item_play, on_open_library (lazy fetch: nav=1=TV, nav=2=Movies, nav=3=Collections, nav=4=Artists)
//     detail             on_play_detail, on_resume_detail, on_close_detail
//     collection         on_open_collection → collection::open_collection_screen
//     artist             on_open_artist → artist::open_artist_screen; on_close_artist;
//                        on_toggle_artist_fav; on_play_artist_all (fetches all album tracks, starts queue)
//     album              on_open_album → album::open_album_screen; on_close_album; on_play_album_track;
//                        on_toggle_album_fav; on_toggle_album_played
//     series             on_open_series, on_series_select_season (cache+gen guard), on_play_series_episode,
//                        on_toggle_series_played, on_toggle_series_fav
//     season             on_open_season_detail, on_close_season_detail, on_toggle_season_fav, on_toggle_season_played
//     person             on_open_person, on_close_person
//     Up Next banner     on_cancel_auto_advance (Skip), on_play_next_ep (Play Now)
//     player controls    wire_controls
//     context menu       wire_context_menu, wire_queue_callbacks
//     queue panel        on_open_queue_panel (mouse entry point for the music-bar ⋮ button —
//                        mirrors keys.rs's 'q' path, plus a keyboard focus re-grab, CR11-6)
//     audio devices      fetch_audio_devices (startup), on_audio_device_selected
//     settings           on_settings_changed
//     fullscreen         on_toggle_fullscreen, launch-fullscreen setting
//     sign-out           on_sign_out (aborts websocket via FjordState.ws_abort)
//     retry connection   on_retry_connection (OfflineScreen's Retry button + Enter key) →
//                        re-invokes spawn_auto_login with fresh clones
// ─────────────────────────────────────────────────────────────────────────────
slint::include_modules!();

mod album;
mod artist;
mod auth;
mod browse;
mod collection;
mod config;
mod context_menu;
mod controls;
mod detail;
mod home;
mod keys;
mod movies;
mod playback;
mod poster;
mod season;
mod series;
mod person;
mod pipewire_fix;
mod settings;
mod stats;
mod ws;

use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32};

use anyhow::Result;
use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, Model, ModelRc, SharedString, StandardListViewItem, VecModel};
use tracing::{debug, info, warn};
use url::Url;

use config::{
    FjordState,
    load_config, save_config, ensure_device_id,
};
use home::{
    HomeSection,
    load_home_cache, save_home_cache, fetch_home_data, push_home_data, push_home_data_preserving_posters,
    refresh_row_preserving_posters, home_data_sections, wire_nw_timer,
    load_movies_cache, save_movies_cache, load_series_cache, save_series_cache,
    load_collections_cache, save_collections_cache,
    load_artists_cache, save_artists_cache,
    load_albums_cache, save_albums_cache,
    load_playlists_cache, save_playlists_cache,
    fetch_movie_collections, run_poster_cache_cleanup,
};
use movies::{spawn_movies_poster_loading, spawn_collections_poster_loading, spawn_artists_poster_loading, spawn_albums_poster_loading, spawn_playlists_poster_loading,};
use playback::{VideoState, start_playback, quit_cleanup, do_stop_playback, wire_rendering_notifier, wire_mpv_timer};
use poster::{spawn_poster_loading, spawn_series_poster_loading};
use series::{ep_to_card, spawn_episode_thumb_loading, open_series_screen};

pub(crate) fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .map(|s| s.as_u16() == 401)
        .unwrap_or(false)
}

pub(crate) fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .map(|s| s.as_u16() == 404)
        .unwrap_or(false)
}

// Self-healing for ghost items (cache-staleness fix S4): when a fetch 404s the
// item no longer exists on the server — remove it from every canonical vec and
// visible model, mark the list caches dirty, and tell the user. Safe to call
// from any thread.
pub(crate) fn purge_deleted_item(
    state: &Arc<Mutex<FjordState>>,
    ww:    &slint::Weak<MainWindow>,
    id:    &str,
) {
    {
        let mut s = state.lock().unwrap();
        s.all_movies.retain(|i| i.id != id);
        s.all_series.retain(|i| i.id != id);
        s.all_collections.retain(|i| i.id != id);
        s.all_artists.retain(|i| i.id != id);
        s.all_albums.retain(|i| i.id != id);
        s.filtered_items.retain(|i| i.id != id);
        s.movie_collections.remove(id);
        for eps in s.series_episode_cache.values_mut() {
            eps.retain(|e| e.id != id);
        }
        // Whatever list this ghost came from is stale — refresh on next grid open.
        s.movies_fetched      = false;
        s.collections_fetched = false;
        s.artists_fetched     = false;
        s.albums_fetched      = false;
    }
    let id2 = id.to_string();
    let ww2 = ww.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww2.upgrade() {
            crate::context_menu::remove_item_from_all_models(&w, &id2);
        }
    });
    show_toast(ww.clone(), "Item was removed from the server".to_string());
}

/// Show a bottom-center error toast.  Safe to call from any thread or the Slint event loop.
/// The Slint Timer in main.slint auto-dismisses it after 4 s.
pub(crate) fn show_toast(ww: slint::Weak<MainWindow>, msg: String) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            let g = AppState::get(&w);
            g.set_toast_message(msg.as_str().into());
            g.set_toast_visible(true);
        }
    });
}

// ── model helpers ─────────────────────────────────────────────────────────────

pub(crate) fn item_to_card_item(i: &MediaItem) -> CardItem {
    let mut h = CardItem::default();
    h.id             = SharedString::from(i.id.as_str());
    h.item_type      = SharedString::from(i.item_type.as_str());
    h.title          = SharedString::from(i.card_title().as_str());
    h.subtitle       = SharedString::from(i.card_subtitle().as_str());
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

/// Apply `fresh` cards to `old`'s model. If `fresh` has the same ids in the same
/// order as what's already there, mutate the EXISTING model row-by-row via
/// set_row_data instead of returning a new ModelRc — swapping the model instance
/// makes Slint destroy and recreate every delegate element (including each card's
/// poster Image), which re-triggers FadeInTrigger's fade-in even when nothing
/// about the card actually changed. Only a genuine membership/order difference
/// rebuilds a new model, which is correct there (a fade is expected). Shared by
/// every place that builds a fresh Vec<CardItem> and pushes it to a model —
/// poster.rs's home/series decode, movies.rs's library decode, home.rs's row
/// merges, context_menu.rs's WS delta-sync upserts (Phase 96 consolidation).
pub(crate) fn apply_cards_preserving_identity(old: &ModelRc<CardItem>, fresh: Vec<CardItem>) -> ModelRc<CardItem> {
    let old_rows: Vec<CardItem> = (0..old.row_count()).filter_map(|i| old.row_data(i)).collect();
    let same_shape = old_rows.len() == fresh.len()
        && old_rows.iter().zip(fresh.iter()).all(|(a, b)| a.id.as_str() == b.id.as_str());
    if same_shape {
        tracing::debug!("apply_cards_preserving_identity: {} row(s), same_shape=true", fresh.len());
        for (i, card) in fresh.into_iter().enumerate() {
            old.set_row_data(i, card);
        }
        return old.clone();
    }
    // Diagnostic: pin down *why* same_shape failed — different length, or same
    // length but reordered/different ids. Left at debug (not removed) since this
    // exact log is what pinpointed Phases 96-99's library-grid flash bugs.
    let first_mismatch = old_rows.iter().zip(fresh.iter()).position(|(a, b)| a.id.as_str() != b.id.as_str());
    tracing::debug!(
        "apply_cards_preserving_identity: old_len={} fresh_len={} same_shape=false first_mismatch_idx={:?}",
        old_rows.len(), fresh.len(), first_mismatch
    );
    ModelRc::new(VecModel::from(fresh))
}

pub(crate) fn push_section_model(window: &MainWindow, sec: HomeSection, model: ModelRc<CardItem>) {
    let g = AppState::get(window);
    match sec {
        HomeSection::ContinueWatching         => g.set_continue_watching(model),
        HomeSection::NextUp                   => g.set_next_up(model),
        HomeSection::RecentlyAdded            => g.set_recently_added(model),
        HomeSection::ContinueWatchingMovies   => g.set_continue_watching_movies(model),
        HomeSection::RecentlyAddedMovies      => g.set_recently_added_movies(model),
        HomeSection::NotWatchedMovies         => g.set_not_watched_movies(model),
        HomeSection::ContinueWatchingTv       => g.set_continue_watching_tv(model),
        HomeSection::RecentlyAddedTv          => g.set_recently_added_tv(model),
        HomeSection::NotWatchedTv             => g.set_not_watched_tv(model),
        HomeSection::RecentlyAddedCollections => g.set_recently_added_collections(model),
        HomeSection::UnwatchedCollections     => g.set_unwatched_collections(model),
        HomeSection::RecentlyAddedAlbums      => g.set_recently_added_albums(model),
        HomeSection::RecentlyPlayedAlbums     => g.set_recently_played_albums(model),
        HomeSection::FavoriteMovies           => g.set_favorite_movies(model),
        HomeSection::FavoriteSeries           => g.set_favorite_series(model),
        HomeSection::FavoriteAlbums           => g.set_favorite_albums(model),
        HomeSection::Playlists                => g.set_music_playlists(model),
    }
}

/// Read-side counterpart to push_section_model — lets a poster-decode pass apply
/// via apply_cards_preserving_identity instead of always building a fresh model.
pub(crate) fn get_section_model(window: &MainWindow, sec: HomeSection) -> ModelRc<CardItem> {
    let g = AppState::get(window);
    match sec {
        HomeSection::ContinueWatching         => g.get_continue_watching(),
        HomeSection::NextUp                   => g.get_next_up(),
        HomeSection::RecentlyAdded            => g.get_recently_added(),
        HomeSection::ContinueWatchingMovies   => g.get_continue_watching_movies(),
        HomeSection::RecentlyAddedMovies      => g.get_recently_added_movies(),
        HomeSection::NotWatchedMovies         => g.get_not_watched_movies(),
        HomeSection::ContinueWatchingTv       => g.get_continue_watching_tv(),
        HomeSection::RecentlyAddedTv          => g.get_recently_added_tv(),
        HomeSection::NotWatchedTv             => g.get_not_watched_tv(),
        HomeSection::RecentlyAddedCollections => g.get_recently_added_collections(),
        HomeSection::UnwatchedCollections     => g.get_unwatched_collections(),
        HomeSection::RecentlyAddedAlbums      => g.get_recently_added_albums(),
        HomeSection::RecentlyPlayedAlbums     => g.get_recently_played_albums(),
        HomeSection::FavoriteMovies           => g.get_favorite_movies(),
        HomeSection::FavoriteSeries           => g.get_favorite_series(),
        HomeSection::FavoriteAlbums           => g.get_favorite_albums(),
        HomeSection::Playlists                => g.get_music_playlists(),
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

// Rebuild queue-items model from current VideoState. The panel shows the
// CURRENT track and what will still play — finished/skipped playlist rows are
// hidden (they were "cleared from the queue"). With Repeat All/One every row
// plays again, so the full playlist stays visible. Playlist rows come first
// (shuffle-aware play order), then context-menu queue rows (is_queued=true).
// QueueEntry.index carries the UNDERLYING index (playlist idx, or
// playlist.len()+queue idx; -1 = synthetic now-playing row) — queue-jump and
// queue-remove consume it directly; the visible number is the display row.
// Also owns queue-count (upcoming playlist tracks + queued items) so callers
// can't drift on its meaning. poster-id is set so spawn_queue_poster_loading
// can fetch art; has_poster/poster are left false/default until the background
// task fills them via set_row_data.
// Must be called on the Slint UI thread (holds &AppState, not Weak).
pub(crate) fn push_queue_display(vs: &crate::playback::VideoState, g: &AppState) {
    use slint::{ModelRc, VecModel};
    let to_entry = |i: i32, qi: &crate::playback::QueueItem, is_current: bool, is_queued: bool| {
        let artist = qi.audio_meta.as_ref()
            .map(|(a, _)| a.as_str())
            .unwrap_or("")
            .to_string();
        // Audio items: poster-id = album_art_id; video items: poster-id = item id.
        let poster_id = qi.audio_meta.as_ref()
            .map(|(_, art)| art.as_str())
            .unwrap_or(qi.id.as_str())
            .to_string();
        crate::QueueEntry {
            id:         qi.id.as_str().into(),
            index:      i,
            title:      qi.title.as_str().into(),
            artist:     artist.as_str().into(),
            is_current,
            is_queued,
            poster_id:  poster_id.as_str().into(),
            has_poster: false,
            poster:     Default::default(),
        }
    };

    let cur_id = vs.item_id.as_deref();
    // Current play IS the playlist row at playlist_index (normal album playback)?
    let cur_is_listed = cur_id.is_some()
        && vs.playlist.get(vs.playlist_index).map(|q| Some(q.id.as_str()) == cur_id)
            .unwrap_or(false);

    let mut items: Vec<crate::QueueEntry> = Vec::new();

    // Off-list play (queue jump / single track): synthetic now-playing row on top.
    if let (Some(id), Some(np)) = (cur_id, vs.now_playing.as_ref()) {
        if !cur_is_listed && np.id == id {
            items.push(to_entry(-1, np, true, false));
        }
    }

    if vs.repeat_mode != crate::playback::RepeatMode::Off {
        // Repeat: everything plays again — show the whole playlist.
        items.extend(vs.playlist.iter().enumerate()
            .map(|(i, qi)| to_entry(i as i32, qi, cur_is_listed && i == vs.playlist_index, false)));
    } else {
        // Play order from the current position (shuffle-aware); rows already
        // played are gone. When the current play is off-list, the slot at
        // playlist_index will not play (advance goes to the next one) — skip it.
        let order: Vec<usize> = if vs.shuffle && !vs.shuffle_order.is_empty() {
            let pos = vs.shuffle_order.iter()
                .position(|&i| i == vs.playlist_index)
                .unwrap_or(0);
            vs.shuffle_order[pos..].to_vec()
        } else {
            (vs.playlist_index..vs.playlist.len()).collect()
        };
        for (k, &i) in order.iter().enumerate() {
            if k == 0 && !cur_is_listed && cur_id.is_some() { continue; }
            if let Some(qi) = vs.playlist.get(i) {
                items.push(to_entry(i as i32, qi, cur_is_listed && i == vs.playlist_index, false));
            }
        }
    }

    let base = vs.playlist.len() as i32;
    items.extend(vs.queue.iter().enumerate()
        .map(|(i, qi)| to_entry(base + i as i32, qi, false, true)));
    g.set_queue_items(ModelRc::new(VecModel::from(items)));
    g.set_queue_count(crate::playback::upcoming_count(vs));
}

// Fetch album art for each QueueEntry and fill in poster/has_poster via set_row_data.
// Reads poster-ids from the current queue-items model snapshot.
pub(crate) fn spawn_queue_poster_loading(
    client:      std::sync::Arc<fjord_api::JellyfinClient>,
    ww:          slint::Weak<MainWindow>,
    rt:          tokio::runtime::Handle,
) {
    use slint::Model;
    // Snapshot poster_ids from the model (must be on UI thread; caller ensures this).
    let Some(w) = ww.upgrade() else { return };
    let model = AppState::get(&w).get_queue_items();
    let entries: Vec<(usize, String)> = (0..model.row_count())
        .filter_map(|i| {
            model.row_data(i).and_then(|e| {
                let pid = e.poster_id.to_string();
                if pid.is_empty() { None } else { Some((i, pid)) }
            })
        })
        .collect();
    drop(w);
    if entries.is_empty() { return; }

    let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    for (row_idx, poster_id) in entries {
        let client2 = std::sync::Arc::clone(&client);
        let ww2     = ww.clone();
        let sem2    = std::sync::Arc::clone(&sem);
        rt.spawn(async move {
            let _permit = sem2.acquire().await;
            if let Some(bytes) = poster::fetch_poster_cached(&client2, &poster_id).await {
                if let Some(spb) = poster::decode_poster_buffer(&bytes) {
                    let _ = slint::invoke_from_event_loop(move || {
                        use slint::Model;
                        if let Some(w) = ww2.upgrade() {
                            let model = AppState::get(&w).get_queue_items();
                            if let Some(mut row) = model.row_data(row_idx) {
                                // Guard: poster-id must still match (playlist may have changed)
                                if row.poster_id.as_str() == poster_id.as_str() {
                                    row.has_poster = true;
                                    row.poster     = slint::Image::from_rgba8(spb);
                                    model.set_row_data(row_idx, row);
                                }
                            }
                        }
                    });
                }
            }
        });
    }
}


// ── spawn_library_fetch ───────────────────────────────────────────────────────
// Network-refresh the library list for `nav` (1=TV posters, 2=Movies,
// 3=Collections, 4=Artists+Albums+Playlists), guarded by the per-session
// *_fetched flags. Extracted from on_open_library so ws.rs can refresh the
// currently open grid when a LibraryChanged event clears those flags
// (cache-staleness fix S3). Each result is applied via
// home::refresh_row_preserving_posters (not a bare items_to_model) so posters
// already decoded from the startup cache push survive this first-open-this-
// session refresh instead of flashing blank (Phase 94).
pub(crate) fn spawn_library_fetch(
    nav:   i32,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    let s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
    if nav == 1 {
        // TV: all_series already loaded at startup; poster loading runs then too.
        let series = s.all_series.clone();
        drop(s);
        let ww2  = ww.clone();
        let rth2 = rt.clone();
        if !series.is_empty() {
            tracing::debug!("spawn_library_fetch[TV]: re-decoding {} already-loaded series (grid opened/switched to)", series.len());
            spawn_series_poster_loading(client, series, ww2, rth2);
        }
        return;
    }
    if nav == 3 {
        // Collections: lazy-fetch from network once per session.
        if s.collections_fetched { return; }
        drop(s);
        let state2 = Arc::clone(&state);
        let ww2  = ww.clone();
        let ww3  = ww.clone();
        let rt3 = rt.clone();
        rt.spawn(async move {
            match client.get_all_boxsets().await {
                Ok(cols) => {
                    {
                        let mut s = state2.lock().unwrap();
                        s.all_collections    = cols.clone();
                        s.collections_fetched = true;
                    }
                    save_collections_cache(&cols);
                    let cols2 = cols.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() {
                            let g = AppState::get(&w);
                            tracing::debug!("spawn_library_fetch[Collections]: network fetch landed, {} item(s)", cols2.len());
                            g.set_all_collections(refresh_row_preserving_posters(&g.get_all_collections(), &cols2));
                            if AppState::get(&w).get_show_library() {
                                browse::refresh_library_display(&w);
                            }
                        }
                    });
                    spawn_collections_poster_loading(client, cols, ww3, rt3);
                }
                Err(e) => warn!("open_library collections: {:#}", e),
            }
        });
        return;
    }
    if nav == 4 {
        let artists_done   = s.artists_fetched;
        let albums_done    = s.albums_fetched;
        let playlists_done = s.playlists_fetched;
        if artists_done && albums_done && playlists_done { return; }
        drop(s);
        // Fetch artists if not yet done.
        if !artists_done {
            let state_a = Arc::clone(&state);
            let ww2     = ww.clone();
            let ww3     = ww.clone();
            let rt3    = rt.clone();
            let client_a = Arc::clone(&client);
            rt.spawn(async move {
                match client_a.get_album_artists().await {
                    Ok(artists) => {
                        {
                            let mut s = state_a.lock().unwrap();
                            s.all_artists     = artists.clone();
                            s.artists_fetched = true;
                        }
                        save_artists_cache(&artists);
                        let artists2 = artists.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                let g = AppState::get(&w);
                                tracing::debug!("spawn_library_fetch[Artists]: network fetch landed, {} item(s)", artists2.len());
                                g.set_all_artists(refresh_row_preserving_posters(&g.get_all_artists(), &artists2));
                                if AppState::get(&w).get_show_library() && AppState::get(&w).get_library_music_view() == 0 {
                                    browse::refresh_library_display(&w);
                                }
                            }
                        });
                        spawn_artists_poster_loading(client_a, artists, ww3, rt3);
                    }
                    Err(e) => warn!("open_library artists: {:#}", e),
                }
            });
        }
        // Fetch albums if not yet done.
        if !albums_done {
            let state_b = Arc::clone(&state);
            let ww2b    = ww.clone();
            let ww3b    = ww.clone();
            let rt3b   = rt.clone();
            let client_b = Arc::clone(&client);
            rt.spawn(async move {
                match client_b.get_all_albums().await {
                    Ok(albums) => {
                        {
                            let mut s = state_b.lock().unwrap();
                            s.all_albums     = albums.clone();
                            s.albums_fetched = true;
                        }
                        save_albums_cache(&albums);
                        let albums2 = albums.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2b.upgrade() {
                                let g = AppState::get(&w);
                                tracing::debug!("spawn_library_fetch[Albums]: network fetch landed, {} item(s)", albums2.len());
                                g.set_all_albums(refresh_row_preserving_posters(&g.get_all_albums(), &albums2));
                                if AppState::get(&w).get_show_library() && AppState::get(&w).get_library_music_view() == 1 {
                                    browse::refresh_library_display(&w);
                                }
                            }
                        });
                        spawn_albums_poster_loading(client_b, albums, ww3b, rt3b);
                    }
                    Err(e) => warn!("open_library albums: {:#}", e),
                }
            });
        }
        // Fetch playlists if not yet done.
        if !playlists_done {
            let state_p  = Arc::clone(&state);
            let ww2p     = ww.clone();
            let ww3p     = ww.clone();
            let rt3p     = rt.clone();
            let client_p = Arc::clone(&client);
            rt.spawn(async move {
                match client_p.get_all_playlists().await {
                    Ok(playlists) => {
                        {
                            let mut s = state_p.lock().unwrap();
                            s.all_playlists     = playlists.clone();
                            s.playlists_fetched = true;
                        }
                        save_playlists_cache(&playlists);
                        let playlists2 = playlists.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2p.upgrade() {
                                let g = AppState::get(&w);
                                tracing::debug!("spawn_library_fetch[Playlists]: network fetch landed, {} item(s)", playlists2.len());
                                g.set_all_playlists(refresh_row_preserving_posters(&g.get_all_playlists(), &playlists2));
                                if AppState::get(&w).get_show_library() && AppState::get(&w).get_library_music_view() == 2 {
                                    browse::refresh_library_display(&w);
                                }
                            }
                        });
                        spawn_playlists_poster_loading(client_p, playlists, ww3p, rt3p);
                    }
                    Err(e) => warn!("open_library playlists: {:#}", e),
                }
            });
        }
        return;
    }
    // Movies (nav == 2): lazy-fetch from network once; cache pre-populates on warm start.
    if s.movies_fetched { return; }
    drop(s);
    let state2 = Arc::clone(&state);
    let ww2  = ww.clone();
    let ww3  = ww.clone();
    let rt3 = rt.clone();
    rt.spawn(async move {
        match client.get_all_movies().await {
            Ok(movies) => {
                {
                    let mut s = state2.lock().unwrap();
                    s.all_movies     = movies.clone();
                    s.movies_fetched = true;
                }
                save_movies_cache(&movies);
                let movies2 = movies.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        tracing::debug!("spawn_library_fetch[Movies]: network fetch landed, {} item(s)", movies2.len());
                        g.set_all_movies(refresh_row_preserving_posters(&g.get_all_movies(), &movies2));
                        if AppState::get(&w).get_show_library() {
                            browse::refresh_library_display(&w);
                        }
                    }
                });
                spawn_movies_poster_loading(client, movies, ww3, rt3);
            }
            Err(e) => warn!("open_library movies: {:#}", e),
        }
    });
}

fn apply_settings_to_window(w: &MainWindow, s: &FjordState) {
    let g = AppState::get(w);
    let c = &s.config;
    g.set_settings_audio_device(ss(&c.audio_device));
    let dev_desc = s.audio_devices.iter()
        .find(|(n, _)| n == &c.audio_device)
        .map(|(_, d)| d.as_str())
        .unwrap_or(if c.audio_device.is_empty() { "" } else { c.audio_device.as_str() })
        .to_string();
    g.set_settings_audio_device_desc(ss(&dev_desc));
    g.set_settings_passthrough_device(ss(&c.audio_device_passthrough));
    let pt_desc = s.audio_devices.iter()
        .find(|(n, _)| n == &c.audio_device_passthrough)
        .map(|(_, d)| d.as_str())
        .unwrap_or(if c.audio_device_passthrough.is_empty() { "" } else { c.audio_device_passthrough.as_str() })
        .to_string();
    g.set_settings_passthrough_device_desc(ss(&pt_desc));
    // The IRQ fix targets the device passthrough actually plays on.
    let effective = if c.audio_device_passthrough.is_empty() { &c.audio_device } else { &c.audio_device_passthrough };
    g.set_settings_device_is_pipewire(pipewire_fix::is_pipewire_device(effective));
    g.set_settings_audio_channels(ss(if c.audio_channels.is_empty() { "auto-safe" } else { &c.audio_channels }));
    g.set_settings_gapless_audio(c.gapless_audio);
    g.set_settings_now_playing_auto_open(c.now_playing_auto_open);
    g.set_settings_audio_spdif(c.audio_spdif);
    g.set_settings_spdif_ac3(c.spdif_ac3);
    g.set_settings_spdif_eac3(c.spdif_eac3);
    g.set_settings_spdif_dts(c.spdif_dts);
    g.set_settings_spdif_dts_hd(c.spdif_dts_hd);
    g.set_settings_spdif_truehd(c.spdif_truehd);
    g.set_settings_hwdec(ss(&c.hwdec));
    g.set_settings_vf(ss(&c.vf));
    g.set_settings_video_sync(ss(&c.video_sync));
    g.set_settings_opengl_early_flush(c.opengl_early_flush);
    g.set_settings_video_latency_hacks(c.video_latency_hacks);
    g.set_settings_interpolation(c.interpolation);
    g.set_settings_tscale(ss(&c.tscale));
    g.set_settings_tone_mapping(ss(&c.tone_mapping));
    g.set_settings_target_colorspace_hint(c.target_colorspace_hint);
    g.set_settings_deinterlace(ss(&c.deinterlace));
    g.set_settings_cache_mb(c.cache_size_mb as i32);
    g.set_settings_video_behind(c.video_behind);
    g.set_settings_launch_fullscreen(c.launch_fullscreen);
    g.set_settings_log_level(ss(&c.log_level));
    g.set_settings_sub_enabled(c.sub_enabled);
    g.set_settings_sub_lang(ss(&c.sub_lang));
    g.set_settings_sub_lang2(ss(&c.sub_lang2));
    g.set_settings_sub_type(ss(&c.sub_type));
    g.set_settings_audio_lang(ss(&c.audio_lang));
    g.set_settings_alsa_irq_scheduling(c.alsa_irq_scheduling);
    g.set_settings_skip_intro_mode(ss(&c.skip_intro_mode));
    g.set_settings_skip_intro_secs(c.skip_intro_secs as i32);
    g.set_settings_skip_recap_mode(ss(&c.skip_recap_mode));
    g.set_settings_skip_recap_secs(c.skip_recap_secs as i32);
    g.set_settings_skip_preview_mode(ss(&c.skip_preview_mode));
    g.set_settings_skip_preview_secs(c.skip_preview_secs as i32);
    g.set_settings_skip_commercial_mode(ss(&c.skip_commercial_mode));
    g.set_settings_skip_commercial_secs(c.skip_commercial_secs as i32);
    g.set_settings_skip_credits_mode(ss(&c.skip_credits_mode));
    g.set_settings_skip_credits_secs(c.skip_credits_secs as i32);
}

fn read_settings_from_window(w: &MainWindow, s: &mut FjordState) {
    let g = AppState::get(w);
    let c = &mut s.config;
    c.audio_spdif            = g.get_settings_audio_spdif();
    c.spdif_ac3              = g.get_settings_spdif_ac3();
    c.spdif_eac3             = g.get_settings_spdif_eac3();
    c.spdif_dts              = g.get_settings_spdif_dts();
    c.spdif_dts_hd           = g.get_settings_spdif_dts_hd();
    c.spdif_truehd           = g.get_settings_spdif_truehd();
    c.hwdec                  = g.get_settings_hwdec().to_string();
    c.vf                     = g.get_settings_vf().to_string();
    c.video_sync             = g.get_settings_video_sync().to_string();
    c.opengl_early_flush     = g.get_settings_opengl_early_flush();
    c.video_latency_hacks    = g.get_settings_video_latency_hacks();
    c.interpolation          = g.get_settings_interpolation();
    c.tscale                 = g.get_settings_tscale().to_string();
    c.tone_mapping           = g.get_settings_tone_mapping().to_string();
    c.target_colorspace_hint = g.get_settings_target_colorspace_hint();
    c.deinterlace            = g.get_settings_deinterlace().to_string();
    c.cache_size_mb          = g.get_settings_cache_mb().max(0) as u32;
    c.video_behind           = g.get_settings_video_behind();
    c.launch_fullscreen      = g.get_settings_launch_fullscreen();
    c.log_level              = g.get_settings_log_level().to_string();
    c.sub_enabled            = g.get_settings_sub_enabled();
    c.sub_lang               = g.get_settings_sub_lang().to_string();
    c.sub_lang2              = g.get_settings_sub_lang2().to_string();
    c.sub_type               = g.get_settings_sub_type().to_string();
    c.audio_lang             = g.get_settings_audio_lang().to_string();
    c.audio_device           = g.get_settings_audio_device().to_string();
    c.audio_device_passthrough = g.get_settings_passthrough_device().to_string();
    c.audio_channels           = g.get_settings_audio_channels().to_string();
    c.gapless_audio            = g.get_settings_gapless_audio();
    c.now_playing_auto_open   = g.get_settings_now_playing_auto_open();
    c.alsa_irq_scheduling    = g.get_settings_alsa_irq_scheduling();
    c.skip_intro_mode        = g.get_settings_skip_intro_mode().to_string();
    c.skip_intro_secs        = g.get_settings_skip_intro_secs().max(0) as u32;
    c.skip_recap_mode        = g.get_settings_skip_recap_mode().to_string();
    c.skip_recap_secs        = g.get_settings_skip_recap_secs().max(0) as u32;
    c.skip_preview_mode      = g.get_settings_skip_preview_mode().to_string();
    c.skip_preview_secs      = g.get_settings_skip_preview_secs().max(0) as u32;
    c.skip_commercial_mode   = g.get_settings_skip_commercial_mode().to_string();
    c.skip_commercial_secs   = g.get_settings_skip_commercial_secs().max(0) as u32;
    c.skip_credits_mode      = g.get_settings_skip_credits_mode().to_string();
    c.skip_credits_secs      = g.get_settings_skip_credits_secs().max(0) as u32;
}

// ── audio device discovery ────────────────────────────────────────────────────

fn fetch_audio_devices() -> Vec<(String, String)> {
    let out = std::process::Command::new("mpv")
        .args(["--no-config", "--audio-device=help"])
        .output();
    let Ok(out) = out else {
        return vec![("auto".into(), "Autoselect device".into())];
    };
    let raw = String::from_utf8_lossy(&out.stdout);
    let text = if raw.trim().is_empty() { String::from_utf8_lossy(&out.stderr).into_owned() } else { raw.into_owned() };
    let mut devices = vec![("auto".into(), "Autoselect device".into())];
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('\'') { continue; }
        let Some(end_q) = line[1..].find('\'') else { continue };
        let name = line[1..end_q + 1].to_string();
        if name == "auto" { continue; }
        let rest = line[end_q + 2..].trim();
        let desc = if rest.starts_with('(') && rest.ends_with(')') {
            rest[1..rest.len() - 1].to_string()
        } else { name.clone() };
        devices.push((name, desc));
    }
    devices
}

// ── startup connectivity gate ────────────────────────────────────────────────
// Push on-disk caches into AppState/FjordState for instant display. Only
// called once the saved-session auth probe has confirmed the server is
// reachable (see spawn_auto_login) — showing cached content before that was
// confirmed made a fully offline cold start look identical to normal quiet
// operation (nothing distinguished "stale but fine" from "can't reach the
// server at all").
fn push_cached_data(
    window:    &MainWindow,
    client:    &Arc<JellyfinClient>,
    state:     &Arc<Mutex<FjordState>>,
    rt_handle: &tokio::runtime::Handle,
) {
    if let Some(cached_home) = load_home_cache() {
        push_home_data(window, &cached_home);
        let sections = home_data_sections(&cached_home);
        spawn_poster_loading(Arc::clone(client), sections, window.as_weak(), rt_handle.clone());
    }
    if let Some(cached_movies) = load_movies_cache() {
        let model = items_to_model(&cached_movies);
        spawn_movies_poster_loading(Arc::clone(client), cached_movies.clone(), window.as_weak(), rt_handle.clone());
        // Display-only: do NOT set movies_fetched — the first grid open this
        // session must still do its network refresh (cache-staleness fix S1).
        state.lock().unwrap().all_movies = cached_movies;
        AppState::get(window).set_all_movies(model);
    }
    if let Some(cached_series) = load_series_cache() {
        AppState::get(window).set_all_series(items_to_model(&cached_series));
        spawn_series_poster_loading(Arc::clone(client), cached_series.clone(), window.as_weak(), rt_handle.clone());
        state.lock().unwrap().all_series = cached_series;
    }
    if let Some(cached_cols) = load_collections_cache() {
        let model = items_to_model(&cached_cols);
        spawn_collections_poster_loading(Arc::clone(client), cached_cols.clone(), window.as_weak(), rt_handle.clone());
        state.lock().unwrap().all_collections = cached_cols;
        AppState::get(window).set_all_collections(model);
    }
    if let Some(cached_artists) = load_artists_cache() {
        let model = items_to_model(&cached_artists);
        spawn_artists_poster_loading(Arc::clone(client), cached_artists.clone(), window.as_weak(), rt_handle.clone());
        state.lock().unwrap().all_artists = cached_artists;
        AppState::get(window).set_all_artists(model);
    }
    if let Some(cached_albums) = load_albums_cache() {
        let model = items_to_model(&cached_albums);
        spawn_albums_poster_loading(Arc::clone(client), cached_albums.clone(), window.as_weak(), rt_handle.clone());
        state.lock().unwrap().all_albums = cached_albums;
        AppState::get(window).set_all_albums(model);
    }
    if let Some(cached_playlists) = load_playlists_cache() {
        let model = items_to_model(&cached_playlists);
        spawn_playlists_poster_loading(Arc::clone(client), cached_playlists.clone(), window.as_weak(), rt_handle.clone());
        state.lock().unwrap().all_playlists = cached_playlists;
        AppState::get(window).set_all_playlists(model);
    }
}

/// Probe the saved session, then either show cached content + refresh in the
/// background (reachable), redirect to login (definite 401), or show
/// OfflineScreen (anything else — DNS/connect/timeout: we genuinely can't
/// tell if the session is still valid). Re-invoked by the Retry button on
/// OfflineScreen with fresh clones, so this must not assume it only runs once.
fn spawn_auto_login(
    client:      Arc<JellyfinClient>,
    state:       Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    let rt_handle2 = rt_handle.clone();
    rt_handle.spawn(async move {
        if let Err(e) = client.check_auth().await {
            if is_unauthorized(&e) {
                warn!("saved token is invalid (401) — showing login screen");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = window_weak.upgrade() {
                        let g = AppState::get(&w);
                        g.set_show_connecting(false);
                        g.set_show_login(true);
                        g.set_status(ss("Session expired — please log in again"));
                    }
                });
                return;
            }
            // Not a definite session failure — we genuinely can't reach the
            // server. Say so plainly instead of quietly proceeding into a
            // dashboard that would just fail a dozen ways in the background.
            warn!("auth probe failed (non-401): {e:#}");
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = window_weak.upgrade() {
                    let g = AppState::get(&w);
                    g.set_show_connecting(false);
                    g.set_show_offline(true);
                    g.set_offline_focused(0);
                    g.set_status(ss("Couldn't reach the server — check your connection."));
                }
            });
            return;
        }

        // Reachable — load on-disk caches now for instant display, then
        // continue refreshing in the background exactly as before.
        let ww_cache     = window_weak.clone();
        let client_cache = Arc::clone(&client);
        let state_cache  = Arc::clone(&state);
        let rt_cache     = rt_handle2.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_cache.upgrade() else { return };
            push_cached_data(&w, &client_cache, &state_cache, &rt_cache);
            let g = AppState::get(&w);
            g.set_show_connecting(false);
            g.set_show_offline(false);
            g.set_show_login(false);
            w.invoke_grab_keyboard_focus();
        });

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
        state.lock().unwrap().all_series = series.clone();

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
                // Preserve already-decoded posters (from the cache push moments earlier)
                // across this unconditional startup refresh instead of flashing every
                // row blank — same rationale as ws.rs's delta-sync task (Phase 91/92).
                push_home_data_preserving_posters(&w, &home_data);
                g.set_all_series(refresh_row_preserving_posters(&g.get_all_series(), &series2));
                g.set_status(ss(""));
                w.invoke_grab_keyboard_focus();
            }
        });
        let client2 = Arc::clone(&client);
        let client3 = Arc::clone(&client);
        let client4 = Arc::clone(&client);
        let state3  = Arc::clone(&state);
        let state4  = Arc::clone(&state);
        let state5  = Arc::clone(&state);
        let ws_abort = ws::start_websocket(client4, Arc::clone(&state4), window_weak.clone(), rt_handle2.clone());
        state4.lock().unwrap().ws_abort = Some(ws_abort);
        spawn_poster_loading(client, sections, window_weak, rt_handle2.clone());
        spawn_series_poster_loading(client2, series, ww3, rt_handle2.clone());
        rt_handle2.spawn(async move {
            let map = fetch_movie_collections(&client3).await;
            state3.lock().unwrap().movie_collections = map;
        });
        rt_handle2.spawn(async move {
            let (movie_ids, series_ids, collection_ids, artist_ids, album_ids, playlist_ids) = {
                let s = state5.lock().unwrap();
                let m  = s.all_movies.iter().map(|i| i.id.clone()).collect();
                let se = s.all_series.iter().map(|i| i.id.clone()).collect();
                let c  = s.all_collections.iter().map(|i| i.id.clone()).collect();
                let a  = s.all_artists.iter().map(|i| i.id.clone()).collect();
                let al = s.all_albums.iter().map(|i| i.id.clone()).collect();
                let pl = s.all_playlists.iter().map(|i| i.id.clone()).collect();
                (m, se, c, a, al, pl)
            };
            run_poster_cache_cleanup(movie_ids, series_ids, collection_ids, artist_ids, album_ids, playlist_ids).await;
        });
    });
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
    // Rotate on every start: fjord.log → fjord.log.old (previous .old replaced).
    // The file was previously appended forever with no rotation — combined with
    // the keep-alive loop bug it reached 6.4 GB (Phase 62).
    let log_path = log_dir.join("fjord.log");
    if log_path.exists() {
        let _ = std::fs::rename(&log_path, log_dir.join("fjord.log.old"));
    }
    let file_appender = tracing_appender::rolling::never(&log_dir, "fjord.log");
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter, Layer};
    // User's Settings→General log-level choice (default "info"), read directly from
    // disk before the subscriber exists — the full config load happens later once
    // `state`/`window` exist, this is just a cheap early peek. Applies on next
    // launch, not live. RUST_LOG still wins over this when set (dev override) —
    // the file used to grow without bound before Phase 62's per-launch rotation,
    // so a debug-level file is now bounded to one session's worth.
    let user_level = load_config().map(|c| c.log_level).unwrap_or_default();
    let level_str = match user_level.as_str() {
        "error" | "warn" | "debug" => user_level.as_str(),
        _ => "info",
    };
    let console_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!("warn,fjord_app={level_str},fjord_player={level_str},fjord_api={level_str}"))
    });
    let file_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!("warn,fjord_app={level_str},fjord_player={level_str},fjord_api={level_str}"))
    });
    tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(console_filter))
        .with(tracing_subscriber::fmt::layer().with_writer(file_writer).with_filter(file_filter))
        .init();
    info!("log file: {}", log_path.display());
    info!("fjord version: {}", env!("FJORD_BUILD_ID"));

    // Panic hook — writes directly to the log file so Slint "Recursion detected"
    // panics (which would otherwise SIGABRT silently) appear in fjord.log.
    let panic_log = log_dir.join("fjord.log");
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let bt  = std::backtrace::Backtrace::force_capture();
        let msg = format!("PANIC: {info}\nBacktrace:\n{bt}\n");
        eprintln!("{msg}");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&panic_log) {
            use std::io::Write;
            let _ = f.write_all(msg.as_bytes());
        }
        default_hook(info);
    }));

    let rt     = tokio::runtime::Runtime::new()?;
    let window = MainWindow::new()?;
    let state  = Arc::new(Mutex::new(FjordState::new()));
    let video  = Arc::new(Mutex::new(VideoState::default()));

    // Shared flag: show_controls() sets it lock-free; the mpv timer reads it
    // while already holding the video lock and resets controls_idle_ticks.
    // This avoids the UI thread blocking on the video mutex during mouse movement.
    let controls_show  = Arc::new(AtomicBool::new(false));
    let seek_suppress  = Arc::new(AtomicU32::new(0));

    wire_rendering_notifier(&window, Arc::clone(&video));
    let mpv_timer = wire_mpv_timer(window.as_weak(), Arc::clone(&video), Arc::clone(&state), rt.handle().clone(), Arc::clone(&controls_show), Arc::clone(&seek_suppress));
    std::mem::forget(mpv_timer);

    let nw_timer = wire_nw_timer(window.as_weak(), Arc::clone(&video), Arc::clone(&state), rt.handle().clone());
    std::mem::forget(nw_timer);

    // ── random logo index — pick from available icons at startup ─────────────
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        const LOGOS: [i32; 6] = [1, 2, 4, 5, 9, 10];
        let n = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().subsec_nanos() as usize;
        AppState::get(&window).set_app_logo_idx(LOGOS[n % LOGOS.len()]);
    }

    // ── apply saved config ────────────────────────────────────────────────────
    if let Some(mut cfg) = load_config() {
        ensure_device_id(&mut cfg);
        // If IRQ scheduling is on AND the device is PipeWire (so the file should
        // exist) but the file is missing, sync down to false so the UI matches reality.
        // Skip this when a direct ALSA device is selected — the file is intentionally
        // absent then and alsa_irq_scheduling should be preserved for when the user
        // switches back to a PipeWire device.
        if cfg.audio_spdif
            && cfg.alsa_irq_scheduling
            && pipewire_fix::is_pipewire_device(
                if cfg.audio_device_passthrough.is_empty() { &cfg.audio_device }
                else { &cfg.audio_device_passthrough })
            && !pipewire_fix::wireplumber_config_exists()
        {
            cfg.alsa_irq_scheduling = false;
            save_config(&cfg);
        }
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
            let Ok(raw_client) = JellyfinClient::new(server_url.clone(), user_id, token, device_id)
                else { tracing::error!("failed to build HTTP client — skipping auto-login"); return Ok(()) };
            let client = Arc::new(raw_client);
            state.lock().unwrap().client = Some(Arc::clone(&client));
            AppState::get(&window).set_server_url(ss(&server_url_str));

            // Startup connectivity gate: show a plain connecting state instead
            // of pushing cached content until the saved session is confirmed
            // reachable — a full outage should be visibly different from
            // normal quiet operation, not hidden behind a stale dashboard.
            // show-login must be explicitly cleared here too — it defaults to
            // true, and keys.rs's handle_key checks it before show-connecting/
            // show-offline, so leaving it at the default would silently eat
            // every key on both new screens (only the 401 branch sets it back
            // to true).
            {
                let g = AppState::get(&window);
                g.set_show_login(false);
                g.set_show_connecting(true);
            }

            spawn_auto_login(Arc::clone(&client), Arc::clone(&state), window.as_weak(), rt.handle().clone());

            let client_retry = Arc::clone(&client);
            let state_retry  = Arc::clone(&state);
            let ww_retry     = window.as_weak();
            let rt_retry     = rt.handle().clone();
            AppState::get(&window).on_retry_connection(move || {
                if let Some(w) = ww_retry.upgrade() {
                    let g = AppState::get(&w);
                    g.set_show_offline(false);
                    g.set_show_login(false);
                    g.set_show_connecting(true);
                }
                spawn_auto_login(Arc::clone(&client_retry), Arc::clone(&state_retry), ww_retry.clone(), rt_retry.clone());
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
                                   series_id, None, &video2b, &ww2, &rth2);
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

            // BoxSet (collection) — open collection screen instead of playing.
            // all_collections is only populated when the library grid is first opened; fall
            // back to the always-present dashboard models when it hasn't been opened yet.
            let boxset_info = s.all_collections.iter()
                .find(|i| i.id == item_id)
                .map(|bs| (bs.id.clone(), bs.name.clone()))
                .or_else(|| {
                    let w = window_weak.upgrade()?;
                    let g = AppState::get(&w);
                    let find_boxset = |model: ModelRc<CardItem>| -> Option<(String, String)> {
                        for idx in 0..model.row_count() {
                            if let Some(c) = model.row_data(idx) {
                                if c.id.as_str() == item_id && c.item_type.as_str() == "BoxSet" {
                                    return Some((c.id.to_string(), c.title.to_string()));
                                }
                            }
                        }
                        None
                    };
                    find_boxset(g.get_recently_added_collections())
                        .or_else(|| find_boxset(g.get_unwatched_collections()))
                });
            if let Some((bs_id, bs_name)) = boxset_info {
                let ww2        = window_weak.clone();
                let state2     = state.clone();
                let rt_handle2 = rt_handle.clone();
                drop(s);
                collection::open_collection_screen(bs_id, bs_name, state2, ww2, rt_handle2);
                return;
            }

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
                                           series_id, None, &video4, &ww2, &rt_handle2);
                        });
                    } else {
                        let _ = slint::invoke_from_event_loop(move || {
                            open_series_screen(item_id, state2, ww2, rt_handle2);
                        });
                    }
                });
                return;
            }

            let mut config  = s.player_config();
            let state_album = Arc::clone(&state);
            let state_purge = Arc::clone(&state);
            drop(s);
            let play_url = client.direct_play_url(&item_id);
            let video3b  = Arc::clone(&video3);
            let ww3      = window_weak.clone();
            let rth3     = rt_handle.clone();
            rt_handle.spawn(async move {
                let detail = match client.get_item_detail(&item_id).await {
                    Ok(d) => Some(d),
                    Err(e) if crate::is_not_found(&e) => {
                        // Ghost card: the item is gone server-side — clean up
                        // instead of handing mpv a dead stream URL (S4).
                        purge_deleted_item(&state_purge, &ww3, &item_id);
                        return;
                    }
                    Err(e) => { warn!("item_play get_item_detail({item_id}): {e:#}"); None }
                };
                let item_type = detail.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
                let series_id = detail.as_ref().and_then(|i| i.series_id.clone());
                let title     = detail.as_ref().map(|i| i.display_name()).unwrap_or_else(|| item_id.clone());
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
                    start_playback(play_url, item_id, &item_type, title, config, client,
                                   series_id, None, &video3b, &ww3, &rth3);
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
            // Synchronously initialise sort/filter/query for this library type before any async work.
            {
                let sort_val = {
                    let s = state_ol.lock().unwrap();
                    match nav {
                        1 => s.config.library_series_sort,
                        2 => s.config.library_movies_sort,
                        3 => s.config.library_collections_sort,
                        4 => match s.config.library_music_view {
                            1 => s.config.library_albums_sort,
                            2 => s.config.library_playlists_sort,
                            _ => s.config.library_artists_sort,
                        },
                        _ => 0,
                    }
                };
                if let Some(w) = ww_ol.upgrade() {
                    let g = AppState::get(&w);
                    g.set_library_sort(sort_val as i32);
                    g.set_library_filter_unwatched(false);
                    g.set_library_filter_favorites(false);
                    g.set_library_query("".into());
                    g.set_library_sort_cursor(0);
                    g.set_library_back_focused(false);
                    g.set_library_has_filters(nav != 3 && nav != 4);
                    if nav == 4 {
                        let music_view = state_ol.lock().unwrap().config.library_music_view as i32;
                        g.set_library_music_view(music_view);
                    }
                    browse::refresh_library_display(&w);
                }
            }
            spawn_library_fetch(nav, Arc::clone(&state_ol), ww_ol.clone(), rth_ol.clone());
        });
    }

    // ── music library view toggle (Artists ↔ Albums) ─────────────────────────
    {
        let state_mv = Arc::clone(&state);
        let ww_mv    = window.as_weak();
        let rth_mv   = rt.handle().clone();
        AppState::get(&window).on_library_music_view_changed(move |view| {
            {
                let mut s = state_mv.lock().unwrap();
                s.config.library_music_view = view.clamp(0, 2) as u8;
                // Restore the correct sort for the new view.
                let sort_val = match view {
                    1 => s.config.library_albums_sort,
                    2 => s.config.library_playlists_sort,
                    _ => s.config.library_artists_sort,
                };
                crate::config::save_config(&s.config);
                drop(s);
                if let Some(w) = ww_mv.upgrade() {
                    let g = AppState::get(&w);
                    g.set_library_music_view(view);
                    g.set_library_sort(sort_val as i32);
                    g.set_library_focused(0);
                    browse::refresh_library_display(&w);
                }
            }
            // Trigger a fetch of the other data source if not yet done.
            let (need_fetch, already_fetched) = {
                let s = state_mv.lock().unwrap();
                match view {
                    1 => (!s.albums_fetched,    s.albums_fetched),
                    2 => (!s.playlists_fetched, s.playlists_fetched),
                    _ => (!s.artists_fetched,   s.artists_fetched),
                }
            };
            let _ = already_fetched; // suppress unused warning
            if need_fetch {
                let state_f = Arc::clone(&state_mv);
                let ww_f    = ww_mv.clone();
                let ww_f2   = ww_mv.clone();
                let Some(client) = state_mv.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
                let client2 = Arc::clone(&client);
                let rth_spawn = rth_mv.clone();
                rth_mv.spawn(async move {
                    if view == 2 {
                        match client.get_all_playlists().await {
                            Ok(playlists) => {
                                { let mut s = state_f.lock().unwrap(); s.all_playlists = playlists.clone(); s.playlists_fetched = true; }
                                save_playlists_cache(&playlists);
                                let playlists2 = playlists.clone();
                                let ww_p = ww_f.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = ww_p.upgrade() {
                                        AppState::get(&w).set_all_playlists(items_to_model(&playlists2));
                                        if AppState::get(&w).get_show_library() && AppState::get(&w).get_library_music_view() == 2 {
                                            browse::refresh_library_display(&w);
                                        }
                                    }
                                });
                                spawn_playlists_poster_loading(client2, playlists, ww_f2, rth_spawn.clone());
                            }
                            Err(e) => warn!("music view playlists fetch: {:#}", e),
                        }
                    } else if view == 1 {
                        match client.get_all_albums().await {
                            Ok(albums) => {
                                { let mut s = state_f.lock().unwrap(); s.all_albums = albums.clone(); s.albums_fetched = true; }
                                save_albums_cache(&albums);
                                let albums2 = albums.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = ww_f.upgrade() {
                                        AppState::get(&w).set_all_albums(items_to_model(&albums2));
                                        if AppState::get(&w).get_show_library() && AppState::get(&w).get_library_music_view() == 1 {
                                            browse::refresh_library_display(&w);
                                        }
                                    }
                                });
                                spawn_albums_poster_loading(client2, albums, ww_f2, rth_spawn.clone());
                            }
                            Err(e) => warn!("music view albums fetch: {:#}", e),
                        }
                    } else {
                        match client.get_album_artists().await {
                            Ok(artists) => {
                                { let mut s = state_f.lock().unwrap(); s.all_artists = artists.clone(); s.artists_fetched = true; }
                                save_artists_cache(&artists);
                                let artists2 = artists.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = ww_f.upgrade() {
                                        AppState::get(&w).set_all_artists(items_to_model(&artists2));
                                        if AppState::get(&w).get_show_library() && AppState::get(&w).get_library_music_view() == 0 {
                                            browse::refresh_library_display(&w);
                                        }
                                    }
                                });
                                spawn_artists_poster_loading(client2, artists, ww_f2, rth_spawn.clone());
                            }
                            Err(e) => warn!("music view artists fetch: {:#}", e),
                        }
                    }
                });
            }
        });
    }

    // ── detail page ───────────────────────────────────────────────────────────
    {
        let state2    = Arc::clone(&state);
        let ww        = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_detail(move |id, item_type| {
            match item_type.as_str() {
                "MusicArtist" => {
                    let title = {
                        let s = state2.lock().unwrap();
                        s.all_artists.iter()
                            .find(|a| a.id == id.as_str())
                            .map(|a| a.display_name())
                            .unwrap_or_else(|| id.to_string())
                    };
                    artist::open_artist_screen(id.to_string(), title, Arc::clone(&state2), ww.clone(), rt_handle.clone());
                }
                "MusicAlbum" => {
                    let title = {
                        let s = state2.lock().unwrap();
                        s.all_albums.iter()
                            .find(|a| a.id == id.as_str())
                            .map(|a| a.display_name())
                            .unwrap_or_else(|| id.to_string())
                    };
                    album::open_album_screen(id.to_string(), title, Arc::clone(&state2), ww.clone(), rt_handle.clone());
                }
                "Playlist" => {
                    let title = {
                        let s = state2.lock().unwrap();
                        s.all_playlists.iter()
                            .find(|p| p.id == id.as_str())
                            .map(|p| p.name.clone())
                            .unwrap_or_else(|| id.to_string())
                    };
                    album::open_playlist_screen(id.to_string(), title, Arc::clone(&state2), ww.clone(), rt_handle.clone());
                }
                _ => {
                    detail::open_detail(id.to_string(), item_type.to_string(), Arc::clone(&state2), ww.clone(), rt_handle.clone());
                }
            }
        });
    }
    // ── collection screen ─────────────────────────────────────────────────────
    {
        let state_col = Arc::clone(&state);
        let ww        = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_collection(move |id, title| {
            collection::open_collection_screen(id.to_string(), title.to_string(), Arc::clone(&state_col), ww.clone(), rt_handle.clone());
        });
    }
    // ── artist screen ─────────────────────────────────────────────────────────
    {
        let state_art = Arc::clone(&state);
        let ww        = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_artist(move |id, title| {
            artist::open_artist_screen(id.to_string(), title.to_string(), Arc::clone(&state_art), ww.clone(), rt_handle.clone());
        });
    }
    {
        let ww_art = window.as_weak();
        AppState::get(&window).on_close_artist(move || {
            if let Some(w) = ww_art.upgrade() { AppState::get(&w).set_show_artist(false); }
        });
    }
    {
        let state_taf = Arc::clone(&state);
        let ww_taf    = window.as_weak();
        // Capture the runtime handle — Handle::current() panics on the Slint
        // event-loop thread because main() never enters the Tokio runtime.
        let rt_taf    = rt.handle().clone();
        AppState::get(&window).on_toggle_artist_fav(move || {
            let Some(w) = ww_taf.upgrade() else { return };
            let g       = AppState::get(&w);
            let id      = g.get_artist_id().to_string();
            let new_fav = !g.get_artist_is_favorite();
            g.set_artist_is_favorite(new_fav);
            let s = state_taf.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let ww3 = ww_taf.clone();
            drop(s);
            let rth = rt_taf.clone();
            rt_taf.spawn(async move {
                let result = if new_fav { client.set_favorite(&id).await } else { client.unset_favorite(&id).await };
                if let Err(e) = result {
                    warn!("toggle_artist_fav: {e}");
                    crate::show_toast(ww3, format!("Favourite error: {e}"));
                    return;
                }
                let ww4 = ww3.clone();
                let id2 = id.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww4.upgrade() {
                        crate::context_menu::update_card_in_all_models(&w, &id2, None, Some(new_fav));
                    }
                });
                crate::home::refresh_favorites(client, ww3, rth);
            });
        });
    }
    {
        let state_paa = Arc::clone(&state);
        let video_paa = Arc::clone(&video);
        let ww_paa    = window.as_weak();
        let rt_paa    = rt.handle().clone();
        AppState::get(&window).on_play_artist_all(move || {
            let Some(w) = ww_paa.upgrade() else { return };
            let g       = AppState::get(&w);
            let albums  = g.get_artist_albums();
            if albums.row_count() == 0 { return }

            let album_ids: Vec<String> = (0..albums.row_count())
                .filter_map(|i| albums.row_data(i))
                .map(|c| c.id.to_string())
                .collect();
            let artist = g.get_artist_title().to_string();

            let s = state_paa.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config   = s.player_config();
            config.start_position_secs = None;
            drop(s);

            let video2 = Arc::clone(&video_paa);
            let ww3    = ww_paa.clone();

            rt_paa.spawn(async move {
                // Fetch tracks for every album in order; track (id, title, album_id)
                let mut all_tracks: Vec<(String, String, String)> = Vec::new();
                for album_id in &album_ids {
                    if let Ok(tracks) = client.get_album_tracks(album_id).await {
                        for t in tracks { all_tracks.push((t.id, t.name, album_id.clone())); }
                    }
                }
                if all_tracks.is_empty() { return }

                let (first_id, first_title, first_alb_id) = all_tracks[0].clone();
                let first_url = client.direct_play_url(&first_id);
                let rt3 = tokio::runtime::Handle::current();

                let _ = slint::invoke_from_event_loop(move || {
                    {
                        let mut vs = video2.lock().unwrap();
                        // Rebuild the playlist but keep vs.queue — Play All plays
                        // now; previously queued items follow after (Phase 56).
                        vs.playlist.clear();
                        vs.playlist_index = 0;
                        vs.shuffle_order.clear();
                        for (id, title, alb_id) in &all_tracks {
                            vs.playlist.push(crate::playback::QueueItem {
                                id:         id.clone(),
                                item_type:  "Audio".into(),
                                series_id:  None,
                                title:      title.clone(),
                                audio_meta: Some((artist.clone(), alb_id.clone())),
                            });
                        }
                        crate::playback::rebuild_shuffle_order(&mut vs);
                        if let Some(w) = ww3.upgrade() {
                            push_queue_display(&vs, &AppState::get(&w));
                        }
                    }
                    start_playback(first_url, first_id, "Audio", first_title, config, client,
                                   None, Some((artist, first_alb_id)),
                                   &video2, &ww3, &rt3);
                });
            });
        });
    }
    // ── album screen ──────────────────────────────────────────────────────────
    {
        let state_alb = Arc::clone(&state);
        let ww        = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_album(move |id, title| {
            album::open_album_screen(id.to_string(), title.to_string(), Arc::clone(&state_alb), ww.clone(), rt_handle.clone());
        });
    }
    {
        let ww_ca = window.as_weak();
        AppState::get(&window).on_close_album(move || {
            if let Some(w) = ww_ca.upgrade() { AppState::get(&w).set_show_album(false); }
        });
    }
    {
        let state_pt  = Arc::clone(&state);
        let video_pt  = Arc::clone(&video);
        let ww        = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_play_album_track(move |track_id| {
            // Spotify-style: Enter on a track plays the WHOLE album/playlist
            // from that track — the visible tracklist becomes the playlist and
            // the rest follows (gapless applies). Was: single track only.
            let track_id = track_id.to_string();
            let Some(w) = ww.upgrade() else { return };
            let g        = AppState::get(&w);
            let tracks   = g.get_album_tracks();
            let count    = tracks.row_count();
            if count == 0 { return }
            let s = state_pt.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            drop(s);
            config.start_position_secs = None;
            let album_id = g.get_album_id().to_string();
            let artist   = g.get_album_artist().to_string();
            let mut start_idx = 0usize;
            {
                let mut vs = video_pt.lock().unwrap();
                // Rebuild the playlist but keep vs.queue (Phase 56).
                vs.playlist.clear();
                vs.shuffle_order.clear();
                for i in 0..count {
                    if let Some(t) = tracks.row_data(i) {
                        if t.id.as_str() == track_id { start_idx = i; }
                        let t_art = if t.artist.is_empty()   { artist.clone() }   else { t.artist.to_string() };
                        let t_alb = if t.album_id.is_empty() { album_id.clone() } else { t.album_id.to_string() };
                        vs.playlist.push(crate::playback::QueueItem {
                            id:         t.id.to_string(),
                            item_type:  "Audio".into(),
                            series_id:  None,
                            title:      t.title.to_string(),
                            audio_meta: Some((t_art, t_alb)),
                        });
                    }
                }
                vs.playlist_index = start_idx;
                crate::playback::rebuild_shuffle_order(&mut vs);
                push_queue_display(&vs, &g);
            }
            if let Some(t) = tracks.row_data(start_idx) {
                let url   = client.direct_play_url(&track_id);
                let t_art = if t.artist.is_empty()   { artist }   else { t.artist.to_string() };
                let t_alb = if t.album_id.is_empty() { album_id } else { t.album_id.to_string() };
                start_playback(url, track_id, "Audio", t.title.to_string(), config, client,
                               None, Some((t_art, t_alb)), &video_pt, &ww, &rt_handle);
            }
        });
    }
    {
        let state_pr = Arc::clone(&state);
        let ww_pr    = window.as_weak();
        let rt_pr    = rt.handle().clone();
        AppState::get(&window).on_playlist_remove_entry(move |idx| {
            let Some(w) = ww_pr.upgrade() else { return };
            let g = AppState::get(&w);
            if !g.get_album_is_playlist() { return; }
            let Some(t) = g.get_album_tracks().row_data(idx as usize) else { return };
            let entry_id = t.entry_id.to_string();
            if entry_id.is_empty() { return; }
            let playlist_id = g.get_album_id().to_string();
            let s = state_pr.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww2 = ww_pr.clone();
            rt_pr.spawn(async move {
                let eid = entry_id.clone();
                if let Err(e) = client.remove_from_playlist(&playlist_id, &[eid]).await {
                    warn!("remove_from_playlist: {e:#}");
                    crate::show_toast(ww2, "Couldn't remove from playlist".to_string());
                    return;
                }
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww2.upgrade() else { return };
                    let g = AppState::get(&w);
                    // Only mutate if the same playlist is still open.
                    if !g.get_album_is_playlist() || g.get_album_id().as_str() != playlist_id { return; }
                    let model = g.get_album_tracks();
                    let mut kept: Vec<crate::TrackItem> = (0..model.row_count())
                        .filter_map(|i| model.row_data(i))
                        .filter(|t| t.entry_id.as_str() != entry_id)
                        .collect();
                    for (i, t) in kept.iter_mut().enumerate() { t.track_number = (i + 1) as i32; }
                    let len = kept.len() as i32;
                    g.set_album_tracks(slint::ModelRc::new(slint::VecModel::from(kept)));
                    if g.get_album_focused_track() >= len && len > 0 {
                        g.set_album_focused_track(len - 1);
                    }
                });
            });
        });
    }
    {
        let state_tf = Arc::clone(&state);
        let ww_tf    = window.as_weak();
        let rt_tf    = rt.handle().clone();
        AppState::get(&window).on_toggle_album_fav(move || {
            let Some(w) = ww_tf.upgrade() else { return };
            let g        = AppState::get(&w);
            let id       = g.get_album_id().to_string();
            let new_fav  = !g.get_album_is_favorite();
            g.set_album_is_favorite(new_fav);
            let s = state_tf.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let ww3 = ww_tf.clone();
            drop(s);
            let rth = rt_tf.clone();
            rt_tf.spawn(async move {
                let result = if new_fav { client.set_favorite(&id).await } else { client.unset_favorite(&id).await };
                if let Err(e) = result {
                    warn!("toggle_album_fav: {e}");
                    crate::show_toast(ww3, format!("Favourite error: {e}"));
                    return;
                }
                let ww4 = ww3.clone();
                let id2 = id.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww4.upgrade() {
                        crate::context_menu::update_card_in_all_models(&w, &id2, None, Some(new_fav));
                    }
                });
                crate::home::refresh_favorites(client, ww3, rth);
            });
        });
    }
    {
        let state_tp = Arc::clone(&state);
        let ww_tp    = window.as_weak();
        let rt_tp    = rt.handle().clone();
        AppState::get(&window).on_toggle_album_played(move || {
            let Some(w) = ww_tp.upgrade() else { return };
            let g          = AppState::get(&w);
            let id         = g.get_album_id().to_string();
            let new_played = !g.get_album_has_played();
            g.set_album_has_played(new_played);
            let s = state_tp.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let ww3 = ww_tp.clone();
            drop(s);
            rt_tp.spawn(async move {
                let result = if new_played { client.mark_played(&id).await } else { client.mark_unplayed(&id).await };
                if let Err(e) = result {
                    warn!("toggle_album_played: {e}");
                    crate::show_toast(ww3, format!("Played error: {e}"));
                }
            });
        });
    }
    // ── Music bar callbacks ───────────────────────────────────────────────────
    {
        let ww_mb = window.as_weak();
        AppState::get(&window).on_music_bar_play_pause(move || {
            // Delegate to pause_play_toggle which also updates is_paused + music_bar_paused.
            if let Some(w) = ww_mb.upgrade() {
                AppState::get(&w).invoke_pause_play_toggle();
            }
        });
    }
    {
        let video_ms  = Arc::clone(&video);
        let ww_ms     = window.as_weak();
        let rt_ms     = rt.handle().clone();
        AppState::get(&window).on_music_bar_stop(move || {
            crate::playback::do_stop_playback(&video_ms, &ww_ms, &rt_ms);
        });
    }
    {
        let video_msk = Arc::clone(&video);
        AppState::get(&window).on_music_bar_seek(move |ratio| {
            let vs = video_msk.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                let dur = p.get_duration();
                if dur > 0.0 { p.seek_to(ratio as f64 * dur); }
            }
        });
    }
    {
        let video_msr = Arc::clone(&video);
        AppState::get(&window).on_music_bar_seek_rel(move |secs| {
            let vs = video_msr.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                if secs >= 0.0 { p.seek_forward(secs as f64); }
                else           { p.seek_backward(-secs as f64); }
            }
        });
    }
    {
        let state_mo = Arc::clone(&state);
        let ww_mo    = window.as_weak();
        let rt_mo    = rt.handle().clone();
        AppState::get(&window).on_music_bar_open_album(move || {
            let Some(w) = ww_mo.upgrade() else { return };
            let g       = AppState::get(&w);
            let id      = g.get_music_bar_album_id().to_string();
            if id.is_empty() { return }
            let title   = "".to_string(); // open_album_screen fetches the real title
            album::open_album_screen(id, title, Arc::clone(&state_mo), ww_mo.clone(), rt_mo.clone());
        });
    }
    {
        let ww_np = window.as_weak();
        AppState::get(&window).on_open_now_playing(move || {
            let Some(w) = ww_np.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_now_playing_back_focused(false);
            g.set_now_playing_in_strip(false);
            g.set_now_playing_ctrl_focused(2); // play/pause
            g.set_now_playing_strip_focused(0);
            // Refreshes queue-items AND kicks off art loading (on_refresh_queue_display
            // → spawn_queue_poster_loading) — the Up Next strip reads the same model
            // the Queue Panel does, but nothing else triggers the art fetch for it.
            g.invoke_refresh_queue_display();
            g.set_show_now_playing(true);
            // The three entry paths (mouse click, m key, idle auto-open) can each
            // land here with the global FocusScope having lost focus in between —
            // most reliably on idle auto-open, since nothing guarantees fs still
            // holds focus after a stretch of mouse-only interaction. Re-grab
            // unconditionally, matching the pattern used by every other
            // screen-open site (season/person/detail/series/auth).
            w.invoke_grab_keyboard_focus();
        });
    }
    {
        let ww_qp = window.as_weak();
        // Mouse entry point for the queue panel (music-bar ⋮ button). Mirrors the
        // 'q' keyboard path in keys.rs, which doesn't need a focus re-grab since a
        // keypress reaching handle_key already proves fs has focus — a mouse click
        // after a stretch of mouse-only interaction can't assume that (CR11-6).
        AppState::get(&window).on_open_queue_panel(move || {
            let Some(w) = ww_qp.upgrade() else { return };
            let g = AppState::get(&w);
            g.invoke_refresh_queue_display();
            g.set_queue_panel_cursor(0);
            let items = g.get_queue_items();
            for i in 0..items.row_count() {
                if let Some(e) = items.row_data(i) {
                    if e.is_current { g.set_queue_panel_cursor(i as i32); break; }
                }
            }
            g.set_show_queue_panel(true);
            w.invoke_grab_keyboard_focus();
        });
    }
    {
        let state_pa = Arc::clone(&state);
        let video_pa = Arc::clone(&video);
        let ww_pa    = window.as_weak();
        let rt_pa    = rt.handle().clone();
        AppState::get(&window).on_play_album_all(move || {
            let Some(w) = ww_pa.upgrade() else { return };
            let g        = AppState::get(&w);
            let tracks   = g.get_album_tracks();
            let count    = tracks.row_count();
            if count == 0 { return }
            let s        = state_pa.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config   = s.player_config();
            drop(s);
            let album_id = g.get_album_id().to_string();
            let artist   = g.get_album_artist().to_string();
            // Populate the full playlist (all tracks) before starting track 0.
            {
                let mut vs = video_pa.lock().unwrap();
                // Rebuild the playlist but keep vs.queue (Phase 56).
                vs.playlist.clear();
                vs.playlist_index = 0;
                vs.shuffle_order.clear();
                for i in 0..count {
                    if let Some(t) = tracks.row_data(i) {
                        // Playlist rows carry their own artist + owning album id
                        // (music-bar art); album rows fall back to screen context.
                        let t_art = if t.artist.is_empty()   { artist.clone() }   else { t.artist.to_string() };
                        let t_alb = if t.album_id.is_empty() { album_id.clone() } else { t.album_id.to_string() };
                        vs.playlist.push(crate::playback::QueueItem {
                            id:         t.id.to_string(),
                            item_type:  "Audio".into(),
                            series_id:  None,
                            title:      t.title.to_string(),
                            audio_meta: Some((t_art, t_alb)),
                        });
                    }
                }
                crate::playback::rebuild_shuffle_order(&mut vs);
                push_queue_display(&vs, &g);
            }
            if let Some(t) = tracks.row_data(0) {
                let track_id  = t.id.to_string();
                let title     = t.title.to_string();
                let url       = client.direct_play_url(&track_id);
                let t_art = if t.artist.is_empty()   { artist }   else { t.artist.to_string() };
                let t_alb = if t.album_id.is_empty() { album_id } else { t.album_id.to_string() };
                let audio_meta = Some((t_art, t_alb));
                config.start_position_secs = None;
                start_playback(url, track_id, "Audio", title, config, client,
                               None, audio_meta, &video_pa, &ww_pa, &rt_pa);
            }
        });
    }
    {
        let state_pd  = Arc::clone(&state);
        let ww        = window.as_weak();
        let video_pd  = Arc::clone(&video);
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_play_detail(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let id = g.get_detail_id().to_string();
            if id.is_empty() || g.get_detail_loading() { return }
            let item_type  = g.get_detail_item_type().to_string();
            let series_id  = g.get_detail_series_id().to_string();
            let series_id  = if series_id.is_empty() { None } else { Some(series_id) };
            let title      = g.get_detail_title().to_string();
            // Flag that this play came from the detail page so start_playback keeps it
            // alive (hidden by !is-playing condition) and reset_playback_ui restores it.
            video_pd.lock().unwrap().from_detail = true;
            let s = state_pd.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            config.start_position_secs = None;
            drop(s);
            let play_url = client.direct_play_url(&id);
            info!("play_detail: {}", id);
            start_playback(play_url, id, &item_type, title, config, client,
                           series_id, None, &video_pd, &ww, &rt_handle);
        });
    }
    {
        let state_rd  = Arc::clone(&state);
        let ww        = window.as_weak();
        let video_rd  = Arc::clone(&video);
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_resume_detail(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let id = g.get_detail_id().to_string();
            if id.is_empty() || g.get_detail_loading() { return }
            let item_type  = g.get_detail_item_type().to_string();
            let series_id  = g.get_detail_series_id().to_string();
            let series_id  = if series_id.is_empty() { None } else { Some(series_id) };
            let title      = g.get_detail_title().to_string();
            let resume_pos = g.get_detail_resume_secs();
            video_rd.lock().unwrap().from_detail = true;
            let s = state_rd.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            config.start_position_secs = if resume_pos > 0.0 { Some(resume_pos as f64) } else { None };
            drop(s);
            let play_url = client.direct_play_url(&id);
            info!("resume_detail: {} from {:?}s", id, config.start_position_secs);
            start_playback(play_url, id, &item_type, title, config, client,
                           series_id, None, &video_rd, &ww, &rt_handle);
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(&window).on_close_detail(move || {
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_detail_scroll(0.0);
                g.set_show_detail(false);
                g.set_detail_id("".into());
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
            let mut s = state_ss.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let series_id  = s.series_open_id.clone();
            let Some(season_id) = s.series_season_ids.get(idx).cloned() else { return };

            // Cache hit — we're on the UI thread (Slint callback), set directly.
            if let Some(cached) = s.series_episode_cache.get(&season_id).cloned() {
                s.series_episode_items = cached.clone();
                drop(s);
                if let Some(w) = ww_ss.upgrade() {
                    if AppState::get(&w).get_series_id().as_str() == series_id {
                        let cards: Vec<CardItem> = cached.iter().map(ep_to_card).collect();
                        let g = AppState::get(&w);
                        g.set_series_episode_cards(ModelRc::new(VecModel::from(cards)));
                        g.set_series_focused_ep(0);
                        g.set_series_loading(false);
                    }
                }
                spawn_episode_thumb_loading(client, cached, series_id, ww_ss.clone(), rth_ss.clone());
                return;
            }

            // Not cached — increment generation counter and fetch from network.
            s.series_season_generation += 1;
            let gen = s.series_season_generation;
            drop(s);

            if let Some(w) = ww_ss.upgrade() {
                let g = AppState::get(&w);
                g.set_series_loading(true);
                g.set_series_episode_cards(ModelRc::new(VecModel::<CardItem>::default()));
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
                {
                    let mut s = state_ss2.lock().unwrap();
                    if s.series_season_generation != gen { return; }
                    s.series_episode_items = eps.clone();
                    s.series_episode_cache.insert(season_id.clone(), eps.clone());
                }
                // Pass Vec<MediaItem> (Send) and build Vec<CardItem> (!Send) inside the closure.
                let eps_send = eps.clone();
                let sid3 = sid2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww_ss2.upgrade() else { return };
                    if AppState::get(&w).get_series_id().as_str() != sid3 { return; }
                    let cards: Vec<CardItem> = eps_send.iter().map(ep_to_card).collect();
                    AppState::get(&w).set_series_episode_cards(ModelRc::new(VecModel::from(cards)));
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
            let series_id = ep_item.as_ref().and_then(|i| i.series_id.clone())
                .or_else(|| Some(s.series_open_id.clone()).filter(|sid| !sid.is_empty()));
            drop(s);
            // Set restore flags synchronously on the UI thread so reset_playback_ui always
            // finds them set, regardless of async timing. Also set vs.from_series so
            // start_playback knows NOT to clear playback_from_series for this play.
            if let Some(w) = ww_pe.upgrade() {
                let g = AppState::get(&w);
                let was_season = g.get_show_season();
                g.set_show_series(false);
                g.set_show_season(false);
                g.set_playback_from_series(true);
                g.set_playback_from_season(was_season);
                video_pe.lock().unwrap().from_series = true;
            }
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
                    start_playback(play_url, id, "Episode", title, config, client,
                                   series_id, None, &video_pe2, &ww_pe2, &rth_pe2);
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
                let g = AppState::get(&w);
                // User explicitly closed the series screen. If the player is minimized
                // and was waiting to restore here on stop, cancel that restore so stop
                // lands on the library/dashboard instead.
                if g.get_has_background_player() {
                    g.set_playback_from_series(false);
                    g.set_playback_from_season(false);
                }
                g.set_show_season(false);
                g.set_season_id("".into());
                g.set_show_series(false);
                g.set_series_id("".into());
            }
            let mut s = state_cs.lock().unwrap();
            s.series_open_id.clear();
            s.series_season_ids.clear();
            s.series_episode_items.clear();
        });
    }

    // ── season detail ─────────────────────────────────────────────────────────
    {
        let state_osd = Arc::clone(&state);
        let ww_osd    = window.as_weak();
        let rth_osd   = rt.handle().clone();
        AppState::get(&window).on_open_season_detail(move |season_id, series_id| {
            season::open_season_screen(season_id.to_string(), series_id.to_string(), state_osd.clone(), ww_osd.clone(), rth_osd.clone());
        });
    }
    {
        let ww_csd = window.as_weak();
        AppState::get(&window).on_close_season_detail(move || {
            if let Some(w) = ww_csd.upgrade() {
                let g = AppState::get(&w);
                // Closing season detail returns to series screen — clear only the
                // season restore flag; series screen will still show (or restore on stop).
                if g.get_has_background_player() {
                    g.set_playback_from_season(false);
                }
                g.set_show_season(false);
                g.set_season_id("".into());
                g.set_season_cast_focused(-1);
            }
        });
    }

    // ── person screen ─────────────────────────────────────────────────────────
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_open_person(move |id, name| {
            let s = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            person::open_person_screen(
                id.to_string(), name.to_string(), client, ww2.clone(), rt2.clone(),
            );
        });
    }
    {
        let ww2 = window.as_weak();
        AppState::get(&window).on_close_person(move || {
            if let Some(w) = ww2.upgrade() {
                AppState::get(&w).set_show_person(false);
            }
        });
    }

    // ── season fav / played toggles ───────────────────────────────────────────
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_toggle_season_fav(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id      = AppState::get(&w).get_season_id().to_string();
            let cur_fav = AppState::get(&w).get_season_is_favorite();
            let s  = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww3    = ww2.clone();
            let state3 = Arc::clone(&state2);
            rt2.spawn(async move {
                let result = if cur_fav { client.unset_favorite(&id).await }
                             else       { client.set_favorite(&id).await };
                if let Err(e) = result { warn!("toggle-season-fav: {e}"); return; }
                let new_fav = !cur_fav;
                state3.lock().unwrap().update_item_user_state(&id, None, Some(new_fav));
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww3.upgrade() {
                        if AppState::get(&w).get_season_id().as_str() == id {
                            AppState::get(&w).set_season_is_favorite(new_fav);
                        }
                    }
                });
            });
        });
    }
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_toggle_season_played(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id       = AppState::get(&w).get_season_id().to_string();
            let cur_play = AppState::get(&w).get_season_has_played();
            // Capture the parent series_id so the series Next Up row can be refreshed.
            let sid      = AppState::get(&w).get_series_id().to_string();
            let s  = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww3    = ww2.clone();
            let state3 = Arc::clone(&state2);
            let rt3    = rt2.clone();
            rt2.spawn(async move {
                let result = if cur_play { client.mark_unplayed(&id).await }
                             else        { client.mark_played(&id).await };
                if let Err(e) = result { warn!("toggle-season-played: {e}"); return; }
                let new_play = !cur_play;
                state3.lock().unwrap().update_item_user_state(&id, Some(new_play), None);
                let client2 = Arc::clone(&client);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww3.upgrade() {
                        if AppState::get(&w).get_season_id().as_str() == id {
                            AppState::get(&w).set_season_has_played(new_play);
                        }
                        if !sid.is_empty() {
                            crate::series::refresh_series_next_up(sid, client2, ww3.clone(), rt3);
                        }
                    }
                });
            });
        });
    }

    // ── Up Next banner: cancel (Skip button) ─────────────────────────────────
    {
        let video_ca = Arc::clone(&video);
        let ww_ca    = window.as_weak();
        AppState::get(&window).on_cancel_auto_advance(move || {
            video_ca.lock().unwrap().next_ep_pending = None;
            if let Some(w) = ww_ca.upgrade() {
                AppState::get(&w).set_show_next_ep_banner(false);
            }
        });
    }

    // ── Up Next banner: play now (Play Now button) ────────────────────────────
    {
        let state_pn = Arc::clone(&state);
        let video_pn = Arc::clone(&video);
        let ww_pn    = window.as_weak();
        let rt_pn    = rt.handle().clone();
        AppState::get(&window).on_play_next_ep(move || {
            let next = video_pn.lock().unwrap().next_ep_pending.take();
            let Some(next) = next else { return; };
            let config = state_pn.lock().unwrap().player_config();
            let cli    = state_pn.lock().unwrap().client.as_ref().map(Arc::clone);
            let Some(cli) = cli else { return; };
            let url        = cli.direct_play_url(&next.id);
            let title      = next.display_name();
            let ep_id      = next.id.clone();
            let series_id  = next.series_id.clone();
            if let Some(w) = ww_pn.upgrade() {
                AppState::get(&w).set_show_next_ep_banner(false);
            }
            start_playback(url, ep_id, "Episode", title, config, cli,
                           series_id, None, &video_pn, &ww_pn, &rt_pn);
        });
    }

    // ── player controls ───────────────────────────────────────────────────────
    controls::wire_controls(&window, Arc::clone(&video), Arc::clone(&controls_show), Arc::clone(&seek_suppress), rt.handle().clone());

    // ── context menu + queue ──────────────────────────────────────────────────
    context_menu::wire_context_menu(&window, Arc::clone(&state), Arc::clone(&video), rt.handle().clone());
    context_menu::wire_queue_callbacks(&window, Arc::clone(&state), Arc::clone(&video), rt.handle().clone());
    context_menu::wire_playlist_picker(&window, Arc::clone(&state), rt.handle().clone());

    // ── queue prev / next / shuffle / repeat ──────────────────────────────────
    {
        let video_qp = Arc::clone(&video);
        let state_qp = Arc::clone(&state);
        let ww_qp    = window.as_weak();
        let rt_qp    = rt.handle().clone();
        AppState::get(&window).on_queue_prev_track(move || {
            let (item, should_seek_start) = {
                let mut vs = video_qp.lock().unwrap();
                let pos = vs.player.as_ref().map(|p| p.get_position()).unwrap_or(0.0);
                let qi = crate::playback::playlist_prev(&mut vs);
                // None means either seek-to-0 (pos >= 2s) or already at start
                (qi, pos >= 2.0)
            };
            match item {
                Some(qi) => {
                    let s = state_qp.lock().unwrap();
                    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
                    let mut config = s.player_config();
                    config.start_position_secs = None;
                    drop(s);
                    let url = client.direct_play_url(&qi.id);
                    let am  = qi.audio_meta.clone();
                    start_playback(url, qi.id.clone(), &qi.item_type, qi.title.clone(),
                                   config, client, qi.series_id.clone(), am,
                                   &video_qp, &ww_qp, &rt_qp);
                }
                None if should_seek_start => {
                    // pos >= 2s and no prev: restart current track from 0
                    video_qp.lock().unwrap().player.as_ref()
                        .map(|p| p.seek_to(0.0));
                }
                None => {} // already at start, nothing to do
            }
        });
    }
    {
        let video_qn = Arc::clone(&video);
        let state_qn = Arc::clone(&state);
        let ww_qn    = window.as_weak();
        let rt_qn    = rt.handle().clone();
        AppState::get(&window).on_queue_next_track(move || {
            let item = {
                let mut vs = video_qn.lock().unwrap();
                crate::playback::playlist_next(&mut vs)
            };
            if let Some(qi) = item {
                let s = state_qn.lock().unwrap();
                let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
                let mut config = s.player_config();
                config.start_position_secs = None;
                drop(s);
                let url = client.direct_play_url(&qi.id);
                let am  = qi.audio_meta.clone();
                start_playback(url, qi.id.clone(), &qi.item_type, qi.title.clone(),
                               config, client, qi.series_id.clone(), am,
                               &video_qn, &ww_qn, &rt_qn);
            }
        });
    }
    {
        let video_ts = Arc::clone(&video);
        let ww_ts    = window.as_weak();
        AppState::get(&window).on_toggle_shuffle(move || {
            let shuffled = {
                let mut vs = video_ts.lock().unwrap();
                crate::playback::toggle_shuffle(&mut vs);
                vs.shuffle
            };
            if let Some(w) = ww_ts.upgrade() {
                let g = AppState::get(&w);
                g.set_queue_shuffle(shuffled);
                push_queue_display(&video_ts.lock().unwrap(), &g);
            }
        });
    }
    {
        let video_cr = Arc::clone(&video);
        let ww_cr    = window.as_weak();
        AppState::get(&window).on_cycle_repeat(move || {
            use crate::playback::RepeatMode;
            let next_mode = {
                let mut vs = video_cr.lock().unwrap();
                crate::playback::invalidate_preload(&mut vs);
                vs.repeat_mode = match vs.repeat_mode {
                    RepeatMode::Off => RepeatMode::All,
                    RepeatMode::All => RepeatMode::One,
                    RepeatMode::One => RepeatMode::Off,
                };
                vs.repeat_mode as i32
            };
            if let Some(w) = ww_cr.upgrade() {
                AppState::get(&w).set_queue_repeat_mode(next_mode);
            }
        });
    }

    // ── queue panel: refresh / jump / remove / clear ──────────────────────────
    {
        let video_rq = Arc::clone(&video);
        let state_rq = Arc::clone(&state);
        let ww_rq    = window.as_weak();
        let rt_rq    = rt.handle().clone();
        AppState::get(&window).on_refresh_queue_display(move || {
            let Some(w) = ww_rq.upgrade() else { return };
            push_queue_display(&video_rq.lock().unwrap(), &AppState::get(&w));
            // Spawn poster loading for the freshly-built model
            let client = state_rq.lock().unwrap().client.as_ref().map(Arc::clone);
            if let Some(cli) = client {
                spawn_queue_poster_loading(cli, ww_rq.clone(), rt_rq.clone());
            }
        });
    }
    {
        let video_qj = Arc::clone(&video);
        let state_qj = Arc::clone(&state);
        let ww_qj    = window.as_weak();
        let rt_qj    = rt.handle().clone();
        AppState::get(&window).on_queue_jump(move |idx| {
            // idx is QueueEntry.index: the UNDERLYING position — 0..playlist.len()
            // are playlist tracks, after that context-menu queue items (CR10-6);
            // -1 is the synthetic now-playing row (already playing — nothing to do).
            if idx < 0 { return; }
            let item = {
                let mut vs = video_qj.lock().unwrap();
                let idx = idx as usize;
                if idx < vs.playlist.len() {
                    vs.playlist_index = idx;
                    vs.playlist[idx].clone()
                } else {
                    let qidx = idx - vs.playlist.len();
                    if qidx >= vs.queue.len() { return }
                    vs.queue.remove(qidx)
                }
            };
            let s = state_qj.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            config.start_position_secs = None;
            drop(s);
            let url = client.direct_play_url(&item.id);
            let am  = item.audio_meta.clone();
            if let Some(w) = ww_qj.upgrade() { AppState::get(&w).set_show_queue_panel(false); }
            start_playback(url, item.id.clone(), &item.item_type, item.title.clone(),
                           config, client, item.series_id.clone(), am,
                           &video_qj, &ww_qj, &rt_qj);
        });
    }
    {
        let video_qr = Arc::clone(&video);
        let ww_qr    = window.as_weak();
        AppState::get(&window).on_queue_remove(move |idx| {
            if idx < 0 { return; } // synthetic now-playing row
            let Some(w) = ww_qr.upgrade() else { return };
            let g = AppState::get(&w);
            {
                let mut vs = video_qr.lock().unwrap();
                let idx = idx as usize;
                // The currently-playing row can't be removed — the track keeps
                // playing regardless, and removing it shifted the is-current
                // highlight onto the wrong row (CR10-17).
                if !vs.playlist.is_empty() && idx == vs.playlist_index { return; }
                if idx < vs.playlist.len() {
                    vs.playlist.remove(idx);
                    // Keep playlist_index valid after removal
                    if vs.playlist_index > idx && vs.playlist_index > 0 {
                        vs.playlist_index -= 1;
                    } else if vs.playlist_index >= vs.playlist.len() && !vs.playlist.is_empty() {
                        vs.playlist_index = vs.playlist.len() - 1;
                    }
                    // Rebuild shuffle_order from scratch (indices shifted)
                    crate::playback::rebuild_shuffle_order(&mut vs);
                } else {
                    // Context-menu queue row (CR10-6)
                    let qidx = idx - vs.playlist.len();
                    if qidx >= vs.queue.len() { return; }
                    vs.queue.remove(qidx);
                }
                crate::playback::invalidate_preload(&mut vs);
                push_queue_display(&vs, &g);
            }
            // Snap cursor if it's past the new end
            let len = g.get_queue_items().row_count() as i32;
            let c   = g.get_queue_panel_cursor();
            if c >= len && len > 0 { g.set_queue_panel_cursor(len - 1); }
            if len == 0 { g.set_show_queue_panel(false); }
        });
    }
    {
        let video_qc = Arc::clone(&video);
        let ww_qc    = window.as_weak();
        AppState::get(&window).on_queue_clear(move || {
            let Some(w) = ww_qc.upgrade() else { return };
            let g = AppState::get(&w);
            {
                let mut vs = video_qc.lock().unwrap();
                vs.playlist.clear();
                vs.playlist_index = 0;
                vs.queue.clear();
                vs.shuffle_order.clear();
                crate::playback::invalidate_preload(&mut vs);
                push_queue_display(&vs, &g); // also zeroes queue-count (CR10-6)
            }
            g.set_show_queue_panel(false);
        });
    }

    // ── lyrics toggle ─────────────────────────────────────────────────────────
    {
        let ww_lyr = window.as_weak();
        AppState::get(&window).on_toggle_lyrics(move || {
            let Some(w) = ww_lyr.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_lyrics_available() {
                g.set_show_lyrics(!g.get_show_lyrics());
            }
        });
    }

    // ── detail page: toggle-fav / toggle-played ───────────────────────────────
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_toggle_detail_fav(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id      = AppState::get(&w).get_detail_id().to_string();
            let cur_fav = AppState::get(&w).get_detail_is_favorite();
            let s  = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww3    = ww2.clone();
            let state3 = Arc::clone(&state2);
            rt2.spawn(async move {
                let result = if cur_fav { client.unset_favorite(&id).await }
                             else       { client.set_favorite(&id).await };
                if let Err(e) = result { warn!("toggle-detail-fav: {e}"); return; }
                let new_fav = !cur_fav;
                state3.lock().unwrap().update_item_user_state(&id, None, Some(new_fav));
                let ww4 = ww3.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww4.upgrade() {
                        if AppState::get(&w).get_detail_id().as_str() == id {
                            AppState::get(&w).set_detail_is_favorite(new_fav);
                        }
                        context_menu::update_card_in_all_models(&w, &id, None, Some(new_fav));
                    }
                });
                let rt3 = tokio::runtime::Handle::current();
                crate::home::refresh_favorites(client, ww3, rt3);
            });
        });
    }
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_toggle_detail_played(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id       = AppState::get(&w).get_detail_id().to_string();
            let cur_play = AppState::get(&w).get_detail_has_played();
            // Capture series_id now (episode detail only); empty for movies.
            let sid      = AppState::get(&w).get_detail_series_id().to_string();
            let s  = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww3    = ww2.clone();
            let state3 = Arc::clone(&state2);
            let rt3    = rt2.clone();
            rt2.spawn(async move {
                let result = if cur_play { client.mark_unplayed(&id).await }
                             else        { client.mark_played(&id).await };
                if let Err(e) = result { warn!("toggle-detail-played: {e}"); return; }
                let new_play = !cur_play;
                state3.lock().unwrap().update_item_user_state(&id, Some(new_play), None);
                let client2 = Arc::clone(&client);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww3.upgrade() {
                        if AppState::get(&w).get_detail_id().as_str() == id {
                            AppState::get(&w).set_detail_has_played(new_play);
                        }
                        context_menu::update_card_in_all_models(&w, &id, Some(new_play), None);
                        if new_play { context_menu::remove_from_dynamic_rows(&w, &id); }
                        if !sid.is_empty() {
                            crate::series::refresh_series_next_up(sid.clone(), client2, ww3.clone(), rt3);
                            let delta = if new_play { -1 } else { 1 };
                            context_menu::update_series_unplayed_count(&w, &sid, delta);
                        }
                    }
                });
            });
        });
    }
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_toggle_series_played(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id       = AppState::get(&w).get_series_id().to_string();
            let cur_play = AppState::get(&w).get_series_has_played();
            let s  = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww3    = ww2.clone();
            let state3 = Arc::clone(&state2);
            let rt3    = rt2.clone();
            rt2.spawn(async move {
                let result = if cur_play { client.mark_unplayed(&id).await }
                             else        { client.mark_played(&id).await };
                if let Err(e) = result { warn!("toggle-series-played: {e}"); return; }
                let new_play = !cur_play;
                state3.lock().unwrap().update_item_user_state(&id, Some(new_play), None);
                let client2 = Arc::clone(&client);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww3.upgrade() {
                        if AppState::get(&w).get_series_id().as_str() == id {
                            AppState::get(&w).set_series_has_played(new_play);
                        }
                        context_menu::update_card_in_all_models(&w, &id, Some(new_play), None);
                        if new_play { context_menu::remove_from_dynamic_rows(&w, &id); }
                        // Refresh the series Next Up row (mark-played → clears it;
                        // mark-unplayed → re-fetches first unwatched episode).
                        crate::series::refresh_series_next_up(id.clone(), client2, ww3.clone(), rt3);
                    }
                });
            });
        });
    }
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_toggle_series_fav(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id      = AppState::get(&w).get_series_id().to_string();
            let cur_fav = AppState::get(&w).get_series_is_favorite();
            let s  = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww3    = ww2.clone();
            let state3 = Arc::clone(&state2);
            rt2.spawn(async move {
                let result = if cur_fav { client.unset_favorite(&id).await }
                             else       { client.set_favorite(&id).await };
                if let Err(e) = result { warn!("toggle-series-fav: {e}"); return; }
                let new_fav = !cur_fav;
                state3.lock().unwrap().update_item_user_state(&id, None, Some(new_fav));
                let ww4 = ww3.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww4.upgrade() {
                        if AppState::get(&w).get_series_id().as_str() == id {
                            AppState::get(&w).set_series_is_favorite(new_fav);
                        }
                        context_menu::update_card_in_all_models(&w, &id, None, Some(new_fav));
                    }
                });
                let rt3 = tokio::runtime::Handle::current();
                crate::home::refresh_favorites(client, ww3, rt3);
            });
        });
    }
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_toggle_collection_fav(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id      = AppState::get(&w).get_collection_id().to_string();
            let cur_fav = AppState::get(&w).get_collection_is_favorite();
            let s  = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww3    = ww2.clone();
            let state3 = Arc::clone(&state2);
            rt2.spawn(async move {
                let result = if cur_fav { client.unset_favorite(&id).await }
                             else       { client.set_favorite(&id).await };
                if let Err(e) = result {
                    warn!("toggle-collection-fav: {e}");
                    crate::show_toast(ww3.clone(), format!("Favourite error: {e}"));
                    return;
                }
                let new_fav = !cur_fav;
                state3.lock().unwrap().update_item_user_state(&id, None, Some(new_fav));
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww3.upgrade() {
                        if AppState::get(&w).get_collection_id().as_str() == id {
                            AppState::get(&w).set_collection_is_favorite(new_fav);
                        }
                        context_menu::update_card_in_all_models(&w, &id, None, Some(new_fav));
                    }
                });
            });
        });
    }
    {
        let state2 = Arc::clone(&state);
        let ww2    = window.as_weak();
        let rt2    = rt.handle().clone();
        AppState::get(&window).on_toggle_collection_played(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id       = AppState::get(&w).get_collection_id().to_string();
            let cur_play = AppState::get(&w).get_collection_has_played();
            let s  = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let ww3    = ww2.clone();
            let state3 = Arc::clone(&state2);
            rt2.spawn(async move {
                let result = if cur_play { client.mark_unplayed(&id).await }
                             else        { client.mark_played(&id).await };
                if let Err(e) = result { warn!("toggle-collection-played: {e}"); return; }
                let new_play = !cur_play;
                state3.lock().unwrap().update_item_user_state(&id, Some(new_play), None);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww3.upgrade() {
                        if AppState::get(&w).get_collection_id().as_str() == id {
                            AppState::get(&w).set_collection_has_played(new_play);
                            // Bulk-update all child cards — marking a BoxSet played/unplayed
                            // implies the same state for every item in the grid.
                            let model = AppState::get(&w).get_collection_items();
                            for i in 0..model.row_count() {
                                if let Some(mut c) = model.row_data(i) {
                                    c.has_played = new_play;
                                    model.set_row_data(i, c);
                                }
                            }
                        }
                        context_menu::update_card_in_all_models(&w, &id, Some(new_play), None);
                    }
                });
            });
        });
    }

    // ── audio device list: fetch once at startup ─────────────────────────────
    {
        let state_ad  = Arc::clone(&state);
        let ww_ad     = window.as_weak();
        let (cfg_device, cfg_pt_device) = {
            let s = state.lock().unwrap();
            (s.config.audio_device.clone(), s.config.audio_device_passthrough.clone())
        };
        rt.spawn(async move {
            let devices = tokio::task::spawn_blocking(fetch_audio_devices).await.unwrap_or_default();
            state_ad.lock().unwrap().audio_devices = devices.clone();
            let display: Vec<slint::SharedString> = devices.iter()
                .map(|(_, d)| slint::SharedString::from(d.as_str()))
                .collect();
            let desc = devices.iter()
                .find(|(n, _)| n.as_str() == cfg_device.as_str())
                .map(|(_, d)| d.as_str())
                .unwrap_or(if cfg_device.is_empty() { "" } else { cfg_device.as_str() })
                .to_string();
            let pt_desc = devices.iter()
                .find(|(n, _)| n.as_str() == cfg_pt_device.as_str())
                .map(|(_, d)| d.as_str())
                .unwrap_or(if cfg_pt_device.is_empty() { "" } else { cfg_pt_device.as_str() })
                .to_string();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww_ad.upgrade() {
                    let g = AppState::get(&w);
                    g.set_settings_audio_device_display(
                        slint::ModelRc::new(slint::VecModel::from(display)),
                    );
                    if !desc.is_empty() {
                        g.set_settings_audio_device_desc(slint::SharedString::from(desc.as_str()));
                    }
                    if !pt_desc.is_empty() {
                        g.set_settings_passthrough_device_desc(slint::SharedString::from(pt_desc.as_str()));
                    }
                }
            });
        });
    }

    // ── audio device selected callback ────────────────────────────────────────
    {
        let state_ad = Arc::clone(&state);
        let ww_ad    = window.as_weak();
        AppState::get(&window).on_audio_device_selected(move |desc| {
            let name = {
                let s = state_ad.lock().unwrap();
                s.audio_devices.iter()
                    .find(|(_, d)| d.as_str() == desc.as_str())
                    .map(|(n, _)| n.clone())
                    .unwrap_or_else(|| "auto".to_string())
            };
            if let Some(w) = ww_ad.upgrade() {
                let g = AppState::get(&w);
                g.set_settings_audio_device(slint::SharedString::from(name.as_str()));
                let pt = g.get_settings_passthrough_device().to_string();
                let effective = if pt.is_empty() { name.as_str() } else { pt.as_str() };
                g.set_settings_device_is_pipewire(pipewire_fix::is_pipewire_device(effective));
                g.set_settings_audio_device_desc(desc);
                g.invoke_settings_changed();
            }
        });
    }

    // ── passthrough device selected callback ─────────────────────────────────
    {
        let state_pd = Arc::clone(&state);
        let ww_pd    = window.as_weak();
        AppState::get(&window).on_passthrough_device_selected(move |desc| {
            let name = {
                let s = state_pd.lock().unwrap();
                s.audio_devices.iter()
                    .find(|(_, d)| d.as_str() == desc.as_str())
                    .map(|(n, _)| n.clone())
                    .unwrap_or_else(|| "auto".to_string())
            };
            if let Some(w) = ww_pd.upgrade() {
                let g = AppState::get(&w);
                // "auto" means "same as audio output" here — store empty so
                // start_playback falls back to the normal device.
                let (store, show_desc) = if name == "auto" {
                    (String::new(), slint::SharedString::default())
                } else {
                    (name.clone(), desc)
                };
                g.set_settings_passthrough_device(slint::SharedString::from(store.as_str()));
                g.set_settings_passthrough_device_desc(show_desc);
                let effective = if store.is_empty() {
                    g.get_settings_audio_device().to_string()
                } else {
                    store
                };
                g.set_settings_device_is_pipewire(pipewire_fix::is_pipewire_device(&effective));
                g.invoke_settings_changed();
            }
        });
    }

    // ── settings changed ──────────────────────────────────────────────────────
    {
        let state      = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle  = rt.handle().clone();
        AppState::get(&window).on_settings_changed(move || {
            let Some(w) = window_weak.upgrade() else { return; };
            let mut s = state.lock().unwrap();
            read_settings_from_window(&w, &mut s);
            let launch_fs = s.config.launch_fullscreen;
            let irq_enable = s.config.audio_spdif
                && s.config.alsa_irq_scheduling
                && pipewire_fix::is_pipewire_device(
                    if s.config.audio_device_passthrough.is_empty() { &s.config.audio_device }
                    else { &s.config.audio_device_passthrough });
            save_config(&s.config);
            drop(s);
            w.window().set_fullscreen(launch_fs);
            rt_handle.spawn_blocking(move || pipewire_fix::apply_alsa_irq_scheduling(irq_enable));
            info!("settings saved");
        });
    }

    // ── keyboard dropdown: mouse pick on overlay ─────────────────────────────
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_dropdown_pick(move || {
            let Some(w) = window_weak.upgrade() else { return; };
            let g = AppState::get(&w);
            let ss     = g.get_settings_section();
            let sf     = g.get_settings_focused();
            let cursor = g.get_settings_dropdown_cursor();
            crate::settings::apply_dropdown_selection(ss, sf, cursor, &g);
            g.set_settings_dropdown_open(false);
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
        let state       = Arc::clone(&state);
        let video_so    = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rth_so      = rt.handle().clone();
        AppState::get(&window).on_sign_out(move || {
            // The queue belongs to the session: clear it BEFORE the stop so
            // do_stop_playback's push_queue_display publishes the empty state.
            {
                let mut vs = video_so.lock().unwrap();
                vs.playlist.clear();
                vs.playlist_index = 0;
                vs.queue.clear();
                vs.shuffle_order.clear();
            }
            // Stop any active playback before clearing state.
            do_stop_playback(&video_so, &window_weak, &rth_so);

            let mut s = state.lock().unwrap();
            // Clear only the session — deleting config.json wholesale (pre-CR10-12)
            // also wiped device_id, so the next login generated a fresh DeviceId and
            // Jellyfin invalidated the other machine's token (the exact scenario the
            // per-install DeviceId exists to prevent). Settings survive sign-out too.
            s.config.server_url.clear();
            s.config.user_id.clear();
            s.config.token.clear();
            save_config(&s.config);
            if let Some(abort) = s.ws_abort.take() { abort.abort(); }
            s.client = None;
            s.all_movies.clear();
            s.all_series.clear();
            s.all_collections.clear();
            s.all_artists.clear();
            s.all_albums.clear();
            s.all_playlists.clear();
            s.filtered_items.clear();
            s.series_open_id.clear();
            s.series_season_ids.clear();
            s.series_episode_items.clear();
            s.series_episode_cache.clear();
            s.movie_collections.clear();
            s.movies_fetched = false;
            s.collections_fetched = false;
            s.artists_fetched = false;
            s.albums_fetched  = false;
            s.playlists_fetched = false;
            s.last_nw_mov_refresh = None;
            s.last_nw_tv_refresh  = None;
            drop(s);
            if let Some(w) = window_weak.upgrade() {
                let g = AppState::get(&w);
                g.set_show_login(true);
                g.set_show_connecting(false);
                g.set_show_offline(false);
                g.set_active_nav(0);
                g.set_show_browse(false);
                g.set_show_library(false);
                g.set_show_detail(false);
                g.set_show_series(false);
                g.set_show_season(false);
                g.set_show_person(false);
                g.set_show_collection(false);
                g.set_show_album(false);
                g.set_show_artist(false);
                g.set_show_context_menu(false);
                g.set_show_now_playing(false);
                g.set_all_collections(items_to_model(&[]));
                g.set_all_artists(items_to_model(&[]));
                g.set_all_albums(items_to_model(&[]));
                g.set_all_playlists(items_to_model(&[]));
                g.set_library_music_view(0);
                g.set_recently_added_collections(items_to_model(&[]));
                g.set_unwatched_collections(items_to_model(&[]));
                g.set_recently_added_albums(items_to_model(&[]));
                g.set_recently_played_albums(items_to_model(&[]));
                g.set_favorite_movies(items_to_model(&[]));
                g.set_favorite_series(items_to_model(&[]));
                g.set_favorite_albums(items_to_model(&[]));
                g.set_music_playlists(items_to_model(&[]));
                g.set_show_next_ep_banner(false);
                g.set_has_background_player(false);
                {
                    let mut vs = video_so.lock().unwrap();
                    vs.playlist.clear();
                    vs.playlist_index = 0;
                    vs.queue.clear();
                    vs.shuffle = false;
                    vs.shuffle_order.clear();
                    vs.repeat_mode = crate::playback::RepeatMode::Off;
                }
                push_queue_display(&video_so.lock().unwrap(), &g);
                g.set_queue_shuffle(false);
                g.set_queue_repeat_mode(0);
                g.set_show_queue_panel(false);
                g.set_float_card_focused(-1);
                g.set_server_url(ss(""));
                g.set_server_name(ss(""));
                g.set_server_version(ss(""));
                g.set_settings_section(-1);
                g.set_settings_focused(-1);
                g.set_keybinding_focused(-1);
            }
        });
    }

    AppState::get(&window).on_quit(|| { slint::quit_event_loop().ok(); });

    // ── keyboard dispatch ────────────────────────────────────────────────────
    {
        let state2  = Arc::clone(&state);
        let video2k = Arc::clone(&video);
        let ww      = window.as_weak();
        let rt2     = rt.handle().clone();
        AppState::get(&window).on_handle_key(move |key, shift, ctrl, repeat| {
            let Some(w) = ww.upgrade() else { return false; };
            // Any key resets the Now Playing idle-auto-open countdown.
            video2k.lock().unwrap().music_idle_ticks = 0;
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

    // Re-grab keyboard focus after any mouse interaction steals it (e.g. ComboBox, CheckBox)
    {
        let ww = window.as_weak();
        AppState::get(&window).on_refocus(move || {
            if let Some(w) = ww.upgrade() { w.invoke_grab_keyboard_focus(); }
        });
    }

    window.invoke_grab_keyboard_focus();
    window.run()?;
    // Send stop report and release screensaver inhibitor if a video was playing when the user quit.
    quit_cleanup(&video, &rt);
    Ok(())
}
