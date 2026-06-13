use std::sync::Arc;
use std::time::{Duration, Instant};

use fjord_api::{models::MediaItem, JellyfinClient};
use fjord_player::PlayerConfig;
use serde::{Deserialize, Serialize};

pub(crate) fn default_hwdec()        -> String { "auto".into()       }
pub(crate) fn default_gpu_api()      -> String { "auto".into()       }
pub(crate) fn default_video_sync()   -> String { "audio".into()      }
pub(crate) fn default_tscale()       -> String { "oversample".into() }
pub(crate) fn default_tone_mapping() -> String { "auto".into()       }

#[derive(Serialize, Deserialize, Default)]
pub(crate) struct Config {
    pub server_url: String,
    pub user_id:    String,
    pub token:      String,
    #[serde(default)] pub device_id: String,

    #[serde(default)]                         pub audio_spdif:           bool,
    #[serde(default = "default_hwdec")]       pub hwdec:                 String,
    #[serde(default)]                         pub hwdec_image_format:    String,
    #[serde(default)]                         pub vf:                    String,
    #[serde(default = "default_gpu_api")]     pub gpu_api:               String,
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

pub(crate) fn item_cache_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("fjord").join("items.json")
}

pub(crate) fn load_item_cache() -> Option<Vec<MediaItem>> {
    let data = std::fs::read_to_string(item_cache_path()).ok()?;
    serde_json::from_str(&data).ok()
}

pub(crate) fn save_item_cache(items: &[MediaItem]) {
    let path = item_cache_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string(items) { let _ = std::fs::write(&path, json); }
}

pub(crate) fn is_item_cache_fresh() -> bool {
    let path = item_cache_path();
    let Ok(meta) = std::fs::metadata(&path) else { return false; };
    let Ok(modified) = meta.modified() else { return false; };
    let Ok(age) = modified.elapsed() else { return false; };
    age < Duration::from_secs(6 * 3600)
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

pub(crate) fn non_empty(s: &str, fallback: String) -> String {
    if s.is_empty() { fallback } else { s.to_string() }
}

// ── app state (library + settings) ───────────────────────────────────────────

pub(crate) struct FjordState {
    pub client:               Option<Arc<JellyfinClient>>,
    pub media_raw:            Vec<MediaItem>,
    pub all_movies:           Vec<MediaItem>,
    pub all_series:           Vec<MediaItem>,
    pub filtered_items:       Vec<MediaItem>,
    pub nav_filter:           usize,
    pub text_query:           String,
    pub series_open_id:       String,
    pub series_season_ids:    Vec<String>,
    pub series_episode_items: Vec<MediaItem>,
    pub next_ep_pending:      Option<MediaItem>,
    pub last_nw_mov_refresh:  Option<Instant>,
    pub last_nw_tv_refresh:   Option<Instant>,
    pub audio_spdif:            bool,
    pub hwdec:                  String,
    pub hwdec_image_format:     String,
    pub vf:                     String,
    pub gpu_api:                String,
    pub video_sync:             String,
    pub opengl_early_flush:     bool,
    pub video_latency_hacks:    bool,
    pub interpolation:          bool,
    pub tscale:                 String,
    pub tone_mapping:           String,
    pub target_colorspace_hint: bool,
    pub deinterlace:            bool,
    pub cache_size_mb:          u32,
    pub video_behind:           bool,
    pub launch_fullscreen:      bool,
}

impl FjordState {
    pub(crate) fn new() -> Self {
        let d = PlayerConfig::default();
        Self {
            client: None, media_raw: vec![], all_movies: vec![], all_series: vec![], filtered_items: vec![],
            nav_filter: 0, text_query: String::new(),
            series_open_id: String::new(), series_season_ids: vec![], series_episode_items: vec![],
            next_ep_pending: None,
            last_nw_mov_refresh: None,
            last_nw_tv_refresh: None,
            audio_spdif:            d.audio_spdif,
            hwdec:                  d.hwdec,
            hwdec_image_format:     d.hwdec_image_format,
            vf:                     d.vf,
            gpu_api:                d.gpu_api,
            video_sync:             d.video_sync,
            opengl_early_flush:     d.opengl_early_flush,
            video_latency_hacks:    d.video_latency_hacks,
            interpolation:          d.interpolation,
            tscale:                 d.tscale,
            tone_mapping:           d.tone_mapping,
            target_colorspace_hint: d.target_colorspace_hint,
            deinterlace:            d.deinterlace,
            cache_size_mb:          d.cache_size_mb,
            video_behind:           false,
            launch_fullscreen:      false,
        }
    }

    pub(crate) fn apply_from_config(&mut self, cfg: &Config) {
        self.audio_spdif            = cfg.audio_spdif;
        self.hwdec                  = non_empty(&cfg.hwdec,        default_hwdec());
        self.hwdec_image_format     = cfg.hwdec_image_format.clone();
        self.vf                     = cfg.vf.clone();
        self.gpu_api                = non_empty(&cfg.gpu_api,      default_gpu_api());
        self.video_sync             = non_empty(&cfg.video_sync,   default_video_sync());
        self.opengl_early_flush     = cfg.opengl_early_flush;
        self.video_latency_hacks    = cfg.video_latency_hacks;
        self.interpolation          = cfg.interpolation;
        self.tscale                 = non_empty(&cfg.tscale,       default_tscale());
        self.tone_mapping           = non_empty(&cfg.tone_mapping, default_tone_mapping());
        self.target_colorspace_hint = cfg.target_colorspace_hint;
        self.deinterlace            = cfg.deinterlace;
        self.cache_size_mb          = cfg.cache_size_mb;
        self.video_behind           = cfg.video_behind;
        self.launch_fullscreen      = cfg.launch_fullscreen;
    }

    pub(crate) fn player_config(&self) -> PlayerConfig {
        PlayerConfig {
            audio_spdif:            self.audio_spdif,
            hwdec:                  self.hwdec.clone(),
            hwdec_image_format:     self.hwdec_image_format.clone(),
            vf:                     self.vf.clone(),
            gpu_api:                self.gpu_api.clone(),
            video_sync:             self.video_sync.clone(),
            opengl_early_flush:     self.opengl_early_flush,
            video_latency_hacks:    self.video_latency_hacks,
            interpolation:          self.interpolation,
            tscale:                 self.tscale.clone(),
            tone_mapping:           self.tone_mapping.clone(),
            target_colorspace_hint: self.target_colorspace_hint,
            deinterlace:            self.deinterlace,
            cache_size_mb:          self.cache_size_mb,
            start_position_secs:    None,
        }
    }

    pub(crate) fn apply_filter(&mut self, query: &str) { self.text_query = query.to_string(); self.refilter(); }
    pub(crate) fn apply_nav(&mut self, nav: usize)     { self.nav_filter = nav;               self.refilter(); }

    pub(crate) fn refilter(&mut self) {
        let q = self.text_query.to_lowercase();
        self.filtered_items = self.media_raw.iter()
            .chain(self.all_series.iter())
            .filter(|item| {
                let type_ok = match self.nav_filter {
                    1 => item.item_type == "Movie",
                    2 => item.item_type == "Episode" || item.item_type == "Series",
                    _ => true,
                };
                let text_ok = q.is_empty() || item.display_name().to_lowercase().contains(&q);
                type_ok && text_ok
            }).cloned().collect();
    }
}
