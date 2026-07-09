// ── fjord-app · ws.rs ─────────────────────────────────────────────────────────
//   start_websocket  spawn reconnect loop; returns AbortHandle for sign-out cleanup
//   ws_loop          outer reconnect loop with exponential backoff (1 s → 60 s max);
//                    owns pending_upsert_ids (LibraryChanged Added/Updated ids +
//                    UserDataChanged favorite/resume candidates, shared accumulator)
//   row_has_id                found-by-id check on a CardItem model (Phase 3 transition gate)
//   sync_open_episodes         Phase 6: if an added/updated episode belongs to the series+season
//                              currently on screen (series screen episode row or season detail
//                              overlay — both read series-episode-cards), upsert + re-sort
//                              FjordState.series_episode_items, rebuild the model, and re-anchor
//                              season-focused-ep/series-focused-ep (§0) by id
//   upsert_library_bucket      upsert a delta batch into one all_X model + library-display
//                              in place (with focus re-anchoring) if that grid+view is open
//   maybe_spawn_delta_refresh  debounced (5 s) shared refresh task, callable from both event
//                              types: fetch_home_data (ranked rows — Continue Watching/Next Up/
//                              Recently Added/Not Watched/Favorites/Recently Played Albums —
//                              already covers Phase 2/3's row content, no bespoke upsert needed)
//                              + get_items_by_ids(pending_upsert_ids) bucketed by type into the
//                              six flat library lists (Phase 1) + Episode (Phase 4: fetches any
//                              missing parent series explicitly so the unplayed-count badge
//                              doesn't depend on Jellyfin reporting the series itself; also feeds
//                              sync_open_episodes and series_episode_cache, Phase 6) +
//                              movie_collections reconciliation for any BoxSet in the batch
//                              (Phase 5); session-guarded (bails if the client that queued it is
//                              no longer FjordState's active one, CR11-2)
//   run_session      process messages until the connection drops; periodic client KeepAlive
//                    every 30 s (server acks ignored — replying looped at wire speed, Phase 62);
//                    LibraryChanged: parse ItemsAdded/Updated/Removed — clear *_fetched flags,
//                    purge removed ids from state/models/poster cache immediately, queue
//                    added/updated ids + maybe_spawn_delta_refresh (no more immediate full
//                    re-fetch of an open grid);
//                    UserDataChanged: patch has-played/is-favorite in place (unchanged), then
//                    immediate removal from dynamic rows (played/position=0) and favorites
//                    (unfavorited) — cheap, no fetch; a genuine favorite/resume transition (not
//                    already present in the row) triggers maybe_spawn_delta_refresh so other-
//                    client changes reach Favorites/Continue Watching within ~5 s;
//                    KeepAlive
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fjord_api::models::MediaItem;
use fjord_api::JellyfinClient;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use slint::{Global, Model, ModelRc, VecModel};

use crate::MainWindow;
use crate::config::{FjordState, upsert_media_item};
use crate::context_menu::{reanchor_focus, update_card_in_all_models, upsert_cards_in_model};
use crate::home::{
    fetch_home_data, home_data_sections, push_home_data, save_home_cache, save_series_cache,
    save_movies_cache, save_collections_cache, save_artists_cache, save_albums_cache, save_playlists_cache,
};
use crate::poster::{fetch_posters_for_delta, spawn_poster_loading};
use crate::CardItem;

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
    #[serde(rename = "ItemsAdded",   default)] items_added:   Vec<String>,
    #[serde(rename = "ItemsUpdated", default)] items_updated: Vec<String>,
    #[serde(rename = "ItemsRemoved", default)] items_removed: Vec<String>,
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
    state:  Arc<Mutex<FjordState>>,
    ww:     slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
) -> tokio::task::AbortHandle {
    rt.spawn(ws_loop(client, state, ww, rt.clone())).abort_handle()
}

// ── reconnect loop ────────────────────────────────────────────────────────────

async fn ws_loop(
    client: Arc<JellyfinClient>,
    state:  Arc<Mutex<FjordState>>,
    ww:     slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
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
        debug!("ws: connecting to {}", url);
        match connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                info!("ws: connected");
                backoff = Duration::from_secs(1);
                run_session(ws, &client, &state, &ww, &rt, &refresh_pending, &pending_upsert_ids).await;
                info!("ws: disconnected — reconnecting in {:?}", backoff);
            }
            Err(e) => {
                warn!("ws: connect error: {e:#} — retrying in {:?}", backoff);
            }
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

// Phase 6: if any of `episodes` belongs to the series+season currently on
// screen (series screen's episode row, or the season detail overlay — both
// read the same series-episode-cards model), rebuild that model from the
// updated, re-sorted FjordState.series_episode_items and re-anchor keyboard
// focus (§0) onto whatever episode was focused before, by id. Only ever
// inserts/updates — removed episodes are handled separately by
// remove_item_from_all_models, so a focused episode can't have vanished out
// from under this function; the None branch below is defensive, matching the
// same clamp behavior used for the library grid. Must run on the UI thread.
fn sync_open_episodes(
    w:        &MainWindow,
    state:    &Arc<Mutex<FjordState>>,
    episodes: &[MediaItem],
    posters:  &std::collections::HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
) {
    if episodes.is_empty() {
        return;
    }
    let g   = crate::AppState::get(w);
    let sid = g.get_series_id().to_string();
    if sid.is_empty() {
        return;
    }
    let idx = g.get_series_season_idx();
    let Some(cur_season_id) = ({
        let s = state.lock().unwrap();
        if s.series_open_id != sid { None } else { s.series_season_ids.get(idx.max(0) as usize).cloned() }
    }) else {
        return;
    };
    let relevant: Vec<&MediaItem> = episodes.iter()
        .filter(|e| e.series_id.as_deref() == Some(sid.as_str()) && e.season_id.as_deref() == Some(cur_season_id.as_str()))
        .collect();
    if relevant.is_empty() {
        return;
    }

    let showing_season_detail = g.get_show_season() && g.get_season_id() == cur_season_id.as_str();
    let showing_series_eps    = g.get_show_series() && !g.get_series_in_season_row();
    let cards_before = g.get_series_episode_cards();
    let focused_before = if showing_season_detail {
        cards_before.row_data(g.get_season_focused_ep().max(0) as usize).map(|c| c.id.to_string())
    } else if showing_series_eps {
        cards_before.row_data(g.get_series_focused_ep().max(0) as usize).map(|c| c.id.to_string())
    } else {
        None
    };

    let sorted: Vec<MediaItem> = {
        let mut s = state.lock().unwrap();
        for ep in &relevant {
            upsert_media_item(&mut s.series_episode_items, (*ep).clone());
        }
        s.series_episode_items.sort_by_key(|e| e.index_number.unwrap_or(0));
        s.series_episode_items.clone()
    };

    let cards: Vec<CardItem> = sorted.iter().map(|ep| {
        let mut c = crate::series::ep_to_card(ep);
        if let Some(buf) = posters.get(&ep.id) {
            c.poster     = slint::Image::from_rgba8(buf.clone());
            c.has_poster = true;
        }
        c
    }).collect();
    let model = ModelRc::new(VecModel::from(cards));
    g.set_series_episode_cards(model.clone());

    let Some(fid) = focused_before else { return };
    let len = model.row_count() as i32;
    match reanchor_focus(&model, &fid) {
        Some(new_idx) => {
            if showing_season_detail      { g.set_season_focused_ep(new_idx as i32); }
            else if showing_series_eps    { g.set_series_focused_ep(new_idx as i32); }
        }
        None => {
            if showing_season_detail      { g.set_season_focused_ep(g.get_season_focused_ep().clamp(0, (len - 1).max(0))); }
            else if showing_series_eps    { g.set_series_focused_ep(g.get_series_focused_ep().clamp(0, (len - 1).max(0))); }
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
    w:           &MainWindow,
    nav:         i32,
    items:       &[MediaItem],
    posters:     &std::collections::HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    view_active: bool,
    get_all:     impl Fn(&crate::AppState) -> ModelRc<CardItem>,
    set_all:     impl Fn(&crate::AppState, ModelRc<CardItem>),
) {
    if items.is_empty() {
        return;
    }
    let g = crate::AppState::get(w);
    set_all(&g, upsert_cards_in_model(get_all(&g), items, posters));

    if view_active && g.get_show_library() && g.get_active_nav() == nav {
        let display     = g.get_library_display();
        let focused_id   = display.row_data(g.get_library_focused().max(0) as usize).map(|c| c.id.to_string());
        crate::browse::refresh_library_display(w);
        let Some(fid) = focused_id else { return };
        let g       = crate::AppState::get(w);
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

// Debounce (5 s) + spawn the shared delta-refresh task: ranked home rows (Continue
// Watching/Next Up/Recently Added/Favorites/Recently Played — Phase 2/3, already
// fully covered by the unconditional fetch_home_data call below, no bespoke upsert
// needed) plus a get_items_by_ids batch for whatever's queued in pending_upsert_ids
// (Phase 1's six flat library lists + Phase 5's movie_collections + Phase 4's
// targeted series unplayed-count refresh). Only one instance runs at a time
// (refresh_pending gate); callers just merge ids first and call this.
fn maybe_spawn_delta_refresh(
    refresh_pending:    &Arc<AtomicBool>,
    pending_upsert_ids: &Arc<Mutex<HashSet<String>>>,
    client:             &Arc<JellyfinClient>,
    state:              &Arc<Mutex<FjordState>>,
    ww:                 &slint::Weak<MainWindow>,
    rt:                 &tokio::runtime::Handle,
) {
    if refresh_pending
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let client2  = Arc::clone(client);
    let state2   = Arc::clone(state);
    let ww2      = ww.clone();
    let rt2      = rt.clone();
    let pending  = Arc::clone(refresh_pending);
    let pending_upsert2 = Arc::clone(pending_upsert_ids);
    rt.spawn(async move {
        tokio::time::sleep(Duration::from_secs(5)).await;
        pending.store(false, Ordering::SeqCst);

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

        // Ranked home rows (Continue Watching/Next Up/Recently Added/Not
        // Watched/Favorites/Recently Played Albums/Playlists) always get a
        // real re-fetch here — Phase 2/3's row content is entirely covered
        // by this one call, so LibraryChanged and UserDataChanged both just
        // need to reach this task; no separate insert-by-date/insert-by-
        // favorite path is needed on top of it.
        let (home_data, items_res) = tokio::join!(
            fetch_home_data(&client2),
            client2.get_items_by_ids(&upsert_ids),
        );

        if !state2.lock().unwrap().client.as_ref()
            .is_some_and(|c| Arc::ptr_eq(c, &client2))
        {
            return;
        }

        save_home_cache(&home_data);
        let mut fetched: Vec<MediaItem> = items_res.unwrap_or_else(|e| {
            warn!("ws items-by-ids refresh: {e:#}");
            Vec::new()
        });

        // Bucket by type — six flat library lists this phase covers, plus
        // Episode for Phase 4's targeted series refresh below. Audio isn't
        // bucketed: Phase 3's Recently Played Albums row is covered by
        // fetch_home_data above, nothing else currently needs raw Audio items.
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

        // Phase 4: an added/updated episode's parent series doesn't necessarily
        // appear in the same LibraryChanged/UserDataChanged report, so its
        // unplayed-count badge (all_series / library grid) would otherwise go
        // stale. Fetch any such series explicitly rather than waiting to be told.
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
        if !movies.is_empty()      { save_movies_cache(&mv); }
        if !series.is_empty()      { save_series_cache(&sr); }
        if !collections.is_empty() { save_collections_cache(&co); }
        if !artists.is_empty()     { save_artists_cache(&ar); }
        if !albums.is_empty()      { save_albums_cache(&al); }
        if !playlists.is_empty()   { save_playlists_cache(&pl); }

        // Phase 6: upsert into any season whose episode list is already cached
        // (series_episode_cache — populated on season-tab switch, see main.rs
        // on_series_select_season). Only touches seasons already known; never
        // speculatively creates a new cache entry. Sorted by episode number so
        // a brand-new episode lands in the right slot, not appended at the end.
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
            push_home_data(&w, &home_data);
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
        spawn_poster_loading(client2, sections, ww2, rt2);
    });
}

// ── session handler ───────────────────────────────────────────────────────────

async fn run_session(
    ws:                 tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    client:             &Arc<JellyfinClient>,
    state:              &Arc<Mutex<FjordState>>,
    ww:                 &slint::Weak<MainWindow>,
    rt:                 &tokio::runtime::Handle,
    refresh_pending:    &Arc<AtomicBool>,
    pending_upsert_ids: &Arc<Mutex<HashSet<String>>>,
) {
    let (mut write, mut read) = ws.split();

    // Client-driven keep-alive. Jellyfin expects a KeepAlive message at least
    // every timeout/2 (default timeout 60 s) and ACKS each one with another
    // KeepAlive. Replying to those acks (pre-Phase 62) created a wire-speed
    // feedback loop — ~9k messages/s and a 6.4 GB debug log.
    let mut keepalive = tokio::time::interval(Duration::from_secs(30));
    keepalive.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);

    loop {
        let text = tokio::select! {
            _ = keepalive.tick() => {
                let ka = json!({"MessageType": "KeepAlive"}).to_string();
                if write.send(Message::Text(ka.into())).await.is_err() {
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
            debug!("ws: non-JSON: {}", text.chars().take(120).collect::<String>());
            continue;
        };

        match msg.message_type.as_str() {
            "ForceKeepAlive" | "KeepAlive" => {
                // ForceKeepAlive announces the timeout; KeepAlive is the ack for
                // our periodic ping. Never reply here — the server acks every
                // KeepAlive, so replying loops forever.
                debug!("ws: keep-alive ack");
            }

            "LibraryChanged" => {
                let payload = serde_json::from_value::<LibraryChangedPayload>(msg.data)
                    .unwrap_or_default();
                info!(
                    "ws: LibraryChanged — {} added, {} updated, {} removed; scheduling refresh in 5 s",
                    payload.items_added.len(), payload.items_updated.len(), payload.items_removed.len()
                );
                let removed = payload.items_removed;

                // Any library change invalidates the per-session list caches:
                // the next grid open (or the open grid, below) re-fetches (S1/S3).
                {
                    let mut s = state.lock().unwrap();
                    s.movies_fetched      = false;
                    s.collections_fetched = false;
                    s.artists_fetched     = false;
                    s.albums_fetched      = false;
                    s.playlists_fetched   = false;
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
                    }
                }

                // Deleted items: drop their cached artwork now — the 24 h orphan
                // sweep otherwise leaves poster-less ghosts in stale grids.
                for id in &removed {
                    let pp = crate::config::poster_cache_path(id);
                    let bp = crate::config::backdrop_cache_path(id);
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
                maybe_spawn_delta_refresh(refresh_pending, pending_upsert_ids, client, state, ww, rt);
            }

            "UserDataChanged" => {
                let Ok(payload) =
                    serde_json::from_value::<UserDataChangedPayload>(msg.data)
                else {
                    continue;
                };
                let items: Vec<(String, bool, bool, i64)> = payload
                    .user_data_list
                    .into_iter()
                    .map(|u| (u.item_id, u.played, u.is_favorite, u.playback_position_ticks))
                    .collect();
                if items.is_empty() {
                    continue;
                }
                info!("ws: UserDataChanged — {} item(s)", items.len());
                {
                    let mut s = state.lock().unwrap();
                    for (id, played, fav, _) in &items {
                        s.update_item_user_state(id, Some(*played), Some(*fav));
                    }
                }
                let ww2 = ww.clone();
                let client2 = Arc::clone(client);
                let state2  = Arc::clone(state);
                let rt2     = rt.clone();
                let refresh_pending2    = Arc::clone(refresh_pending);
                let pending_upsert_ids2 = Arc::clone(pending_upsert_ids);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww2.upgrade() else { return };
                    let g = crate::AppState::get(&w);
                    // Phase 3: removal is immediate and cheap (no fetch needed — the card's
                    // already in a visible model or it isn't). Insertion of a *new* favorite/
                    // resumable item instead waits for the shared debounced refresh below,
                    // whose unconditional fetch_home_data call already re-fetches every home
                    // row (Favorites/Continue Watching/Recently Played) from the server —
                    // a bespoke client-side insert-by-id path would just be immediately
                    // overwritten by that fetch, so there's nothing to build here beyond
                    // deciding *whether* a transition happened worth waking that task for.
                    let mut needs_refresh = false;
                    for (id, played, fav, pos_ticks) in &items {
                        update_card_in_all_models(&w, id, Some(*played), Some(*fav));
                        if *played || *pos_ticks == 0 {
                            crate::context_menu::remove_from_dynamic_rows(&w, id);
                        }
                        if !*fav {
                            crate::context_menu::remove_from_favorites(&w, id);
                        }
                        let new_favorite = *fav
                            && !row_has_id(&g.get_favorite_movies(), id)
                            && !row_has_id(&g.get_favorite_series(), id)
                            && !row_has_id(&g.get_favorite_albums(), id);
                        let new_resumable = *pos_ticks > 0 && !*played
                            && !row_has_id(&g.get_continue_watching(), id);
                        if new_favorite || new_resumable {
                            needs_refresh = true;
                        }
                    }
                    if needs_refresh {
                        maybe_spawn_delta_refresh(&refresh_pending2, &pending_upsert_ids2, &client2, &state2, &ww2, &rt2);
                    }
                });
            }

            other => {
                debug!("ws: unhandled message type: {}", other);
            }
        }
    }
}
