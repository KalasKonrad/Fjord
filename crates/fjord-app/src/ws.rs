// ── fjord-app · ws.rs ─────────────────────────────────────────────────────────
//   start_websocket  spawn reconnect loop; returns AbortHandle for sign-out cleanup
//   ws_loop          outer reconnect loop with exponential backoff (1 s → 60 s max)
//   run_session      process messages until the connection drops
//   run_session      periodic client KeepAlive every 30 s (server acks are ignored —
//                    replying to acks looped at wire speed, Phase 62);
//                    LibraryChanged: parse ItemsAdded/Updated/Removed — clear *_fetched flags,
//                    purge removed ids from state/models/poster cache, refresh open grid,
//                    debounced home + series refresh; UserDataChanged; KeepAlive
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use fjord_api::JellyfinClient;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::json;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use tracing::{debug, info, warn};

use slint::Global;

use crate::MainWindow;
use crate::config::FjordState;
use crate::context_menu::update_card_in_all_models;
use crate::home::{fetch_home_data, home_data_sections, push_home_data, save_home_cache, save_series_cache};
use crate::poster::spawn_poster_loading;

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
    let mut backoff = Duration::from_secs(1);

    loop {
        debug!("ws: connecting to {}", url);
        match connect_async(url.as_str()).await {
            Ok((ws, _)) => {
                info!("ws: connected");
                backoff = Duration::from_secs(1);
                run_session(ws, &client, &state, &ww, &rt, &refresh_pending).await;
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

// ── session handler ───────────────────────────────────────────────────────────

async fn run_session(
    ws:              tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>,
    client:          &Arc<JellyfinClient>,
    state:           &Arc<Mutex<FjordState>>,
    ww:              &slint::Weak<MainWindow>,
    rt:              &tokio::runtime::Handle,
    refresh_pending: &Arc<AtomicBool>,
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

                // UI thread: remove deleted ids from every visible model, and if a
                // library grid is open kick its background refresh right away
                // (the *_fetched flags were just cleared).
                {
                    let ww2    = ww.clone();
                    let state2 = Arc::clone(state);
                    let rt2    = rt.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(w) = ww2.upgrade() else { return };
                        for id in &removed {
                            crate::context_menu::remove_item_from_all_models(&w, id);
                        }
                        let g = crate::AppState::get(&w);
                        if g.get_show_library() {
                            crate::spawn_library_fetch(g.get_active_nav(), state2, ww2.clone(), rt2);
                        }
                    });
                }

                // Debounce: only one home/series refresh task outstanding at a time.
                if refresh_pending
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let client2  = Arc::clone(client);
                    let state2   = Arc::clone(state);
                    let ww2      = ww.clone();
                    let rt2      = rt.clone();
                    let pending  = Arc::clone(refresh_pending);
                    rt.spawn(async move {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        pending.store(false, Ordering::SeqCst);

                        // Series list piggybacks on the debounced refresh — it has
                        // no *_fetched flag (login refreshes it), so mid-session
                        // adds/renames would otherwise wait for the next login.
                        let (home_data, series_res) = tokio::join!(
                            fetch_home_data(&client2),
                            client2.get_all_series(),
                        );
                        save_home_cache(&home_data);
                        let series = match series_res {
                            Ok(v)  => { save_series_cache(&v); Some(v) }
                            Err(e) => { warn!("ws series refresh: {e:#}"); None }
                        };
                        if let Some(ref v) = series {
                            state2.lock().unwrap().all_series = v.clone();
                        }
                        let sections = home_data_sections(&home_data);
                        let ww3 = ww2.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww3.upgrade() {
                                push_home_data(&w, &home_data);
                                if let Some(v) = series {
                                    crate::AppState::get(&w).set_all_series(crate::items_to_model(&v));
                                }
                            }
                        });
                        spawn_poster_loading(client2, sections, ww2, rt2);
                    });
                }
            }

            "UserDataChanged" => {
                let Ok(payload) =
                    serde_json::from_value::<UserDataChangedPayload>(msg.data)
                else {
                    continue;
                };
                let items: Vec<(String, bool, bool)> = payload
                    .user_data_list
                    .into_iter()
                    .map(|u| (u.item_id, u.played, u.is_favorite))
                    .collect();
                if items.is_empty() {
                    continue;
                }
                info!("ws: UserDataChanged — {} item(s)", items.len());
                {
                    let mut s = state.lock().unwrap();
                    for (id, played, fav) in &items {
                        s.update_item_user_state(id, Some(*played), Some(*fav));
                    }
                }
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        for (id, played, fav) in items {
                            update_card_in_all_models(&w, &id, Some(played), Some(fav));
                        }
                    }
                });
            }

            other => {
                debug!("ws: unhandled message type: {}", other);
            }
        }
    }
}
