// ── fjord-app · config.rs ────────────────────────────────────────────────────
//   default_* fns   serde defaults for Config string fields
//   Config          persisted JSON: server, user, token, device_id, all settings
//   FjordState      runtime app state: config (auth + all settings, canonical),
//                   client, library vecs, filtered lists, series cache, keybindings.
//                   Adding a setting: add to Config only — FjordState.config is the copy.
//                   movies_fetched: true after first network fetch (guards re-fetch)
//                   next_ep_pending moved to VideoState — cleared automatically on start_playback
//   path helpers    config_path, poster_cache_path, backdrop_cache_path, keybindings_path
//   config I/O      load_config, save_config, ensure_device_id
//   keybindings I/O load_keybindings, save_keybindings
//   fmt_resume_label  format resume position as "1h 23m 45s"
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;
use std::time::Instant;

use fjord_api::{models::MediaItem, JellyfinClient};
use fjord_player::PlayerConfig;
use serde::{Deserialize, Serialize};

use crate::keys::{Keybindings, default_keybindings};

pub(crate) fn default_hwdec()        -> String { "auto".into()       }
pub(crate) fn default_video_sync()   -> String { "audio".into()      }
pub(crate) fn default_tscale()       -> String { "oversample".into() }
pub(crate) fn default_tone_mapping() -> String { "auto".into()       }
fn default_sub_enabled()             -> bool   { true                }

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Config {
    pub server_url: String,
    pub user_id:    String,
    pub token:      String,
    #[serde(default)] pub device_id: String,

    #[serde(default)]                         pub audio_spdif:           bool,
    #[serde(default = "default_hwdec")]       pub hwdec:                 String,
    #[serde(default)]                         pub vf:                    String,
    #[serde(default = "default_video_sync")]  pub video_sync:            String,
    #[serde(default)]                         pub opengl_early_flush:    bool,
    #[serde(default)]                         pub video_latency_hacks:   bool,
    #[serde(default)]                         pub interpolation:         bool,
    #[serde(default = "default_tscale")]      pub tscale:                String,
    #[serde(default = "default_tone_mapping")]pub tone_mapping:          String,
    #[serde(default)]                         pub target_colorspace_hint:bool,
    #[serde(default)]                         pub deinterlace:           bool,
    #[serde(default)]                         pub cache_size_mb:         u32,
    #[serde(default)]                         pub video_behind:          bool,
    #[serde(default)]                         pub launch_fullscreen:     bool,
    #[serde(default = "default_sub_enabled")] pub sub_enabled:           bool,
    #[serde(default)]                         pub sub_lang:              String,
    #[serde(default)]                         pub sub_lang2:             String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: String::new(), user_id: String::new(),
            token: String::new(),     device_id: String::new(),
            audio_spdif: false, opengl_early_flush: false, video_latency_hacks: false,
            interpolation: false, target_colorspace_hint: false, deinterlace: false,
            video_behind: false, launch_fullscreen: false, cache_size_mb: 0,
            sub_enabled: true, sub_lang: String::new(), sub_lang2: String::new(),
            hwdec:        default_hwdec(),
            video_sync:   default_video_sync(),
            tscale:       default_tscale(),
            tone_mapping: default_tone_mapping(),
            vf:           String::new(),
        }
    }
}

pub(crate) fn config_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".config")
        });
    base.join("fjord").join("config.json")
}


pub(crate) fn poster_cache_path(item_id: &str) -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("fjord").join("posters").join(item_id)
}

pub(crate) fn backdrop_cache_path(item_id: &str) -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("fjord").join("backdrops").join(item_id)
}

pub(crate) fn fmt_resume_label(secs: f64) -> String {
    let s = secs as u64;
    let h = s / 3600; let m = (s % 3600) / 60; let s = s % 60;
    if h > 0 { format!("Resume from {}:{:02}:{:02}", h, m, s) }
    else { format!("Resume from {}:{:02}", m, s) }
}

pub(crate) fn load_config() -> Option<Config> {
    let data = std::fs::read_to_string(config_path()).ok()?;
    serde_json::from_str(&data).ok()
}

pub(crate) fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string_pretty(cfg) { let _ = std::fs::write(&path, json); }
}

pub(crate) fn ensure_device_id(cfg: &mut Config) {
    if !cfg.device_id.is_empty() { return; }
    cfg.device_id = std::fs::read_to_string("/proc/sys/kernel/random/uuid")
        .unwrap_or_default()
        .trim()
        .to_string();
    if cfg.device_id.is_empty() {
        cfg.device_id = format!("fjord-{:016x}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    }
    save_config(cfg);
    tracing::info!("generated device id: {}", cfg.device_id);
}

pub(crate) fn keybindings_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".config")
        });
    base.join("fjord").join("keybindings.json")
}

/// Load keybindings from `~/.config/fjord/keybindings.json`.
/// The file is loaded as-is (no default merge) so that explicit removals persist.
/// Missing or unparseable file → compiled-in defaults.
pub(crate) fn load_keybindings() -> Keybindings {
    let Ok(data) = std::fs::read_to_string(keybindings_path()) else {
        return default_keybindings();
    };
    serde_json::from_str(&data).unwrap_or_else(|e| {
        tracing::warn!("keybindings.json parse error: {e:#} — using defaults");
        default_keybindings()
    })
}

/// Save the full effective keybindings to `~/.config/fjord/keybindings.json`.
pub(crate) fn save_keybindings(kb: &Keybindings) {
    let path = keybindings_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string_pretty(kb) {
        let _ = std::fs::write(&path, json);
    }
}

// ── app state (library + settings) ───────────────────────────────────────────

pub(crate) struct FjordState {
    pub config:               Config,          // authoritative settings + auth; saved on change
    pub client:               Option<Arc<JellyfinClient>>,
    pub keybindings:          Keybindings,
    pub all_movies:           Vec<MediaItem>,
    pub all_series:           Vec<MediaItem>,
    pub movies_fetched:       bool,
    pub filtered_items:       Vec<MediaItem>,
    pub series_open_id:       String,
    pub series_season_ids:    Vec<String>,
    pub series_episode_items: Vec<MediaItem>,
    pub last_nw_mov_refresh:  Option<Instant>,
    pub last_nw_tv_refresh:   Option<Instant>,
}

impl FjordState {
    pub(crate) fn new() -> Self {
        Self {
            config: Config::default(),
            client: None, keybindings: load_keybindings(),
            all_movies: vec![], all_series: vec![], movies_fetched: false, filtered_items: vec![],
            series_open_id: String::new(), series_season_ids: vec![], series_episode_items: vec![],
            last_nw_mov_refresh: None,
            last_nw_tv_refresh: None,
        }
    }

    pub(crate) fn player_config(&self) -> PlayerConfig {
        let c = &self.config;
        PlayerConfig {
            audio_spdif:            c.audio_spdif,
            hwdec:                  c.hwdec.clone(),
            vf:                     c.vf.clone(),
            video_sync:             c.video_sync.clone(),
            opengl_early_flush:     c.opengl_early_flush,
            video_latency_hacks:    c.video_latency_hacks,
            interpolation:          c.interpolation,
            tscale:                 c.tscale.clone(),
            tone_mapping:           c.tone_mapping.clone(),
            target_colorspace_hint: c.target_colorspace_hint,
            deinterlace:            c.deinterlace,
            cache_size_mb:          c.cache_size_mb,
            start_position_secs:    None,
        }
    }

    // Update user state (played / is_favorite) in all canonical Rust-side vecs.
    // Call this before patching Slint models so any model rebuild reads correct data.
    pub(crate) fn update_item_user_state(&mut self, id: &str, played: Option<bool>, fav: Option<bool>) {
        for list in [&mut self.all_movies, &mut self.all_series, &mut self.filtered_items] {
            for item in list.iter_mut() {
                if item.id == id {
                    if let Some(p) = played { item.user_data.played       = p; }
                    if let Some(f) = fav    { item.user_data.is_favorite  = f; }
                }
            }
        }
    }

}
