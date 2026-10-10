// ── fjord-app · ws.rs ─────────────────────────────────────────────────────────
//   start_websocket  spawn the reconnect loop; returns the AbortHandle for sign-out cleanup
//   ws_loop          reconnect loop with exponential backoff (1 s → 60 s); owns pending_upsert_ids
//                    (LibraryChanged added/updated ids + UserDataChanged favorite/resume
//                    candidates); the URL carries api_key — logged only via redact_api_key
//   row_has_id       found-by-id check on a CardItem model (transition gate)
//   sync_open_episodes  an added/updated episode of the series+season on screen: upsert + re-sort
//                    series_episode_items, rebuild series-episode-cards, re-anchor focus by id
//   upsert_library_bucket  upsert a delta batch into one all_X list + library-display in place
//                    (focus re-anchored) when that grid is open
//   maybe_spawn_delta_refresh  debounced (5 s), one at a time, session-guarded: fetch_home_data
//                    (every ranked home row) + get_items_by_ids(pending_upsert_ids) bucketed into
//                    the six flat library lists and Episode (missing parent series fetched for
//                    their unplayed counts; feeds sync_open_episodes + series_episode_cache);
//                    BoxSets reconcile movie_collections; a detailed batch refreshes
//                    item_detail_cache and invalidates relationship-cache keys; removes watched,
//                    watchlisted series from the Seerr watchlist once they stop Continuing
//   run_session      messages until the connection drops; client KeepAlive every 30 s (server
//                    acks ignored). LibraryChanged: clear *_fetched flags, purge removed ids from
//                    state/models/poster cache/screen caches, queue added/updated ids → delta
//                    refresh. UserDataChanged: patch played/favorite in place; immediate removal
//                    (played → every dynamic row; position reset → Continue Watching only;
//                    unfavorited → Favorites); invalidate item_detail_cache; a new
//                    favorite/resume, or a watched movie (collections), wakes the delta refresh;
//                    a watched watchlisted item leaves the Seerr watchlist — except a Continuing
//                    series (see maybe_spawn_delta_refresh). KeepAlive.
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fjord_api::JellyfinClient;
use fjord_api::models::MediaItem;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use slint::{Global, Model, ModelRc};

use crate::CardItem;
use crate::MainWindow;
use crate::config::{FjordState, upsert_media_item};
use crate::context_menu::{reanchor_focus, update_card_in_all_models, upsert_cards_in_model};
use crate::home::{
    fetch_home_data, home_data_sections, push_home_data_preserving_posters, save_albums_cache,
    save_artists_cache, save_collections_cache, save_home_cache, save_movies_cache,
    save_playlists_cache, save_series_cache,
};
use crate::poster::{fetch_posters_for_delta, spawn_poster_loading};

// ── wire types ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct WsMsg {
    #[serde(rename = "MessageType")]
    message_type: String,
    #[serde(rename = "Data", default)]
    data: serde_json::Value,
}

#[derive(Deserialize, Default)]
struct LibraryChangedPayload {
    #[serde(rename = "ItemsAdded", default)]
    items_added: Vec<String>,
    #[serde(rename = "ItemsUpdated", default)]
    items_updated: Vec<String>,
    #[serde(rename = "ItemsRemoved", default)]
    items_removed: Vec<String>,
}

#[derive(Deserialize)]
struct UserDataChangedPayload {
    #[serde(rename = "UserDataList", default)]
    user_data_list: Vec<WsUserItem>,
}

#[derive(Deserialize)]
struct WsUserItem {
    #[serde(rename = "ItemId")]
    item_id: String,
    #[serde(rename = "Played", default)]
    played: bool,
    #[serde(rename = "IsFavorite", default)]
    is_favorite: bool,
    #[serde(rename = "PlaybackPositionTicks", default)]
    playback_position_ticks: i64,
}

// ── public API ────────────────────────────────────────────────────────────────

/// Spawn the WebSocket reconnect loop. Returns an AbortHandle — call
/// `abort()` on sign-out to stop it cleanly.
pub(crate) fn start_websocket(
    client: Arc<JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) -> tokio::task::AbortHandle {
    rt.spawn(ws_loop(client, state, ww, rt.clone()))
        .abort_handle()
}

// ── reconnect loop ────────────────────────────────────────────────────────────

async fn ws_loop(
    client: Arc<JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let url = client.ws_url();
    // One AtomicBool shared across reconnects so a debounced refresh spawned
    // before a disconnect doesn't leave `pending` stuck at true.
    let refresh_pending = Arc::new(AtomicBool::new(false));
    // LibraryChanged ItemsAdded/ItemsUpdated ids awaiting the same debounced
    // get_items_by_ids fetch (treated identically — upsert is replace-or-append
    // either way). Recently Added row freshness doesn't need its own tracking
    // here: fetch_home_data below already re-fetches those rows from the server
    // on every debounce cycle, sorted correctly, so a separate insert-by-
    // date_created path would just be immediately overwritten by push_home_data.
    let pending_upsert_ids: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
    let mut backoff = Duration::from_secs(1);

    loop {
        // The URL carries the Jellyfin token (api_key=…) — never log it raw
        // (2026-10-08: it was, at every connect).
        debug!("ws: connecting to {}", fjord_player::redact_api_key(&url));
        match connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                info!("ws: connected");
                // Connection-health signal (see FjordState.ws_connected): stall recovery in
                // wire_mpv_timer uses it to tell a broken connection from a stalled stream on a
                // healthy one (e.g. a spinning-up server disk).
                state.lock().unwrap().ws_connected = true;
                backoff = Duration::from_secs(1);
                run_session(
                    ws,
                    &client,
                    &state,
                    &ww,
                    &rt,
                    &refresh_pending,
                    &pending_upsert_ids,
                )
                .await;
                state.lock().unwrap().ws_connected = false;
                info!("ws: disconnected — reconnecting in {:?}", backoff);
            }
            Err(e) => {
                state.lock().unwrap().ws_connected = false;
                // tungstenite's "Unable to connect to <url>" quotes the URL.
                let e = fjord_player::redact_api_key(&format!("{e:#}"));
                warn!("ws: connect error: {e} — retrying in {:?}", backoff);
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

// If any of `episodes` belongs to the series+season on screen (series episode row or season
// overlay — both read series-episode-cards), rebuild that model from the re-sorted
// FjordState.series_episode_items and re-anchor keyboard focus on the same episode by id.
// Only inserts/updates (removals go through remove_item_from_all_models); the None branch
// is defensive. UI thread only.
fn sync_open_episodes(
    w: &MainWindow,
    state: &Arc<Mutex<FjordState>>,
    episodes: &[MediaItem],
    posters: &std::collections::HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
) {
    if episodes.is_empty() {
        return;
    }
    let g = crate::AppState::get(w);
    let sid = g.get_series_id().to_string();
    if sid.is_empty() {
        return;
    }
    let idx = g.get_series_season_idx();
    let Some(cur_season_id) = ({
        let s = state.lock().unwrap();
        if s.series_open_id != sid {
            None
        } else {
            s.series_season_ids.get(idx.max(0) as usize).cloned()
        }
    }) else {
        return;
    };
    let relevant: Vec<&MediaItem> = episodes
        .iter()
        .filter(|e| {
            e.series_id.as_deref() == Some(sid.as_str())
                && e.season_id.as_deref() == Some(cur_season_id.as_str())
        })
        .collect();
    if relevant.is_empty() {
        return;
    }

    let showing_season_detail = g.get_show_season() && g.get_season_id() == cur_season_id.as_str();
    let showing_series_eps = g.get_show_series() && !g.get_series_in_season_row();
    let cards_before = g.get_series_episode_cards();
    let focused_before = if showing_season_detail {
        cards_before
            .row_data(g.get_season_focused_ep().max(0) as usize)
            .map(|c| c.id.to_string())
    } else if showing_series_eps {
        cards_before
            .row_data(g.get_series_focused_ep().max(0) as usize)
            .map(|c| c.id.to_string())
    } else {
        None
    };

    let sorted: Vec<MediaItem> = {
        let mut s = state.lock().unwrap();
        for ep in &relevant {
            upsert_media_item(&mut s.series_episode_items, (*ep).clone());
        }
        s.series_episode_items
            .sort_by_key(|e| e.index_number.unwrap_or(0));
        s.series_episode_items.clone()
    };

    let cards: Vec<CardItem> = sorted
        .iter()
        .map(|ep| {
            let mut c = crate::series::ep_to_card(ep);
            if let Some(buf) = posters.get(&ep.id) {
                c.poster = slint::Image::from_rgba8(buf.clone());
                c.has_poster = true;
            }
            c
        })
        .collect();
    // In place when the season's ids/order are unchanged, so other episode cards keep their
    // poster Images (no re-fade).
    let model = crate::apply_cards_preserving_identity(&g.get_series_episode_cards(), cards);
    g.set_series_episode_cards(model.clone());

    let Some(fid) = focused_before else { return };
    let len = model.row_count() as i32;
    match reanchor_focus(&model, &fid) {
        Some(new_idx) => {
            if showing_season_detail {
                g.set_season_focused_ep(new_idx as i32);
            } else if showing_series_eps {
                g.set_series_focused_ep(new_idx as i32);
            }
        }
        None => {
            if showing_season_detail {
                g.set_season_focused_ep(g.get_season_focused_ep().clamp(0, (len - 1).max(0)));
            } else if showing_series_eps {
                g.set_series_focused_ep(g.get_series_focused_ep().clamp(0, (len - 1).max(0)));
            }
        }
    }
}

// Upsert a delta batch into one of the six all_X models, and — if that library
// grid + view is currently open — refresh library-display in place with focus
// re-anchoring (§0 of the sync plan) instead of leaving it stale until the grid
// is next opened. `view_active` lets Music's three sub-views (Artists/Albums/
// Playlists) share one nav id (4) while only the visible one touches
// library-display. Must be called on the UI thread.
fn upsert_library_bucket(
    w: &MainWindow,
    nav: i32,
    items: &[MediaItem],
    posters: &std::collections::HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    view_active: bool,
    get_all: impl Fn(&crate::AppState) -> ModelRc<CardItem>,
    set_all: impl Fn(&crate::AppState, ModelRc<CardItem>),
) {
    if items.is_empty() {
        return;
    }
    let g = crate::AppState::get(w);
    set_all(&g, upsert_cards_in_model(get_all(&g), items, posters));

    if view_active && g.get_show_library() && g.get_active_nav() == nav {
        let display = g.get_library_display();
        let focused_id = display
            .row_data(g.get_library_focused().max(0) as usize)
            .map(|c| c.id.to_string());
        crate::browse::refresh_library_display(w);
        let Some(fid) = focused_id else { return };
        let g = crate::AppState::get(w);
        let display = g.get_library_display();
        match reanchor_focus(&display, &fid) {
            Some(idx) => g.set_library_focused(idx as i32),
            None => {
                let len = display.row_count() as i32;
                g.set_library_focused(g.get_library_focused().clamp(0, (len - 1).max(0)));
            }
        }
    }
}

// True if `id` is present in `model` — used to detect a genuine favorite/resumable
// *transition* (Phase 3) so a full home refresh is only triggered on the first
// UserDataChanged report of a new state, not on every playback-position tick.
fn row_has_id(model: &ModelRc<CardItem>, id: &str) -> bool {
    (0..model.row_count()).any(|i| model.row_data(i).is_some_and(|c| c.id.as_str() == id))
}

// Debounce (5 s) + spawn the shared delta refresh: fetch_home_data for the ranked home rows,
// plus one get_items_by_ids batch for pending_upsert_ids (the flat library lists,
// movie_collections, series unplayed counts). One at a time (refresh_pending gate); callers
// merge ids first and call this.
fn maybe_spawn_delta_refresh(
    refresh_pending: &Arc<AtomicBool>,
    pending_upsert_ids: &Arc<Mutex<HashSet<String>>>,
    client: &Arc<JellyfinClient>,
    state: &Arc<Mutex<FjordState>>,
    ww: &slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
) {
    if refresh_pending
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        info!("ws: delta refresh already pending, not scheduling another");
        return;
    }
    info!("ws: delta refresh scheduled (fires in 5s)");
    let client2 = Arc::clone(client);
    let state2 = Arc::clone(state);
    let ww2 = ww.clone();
    let rt2 = rt.clone();
    let pending = Arc::clone(refresh_pending);
    let pending_upsert2 = Arc::clone(pending_upsert_ids);
    rt.spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        // NOTE: the flag is reset before fetch_home_data completes, so a second refresh can start
        // while this one runs — a suspected cause of a favorite briefly flashing unfavorited.
        // Logged until confirmed.
        pending.store(false, Ordering::SeqCst);
        info!("ws: delta refresh task woke, starting fetch_home_data + get_items_by_ids");

        // This task isn't covered by ws_abort (only the outer reconnect
        // loop is) — sign-out during the 5 s window doesn't cancel it.
        // Bail if the session that queued this refresh is no longer the
        // active one (signed out, or a different account signed back in
        // on a shared HTPC) so its data never lands in the new session.
        let still_current = state2.lock().unwrap().client.as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, &client2));
        if !still_current {
            return;
        }

        let upsert_ids: Vec<String> = std::mem::take(&mut *pending_upsert2.lock().unwrap()).into_iter().collect();

        // The ranked home rows always get a real re-fetch here (covers LibraryChanged and
        // UserDataChanged; no separate insert path needed). Screen-open caches: a detailed batch of
        // the same ids refreshes item_detail_cache; any id that is a key in one of the 5
        // relationship caches is invalidated (no batch endpoint, and the event doesn't say whether
        // metadata or membership changed).
        let (home_data, items_res, detailed_res) = tokio::join!(
            fetch_home_data(&client2, true),
            client2.get_items_by_ids(&upsert_ids),
            client2.get_items_by_ids_detailed(&upsert_ids),
        );
        {
            let mut s = state2.lock().unwrap();
            match detailed_res {
                Ok(detailed) => {
                    for item in detailed { s.item_detail_cache.insert(item.id.clone(), item); }
                }
                Err(e) => warn!("ws screen-cache detail refresh: {e:#}"),
            }
            for id in &upsert_ids {
                s.similar_items_cache.remove(id);
                s.boxset_items_cache.remove(id);
                s.artist_albums_cache.remove(id);
                s.person_filmography_cache.remove(id);
                s.container_tracks_cache.remove(id);
            }
        }

        if !state2.lock().unwrap().client.as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, &client2))
        {
            return;
        }
        // client2.user_id is the session this whole delta-refresh task is
        // for — reused for every disk-cache save below in this task, more
        // direct than re-deriving from state2.config.active() (and the
        // ptr_eq guard above already confirms it's still the live session).
        let user_id = client2.user_id.clone();

        info!(
            "ws: delta refresh fetch_home_data landed — favorite_movies={} favorite_series={} favorite_albums={} continue_watching={}",
            home_data.favorite_movies.len(), home_data.favorite_series.len(),
            home_data.favorite_albums.len(), home_data.continue_watching.len()
        );
        save_home_cache(&user_id, &home_data);
        let mut fetched: Vec<MediaItem> = items_res.unwrap_or_else(|e| {
            warn!("ws items-by-ids refresh: {e:#}");
            Vec::new()
        });

        // Bucket by type: the six flat library lists, plus Episode for the series refresh below.
        // Audio isn't needed (fetch_home_data covers Recently Played Albums).
        let mut movies      = Vec::new();
        let mut series       = Vec::new();
        let mut collections = Vec::new();
        let mut artists      = Vec::new();
        let mut albums       = Vec::new();
        let mut playlists   = Vec::new();
        let mut episodes    = Vec::new();
        for item in &fetched {
            match item.item_type.as_str() {
                "Movie"       => movies.push(item.clone()),
                "Series"      => series.push(item.clone()),
                "BoxSet"      => collections.push(item.clone()),
                "MusicArtist" => artists.push(item.clone()),
                "MusicAlbum"  => albums.push(item.clone()),
                "Playlist"    => playlists.push(item.clone()),
                "Episode"     => episodes.push(item.clone()),
                _ => {}
            }
        }

        // An updated episode's series isn't necessarily in the same event, so fetch it explicitly —
        // otherwise its unplayed-count badge goes stale.
        let missing_series: Vec<String> = episodes.iter()
            .filter_map(|e| e.series_id.clone())
            .filter(|sid| !series.iter().any(|s| &s.id == sid))
            .collect::<HashSet<_>>()
            .into_iter()
            .collect();
        if !missing_series.is_empty() {
            match client2.get_items_by_ids(&missing_series).await {
                Ok(more) => { fetched.extend(more.iter().cloned()); series.extend(more); }
                Err(e)   => warn!("ws series unplayed-count refresh: {e:#}"),
            }
        }

        let poster_map = fetch_posters_for_delta(&client2, &fetched).await;

        // movie_collections reconciliation (each updated/added BoxSet's
        // current membership) — network calls, so done here in the async
        // task, sequentially (collection changes are infrequent).
        for boxset in &collections {
            match client2.get_boxset_items(&boxset.id).await {
                Ok(members) => {
                    let mut s = state2.lock().unwrap();
                    let member_ids: HashSet<String> = members.iter().map(|m| m.id.clone()).collect();
                    for m in &members {
                        s.movie_collections.insert(m.id.clone(), (boxset.id.clone(), boxset.name.clone()));
                    }
                    s.movie_collections.retain(|id, (bid, _)| bid != &boxset.id || member_ids.contains(id));
                }
                Err(e) => warn!("ws movie_collections refresh for {}: {e:#}", boxset.id),
            }
        }

        // Persist the six lists to FjordState + on-disk cache.
        let (mv, sr, co, ar, al, pl) = {
            let mut s = state2.lock().unwrap();
            for i in movies.iter().cloned()      { upsert_media_item(&mut s.all_movies, i); }
            for i in series.iter().cloned()      { upsert_media_item(&mut s.all_series, i); }
            for i in collections.iter().cloned() { upsert_media_item(&mut s.all_collections, i); }
            for i in artists.iter().cloned()     { upsert_media_item(&mut s.all_artists, i); }
            for i in albums.iter().cloned()      { upsert_media_item(&mut s.all_albums, i); }
            for i in playlists.iter().cloned()   { upsert_media_item(&mut s.all_playlists, i); }
            (s.all_movies.clone(), s.all_series.clone(), s.all_collections.clone(),
             s.all_artists.clone(), s.all_albums.clone(), s.all_playlists.clone())
        };
        if !movies.is_empty()      { save_movies_cache(&user_id, &mv); }
        if !series.is_empty()      { save_series_cache(&user_id, &sr); }
        if !collections.is_empty() { save_collections_cache(&user_id, &co); }
        if !artists.is_empty()     { save_artists_cache(&user_id, &ar); }
        if !albums.is_empty()      { save_albums_cache(&user_id, &al); }
        // Watchlisted series that stopped Continuing: the deferred half of run_session's
        // watch-removal (a fully watched series stays on the watchlist while still airing). Shows
        // up here via LibraryChanged when Jellyfin's metadata refresh changes Status. Re-checks the
        // whole played+watchlist+status condition each time — once removed, the series is no longer
        // in jellyfin_watchlist_ids, so re-running is a no-op.
        let series_to_remove: Vec<(i64, String)> = {
            let s = state2.lock().unwrap();
            series.iter()
                .filter(|item| item.user_data.played && item.status.as_deref() != Some("Continuing"))
                .filter(|item| s.jellyfin_watchlist_ids.contains(&item.id))
                .filter_map(|item| item.provider_ids.get("Tmdb").and_then(|t| t.parse::<i64>().ok()))
                .map(|tmdb_id| (tmdb_id, "tv".to_string()))
                .collect()
        };
        for (tmdb_id, media_type) in series_to_remove {
            info!("ws: watched, watchlisted series tmdb={tmdb_id} is no longer Continuing — removing from watchlist");
            crate::discover::discover_toggle_watchlist(
                Arc::clone(&state2), ww2.clone(), rt2.clone(), tmdb_id, media_type, String::new(), false, None,
            );
        }
        if !playlists.is_empty()   { save_playlists_cache(&user_id, &pl); }

        // Upsert into any season already in series_episode_cache (filled on season-tab switch,
        // series.rs's on_series_select_season); never creates entries. Sorted by episode number so
        // a new episode lands in place.
        {
            let mut s = state2.lock().unwrap();
            for ep in &episodes {
                let Some(season_id) = &ep.season_id else { continue };
                if let Some(cached) = s.series_episode_cache.get_mut(season_id) {
                    upsert_media_item(cached, ep.clone());
                    cached.sort_by_key(|e| e.index_number.unwrap_or(0));
                }
            }
        }

        let sections = home_data_sections(&home_data);
        let ww3 = ww2.clone();
        let state3 = Arc::clone(&state2);
        let episodes2 = episodes.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww3.upgrade() else { return };
            push_home_data_preserving_posters(&w, &home_data);
            sync_open_episodes(&w, &state3, &episodes2, &poster_map);

            // Six near-identical blocks (nav id matches active-nav; music
            // sub-views only refresh library-display when that sub-view is
            // the one currently shown) — matching remove_item_from_all_models's
            // existing straight-line style rather than a dispatch table.
            upsert_library_bucket(&w, 2, &movies,      &poster_map, true,
                |g| g.get_all_movies(),      |g, m| g.set_all_movies(m));
            upsert_library_bucket(&w, 1, &series,      &poster_map, true,
                |g| g.get_all_series(),      |g, m| g.set_all_series(m));
            upsert_library_bucket(&w, 3, &collections, &poster_map, true,
                |g| g.get_all_collections(), |g, m| g.set_all_collections(m));
            let music_view = crate::AppState::get(&w).get_library_music_view();
            upsert_library_bucket(&w, 4, &artists,   &poster_map, music_view == 0,
                |g| g.get_all_artists(),   |g, m| g.set_all_artists(m));
            upsert_library_bucket(&w, 4, &albums,    &poster_map, music_view == 1,
                |g| g.get_all_albums(),    |g, m| g.set_all_albums(m));
            upsert_library_bucket(&w, 4, &playlists, &poster_map, music_view == 2,
                |g| g.get_all_playlists(), |g, m| g.set_all_playlists(m));
        });
        spawn_poster_loading(client2, sections, ww2, rt2, state2);
    });
}

// ── session handler ───────────────────────────────────────────────────────────

async fn run_session(
    ws: tokio_tungstenite::WebSocketStream<
        tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>,
    >,
    client: &Arc<JellyfinClient>,
    state: &Arc<Mutex<FjordState>>,
    ww: &slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    refresh_pending: &Arc<AtomicBool>,
    pending_upsert_ids: &Arc<Mutex<HashSet<String>>>,
) {
    let (mut write, mut read) = ws.split();

    // Client-driven keep-alive: Jellyfin expects a KeepAlive at least every timeout/2 (default
    // 60 s) and acks each with another KeepAlive — never reply to those (that looped at wire
    // speed).
    let mut keepalive = tokio::time::interval(Duration::from_secs(30));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let text = tokio::select! {
            _ = keepalive.tick() => {
                let ka = json!({"MessageType": "KeepAlive"}).to_string();
                if write.send(Message::Text(ka)).await.is_err() {
                    warn!("ws: keep-alive send failed");
                    break;
                }
                continue;
            }
            msg = read.next() => match msg {
                None                        => break,
                Some(Ok(Message::Text(t)))  => t,
                Some(Ok(Message::Close(_))) => { info!("ws: server closed"); break; }
                Some(Ok(_))                 => continue,
                Some(Err(e))                => { warn!("ws: stream error: {e:#}"); break; }
            }
        };

        let Ok(msg) = serde_json::from_str::<WsMsg>(&text) else {
            // chars().take(): byte-index slicing panics mid-UTF-8-char, and a
            // panic here kills the whole ws_loop task — reconnects included (CR10-11).
            debug!(
                "ws: non-JSON: {}",
                text.chars().take(120).collect::<String>()
            );
            continue;
        };

        match msg.message_type.as_str() {
            "ForceKeepAlive" | "KeepAlive" => {
                // ForceKeepAlive announces the timeout; KeepAlive is the ack for
                // our periodic ping. Never reply here — the server acks every
                // KeepAlive, so replying loops forever.
                debug!("ws: keep-alive ack");
                // Live connection-health timestamp — see FjordState's own
                // doc comment on ws_last_keepalive_at.
                state.lock().unwrap().ws_last_keepalive_at = Some(std::time::Instant::now());
            }

            "LibraryChanged" => {
                let payload =
                    serde_json::from_value::<LibraryChangedPayload>(msg.data).unwrap_or_default();
                info!(
                    "ws: LibraryChanged — {} added, {} updated, {} removed; scheduling refresh in 5 s",
                    payload.items_added.len(),
                    payload.items_updated.len(),
                    payload.items_removed.len()
                );
                let removed = payload.items_removed;

                // Any library change invalidates the per-session list caches:
                // the next grid open (or the open grid, below) re-fetches (S1/S3).
                {
                    let mut s = state.lock().unwrap();
                    s.movies_fetched = false;
                    s.movie_posters_loaded = false;
                    s.collections_fetched = false;
                    s.artists_fetched = false;
                    s.albums_fetched = false;
                    s.playlists_fetched = false;
                    s.browse_populated = false;
                    for id in &removed {
                        s.all_movies.retain(|i| &i.id != id);
                        s.all_series.retain(|i| &i.id != id);
                        s.all_collections.retain(|i| &i.id != id);
                        s.all_artists.retain(|i| &i.id != id);
                        s.all_albums.retain(|i| &i.id != id);
                        s.all_playlists.retain(|i| &i.id != id);
                        s.filtered_items.retain(|i| &i.id != id);
                        s.movie_collections.remove(id);
                        for eps in s.series_episode_cache.values_mut() {
                            eps.retain(|e| &e.id != id);
                        }
                        // Screen-open caches (Phase 103): a deleted item can't stay
                        // cached anywhere, whether as an item_detail_cache key or as
                        // the container key of one of the 5 relationship caches.
                        s.item_detail_cache.remove(id);
                        s.similar_items_cache.remove(id);
                        s.boxset_items_cache.remove(id);
                        s.artist_albums_cache.remove(id);
                        s.person_filmography_cache.remove(id);
                        s.container_tracks_cache.remove(id);
                    }
                }

                // Deleted items: drop their cached artwork now — the 24 h orphan
                // sweep otherwise leaves poster-less ghosts in stale grids.
                for id in &removed {
                    // None for an id that isn't a valid cache name — nothing to delete.
                    let (Some(pp), Some(bp)) = (
                        crate::config::poster_cache_path(id),
                        crate::config::backdrop_cache_path(id),
                    ) else {
                        continue;
                    };
                    rt.spawn(async move {
                        let _ = tokio::fs::remove_file(pp.with_extension("tag")).await;
                        let _ = tokio::fs::remove_file(bp.with_extension("tag")).await;
                        let _ = tokio::fs::remove_file(pp).await;
                        let _ = tokio::fs::remove_file(bp).await;
                    });
                }

                // UI thread: remove deleted ids from every visible model. No longer
                // triggers a full network re-fetch of an open grid here — the
                // debounced batch below upserts just the added/updated ids instead.
                {
                    let ww2 = ww.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(w) = ww2.upgrade() else { return };
                        for id in &removed {
                            crate::context_menu::remove_item_from_all_models(&w, id);
                        }
                    });
                }

                // Merge added/updated ids into the shared batch, drained by the
                // debounce task below.
                {
                    let mut ids = pending_upsert_ids.lock().unwrap();
                    ids.extend(payload.items_added.iter().cloned());
                    ids.extend(payload.items_updated.iter().cloned());
                }
                maybe_spawn_delta_refresh(
                    refresh_pending,
                    pending_upsert_ids,
                    client,
                    state,
                    ww,
                    rt,
                );
            }

            "UserDataChanged" => {
                let Ok(payload) = serde_json::from_value::<UserDataChangedPayload>(msg.data) else {
                    continue;
                };
                let items: Vec<(String, bool, bool, i64)> = payload
                    .user_data_list
                    .into_iter()
                    .map(|u| {
                        (
                            u.item_id,
                            u.played,
                            u.is_favorite,
                            u.playback_position_ticks,
                        )
                    })
                    .collect();
                if items.is_empty() {
                    continue;
                }
                info!("ws: UserDataChanged — {} item(s)", items.len());
                for (id, played, fav, pos_ticks) in &items {
                    info!(
                        "ws: UserDataChanged item id={id} played={played} favorite={fav} position_ticks={pos_ticks}"
                    );
                }
                // A watchlisted item marked watched leaves the Seerr watchlist (one hook covers
                // Fjord's own Mark Played, the credits auto-mark and other clients — Jellyfin
                // echoes all of them here). Resolved under this lock (local lookups only); the
                // removal (discover_toggle_watchlist) runs after the lock is dropped — it locks
                // `state` itself.
                let mut newly_watched_on_watchlist: Vec<(i64, String)> = Vec::new();
                {
                    let mut s = state.lock().unwrap();
                    for (id, played, fav, _) in &items {
                        s.update_item_user_state(id, Some(*played), Some(*fav));
                        // Played/favorite state also lives in the cached MediaItem: invalidate (the
                        // next open fetches). UserDataChanged never changes list membership, so the
                        // relationship caches stay.
                        s.item_detail_cache.remove(id);
                        if *played {
                            // jellyfin_watchlist_ids only ever holds Movie/Series
                            // ids (Seerr's watchlist has no Episode/BoxSet
                            // concept — see resolve_tmdb_for_jellyfin_item's own
                            // doc comment), so this membership check alone is
                            // enough to skip every Episode UserDataChanged event
                            // without needing this payload's (nonexistent) item
                            // type field.
                            if s.jellyfin_watchlist_ids.contains(id)
                                && let Some((tmdb_id_str, media_type)) =
                                    crate::context_menu::resolve_tmdb_for_jellyfin_item(
                                        &s, id, "Movie",
                                    )
                                    .or_else(|| {
                                        crate::context_menu::resolve_tmdb_for_jellyfin_item(
                                            &s, id, "Series",
                                        )
                                    })
                            {
                                // A still-airing (Continuing) series stays on the watchlist even
                                // when fully watched — another season may come;
                                // maybe_spawn_delta_refresh removes it once it stops Continuing.
                                // Movies and Ended/unknown series are removed here.
                                let still_continuing = media_type == "tv"
                                    && s.all_series
                                        .iter()
                                        .find(|m| &m.id == id)
                                        .and_then(|m| m.status.as_deref())
                                        == Some("Continuing");
                                if !still_continuing && let Ok(tmdb_id) = tmdb_id_str.parse::<i64>()
                                {
                                    newly_watched_on_watchlist
                                        .push((tmdb_id, media_type.to_string()));
                                }
                            }
                        }
                    }
                }
                for (tmdb_id, media_type) in newly_watched_on_watchlist {
                    info!(
                        "ws: watched item tmdb={tmdb_id} media_type={media_type} was on the watchlist — removing"
                    );
                    crate::discover::discover_toggle_watchlist(
                        Arc::clone(state),
                        ww.clone(),
                        rt.clone(),
                        tmdb_id,
                        media_type,
                        String::new(),
                        false,
                        None,
                    );
                }
                let ww2 = ww.clone();
                let client2 = Arc::clone(client);
                let state2 = Arc::clone(state);
                let rt2 = rt.clone();
                let refresh_pending2 = Arc::clone(refresh_pending);
                let pending_upsert_ids2 = Arc::clone(pending_upsert_ids);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww2.upgrade() else { return };
                    let g = crate::AppState::get(&w);
                    // Removal is immediate (no fetch). A NEW favorite/resumable item instead waits
                    // for the debounced refresh, whose fetch_home_data re-fetches every home row
                    // anyway — so this only decides whether a transition happened that's worth
                    // waking it.
                    let mut needs_refresh = false;
                    for (id, played, fav, pos_ticks) in &items {
                        update_card_in_all_models(&w, id, Some(*played), Some(*fav));
                        // Not Watched rows have the OPPOSITE membership rule from Continue
                        // Watching (untouched = position 0 vs in-progress = position > 0), so
                        // a single played-or-position==0 condition can't correctly gate removal
                        // from both: played==true leaves both correctly, but position==0 alone
                        // (e.g. from a plain favorite toggle on a never-watched item, which
                        // naturally has position 0 regardless of what changed) must only drop
                        // it from Continue Watching, not Not Watched — otherwise a favorite
                        // toggle spuriously vanishes the item from Not Watched.
                        if *played {
                            crate::context_menu::remove_from_dynamic_rows(&w, id);
                        } else if *pos_ticks == 0 {
                            crate::context_menu::remove_from_continue_watching(&w, id);
                        }
                        if !*fav {
                            crate::context_menu::remove_from_favorites(&w, id);
                        }
                        let new_favorite = *fav
                            && !row_has_id(&g.get_favorite_movies(), id)
                            && !row_has_id(&g.get_favorite_series(), id)
                            && !row_has_id(&g.get_favorite_albums(), id);
                        let new_resumable = *pos_ticks > 0
                            && !*played
                            && !row_has_id(&g.get_continue_watching(), id);
                        // A watched movie may complete a collection, but a BoxSet's id never
                        // matches a member's, so remove_from_dynamic_rows can't drop it locally.
                        // Wake the debounced refresh instead: fetch_home_data re-derives unwatched
                        // collections from the server (harmless when the collection isn't complete
                        // yet).
                        let in_known_collection =
                            *played && state2.lock().unwrap().movie_collections.contains_key(id);
                        info!(
                            "ws: UserDataChanged item id={id} new_favorite={new_favorite} new_resumable={new_resumable} in_known_collection={in_known_collection}"
                        );
                        if new_favorite || new_resumable || in_known_collection {
                            needs_refresh = true;
                        }
                    }
                    info!("ws: UserDataChanged batch processed, needs_refresh={needs_refresh}");
                    if needs_refresh {
                        maybe_spawn_delta_refresh(
                            &refresh_pending2,
                            &pending_upsert_ids2,
                            &client2,
                            &state2,
                            &ww2,
                            &rt2,
                        );
                    }
                });
            }

            other => {
                debug!("ws: unhandled message type: {}", other);
            }
        }
    }
}
