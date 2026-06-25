// ── fjord-app · ws.rs ─────────────────────────────────────────────────────────
//   start_websocket  spawn reconnect loop; returns AbortHandle for sign-out cleanup
//   ws_loop          outer reconnect loop with exponential backoff (1 s → 60 s max)
//   run_session      process messages until the connection drops
//   handle_message   route LibraryChanged, UserDataChanged, ForceKeepAlive/KeepAlive
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

use crate::MainWindow;
use crate::config::FjordState;
use crate::context_menu::update_card_in_all_models;
use crate::home::{fetch_home_data, home_data_sections, push_home_data, save_home_cache};
use crate::poster::spawn_poster_loading;

// ── wire types ────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
struct WsMsg {
    #[serde(rename = "MessageType")]
    message_type: String,
    #[serde(rename = "Data", default)]
    data: serde_json::Value,
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

    while let Some(msg_result) = read.next().await {
        let text = match msg_result {
            Ok(Message::Text(t))  => t,
            Ok(Message::Close(_)) => { info!("ws: server closed"); break; }
            Ok(_)                 => continue,
            Err(e)                => { warn!("ws: stream error: {e:#}"); break; }
        };

        let Ok(msg) = serde_json::from_str::<WsMsg>(&text) else {
            debug!("ws: non-JSON: {}", &text[..text.len().min(120)]);
            continue;
        };

        match msg.message_type.as_str() {
            "ForceKeepAlive" | "KeepAlive" => {
                debug!("ws: keep-alive");
                let pong = json!({"MessageType": "KeepAlive"}).to_string();
                let _ = write.send(Message::Text(pong.into())).await;
            }

            "LibraryChanged" => {
                info!("ws: LibraryChanged — scheduling home refresh in 5 s");
                // Debounce: only one refresh task outstanding at a time.
                if refresh_pending
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_ok()
                {
                    let client2  = Arc::clone(client);
                    let ww2      = ww.clone();
                    let rt2      = rt.clone();
                    let pending  = Arc::clone(refresh_pending);
                    rt.spawn(async move {
                        tokio::time::sleep(Duration::from_secs(5)).await;
                        pending.store(false, Ordering::SeqCst);

                        let home_data = fetch_home_data(&client2).await;
                        save_home_cache(&home_data);
                        let sections = home_data_sections(&home_data);
                        let ww3 = ww2.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww3.upgrade() {
                                push_home_data(&w, &home_data);
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
