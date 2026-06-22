// ── fjord-app · config.rs ────────────────────────────────────────────────────
//   default_* fns   serde defaults for Config string fields
//   Config          persisted JSON: server, user, token, device_id, all settings;
//                   skip_*_mode: "always-skip"|"ask"|"ask-timed"|"never-skip";
//                   skip_*_secs: auto-skip countdown (ask-timed); credits secs for Up Next banner
//   FjordState      runtime app state: config (auth + all settings, canonical),
//                   client, library vecs, filtered lists, series cache, keybindings.
//                   audio_devices: Vec<(name, description)> fetched at startup from mpv.
//                   movie_collections: HashMap<movie_id, (boxset_id, boxset_name)> built in background.
//                   series_episode_cache: HashMap<season_id, Vec<MediaItem>> avoids re-fetching
//                     already-seen seasons; cleared when a new series is opened.
//                   series_season_generation: incremented on each season switch; async tasks compare
//                     on completion to discard stale results from rapid navigation.
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
fn default_true()                    -> bool   { true                }
fn default_deinterlace()             -> String { "no".into()         }
fn default_skip_mode()               -> String { "ask".into()        }
fn default_skip_secs()               -> u32    { 8                   }
fn default_credits_secs()            -> u32    { 30                  }

// Migrate old bool (false/true) stored by earlier versions to "no"/"yes".
// Option<> wrapper accepts JSON null without error (maps to "no").
fn deser_deinterlace<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum BoolOrStr { Bool(bool), Str(String) }
    Ok(match Option::<BoolOrStr>::deserialize(d)? {
        Some(BoolOrStr::Bool(b)) => if b { "yes" } else { "no" }.into(),
        Some(BoolOrStr::Str(s))  => s,
        None                     => "no".into(),
    })
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Config {
    pub server_url: String,
    pub user_id:    String,
    pub token:      String,
    #[serde(default)] pub device_id: String,

    #[serde(default)]                         pub audio_spdif:           bool,
    #[serde(default = "default_true")]        pub spdif_ac3:             bool,
    #[serde(default = "default_true")]        pub spdif_eac3:            bool,
    #[serde(default = "default_true")]        pub spdif_dts:             bool,
    #[serde(default = "default_true")]        pub spdif_dts_hd:          bool,
    #[serde(default = "default_true")]        pub spdif_truehd:          bool,
    #[serde(default = "default_hwdec")]       pub hwdec:                 String,
    #[serde(default)]                         pub vf:                    String,
    #[serde(default = "default_video_sync")]  pub video_sync:            String,
    #[serde(default)]                         pub opengl_early_flush:    bool,
    #[serde(default)]                         pub video_latency_hacks:   bool,
    #[serde(default)]                         pub interpolation:         bool,
    #[serde(default = "default_tscale")]      pub tscale:                String,
    #[serde(default = "default_tone_mapping")]pub tone_mapping:          String,
    #[serde(default)]                         pub target_colorspace_hint:bool,
    #[serde(default = "default_deinterlace", deserialize_with = "deser_deinterlace")]
                                              pub deinterlace:           String,
    #[serde(default)]                         pub cache_size_mb:         u32,
    #[serde(default)]                         pub video_behind:          bool,
    #[serde(default)]                         pub launch_fullscreen:     bool,
    #[serde(default = "default_true")]         pub sub_enabled:           bool,
    #[serde(default)]                         pub sub_lang:              String,
    #[serde(default)]                         pub sub_lang2:             String,
    #[serde(default)]                         pub audio_lang:            String,
    #[serde(default)]                         pub audio_device:          String,
    #[serde(default)]                         pub alsa_irq_scheduling:   bool,

    // ── Intro Skipper skip modes ─────────────────────────────────────────────
    // "always-skip" | "ask" | "ask-timed" | "never-skip"  (Intro/Recap/Preview/Commercial)
    // "always-skip" | "ask" | "never-skip"                 (Credits)
    #[serde(default = "default_skip_mode")] pub skip_intro_mode:      String,
    #[serde(default = "default_skip_secs")] pub skip_intro_secs:      u32,
    #[serde(default = "default_skip_mode")] pub skip_recap_mode:      String,
    #[serde(default = "default_skip_secs")] pub skip_recap_secs:      u32,
    #[serde(default = "default_skip_mode")] pub skip_preview_mode:    String,
    #[serde(default = "default_skip_secs")] pub skip_preview_secs:    u32,
    #[serde(default = "default_skip_mode")] pub skip_commercial_mode: String,
    #[serde(default = "default_skip_secs")] pub skip_commercial_secs: u32,
    #[serde(default = "default_skip_mode")]    pub skip_credits_mode:    String,
    #[serde(default = "default_credits_secs")] pub skip_credits_secs:    u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server_url: String::new(), user_id: String::new(),
            token: String::new(),     device_id: String::new(),
            audio_spdif: false,
            spdif_ac3: true, spdif_eac3: true, spdif_dts: true, spdif_dts_hd: true, spdif_truehd: true,
            opengl_early_flush: false, video_latency_hacks: false,
            interpolation: false, target_colorspace_hint: false, deinterlace: "no".into(),
            video_behind: false, launch_fullscreen: false, cache_size_mb: 0,
            sub_enabled: true, sub_lang: String::new(), sub_lang2: String::new(), audio_lang: String::new(),
            audio_device: String::new(),
            alsa_irq_scheduling: false,
            skip_intro_mode:      default_skip_mode(),
            skip_intro_secs:      8,
            skip_recap_mode:      default_skip_mode(),
            skip_recap_secs:      8,
            skip_preview_mode:    default_skip_mode(),
            skip_preview_secs:    8,
            skip_commercial_mode: default_skip_mode(),
            skip_commercial_secs: 8,
            skip_credits_mode:    default_skip_mode(),
            skip_credits_secs:    30,
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
    pub series_open_id:         String,
    pub series_season_ids:      Vec<String>,
    pub series_episode_items:   Vec<MediaItem>,
    pub series_episode_cache:   std::collections::HashMap<String, Vec<MediaItem>>,
    pub series_season_generation: u64,
    pub last_nw_mov_refresh:    Option<Instant>,
    pub last_nw_tv_refresh:   Option<Instant>,
    pub audio_devices:        Vec<(String, String)>,  // (mpv name, description)
    pub movie_collections:    std::collections::HashMap<String, (String, String)>, // movie_id → (boxset_id, boxset_name)
}

impl FjordState {
    pub(crate) fn new() -> Self {
        Self {
            config: Config::default(),
            client: None, keybindings: load_keybindings(),
            all_movies: vec![], all_series: vec![], movies_fetched: false, filtered_items: vec![],
            series_open_id: String::new(), series_season_ids: vec![], series_episode_items: vec![],
            series_episode_cache: std::collections::HashMap::new(), series_season_generation: 0,
            last_nw_mov_refresh: None,
            last_nw_tv_refresh: None,
            audio_devices: vec![],
            movie_collections: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn player_config(&self) -> PlayerConfig {
        let c = &self.config;
        PlayerConfig {
            audio_device:           c.audio_device.clone(),
            audio_spdif_formats:    if c.audio_spdif {
                                        let mut f = Vec::new();
                                        if c.spdif_ac3    { f.push("ac3"); }
                                        if c.spdif_eac3   { f.push("eac3"); }
                                        if c.spdif_dts    { f.push("dts"); }
                                        if c.spdif_dts_hd { f.push("dts-hd"); }
                                        if c.spdif_truehd { f.push("truehd"); }
                                        f.join(",")
                                    } else { String::new() },
            hwdec:                  c.hwdec.clone(),
            vf:                     c.vf.clone(),
            video_sync:             c.video_sync.clone(),
            opengl_early_flush:     c.opengl_early_flush,
            video_latency_hacks:    c.video_latency_hacks,
            interpolation:          c.interpolation,
            tscale:                 c.tscale.clone(),
            tone_mapping:           c.tone_mapping.clone(),
            target_colorspace_hint: c.target_colorspace_hint,
            deinterlace:            c.deinterlace.clone(),
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
