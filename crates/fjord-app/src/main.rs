slint::include_modules!();

use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::{models::MediaItem, JellyfinClient};
use serde::{Deserialize, Serialize};
use slint::{ModelRc, SharedString, StandardListViewItem, VecModel};
use tracing::{error, info, warn};
use url::Url;

// ── saved session ────────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct Config {
    server_url: String,
    user_id: String,
    token: String,
}

fn config_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".config")
        });
    base.join("fjord").join("config.json")
}

fn load_config() -> Option<Config> {
    let path = config_path();
    let data = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let _ = std::fs::write(&path, json);
    }
}

// ── shared app state ─────────────────────────────────────────────────────────

struct AppState {
    client: Option<Arc<JellyfinClient>>,
    all_items: Vec<MediaItem>,
    filtered_items: Vec<MediaItem>,
}

impl AppState {
    fn new() -> Self {
        Self {
            client: None,
            all_items: vec![],
            filtered_items: vec![],
        }
    }

    fn apply_filter(&mut self, query: &str) {
        if query.is_empty() {
            self.filtered_items = self.all_items.clone();
        } else {
            let q = query.to_lowercase();
            self.filtered_items = self
                .all_items
                .iter()
                .filter(|item| item.display_name().to_lowercase().contains(&q))
                .cloned()
                .collect();
        }
    }
}

// ── helpers ───────────────────────────────────────────────────────────────────

/// Build a Slint list model from display-name strings.
/// Must be called on the UI thread (ModelRc wraps Rc and is !Send).
fn to_slint_model(names: Vec<String>) -> ModelRc<StandardListViewItem> {
    let items: Vec<StandardListViewItem> = names
        .into_iter()
        .map(|name| {
            let mut entry = StandardListViewItem::default();
            entry.text = SharedString::from(name.as_str());
            entry
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

fn display_names(items: &[MediaItem]) -> Vec<String> {
    items.iter().map(|i| i.display_name()).collect()
}

// ── entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();

    // Phase 2 compatibility: direct-play path from CLI arg
    let args: Vec<String> = std::env::args().collect();
    if let Some(url) = args.get(1) {
        info!("direct play: {}", url);
        return fjord_player::Player::play(url)?.wait();
    }

    // Tokio runtime for API calls (UI stays on main thread)
    let rt = tokio::runtime::Runtime::new()?;

    let window = MainWindow::new()?;
    let state = Arc::new(Mutex::new(AppState::new()));

    // ── auto-login from saved config ──────────────────────────────────────────
    if let Some(cfg) = load_config() {
        if let Ok(server_url) = Url::parse(&cfg.server_url) {
            let client = Arc::new(JellyfinClient::new(server_url, cfg.user_id, cfg.token));
            state.lock().unwrap().client = Some(Arc::clone(&client));

            let window_weak = window.as_weak();
            let state2 = Arc::clone(&state);

            rt.spawn(async move {
                info!("auto-login: fetching items");
                let ww_progress = window_weak.clone();
                match client.get_all_items(move |n| {
                    let ww = ww_progress.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() {
                            w.set_status(SharedString::from(format!("Loading… {n}")));
                        }
                    });
                }).await {
                    Ok(items) => {
                        let mut s = state2.lock().unwrap();
                        s.all_items = items;
                        s.apply_filter("");
                        let names = display_names(&s.filtered_items);
                        drop(s);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = window_weak.upgrade() {
                                w.set_media_items(to_slint_model(names));
                                w.set_show_login(false);
                                w.set_status(SharedString::from(""));
                            }
                        });
                    }
                    Err(e) => {
                        warn!("auto-login failed: {:#}", e);
                        // Fall through to login screen — saved token may have expired
                    }
                }
            });
        }
    }

    // ── login callback ────────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();

        window.on_do_login(move |server, user, pass| {
            let server = server.to_string();
            let user = user.to_string();
            let pass = pass.to_string();
            let state = Arc::clone(&state);
            let window_weak = window_weak.clone();

            // Show "Connecting…" while the request is in flight
            if let Some(w) = window_weak.upgrade() {
                w.set_status(SharedString::from("Connecting…"));
            }

            rt_handle.spawn(async move {
                let result: Result<()> = async {
                    let server_url = Url::parse(&server)?;
                    let http = reqwest::Client::new();
                    let auth = fjord_api::authenticate(&http, &server_url, &user, &pass).await?;

                    info!("authenticated as {}", auth.user.name);

                    let client = Arc::new(JellyfinClient::new(
                        server_url.clone(),
                        auth.user.id.clone(),
                        auth.access_token.clone(),
                    ));

                    save_config(&Config {
                        server_url: server_url.to_string(),
                        user_id: auth.user.id,
                        token: auth.access_token,
                    });

                    let ww_p = window_weak.clone();
                    let items = client.get_all_items(move |n| {
                        let ww = ww_p.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww.upgrade() {
                                w.set_status(SharedString::from(format!("Loading… {n}")));
                            }
                        });
                    }).await?;
                    info!("loaded {} items", items.len());

                    let mut s = state.lock().unwrap();
                    s.client = Some(client);
                    s.all_items = items;
                    s.apply_filter("");
                    let names = display_names(&s.filtered_items);
                    drop(s);

                    let ww = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() {
                            w.set_media_items(to_slint_model(names));
                            w.set_show_login(false);
                            w.set_status(SharedString::from(""));
                        }
                    });

                    Ok(())
                }
                .await;

                if let Err(e) = result {
                    error!("login failed: {:#}", e);
                    let msg = format!("{:#}", e);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = window_weak.upgrade() {
                            w.set_status(SharedString::from(msg));
                        }
                    });
                }
            });
        });
    }

    // ── filter callback ───────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();

        window.on_filter_changed(move |query| {
            // filter callback runs on the UI thread — can build ModelRc directly
            let mut s = state.lock().unwrap();
            s.apply_filter(&query);
            let names = display_names(&s.filtered_items);
            drop(s);
            if let Some(w) = window_weak.upgrade() {
                w.set_media_items(to_slint_model(names));
            }
        });
    }

    // ── play callback ─────────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();

        window.on_play_item(move |idx| {
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else {
                return;
            };
            let Some(item) = s.filtered_items.get(idx as usize) else {
                return;
            };
            let item_id = item.id.clone();
            let play_url = client.direct_play_url(&item_id);
            drop(s);

            info!("playing {} — {}", item_id, play_url);

            // Do NOT hide the Slint window: hiding the only visible window exits the
            // Slint event loop, killing the tokio runtime before mpv starts.
            // mpv opens fullscreen and covers the app window instead.
            rt_handle.spawn(async move {
                let _ = client.report_playback_start(&item_id).await;

                let url = play_url.clone();
                let _ = tokio::task::spawn_blocking(move || {
                    fjord_player::Player::play(&url).and_then(|p| p.wait())
                })
                .await;

                let _ = client.report_playback_stopped(&item_id, 0).await;
            });

            // window_weak kept for potential future use (e.g. wid embedding)
            let _ = window_weak;
        });
    }

    window.run()?;
    Ok(())
}
