slint::include_modules!();

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use fjord_api::{models::{IntroTimestamps, MediaItem}, JellyfinClient};
use fjord_player::{MpvRenderCtx, Player, PlayerConfig, PollResult, TrackInfo};
use serde::{Deserialize, Serialize};
use slint::{Model, ModelRc, SharedString, StandardListViewItem, VecModel};
use tracing::{debug, error, info, warn};
use url::Url;

fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .map(|s| s.as_u16() == 401)
        .unwrap_or(false)
}

// ── saved session + settings ──────────────────────────────────────────────────

fn default_hwdec()        -> String { "auto".into()       }
fn default_gpu_api()      -> String { "auto".into()       }
fn default_video_sync()   -> String { "audio".into()      }
fn default_tscale()       -> String { "oversample".into() }
fn default_tone_mapping() -> String { "auto".into()       }

#[derive(Serialize, Deserialize, Default)]
struct Config {
    server_url: String,
    user_id:    String,
    token:      String,
    #[serde(default)] device_id: String,

    #[serde(default)]                         audio_spdif:           bool,
    #[serde(default = "default_hwdec")]       hwdec:                 String,
    #[serde(default)]                         hwdec_image_format:    String,
    #[serde(default)]                         vf:                    String,
    #[serde(default = "default_gpu_api")]     gpu_api:               String,
    #[serde(default = "default_video_sync")]  video_sync:            String,
    #[serde(default)]                         opengl_early_flush:    bool,
    #[serde(default)]                         video_latency_hacks:   bool,
    #[serde(default)]                         interpolation:         bool,
    #[serde(default = "default_tscale")]      tscale:                String,
    #[serde(default = "default_tone_mapping")]tone_mapping:          String,
    #[serde(default)]                         target_colorspace_hint:bool,
    #[serde(default)]                         deinterlace:           bool,
    #[serde(default)]                         cache_size_mb:         u32,
    #[serde(default)]                         video_behind:          bool,
    #[serde(default)]                         launch_fullscreen:     bool,
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

fn item_cache_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("fjord").join("items.json")
}

fn load_item_cache() -> Option<Vec<MediaItem>> {
    let data = std::fs::read_to_string(item_cache_path()).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_item_cache(items: &[MediaItem]) {
    let path = item_cache_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string(items) { let _ = std::fs::write(&path, json); }
}

fn is_item_cache_fresh() -> bool {
    let path = item_cache_path();
    let Ok(meta) = std::fs::metadata(&path) else { return false; };
    let Ok(modified) = meta.modified() else { return false; };
    let Ok(age) = modified.elapsed() else { return false; };
    age < Duration::from_secs(6 * 3600)
}

fn poster_cache_path(item_id: &str) -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("fjord").join("posters").join(item_id)
}

async fn fetch_poster_cached(client: &JellyfinClient, item_id: &str) -> Option<Vec<u8>> {
    let path = poster_cache_path(item_id);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return tokio::fs::read(&path).await.ok();
    }
    let bytes = client.fetch_poster_bytes(item_id).await.ok()?;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&path, &bytes).await;
    Some(bytes)
}

fn backdrop_cache_path(item_id: &str) -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("fjord").join("backdrops").join(item_id)
}

async fn fetch_backdrop_cached(client: &JellyfinClient, item_id: &str) -> Option<Vec<u8>> {
    let path = backdrop_cache_path(item_id);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return tokio::fs::read(&path).await.ok();
    }
    let bytes = client.fetch_backdrop_bytes(item_id).await.ok()?;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&path, &bytes).await;
    Some(bytes)
}

fn fmt_resume_label(secs: f64) -> String {
    let s = secs as u64;
    let h = s / 3600; let m = (s % 3600) / 60; let s = s % 60;
    if h > 0 { format!("Resume from {}:{:02}:{:02}", h, m, s) }
    else { format!("Resume from {}:{:02}", m, s) }
}

fn load_config() -> Option<Config> {
    let data = std::fs::read_to_string(config_path()).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string_pretty(cfg) { let _ = std::fs::write(&path, json); }
}

fn ensure_device_id(cfg: &mut Config) {
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
    info!("generated device id: {}", cfg.device_id);
}

// ── app state (library + settings) ───────────────────────────────────────────

struct AppState {
    client:               Option<Arc<JellyfinClient>>,
    media_raw:            Vec<MediaItem>,  // Movie + Episode from paginated fetch
    all_movies:           Vec<MediaItem>,  // Movie only — drives the movies grid
    all_series:           Vec<MediaItem>,  // Series only — drives the TV grid
    filtered_items:       Vec<MediaItem>,  // filtered view of media_raw + all_series for browse
    nav_filter:           usize,
    text_query:           String,
    // Series drill-down state
    series_open_id:       String,
    series_season_ids:    Vec<String>,
    series_episode_items: Vec<MediaItem>,
    // Auto-advance: pending next episode (None = no countdown / cancelled)
    next_ep_pending:      Option<MediaItem>,
    // Last time the not-watched rows were refreshed (None = never)
    last_nw_mov_refresh:      Option<Instant>,
    last_nw_tv_refresh:   Option<Instant>,
    // player settings kept in sync with the Settings screen
    audio_spdif:            bool,
    hwdec:                  String,
    hwdec_image_format:     String,
    vf:                     String,
    gpu_api:                String,
    video_sync:             String,
    opengl_early_flush:     bool,
    video_latency_hacks:    bool,
    interpolation:          bool,
    tscale:                 String,
    tone_mapping:           String,
    target_colorspace_hint: bool,
    deinterlace:            bool,
    cache_size_mb:          u32,
    video_behind:           bool,
    launch_fullscreen:      bool,
}

impl AppState {
    fn new() -> Self {
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

    fn apply_from_config(&mut self, cfg: &Config) {
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

    fn player_config(&self) -> PlayerConfig {
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

    fn apply_filter(&mut self, query: &str) { self.text_query = query.to_string(); self.refilter(); }
    fn apply_nav(&mut self, nav: usize)     { self.nav_filter = nav;               self.refilter(); }

    fn refilter(&mut self) {
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

fn non_empty(s: &str, fallback: String) -> String {
    if s.is_empty() { fallback } else { s.to_string() }
}

// ── video state (player + render context + GL objects) ────────────────────────

struct VideoState {
    player:     Option<Player>,
    render_ctx: Option<MpvRenderCtx>,
    // Two FBO+texture pairs — we alternate each frame so Slint sees a
    // different texture ID every frame and always re-renders the Image.
    fbos:       [u32; 2],
    textures:   [u32; 2],
    fbo_w:      u32,
    fbo_h:      u32,
    back:       usize, // index of the buffer mpv renders into next
    // metadata for reporting Jellyfin playback events
    item_id:          Option<String>,
    playing_series_id: Option<String>,
    client:           Option<Arc<JellyfinClient>>,
    play_start:     Option<Instant>,
    decoder_logged:     bool,
    tracks_loaded:      bool,
    pos_tick:           u32,
    controls_idle_ticks: u32,
    intro_timestamps:    Option<IntroTimestamps>,
    intro_skip_shown:    bool,
    did_render:          bool,
}

impl Default for VideoState {
    fn default() -> Self {
        Self {
            player: None, render_ctx: None,
            fbos: [0; 2], textures: [0; 2],
            fbo_w: 0, fbo_h: 0, back: 0,
            item_id: None, playing_series_id: None, client: None,
            play_start: None, decoder_logged: false,
            tracks_loaded: false, pos_tick: 0,
            controls_idle_ticks: 0,
            intro_timestamps: None, intro_skip_shown: false,
            did_render: false,
        }
    }
}

// ── playback helpers ──────────────────────────────────────────────────────────

fn fmt_secs(secs: f64) -> SharedString {
    if secs <= 0.0 { return "0:00".into(); }
    let s = secs as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let s = s % 60;
    if h > 0 {
        SharedString::from(format!("{}:{:02}:{:02}", h, m, s).as_str())
    } else {
        SharedString::from(format!("{}:{:02}", m, s).as_str())
    }
}

fn build_track_model(tracks: &[TrackInfo], kind: &str) -> ModelRc<TrackEntry> {
    let entries: Vec<TrackEntry> = tracks.iter()
        .filter(|t| t.track_type == kind)
        .map(|t| {
            let mut label = String::new();
            if !t.lang.is_empty()  { label.push_str(&t.lang); }
            if !t.title.is_empty() {
                if !label.is_empty() { label.push(' '); }
                label.push_str(&t.title);
            }
            if !t.codec.is_empty() {
                label.push_str(&format!(" ({})", t.codec));
            }
            if label.is_empty() { label = format!("Track {}", t.id); }
            TrackEntry { id: t.id as i32, label: label.into() }
        })
        .collect();
    ModelRc::new(VecModel::from(entries))
}

// ── model helpers ─────────────────────────────────────────────────────────────

// Returns a Send-able pixel buffer rather than slint::Image (which is !Send).
// Callers must call Image::from_rgba8 on the UI thread.
fn decode_poster_buffer(bytes: &[u8]) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        img.as_raw(), w, h,
    ))
}

fn item_to_card_item(i: &MediaItem) -> CardItem {
    let mut h = CardItem::default();
    h.id             = SharedString::from(i.id.as_str());
    h.title          = SharedString::from(i.display_name().as_str());
    h.year           = i.production_year.unwrap_or(0) as i32;
    h.has_played     = i.user_data.played;
    h.resume_pct     = i.resume_pct();
    h.unplayed_count = i.user_data.unplayed_item_count;
    h
}

fn items_to_model(items: &[MediaItem]) -> ModelRc<CardItem> {
    ModelRc::new(VecModel::from(items.iter().map(item_to_card_item).collect::<Vec<_>>()))
}

fn push_section_model(window: &MainWindow, sec: usize, model: ModelRc<CardItem>) {
    match sec {
        0 => window.set_continue_watching(model),
        1 => window.set_next_up(model),
        2 => window.set_recently_added(model),
        3 => window.set_continue_watching_movies(model),
        4 => window.set_recently_added_movies(model),
        5 => window.set_not_watched_movies(model),
        6 => window.set_continue_watching_tv(model),
        7 => window.set_recently_added_tv(model),
        8 => window.set_not_watched_tv(model),
        9 => window.set_all_series(model),
        _ => {}
    }
}

fn spawn_series_poster_loading(
    client:      Arc<JellyfinClient>,
    series:      Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    rt_handle.spawn(async move {
        use std::collections::HashSet;
        use std::sync::Arc as SArc;

        let meta: Vec<(String, String, i32, bool, f32, i32)> = series.iter()
            .map(|i| (i.id.clone(), i.display_name(), i.production_year.unwrap_or(0) as i32, i.user_data.played, i.resume_pct(), i.user_data.unplayed_item_count))
            .collect();
        let mut pending: HashSet<String> = meta.iter().map(|(id, _, _, _, _, _)| id.clone()).collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for (id, _, _, _, _, _) in &meta {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            let id     = id.clone();
            fetch_set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let bytes   = fetch_poster_cached(&*client, &id).await.map(SArc::new);
                (id, bytes)
            });
        }

        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();

        while let Some(res) = fetch_set.join_next().await {
            let Ok((id, bytes)) = res else { continue };
            if let Some(b) = bytes { poster_map.insert(id.clone(), b); }
            pending.remove(&id);
            if !pending.is_empty() { continue; }

            type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
            let decoded: Vec<(SharedString, SharedString, i32, bool, f32, i32, Option<Buf>)> =
                meta.iter().map(|(cid, title, year, played, rpct, upc)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (SharedString::from(cid.as_str()), SharedString::from(title.as_str()), *year, *played, *rpct, *upc, buf)
                }).collect();
            let ww = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let items: Vec<CardItem> = decoded.into_iter().map(|(id, title, year, played, rpct, upc, buf)| {
                        let mut h = CardItem::default();
                        h.id             = id;
                        h.title          = title;
                        h.year           = year;
                        h.has_played     = played;
                        h.resume_pct     = rpct;
                        h.unplayed_count = upc;
                        if let Some(spb) = buf { h.poster = slint::Image::from_rgba8(spb); h.has_poster = true; }
                        h
                    }).collect();
                    w.set_all_series(ModelRc::new(VecModel::from(items)));
                }
            });
        }
    });
}

fn spawn_movies_poster_loading(
    client:      Arc<JellyfinClient>,
    movies:      Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    rt_handle.spawn(async move {
        use std::collections::HashSet;
        use std::sync::Arc as SArc;

        let meta: Vec<(String, String, i32, bool, f32)> = movies.iter()
            .map(|i| (i.id.clone(), i.display_name(), i.production_year.unwrap_or(0) as i32, i.user_data.played, i.resume_pct()))
            .collect();
        let mut pending: HashSet<String> = meta.iter().map(|(id, _, _, _, _)| id.clone()).collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for (id, _, _, _, _) in &meta {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            let id     = id.clone();
            fetch_set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let bytes   = fetch_poster_cached(&*client, &id).await.map(SArc::new);
                (id, bytes)
            });
        }

        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();

        while let Some(res) = fetch_set.join_next().await {
            let Ok((id, bytes)) = res else { continue };
            if let Some(b) = bytes { poster_map.insert(id.clone(), b); }
            pending.remove(&id);
            if !pending.is_empty() { continue; }

            type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
            let decoded: Vec<(SharedString, SharedString, i32, bool, f32, Option<Buf>)> =
                meta.iter().map(|(cid, title, year, played, rpct)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (SharedString::from(cid.as_str()), SharedString::from(title.as_str()), *year, *played, *rpct, buf)
                }).collect();
            let ww = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let items: Vec<CardItem> = decoded.into_iter().map(|(id, title, year, played, rpct, buf)| {
                        let mut h = CardItem::default();
                        h.id         = id;
                        h.title      = title;
                        h.year       = year;
                        h.has_played = played;
                        h.resume_pct = rpct;
                        if let Some(spb) = buf { h.poster = slint::Image::from_rgba8(spb); h.has_poster = true; }
                        h
                    }).collect();
                    w.set_all_movies(ModelRc::new(VecModel::from(items)));
                }
            });
        }
    });
}

fn to_slint_model(names: Vec<String>) -> ModelRc<StandardListViewItem> {
    let items: Vec<StandardListViewItem> = names.into_iter().map(|name| {
        let mut e = StandardListViewItem::default();
        e.text = SharedString::from(name.as_str());
        e
    }).collect();
    ModelRc::new(VecModel::from(items))
}

fn display_names(items: &[MediaItem]) -> Vec<String> {
    items.iter().map(|i| i.display_name()).collect()
}


fn ss(s: &str) -> SharedString { SharedString::from(s) }

fn apply_settings_to_window(w: &MainWindow, s: &AppState) {
    w.set_settings_audio_spdif(s.audio_spdif);
    w.set_settings_hwdec(ss(&s.hwdec));
    w.set_settings_hwdec_image_format(ss(&s.hwdec_image_format));
    w.set_settings_vf(ss(&s.vf));
    w.set_settings_gpu_api(ss(&s.gpu_api));
    w.set_settings_video_sync(ss(&s.video_sync));
    w.set_settings_opengl_early_flush(s.opengl_early_flush);
    w.set_settings_video_latency_hacks(s.video_latency_hacks);
    w.set_settings_interpolation(s.interpolation);
    w.set_settings_tscale(ss(&s.tscale));
    w.set_settings_tone_mapping(ss(&s.tone_mapping));
    w.set_settings_target_colorspace_hint(s.target_colorspace_hint);
    w.set_settings_deinterlace(s.deinterlace);
    w.set_settings_cache_mb(s.cache_size_mb as i32);
    w.set_settings_video_behind(s.video_behind);
    w.set_settings_launch_fullscreen(s.launch_fullscreen);
}

fn read_settings_from_window(w: &MainWindow, s: &mut AppState) {
    s.audio_spdif            = w.get_settings_audio_spdif();
    s.hwdec                  = w.get_settings_hwdec().to_string();
    s.hwdec_image_format     = w.get_settings_hwdec_image_format().to_string();
    s.vf                     = w.get_settings_vf().to_string();
    s.gpu_api                = w.get_settings_gpu_api().to_string();
    s.video_sync             = w.get_settings_video_sync().to_string();
    s.opengl_early_flush     = w.get_settings_opengl_early_flush();
    s.video_latency_hacks    = w.get_settings_video_latency_hacks();
    s.interpolation          = w.get_settings_interpolation();
    s.tscale                 = w.get_settings_tscale().to_string();
    s.tone_mapping           = w.get_settings_tone_mapping().to_string();
    s.target_colorspace_hint = w.get_settings_target_colorspace_hint();
    s.deinterlace            = w.get_settings_deinterlace();
    s.cache_size_mb          = w.get_settings_cache_mb().max(0) as u32;
    s.video_behind           = w.get_settings_video_behind();
    s.launch_fullscreen      = w.get_settings_launch_fullscreen();
}

// ── home screen data ──────────────────────────────────────────────────────────

#[derive(Serialize, Deserialize, Default)]
struct HomeData {
    continue_watching:     Vec<MediaItem>,
    next_up:               Vec<MediaItem>,
    recently_added:        Vec<MediaItem>,
    recently_added_movies: Vec<MediaItem>,
    recently_added_tv:     Vec<MediaItem>,
    not_watched_movies:    Vec<MediaItem>,
    not_watched_tv:        Vec<MediaItem>,
}

fn home_cache_path() -> std::path::PathBuf {
    let base = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            let home = std::env::var("HOME").unwrap_or_default();
            std::path::PathBuf::from(home).join(".cache")
        });
    base.join("fjord").join("home.json")
}

fn load_home_cache() -> Option<HomeData> {
    let data = std::fs::read_to_string(home_cache_path()).ok()?;
    serde_json::from_str(&data).ok()
}

fn save_home_cache(hd: &HomeData) {
    let path = home_cache_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string(hd) { let _ = std::fs::write(&path, json); }
}

async fn fetch_home_data(client: &JellyfinClient) -> HomeData {
    let (cw, nu, ra, ram, rat, nwm, nwt) = tokio::join!(
        client.get_continue_watching(),
        client.get_next_up(),
        client.get_recently_added(Some("Series")),
        client.get_recently_added(Some("Movie")),
        client.get_recently_added(Some("Series")),
        client.get_unwatched(Some("Movie")),
        client.get_unwatched(Some("Series")),
    );
    HomeData {
        continue_watching:     cw.unwrap_or_else(|e|  { warn!("continue_watching: {:#}", e);     vec![] }),
        next_up:               nu.unwrap_or_else(|e|  { warn!("next_up: {:#}", e);               vec![] }),
        recently_added:        ra.unwrap_or_else(|e|  { warn!("recently_added: {:#}", e);        vec![] }),
        recently_added_movies: ram.unwrap_or_else(|e| { warn!("recently_added_movies: {:#}", e); vec![] }),
        recently_added_tv:     rat.unwrap_or_else(|e| { warn!("recently_added_tv: {:#}", e);     vec![] }),
        not_watched_movies:    nwm.unwrap_or_else(|e| { warn!("not_watched_movies: {:#}", e);    vec![] }),
        not_watched_tv:        nwt.unwrap_or_else(|e| { warn!("not_watched_tv: {:#}", e);        vec![] }),
    }
}

fn push_home_data(window: &MainWindow, hd: &HomeData) {
    let cw_movies: Vec<_> = hd.continue_watching.iter().filter(|i| i.item_type == "Movie").cloned().collect();
    let cw_tv:     Vec<_> = hd.continue_watching.iter().filter(|i| i.item_type == "Episode").cloned().collect();
    window.set_continue_watching(items_to_model(&hd.continue_watching));
    window.set_next_up(items_to_model(&hd.next_up));
    window.set_recently_added(items_to_model(&hd.recently_added));
    window.set_continue_watching_movies(items_to_model(&cw_movies));
    window.set_recently_added_movies(items_to_model(&hd.recently_added_movies));
    window.set_not_watched_movies(items_to_model(&hd.not_watched_movies));
    window.set_continue_watching_tv(items_to_model(&cw_tv));
    window.set_recently_added_tv(items_to_model(&hd.recently_added_tv));
    window.set_not_watched_tv(items_to_model(&hd.not_watched_tv));
}

fn home_data_sections(hd: &HomeData) -> [Vec<MediaItem>; 9] {
    let cw_movies = hd.continue_watching.iter().filter(|i| i.item_type == "Movie").cloned().collect();
    let cw_tv     = hd.continue_watching.iter().filter(|i| i.item_type == "Episode").cloned().collect();
    [
        hd.continue_watching.clone(),
        hd.next_up.clone(),
        hd.recently_added.clone(),
        cw_movies,
        hd.recently_added_movies.clone(),
        hd.not_watched_movies.clone(),
        cw_tv,
        hd.recently_added_tv.clone(),
        hd.not_watched_tv.clone(),
    ]
}

fn spawn_poster_loading(
    client:      Arc<JellyfinClient>,
    sections:    [Vec<MediaItem>; 9],
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    rt_handle.spawn(async move {
        use std::collections::{HashMap, HashSet};
        use std::sync::Arc as SArc;

        // Per-section card metadata: (item_id, poster_id, title, year, played, resume_pct).
        // For episodes, poster_id = series_id so we show the series poster, not an episode thumb.
        let section_meta: Vec<Vec<(String, String, String, i32, bool, f32)>> = sections.iter()
            .map(|items| items.iter().map(|i| {
                let poster_id = if i.item_type == "Episode" {
                    i.series_id.clone().unwrap_or_else(|| i.id.clone())
                } else {
                    i.id.clone()
                };
                (i.id.clone(), poster_id, i.display_name(),
                 i.production_year.unwrap_or(0) as i32, i.user_data.played, i.resume_pct())
            }).collect())
            .collect();

        // Pending set per section — keyed by poster_id, removed as each poster arrives.
        let mut section_pending: Vec<HashSet<String>> = section_meta.iter()
            .map(|cards| cards.iter().map(|(_, poster_id, _, _, _, _)| poster_id.clone()).collect())
            .collect();

        // Deduplicate: each unique poster_id is fetched exactly once.
        let unique_ids: HashSet<String> = section_meta.iter().flatten()
            .map(|(_, poster_id, _, _, _, _)| poster_id.clone())
            .collect();

        // Fetch each unique poster: disk cache first, network on miss, semaphore-limited.
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for poster_id in unique_ids {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            fetch_set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let bytes   = fetch_poster_cached(&*client, &poster_id).await.map(SArc::new);
                (poster_id, bytes)
            });
        }

        let mut poster_map: HashMap<String, SArc<Vec<u8>>> = HashMap::new();

        while let Some(res) = fetch_set.join_next().await {
            let Ok((poster_id, bytes)) = res else { continue };
            if let Some(b) = bytes { poster_map.insert(poster_id.clone(), b); }

            // Mark this poster_id done in every section that references it.
            // Push a section the moment its last pending poster is resolved.
            for sec_idx in 0..9usize {
                if !section_pending[sec_idx].remove(&poster_id) { continue; }
                if !section_pending[sec_idx].is_empty()         { continue; }
                // Decode JPEG/PNG here (async worker thread) — produces Send-able
                // SharedPixelBuffer.  Image::from_rgba8 runs on the UI thread below.
                type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
                let decoded: Vec<(SharedString, SharedString, i32, bool, f32, Option<Buf>)> =
                    section_meta[sec_idx].iter().map(|(item_id, poster_id, title, year, played, rpct)| {
                        let buf = poster_map.get(poster_id).and_then(|b| decode_poster_buffer(b));
                        (SharedString::from(item_id.as_str()), SharedString::from(title.as_str()), *year, *played, *rpct, buf)
                    }).collect();
                let ww = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let items: Vec<CardItem> = decoded.into_iter().map(|(id, title, year, played, rpct, buf)| {
                            let mut h = CardItem::default();
                            h.id         = id;
                            h.title      = title;
                            h.year       = year;
                            h.has_played = played;
                            h.resume_pct = rpct;
                            if let Some(spb) = buf {
                                h.poster     = slint::Image::from_rgba8(spb);
                                h.has_poster = true;
                            }
                            h
                        }).collect();
                        push_section_model(&w, sec_idx, ModelRc::new(VecModel::from(items)));
                    }
                });
            }
        }
    });
}

// ── series drill-down helpers ─────────────────────────────────────────────────

// Intermediate Send-able representation of an episode (EpisodeEntry contains slint::Image = !Send).
struct EpisodeRaw {
    id:         String,
    title:      String,
    ep_num:     i32,
    season_num: i32,
    overview:   String,
    has_played: bool,
    resume_pct: f32,
    runtime:    String,
}

fn make_episode_raw(ep: &MediaItem) -> EpisodeRaw {
    let resume_pct = if let Some(ticks) = ep.run_time_ticks {
        let pos = ep.user_data.playback_position_ticks;
        if ticks > 0 { (pos as f32 / ticks as f32).clamp(0.0, 1.0) } else { 0.0 }
    } else { 0.0 };
    EpisodeRaw {
        id:         ep.id.clone(),
        title:      ep.name.clone(),
        ep_num:     ep.index_number.unwrap_or(0) as i32,
        season_num: ep.parent_index_number.unwrap_or(0) as i32,
        overview:   ep.overview.clone().unwrap_or_default(),
        has_played: ep.user_data.played,
        resume_pct,
        runtime:    ep.runtime_string().unwrap_or_default(),
    }
}

fn raw_to_entry(r: EpisodeRaw) -> EpisodeEntry {
    EpisodeEntry {
        id:         r.id.as_str().into(),
        title:      r.title.as_str().into(),
        ep_num:     r.ep_num,
        season_num: r.season_num,
        overview:   r.overview.as_str().into(),
        has_played: r.has_played,
        resume_pct: r.resume_pct,
        runtime:    r.runtime.as_str().into(),
        has_thumb:  false,
        thumb:      Default::default(),
    }
}

fn spawn_episode_thumb_loading(
    client:      Arc<JellyfinClient>,
    episodes:    Vec<MediaItem>,
    series_id:   String,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    if episodes.is_empty() { return; }
    rt_handle.spawn(async move {
        let sem = Arc::new(tokio::sync::Semaphore::new(6));
        let mut tasks: tokio::task::JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> =
            tokio::task::JoinSet::new();
        for (idx, ep) in episodes.iter().enumerate() {
            let client2 = Arc::clone(&client);
            let sem2    = Arc::clone(&sem);
            let id      = ep.id.clone();
            tasks.spawn(async move {
                let _permit = sem2.acquire_owned().await.ok();
                let bytes = fetch_poster_cached(&*client2, &id).await;
                (idx, bytes.as_deref().and_then(|b| decode_poster_buffer(b)))
            });
        }
        while let Some(res) = tasks.join_next().await {
            let Ok((idx, Some(buf))) = res else { continue };
            let ww  = window_weak.clone();
            let sid = series_id.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww.upgrade() else { return };
                if w.get_series_id().as_str() != sid { return; }
                let eps = w.get_series_episodes();
                if let Some(mut ep) = eps.row_data(idx) {
                    ep.thumb     = slint::Image::from_rgba8(buf);
                    ep.has_thumb = true;
                    eps.set_row_data(idx, ep);
                }
            });
        }
    });
}

fn open_series_screen(
    id:        String,
    state:     Arc<Mutex<AppState>>,
    ww:        slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    let s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
    let basic = s.all_series.iter().find(|i| i.id == id).cloned();
    drop(s);

    info!("open_series: id={} name={:?}", id, basic.as_ref().map(|i| i.name.as_str()));

    if let Some(w) = ww.upgrade() {
        w.set_show_series(true);
        w.set_series_id(id.as_str().into());
        w.set_series_loading(true);
        w.set_series_in_season_row(false);
        w.set_series_season_idx(0);
        w.set_series_focused_ep(0);
        w.set_series_seasons(ModelRc::new(VecModel::<SeasonEntry>::default()));
        w.set_series_episodes(ModelRc::new(VecModel::<EpisodeEntry>::default()));
        w.set_series_has_backdrop(false);
        w.set_series_has_poster(false);
        if let Some(ref item) = basic {
            w.set_series_title(item.name.as_str().into());
            w.set_series_overview(item.overview.clone().unwrap_or_default().as_str().into());
        }
    }

    let id2       = id.clone();
    let ww2       = ww.clone();
    let ww3       = ww.clone();
    let state2    = Arc::clone(&state);
    let rth2      = rt_handle.clone();
    rt_handle.spawn(async move {
        let (detail_res, poster_bytes, seasons_res) = tokio::join!(
            client.get_item_detail(&id2),
            fetch_poster_cached(&client, &id2),
            client.get_seasons(&id2),
        );
        let backdrop_bytes = match &detail_res {
            Ok(d) if !d.backdrop_image_tags.is_empty() => fetch_backdrop_cached(&client, &id2).await,
            _ => None,
        };
        let seasons = seasons_res.unwrap_or_else(|e| { warn!("get_seasons {}: {:#}", id2, e); vec![] });
        debug!("series {} — {} season(s)", id2, seasons.len());

        let season_ids: Vec<String> = seasons.iter().map(|s| s.id.clone()).collect();
        {
            let mut s = state2.lock().unwrap();
            s.series_open_id    = id2.clone();
            s.series_season_ids = season_ids;
        }

        let first_eps = if let Some(first) = seasons.first() {
            client.get_season_episodes(&id2, &first.id).await.unwrap_or_else(|e| {
                warn!("get_season_episodes {} {}: {:#}", id2, first.id, e);
                vec![]
            })
        } else { vec![] };
        debug!("series {} season 0 — {} episode(s)", id2, first_eps.len());
        {
            let mut s = state2.lock().unwrap();
            s.series_episode_items = first_eps.clone();
        }

        let season_entries: Vec<SeasonEntry> = seasons.iter()
            .map(|s| SeasonEntry { id: s.id.as_str().into(), name: s.name.as_str().into() })
            .collect();
        let ep_raws: Vec<EpisodeRaw> = first_eps.iter().map(make_episode_raw).collect();

        let detail_name     = detail_res.as_ref().map(|d| d.name.clone()).ok().unwrap_or_default();
        let detail_overview = detail_res.as_ref().ok().and_then(|d| d.overview.clone()).unwrap_or_default();
        let id3 = id2.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            if w.get_series_id().as_str() != id3 { return; }
            if !detail_name.is_empty()     { w.set_series_title(detail_name.as_str().into()); }
            if !detail_overview.is_empty() { w.set_series_overview(detail_overview.as_str().into()); }
            w.set_series_seasons(ModelRc::new(VecModel::from(season_entries)));
            let ep_entries: Vec<EpisodeEntry> = ep_raws.into_iter().map(raw_to_entry).collect();
            w.set_series_episodes(ModelRc::new(VecModel::from(ep_entries)));
            w.set_series_loading(false);
            if let Some(bytes) = poster_bytes {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let rgba = img.to_rgba8();
                    let (pw, ph) = rgba.dimensions();
                    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba.as_raw(), pw, ph);
                    w.set_series_poster(slint::Image::from_rgba8(buf));
                    w.set_series_has_poster(true);
                }
            }
            if let Some(bytes) = backdrop_bytes {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let rgba = img.to_rgba8();
                    let (bw, bh) = rgba.dimensions();
                    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(rgba.as_raw(), bw, bh);
                    w.set_series_backdrop(slint::Image::from_rgba8(buf));
                    w.set_series_has_backdrop(true);
                }
            }
        });
        spawn_episode_thumb_loading(client, first_eps, id2, ww3, rth2);
    });
}

// ── stats formatting ──────────────────────────────────────────────────────────

fn update_stats_window(w: &MainWindow, s: &fjord_player::StatsData) {
    // ── Video input ───────────────────────────────────────────────────────────
    let vid_in = if s.width > 0 {
        let codec = if s.video_codec.is_empty() { "?" } else { &s.video_codec };
        let fmt   = if s.video_pix_fmt.is_empty() { String::new() } else { format!("  ·  {}", s.video_pix_fmt) };
        format!("{}  ·  {}×{}  ·  {:.2} fps{}", codec, s.width, s.height, s.fps, fmt)
    } else {
        "Buffering…".into()
    };

    // ── Video output (after filters) ─────────────────────────────────────────
    let vid_out = if s.video_out_w > 0 {
        let scale = if s.video_out_w != s.width || s.video_out_h != s.height {
            format!("{}×{}", s.video_out_w, s.video_out_h)
        } else {
            format!("{}×{}", s.width, s.height)
        };
        let fmt = if s.video_out_pix_fmt.is_empty() { String::new() } else { format!("  ·  {}", s.video_out_pix_fmt) };
        format!("{}{}", scale, fmt)
    } else {
        "—".into()
    };

    // ── Colour / HDR ─────────────────────────────────────────────────────────
    let color = {
        let prim  = s.video_primaries.as_str();
        let gamma = s.video_gamma.as_str();
        let hdr   = match gamma {
            "pq"  => format!("  ·  HDR10 (peak {:.0} nits)", s.video_sig_peak * 100.0),
            "hlg" => "  ·  HLG".into(),
            _     => String::new(),
        };
        if prim.is_empty() && gamma.is_empty() { "—".into() }
        else { format!("{}  ·  {}{}", prim, gamma, hdr) }
    };

    // ── HW decode ─────────────────────────────────────────────────────────────
    let hwdec = match s.hwdec_current.as_str() {
        "" | "no" => "CPU (software)".into(),
        v         => v.to_string(),
    };

    // ── Audio input ───────────────────────────────────────────────────────────
    let aud_in = {
        let name = if !s.audio_codec_name.is_empty() { &s.audio_codec_name } else { &s.audio_codec };
        if name.is_empty() {
            "—".into()
        } else {
            let ch  = if s.audio_channels.is_empty()  { String::new() } else { format!("  ·  {}", s.audio_channels) };
            let sr  = if s.audio_samplerate == 0       { String::new() } else { format!("  ·  {} Hz", s.audio_samplerate) };
            format!("{}{}{}", name, ch, sr)
        }
    };

    // ── Audio output ──────────────────────────────────────────────────────────
    let aud_out = if s.current_ao.is_empty() {
        "—".into()
    } else {
        let passthrough = s.audio_out_format.starts_with("iec61937");
        if passthrough {
            format!("{}  ·  passthrough  ({})", s.current_ao, s.audio_out_format)
        } else {
            let fmt = if s.audio_out_format.is_empty()     { String::new() } else { format!("  ·  {}", s.audio_out_format) };
            let ch  = if s.audio_out_channels.is_empty()   { String::new() } else { format!("  ·  {}", s.audio_out_channels) };
            let sr  = if s.audio_out_samplerate == 0       { String::new() } else { format!("  ·  {} Hz", s.audio_out_samplerate) };
            format!("{}{}{}{}", s.current_ao, fmt, sr, ch)
        }
    };

    // ── Display ───────────────────────────────────────────────────────────────
    let display = if s.display_fps > 0.0 { format!("{:.3} Hz", s.display_fps) } else { "—".into() };

    // ── Timing / performance ──────────────────────────────────────────────────
    let vsync = if s.vsync_ratio == 0.0 {
        "N/A  (audio-sync mode)".into()
    } else {
        format!("{:.4}  (ideal 1.0000)", s.vsync_ratio)
    };
    let avsync  = format!("{:+.3}s", s.avsync);
    let drop_   = format!("{} dropped", s.dropped_frames);
    let bitrate = format!("V: {:.1} Mbps  A: {:.0} kbps",
        s.video_bitrate / 1_000_000.0, s.audio_bitrate / 1_000.0);
    let cache   = format!("{}%", s.cache_state);

    w.set_stat_vid_in(ss(&vid_in));
    w.set_stat_vid_out(ss(&vid_out));
    w.set_stat_color(ss(&color));
    w.set_stat_hwdec(ss(&hwdec));
    w.set_stat_aud_in(ss(&aud_in));
    w.set_stat_aud_out(ss(&aud_out));
    w.set_stat_display(ss(&display));
    w.set_stat_vsync(ss(&vsync));
    w.set_stat_avsync(ss(&avsync));
    w.set_stat_drop(ss(&drop_));
    w.set_stat_bitrate(ss(&bitrate));
    w.set_stat_cache(ss(&cache));
}

// ── GL helper: create texture + FBO ──────────────────────────────────────────

unsafe fn create_fbo(w: u32, h: u32) -> Option<(u32, u32)> {
    let mut tex = 0u32;
    gl::GenTextures(1, &mut tex);
    gl::BindTexture(gl::TEXTURE_2D, tex);
    gl::TexImage2D(
        gl::TEXTURE_2D, 0, gl::RGBA as i32,
        w as i32, h as i32, 0,
        gl::RGBA, gl::UNSIGNED_BYTE, std::ptr::null(),
    );
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
    gl::BindTexture(gl::TEXTURE_2D, 0);

    let mut fbo = 0u32;
    gl::GenFramebuffers(1, &mut fbo);
    gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
    gl::FramebufferTexture2D(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::TEXTURE_2D, tex, 0);
    let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER);
    gl::BindFramebuffer(gl::FRAMEBUFFER, 0);

    if status != gl::FRAMEBUFFER_COMPLETE {
        tracing::error!("FBO not complete: {:#x}", status);
        gl::DeleteFramebuffers(1, &fbo);
        gl::DeleteTextures(1, &tex);
        return None;
    }
    Some((fbo, tex))
}

unsafe fn delete_fbo(fbo: u32, tex: u32) {
    if fbo != 0 { gl::DeleteFramebuffers(1, &fbo); }
    if tex != 0 { gl::DeleteTextures(1, &tex); }
}

// ── entry point ───────────────────────────────────────────────────────────────

fn main() -> Result<()> {
    // Log to both stderr and ~/.cache/fjord/fjord.log for HTPC debugging.
    let log_dir = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
        })
        .join("fjord");
    let _ = std::fs::create_dir_all(&log_dir);
    let file_appender = tracing_appender::rolling::never(&log_dir, "fjord.log");
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
    use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt, EnvFilter};
    // External crates (winit, sctk, reqwest, …) flood the log at DEBUG.
    // Default to WARN for everything; our own crates stay at DEBUG.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new("warn,fjord_app=debug,fjord_player=debug,fjord_api=debug")
    });
    tracing_subscriber::registry()
        .with(filter)
        .with(tracing_subscriber::fmt::layer())                          // stderr
        .with(tracing_subscriber::fmt::layer().with_writer(file_writer)) // file
        .init();
    info!("log file: {}", log_dir.join("fjord.log").display());

    let rt     = tokio::runtime::Runtime::new()?;
    let window = MainWindow::new()?;
    let state  = Arc::new(Mutex::new(AppState::new()));
    let video  = Arc::new(Mutex::new(VideoState::default()));

    // ── rendering notifier: set up GL, create render ctx, draw frames ─────────
    {
        let video_rn   = Arc::clone(&video);
        let window_rn  = window.as_weak();

        window.window().set_rendering_notifier({
            let mut gl_loaded = false;
            let mut last_stats = Instant::now();

            move |state_rn, api| {
                match state_rn {
                    slint::RenderingState::RenderingSetup => {
                        if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                            if !gl_loaded {
                                gl::load_with(|name| {
                                    let cname = std::ffi::CString::new(name).unwrap();
                                    get_proc_address(cname.as_c_str())
                                });
                                gl_loaded = true;
                                info!("OpenGL loaded");
                            }
                        }
                    }

                    slint::RenderingState::BeforeRendering => {
                        let Some(win) = window_rn.upgrade() else { return; };
                        let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api else { return; };

                        let mut vs = video_rn.lock().unwrap();
                        vs.did_render = false;

                        // Clean up orphaned GL objects when playback has ended
                        if vs.fbos[0] != 0 && vs.player.is_none() {
                            unsafe {
                                delete_fbo(vs.fbos[0], vs.textures[0]);
                                delete_fbo(vs.fbos[1], vs.textures[1]);
                            }
                            vs.fbos = [0; 2]; vs.textures = [0; 2];
                            vs.fbo_w = 0; vs.fbo_h = 0;
                            return;
                        }

                        if vs.player.is_none() { return; }

                        // Lazily create render context (needs GL current + raw handle)
                        if vs.render_ctx.is_none() {
                            let handle = vs.player.as_ref().unwrap().raw_handle_ptr();
                            match unsafe { MpvRenderCtx::new(handle, get_proc_address) } {
                                Ok(mut ctx) => {
                                    let ww = window_rn.clone();
                                    ctx.set_update_callback(move || {
                                        let ww2 = ww.clone();
                                        let _ = slint::invoke_from_event_loop(move || {
                                            if let Some(w) = ww2.upgrade() {
                                                w.window().request_redraw();
                                            }
                                        });
                                    });
                                    vs.render_ctx = Some(ctx);
                                    info!("mpv render context created");
                                }
                                Err(e) => { error!("MpvRenderCtx::new: {:#}", e); return; }
                            }
                        }

                        // Physical pixel size for the FBO
                        let phys = win.window().size();
                        let w = phys.width.max(1);
                        let h = phys.height.max(1);

                        // (Re)create both FBOs if size changed
                        if vs.fbos[0] == 0 || vs.fbo_w != w || vs.fbo_h != h {
                            unsafe {
                                delete_fbo(vs.fbos[0], vs.textures[0]);
                                delete_fbo(vs.fbos[1], vs.textures[1]);
                            }
                            let r0 = unsafe { create_fbo(w, h) };
                            let r1 = unsafe { create_fbo(w, h) };
                            match (r0, r1) {
                                (Some((f0, t0)), Some((f1, t1))) => {
                                    vs.fbos = [f0, f1]; vs.textures = [t0, t1];
                                    vs.fbo_w = w; vs.fbo_h = h; vs.back = 0;
                                }
                                (p0, p1) => {
                                    if let Some((f, t)) = p0 { unsafe { delete_fbo(f, t); } }
                                    if let Some((f, t)) = p1 { unsafe { delete_fbo(f, t); } }
                                    vs.fbos = [0; 2]; vs.textures = [0; 2];
                                    return;
                                }
                            }
                        }

                        // Render mpv frame into the back buffer, then flip.
                        // Alternating texture IDs force Slint to re-render the Image
                        // every frame (same ID = Slint considers it unchanged = stale).
                        if let Some(ctx) = vs.render_ctx.as_ref() {
                            let b = vs.back;
                            if let Err(e) = ctx.render(vs.fbos[b] as i32, w as i32, h as i32, true) {
                                warn!("mpv render: {:#}", e);
                            } else {
                                vs.did_render = true;
                            }

                            if let Some(tex_id) = NonZeroU32::new(vs.textures[b]) {
                                let size = euclid::default::Size2D::new(w, h);
                                let img = unsafe {
                                    slint::BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(tex_id, size)
                                        .origin(slint::BorrowedOpenGLTextureOrigin::BottomLeft)
                                        .build()
                                };
                                win.set_video_frame(img);
                            }

                            vs.back = 1 - b; // next frame uses the other buffer
                        }

                        // Stats refresh every 500 ms
                        if last_stats.elapsed() >= Duration::from_millis(500) {
                            if let Some(player) = vs.player.as_ref() {
                                let stats = player.poll_stats();
                                if let Some(w) = window_rn.upgrade() {
                                    update_stats_window(&w, &stats);
                                }
                            }
                            last_stats = Instant::now();
                        }
                    }

                    slint::RenderingState::AfterRendering => {
                        let vs = video_rn.lock().unwrap();
                        if vs.did_render {
                            if let Some(ctx) = vs.render_ctx.as_ref() {
                                ctx.report_swap();
                            }
                        }
                    }

                    slint::RenderingState::RenderingTeardown => {
                        let mut vs = video_rn.lock().unwrap();
                        vs.render_ctx = None; // must drop before player
                        unsafe {
                            delete_fbo(vs.fbos[0], vs.textures[0]);
                            delete_fbo(vs.fbos[1], vs.textures[1]);
                        }
                        vs.fbos = [0; 2]; vs.textures = [0; 2];
                    }

                    _ => {}
                }
            }
        }).ok();
    }

    // ── mpv event-poll timer (16 ms ≈ 60 fps) ────────────────────────────────
    {
        let video_timer  = Arc::clone(&video);
        let window_timer = window.as_weak();
        let state_timer  = Arc::clone(&state);
        let rt_handle    = rt.handle().clone();

        let timer = slint::Timer::default();
        timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
            let finished = {
                let mut vs = video_timer.lock().unwrap();

                if vs.player.is_some() {
                    let elapsed_ok = vs.play_start.map_or(false, |t| t.elapsed() >= Duration::from_secs(2));

                    // 2 s after start: log decoder and load tracks
                    if elapsed_ok && !vs.decoder_logged {
                        if let Some(p) = vs.player.as_ref() {
                            p.log_decoder_info();
                            p.apply_auto_vf();
                        }
                        vs.decoder_logged = true;
                    }
                    if elapsed_ok && !vs.tracks_loaded {
                        if let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                            let tracks = p.get_tracks();
                            debug!("track-list ({} entries):", tracks.len());
                            for t in &tracks {
                                debug!("  [{:>2}] {:5}  selected={}  lang={:5}  title={:?}  codec={}",
                                    t.id, t.track_type, t.selected, t.lang, t.title, t.codec);
                            }
                            let sub_model   = build_track_model(&tracks, "sub");
                            let audio_model = build_track_model(&tracks, "audio");
                            let video_model = build_track_model(&tracks, "video");
                            let cur_sub   = tracks.iter().find(|t| t.track_type == "sub"   && t.selected).map(|t| t.id).unwrap_or(0);
                            let cur_audio = tracks.iter().find(|t| t.track_type == "audio" && t.selected).map(|t| t.id).unwrap_or(1);
                            let cur_video = tracks.iter().find(|t| t.track_type == "video" && t.selected).map(|t| t.id).unwrap_or(1);
                            debug!("active tracks: sub={} audio={} video={}", cur_sub, cur_audio, cur_video);
                            w.set_sub_tracks(sub_model);
                            w.set_audio_tracks(audio_model);
                            w.set_video_tracks(video_model);
                            w.set_current_sub_id(cur_sub as i32);
                            w.set_current_audio_id(cur_audio as i32);
                            w.set_current_video_id(cur_video as i32);
                        }
                        vs.tracks_loaded = true;
                    }

                    // Update seek bar ~every 500 ms (every 30 ticks × 16 ms)
                    vs.pos_tick = vs.pos_tick.wrapping_add(1);
                    if vs.pos_tick % 30 == 0 {
                        if let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                            let pos = p.get_position();
                            let dur = p.get_duration();
                            let ratio = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
                            w.set_playback_pos(ratio);
                            w.set_playback_time(fmt_secs(pos));
                            w.set_playback_total(fmt_secs(dur));
                        }
                    }

                    // Intro skip prompt: show/hide based on playback position
                    if let Some(ref ts) = vs.intro_timestamps {
                        if let Some(p) = vs.player.as_ref() {
                            let pos = p.get_position();
                            let should_show = pos >= ts.show_skip_prompt_at
                                && pos < ts.hide_skip_prompt_at;
                            if should_show != vs.intro_skip_shown {
                                vs.intro_skip_shown = should_show;
                                if let Some(w) = window_timer.upgrade() {
                                    w.set_show_skip_intro(should_show);
                                }
                            }
                        }
                    }

                    // Controls auto-hide: fade out after ~3 s idle (187 ticks × 16 ms)
                    vs.controls_idle_ticks = vs.controls_idle_ticks.saturating_add(1);
                    if vs.controls_idle_ticks == 187 {
                        if let Some(w) = window_timer.upgrade() {
                            w.set_controls_visible(false);
                        }
                    }
                }

                if let Some(player) = vs.player.as_mut() {
                    matches!(player.poll(), PollResult::Finished)
                } else {
                    false
                }
            };

            if finished {
                info!("playback finished — tearing down");
                let (item_id, playing_series_id, client) = {
                    let mut vs = video_timer.lock().unwrap();
                    // render_ctx MUST be dropped before player
                    vs.render_ctx = None;
                    vs.player     = None;
                    (vs.item_id.take(), vs.playing_series_id.take(), vs.client.take())
                };

                if let Some(w) = window_timer.upgrade() {
                    w.set_is_playing(false);
                    w.set_has_background_player(false);
                    w.set_video_behind_ui(false);
                    w.set_is_paused(false);
                    w.set_stats_visible(false);
                    w.set_playback_pos(0.0);
                    w.set_playback_time("0:00".into());
                    w.set_playback_total("0:00".into());
                    w.set_sub_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
                    w.set_audio_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
                    w.set_video_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
                    w.set_player_open_panel(0);
                    w.set_controls_visible(true);
                    w.set_show_skip_intro(false);
                }

                // Report the finished episode as stopped at position 0 (natural end)
                if let Some(id) = item_id.as_deref() {
                    if let Some(cli) = client.as_ref().map(Arc::clone) {
                        let id2 = id.to_string();
                        rt_handle.spawn(async move {
                            let _ = cli.report_playback_stopped(&id2, 0).await;
                        });
                    }
                }

                // Auto-advance: query Jellyfin for the true next episode, then countdown
                if let Some(series_id) = playing_series_id {
                    if let Some(cli) = client {
                        let state_adv  = Arc::clone(&state_timer);
                        let video_adv  = Arc::clone(&video_timer);
                        let ww_adv     = window_timer.clone();
                        let rt_adv     = rt_handle.clone();
                        rt_handle.spawn(async move {
                            let Ok(Some(next)) = cli.get_next_up_for_series(&series_id).await else {
                                return; // no next episode (end of series)
                            };
                            info!("auto-advance: next episode is {}", next.id);

                            // Store pending episode so cancel can clear it
                            state_adv.lock().unwrap().next_ep_pending = Some(next.clone());

                            // Show the banner with initial countdown
                            let title_str = next.display_name();
                            let ww1 = ww_adv.clone();
                            let t1   = SharedString::from(title_str.as_str());
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww1.upgrade() {
                                    w.set_next_ep_title(t1);
                                    w.set_next_ep_secs(5);
                                    w.set_show_next_ep_banner(true);
                                }
                            });

                            // Countdown: tick once per second for 5 seconds
                            for remaining in (0i32..5).rev() {
                                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                                if state_adv.lock().unwrap().next_ep_pending.is_none() {
                                    return; // cancelled
                                }
                                let ww2 = ww_adv.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = ww2.upgrade() {
                                        w.set_next_ep_secs(remaining);
                                    }
                                });
                            }

                            // Take the pending episode (may have been cancelled)
                            let next = state_adv.lock().unwrap().next_ep_pending.take();
                            let Some(next) = next else { return; };

                            let config = state_adv.lock().unwrap().player_config();
                            let cli2   = state_adv.lock().unwrap().client.as_ref().map(Arc::clone);
                            let Some(cli2) = cli2 else { return; };

                            let url   = cli2.direct_play_url(&next.id);
                            let title = next.display_name();
                            let id    = next.id.clone();
                            info!("auto-advancing to: {}", id);

                            let series_id2 = next.series_id.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww_adv.upgrade() {
                                    w.set_show_next_ep_banner(false);
                                }
                                start_playback(url, id, "Episode", title, config, cli2,
                                               &video_adv, &ww_adv, &rt_adv);
                                video_adv.lock().unwrap().playing_series_id = series_id2;
                            });
                        });
                    }
                }
            }
        });
        // Keep timer alive for the duration of main
        std::mem::forget(timer);
    }

    // ── Not-watched refresh (polls every 30 s, refreshes when tab visible + 10 min elapsed) ──
    {
        let state_nw  = Arc::clone(&state);
        let video_nw  = Arc::clone(&video);
        let window_nw = window.as_weak();
        let rt_nw     = rt.handle().clone();
        let timer_nw  = slint::Timer::default();
        timer_nw.start(slint::TimerMode::Repeated, Duration::from_secs(30), move || {
            // Skip if playing — avoids decode CPU spikes during video
            if video_nw.lock().unwrap().player.is_some() { return; }
            let Some(w) = window_nw.upgrade() else { return };
            let nav = w.get_active_nav();
            // Only act when Movies (1) or TV (2) tab is visible
            if nav != 1 && nav != 2 { return; }

            let (due_movies, due_tv) = {
                let s = state_nw.lock().unwrap();
                (
                    nav == 1 && s.last_nw_mov_refresh.map_or(true,    |t| t.elapsed() >= Duration::from_secs(600)),
                    nav == 2 && s.last_nw_tv_refresh.map_or(true, |t| t.elapsed() >= Duration::from_secs(600)),
                )
            };
            if !due_movies && !due_tv { return; }

            let client = state_nw.lock().unwrap().client.as_ref().map(Arc::clone);
            let Some(client) = client else { return };

            // Stamp before spawning so concurrent ticks can't double-fire
            {
                let mut s = state_nw.lock().unwrap();
                if due_movies { s.last_nw_mov_refresh    = Some(Instant::now()); }
                if due_tv     { s.last_nw_tv_refresh = Some(Instant::now()); }
            }

            let ww  = window_nw.clone();
            let rt2 = rt_nw.clone();
            rt_nw.spawn(async move {
                if due_movies {
                    let Ok(items) = client.get_unwatched(Some("Movie")).await else { return };
                    let ww2    = ww.clone();
                    let items2 = items.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() { w.set_not_watched_movies(items_to_model(&items2)); }
                    });
                    let mut sections: [Vec<MediaItem>; 9] = Default::default();
                    sections[5] = items;
                    spawn_poster_loading(Arc::clone(&client), sections, ww.clone(), rt2.clone());
                }
                if due_tv {
                    let Ok(items) = client.get_unwatched(Some("Series")).await else { return };
                    let ww2    = ww.clone();
                    let items2 = items.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() { w.set_not_watched_tv(items_to_model(&items2)); }
                    });
                    let mut sections: [Vec<MediaItem>; 9] = Default::default();
                    sections[8] = items;
                    spawn_poster_loading(client, sections, ww, rt2);
                }
            });
        });
        std::mem::forget(timer_nw);
    }

    // ── apply saved config ────────────────────────────────────────────────────
    if let Some(mut cfg) = load_config() {
        ensure_device_id(&mut cfg);
        {
            let mut s = state.lock().unwrap();
            s.apply_from_config(&cfg);
        }
        apply_settings_to_window(&window, &state.lock().unwrap());
        if cfg.launch_fullscreen {
            window.window().set_fullscreen(true);
        }

        if let Ok(server_url) = Url::parse(&cfg.server_url) {
            let client = Arc::new(JellyfinClient::new(server_url.clone(), cfg.user_id, cfg.token, cfg.device_id.clone()));
            state.lock().unwrap().client = Some(Arc::clone(&client));
            window.set_server_url(ss(cfg.server_url.as_str()));

            if let Some(cached) = load_item_cache() {
                info!("item cache: {} items loaded instantly", cached.len());
                let mut s = state.lock().unwrap();
                s.all_movies = cached.iter().filter(|i| i.item_type == "Movie").cloned().collect();
                s.media_raw  = cached;
                s.apply_filter("");
                let names       = display_names(&s.filtered_items);
                let movie_model = items_to_model(&s.all_movies);
                drop(s);
                // Still on the main thread before window.run() — set directly,
                // no invoke_from_event_loop needed (avoids a one-frame login flash).
                window.set_media_items(to_slint_model(names));
                window.set_all_movies(movie_model);
                window.set_show_login(false);
                window.set_status(ss(""));
            }

            // Show cached home data immediately so no "Loading library…" flash.
            if let Some(cached_home) = load_home_cache() {
                push_home_data(&window, &cached_home);
            }

            let window_weak = window.as_weak();
            let state2      = Arc::clone(&state);
            let rt_handle2  = rt.handle().clone();
            rt.spawn(async move {
                // Probe auth before heavy refresh — show login screen cleanly on 401.
                if let Err(e) = client.check_auth().await {
                    if is_unauthorized(&e) {
                        warn!("saved token is invalid (401) — showing login screen");
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = window_weak.upgrade() {
                                w.set_show_login(true);
                                w.set_status(ss("Session expired — please log in again"));
                            }
                        });
                        return;
                    }
                    warn!("auth probe failed (non-401): {e:#}");
                }

                // Skip the expensive full-library refresh when the cache is recent.
                // Home data (continue watching, next up, etc.) always refreshes.
                let (maybe_new_items, home_data, series_res) = if is_item_cache_fresh() {
                    info!("auto-login: item cache fresh — refreshing home data only");
                    let (hd, sr) = tokio::join!(fetch_home_data(&client), client.get_all_series());
                    (None::<Vec<MediaItem>>, hd, sr)
                } else {
                    info!("auto-login: refreshing library + home data (background)");
                    let (items_res, hd, sr) = tokio::join!(
                        client.get_all_items(|_| {}),
                        fetch_home_data(&client),
                        client.get_all_series(),
                    );
                    match items_res {
                        Ok(items) => (Some(items), hd, sr),
                        Err(e)    => { warn!("library refresh failed: {:#}", e); (None, hd, sr) }
                    }
                };

                if let Some(items) = maybe_new_items {
                    save_item_cache(&items);
                    let mut s = state2.lock().unwrap();
                    s.all_movies = items.iter().filter(|i| i.item_type == "Movie").cloned().collect();
                    s.media_raw  = items;
                    s.apply_filter("");
                    let names   = display_names(&s.filtered_items);
                    let movies  = s.all_movies.clone();  // Vec<MediaItem> is Send
                    drop(s);
                    let ww = window_weak.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() {
                            w.set_media_items(to_slint_model(names));
                            w.set_all_movies(items_to_model(&movies));
                        }
                    });
                }

                let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
                info!("loaded {} series", series.len());
                {
                    let mut s = state2.lock().unwrap();
                    s.all_series = series.clone();
                    s.refilter();
                    // browse list now includes series — push updated names to UI below
                }

                let browse_names = {
                    let s = state2.lock().unwrap();
                    display_names(&s.filtered_items)
                };
                save_home_cache(&home_data);
                let sections = home_data_sections(&home_data);
                let ww2 = window_weak.clone();
                let ww3 = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        push_home_data(&w, &home_data);
                        w.set_media_items(to_slint_model(browse_names));
                        w.set_show_login(false);
                        w.set_status(ss(""));
                    }
                });
                let movies_for_poster = state2.lock().unwrap().all_movies.clone();
                let client2 = Arc::clone(&client);
                let client3 = Arc::clone(&client);
                let ww4 = window_weak.clone();
                spawn_poster_loading(client, sections, window_weak, rt_handle2.clone());
                spawn_series_poster_loading(client2, series, ww3, rt_handle2.clone());
                spawn_movies_poster_loading(client3, movies_for_poster, ww4, rt_handle2);
            });
        }
    }

    // ── login ─────────────────────────────────────────────────────────────────
    {
        let state       = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle   = rt.handle().clone();

        window.on_do_login(move |server, user, pass| {
            let (server, user, pass) = (server.to_string(), user.to_string(), pass.to_string());
            let state         = Arc::clone(&state);
            let window_weak   = window_weak.clone();
            let rt_handle_sp  = rt_handle.clone();
            if let Some(w) = window_weak.upgrade() { w.set_status(ss("Connecting…")); }

            rt_handle.spawn(async move {
                let rt_handle = rt_handle_sp;
                let result: Result<()> = async {
                    let server_url = Url::parse(&server)?;
                    let mut cfg = load_config().unwrap_or_default();
                    ensure_device_id(&mut cfg);
                    let auth = fjord_api::authenticate(
                        &reqwest::Client::new(), &server_url, &user, &pass, &cfg.device_id,
                    ).await?;
                    info!("authenticated as {}", auth.user.name);
                    cfg.server_url = server_url.to_string();
                    cfg.user_id    = auth.user.id.clone();
                    cfg.token      = auth.access_token.clone();
                    save_config(&cfg);

                    let client = Arc::new(JellyfinClient::new(
                        server_url.clone(), auth.user.id, auth.access_token.clone(), cfg.device_id,
                    ));

                    let ww_p = window_weak.clone();
                    let (items_result, home_data, series_res) = tokio::join!(
                        client.get_all_items(move |n| {
                            let ww = ww_p.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww.upgrade() { w.set_status(ss(&format!("Loading… {n}"))); }
                            });
                        }),
                        fetch_home_data(&client),
                        client.get_all_series(),
                    );

                    let items = items_result?;
                    info!("loaded {} items", items.len());
                    save_item_cache(&items);
                    let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
                    info!("loaded {} series", series.len());
                    let mut s = state.lock().unwrap();
                    s.client     = Some(Arc::clone(&client));
                    s.all_movies = items.iter().filter(|i| i.item_type == "Movie").cloned().collect();
                    s.media_raw  = items;
                    s.all_series = series.clone();
                    s.apply_filter("");  // refilter includes series now
                    let names  = display_names(&s.filtered_items);
                    let movies = s.all_movies.clone();
                    let movies_for_poster = movies.clone();
                    drop(s);

                    let sections        = home_data_sections(&home_data);
                    let server_str      = server_url.to_string();
                    let ww              = window_weak.clone();
                    let ww_poster       = window_weak.clone();
                    let ww_series       = window_weak.clone();
                    let rt_handle_inner = rt_handle.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww.upgrade() {
                            w.set_server_url(ss(&server_str));
                            w.set_media_items(to_slint_model(names));
                            w.set_all_movies(items_to_model(&movies));
                            push_home_data(&w, &home_data);
                            w.set_show_login(false);
                            w.set_status(ss(""));
                        }
                    });
                    let client2 = Arc::clone(&client);
                    let client3 = Arc::clone(&client);
                    let ww_movies = window_weak.clone();
                    spawn_poster_loading(client, sections, ww_poster, rt_handle_inner.clone());
                    spawn_series_poster_loading(client2, series, ww_series, rt_handle_inner.clone());
                    spawn_movies_poster_loading(client3, movies_for_poster, ww_movies, rt_handle_inner);
                    Ok(())
                }.await;

                if let Err(e) = result {
                    error!("login failed: {:#}", e);
                    let msg = format!("{:#}", e);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = window_weak.upgrade() { w.set_status(ss(&msg)); }
                    });
                }
            });
        });
    }

    // ── filter ────────────────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        window.on_filter_changed(move |query| {
            let mut s = state.lock().unwrap();
            s.apply_filter(&query);
            let names = display_names(&s.filtered_items);
            drop(s);
            if let Some(w) = window_weak.upgrade() { w.set_media_items(to_slint_model(names)); }
        });
    }

    // ── library grid search ───────────────────────────────────────────────────
    fn update_library_filter(w: &MainWindow, query: &str) {
        let nav = w.get_active_nav();
        w.set_library_query(query.into());
        let full: ModelRc<CardItem> = if nav == 1 { w.get_all_movies() } else { w.get_all_series() };
        if query.is_empty() {
            w.set_library_display(full);
            return;
        }
        let q = query.to_lowercase();
        let filtered: Vec<CardItem> = (0..full.row_count())
            .filter_map(|i| full.row_data(i))
            .filter(|item| item.title.to_lowercase().contains(q.as_str()))
            .collect();
        w.set_library_display(ModelRc::new(VecModel::from(filtered)));
    }
    {
        let ww = window.as_weak();
        window.on_library_search_append(move |ch| {
            let Some(w) = ww.upgrade() else { return };
            let mut q = w.get_library_query().to_string();
            q.push_str(ch.as_str());
            update_library_filter(&w, &q);
        });
    }
    {
        let ww = window.as_weak();
        window.on_library_search_backspace(move || {
            let Some(w) = ww.upgrade() else { return };
            let mut q = w.get_library_query().to_string();
            q.pop();
            update_library_filter(&w, &q);
        });
    }
    {
        let ww = window.as_weak();
        window.on_library_search_clear(move || {
            let Some(w) = ww.upgrade() else { return };
            update_library_filter(&w, "");
        });
    }

    // ── nav ───────────────────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        window.on_nav_selected(move |nav| {
            let mut s = state.lock().unwrap();
            s.apply_nav(nav as usize);
            let names = display_names(&s.filtered_items);
            drop(s);
            if let Some(w) = window_weak.upgrade() {
                w.set_media_items(to_slint_model(names));
                w.set_current_item(-1);
            }
        });
    }

    // ── play helper ───────────────────────────────────────────────────────────

    fn start_playback(
        url:         String,
        item_id:     String,
        item_type:   &str,
        title:       String,
        config:      PlayerConfig,
        client:      Arc<JellyfinClient>,
        video:       &Arc<Mutex<VideoState>>,
        window_weak: &slint::Weak<MainWindow>,
        rt_handle:   &tokio::runtime::Handle,
    ) {
        info!("starting playback: {} — {}", item_id, url);

        // Report playback start in the background (fire and forget)
        {
            let client2  = Arc::clone(&client);
            let item_id2 = item_id.clone();
            rt_handle.spawn(async move {
                let _ = client2.report_playback_start(&item_id2).await;
            });
        }

        // Fetch intro timestamps for episodes (Intro Skipper plugin)
        if item_type == "Episode" {
            let client_ts  = Arc::clone(&client);
            let video_ts   = Arc::clone(video);
            let item_id_ts = item_id.clone();
            rt_handle.spawn(async move {
                match client_ts.get_intro_timestamps(&item_id_ts).await {
                    Ok(Some(ts)) => {
                        debug!(
                            "intro timestamps: show_at={:.1}s hide_at={:.1}s end={:.1}s",
                            ts.show_skip_prompt_at, ts.hide_skip_prompt_at, ts.intro_end
                        );
                        video_ts.lock().unwrap().intro_timestamps = Some(ts);
                    }
                    Ok(None) => debug!("no intro timestamps for {}", item_id_ts),
                    Err(e)   => debug!("intro timestamps unavailable: {:#}", e),
                }
            });
        }

        match Player::new(&url, &config) {
            Ok(player) => {
                {
                    let mut vs      = video.lock().unwrap();
                    vs.player       = Some(player);
                    vs.item_id      = Some(item_id);
                    vs.client       = Some(client);
                    vs.play_start     = Some(Instant::now());
                    vs.decoder_logged = false;
                    vs.tracks_loaded       = false;
                    vs.pos_tick            = 0;
                    vs.controls_idle_ticks = 0;
                    vs.intro_timestamps    = None;
                    vs.intro_skip_shown    = false;
                }
                if let Some(w) = window_weak.upgrade() {
                    w.set_playing_title(ss(&title));
                    w.set_is_playing(true);
                    w.set_has_background_player(false);
                    w.set_video_behind_ui(false);
                    w.set_is_paused(false);
                }
            }
            Err(e) => error!("player init failed: {:#}", e),
        }
    }

    // ── play from browse list ─────────────────────────────────────────────────
    {
        let state        = Arc::clone(&state);
        let video2       = Arc::clone(&video);
        let window_weak  = window.as_weak();
        let rt_handle    = rt.handle().clone();

        window.on_play_item(move |idx| {
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return; };
            let Some(item)   = s.filtered_items.get(idx as usize) else { return; };
            let item_id    = item.id.clone();
            let item_title = item.display_name();
            // Series items in browse results route to the drill-down screen
            if item.item_type == "Series" {
                let state2     = state.clone();
                let ww2        = window_weak.clone();
                let rt_handle2 = rt_handle.clone();
                drop(s);
                open_series_screen(item_id, state2, ww2, rt_handle2);
                return;
            }
            let play_url   = client.direct_play_url(&item_id);
            let mut config = s.player_config();
            config.start_position_secs = item.resume_position_secs();
            let item_type  = item.item_type.clone();
            let series_id  = item.series_id.clone();
            drop(s);

            start_playback(play_url, item_id, &item_type, item_title, config, client,
                           &video2, &window_weak, &rt_handle);
            video2.lock().unwrap().playing_series_id = series_id;
        });
    }

    // ── play from home / library rows ─────────────────────────────────────────
    {
        let state       = Arc::clone(&state);
        let video3      = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle   = rt.handle().clone();

        window.on_item_play(move |item_id| {
            let item_id = item_id.to_string();
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return; };

            // Route series items to the series screen
            if s.all_series.iter().any(|i| i.id == item_id) {
                let state2    = state.clone();
                let ww2       = window_weak.clone();
                let rt_handle2 = rt_handle.clone();
                drop(s);
                open_series_screen(item_id, state2, ww2, rt_handle2);
                return;
            }

            let mut config = s.player_config();
            let found = s.media_raw.iter().find(|i| i.id == item_id).cloned();
            config.start_position_secs = found.as_ref().and_then(|i| i.resume_position_secs());
            let item_type = found.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
            let series_id = found.and_then(|i| i.series_id);
            drop(s);
            let play_url = client.direct_play_url(&item_id);
            let title    = item_id.clone();

            start_playback(play_url, item_id, &item_type, title, config, client,
                           &video3, &window_weak, &rt_handle);
            video3.lock().unwrap().playing_series_id = series_id;
        });
    }

    // ── detail page ───────────────────────────────────────────────────────────
    {
        let state2    = Arc::clone(&state);
        let ww        = window.as_weak();
        let rt_handle = rt.handle().clone();
        window.on_open_detail(move |id| {
            let id  = id.to_string();
            let s   = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };

            // Route series items to the series drill-down screen
            if s.all_series.iter().any(|i| i.id == id) {
                let state3 = state2.clone();
                let ww3    = ww.clone();
                let rth3   = rt_handle.clone();
                drop(s);
                open_series_screen(id, state3, ww3, rth3);
                return;
            }

            // Populate immediately from the in-memory item list (title, year, resume)
            let basic = s.media_raw.iter().find(|i| i.id == id).cloned();
            drop(s);

            let ww2 = ww.clone();
            if let Some(w) = ww.upgrade() {
                w.set_show_detail(true);
                w.set_detail_id(id.as_str().into());
                w.set_detail_loading(true);
                w.set_detail_has_backdrop(false);
                w.set_detail_cast(ModelRc::new(VecModel::<CastMember>::default()));

                if let Some(ref item) = basic {
                    w.set_detail_title(item.name.as_str().into());
                    let resume_secs = item.resume_position_secs().unwrap_or(0.0);
                    w.set_detail_can_resume(resume_secs > 0.0);
                    w.set_detail_resume_label(fmt_resume_label(resume_secs).into());
                }
            }

            // Fetch full detail + poster + backdrop in background
            let id2     = id.clone();
            let ww3     = ww2.clone();
            rt_handle.spawn(async move {
                let detail = match client.get_item_detail(&id2).await {
                    Ok(d)  => d,
                    Err(e) => { warn!("get_item_detail {}: {:#}", id2, e); return; }
                };
                debug!("detail fetched: {} | genres={:?} | people={}", detail.name, detail.genres, detail.people.len());

                // Poster (reuse cached)
                let poster_bytes = fetch_poster_cached(&client, &id2).await;

                // Backdrop (only if the item has one)
                let backdrop_bytes = if detail.backdrop_image_tags.is_empty() {
                    None
                } else {
                    fetch_backdrop_cached(&client, &id2).await
                };

                // Build cast list (actors only, max 12)
                let cast: Vec<CastMember> = detail.people.iter()
                    .filter(|p| p.person_type == "Actor")
                    .take(12)
                    .map(|p| CastMember { name: p.name.as_str().into(), role: p.role.as_str().into() })
                    .collect();

                // Build meta string: "2023 • PG-13 • 2h 14m"
                let mut meta_parts: Vec<String> = vec![];
                if let Some(y) = detail.production_year { meta_parts.push(y.to_string()); }
                if let Some(ref r) = detail.official_rating { meta_parts.push(r.clone()); }
                if let Some(ref rt_str) = detail.runtime_string() { meta_parts.push(rt_str.clone()); }
                let meta = meta_parts.join(" • ");

                let genres = detail.genres.join(", ");
                let overview = detail.overview.clone().unwrap_or_default();
                let rating_label = detail.community_rating
                    .map(|r| format!("★ {:.1}", r))
                    .unwrap_or_default();
                let series_label = if detail.item_type == "Episode" {
                    let s = detail.parent_index_number.unwrap_or(0);
                    let e = detail.index_number.unwrap_or(0);
                    let series = detail.series_name.as_deref().unwrap_or("");
                    format!("{} — S{:02}E{:02}", series, s, e)
                } else { String::new() };
                let resume_secs = detail.resume_position_secs().unwrap_or(0.0);

                slint::invoke_from_event_loop(move || {
                    let Some(w) = ww3.upgrade() else { return };
                    // Only update if still showing this item
                    if w.get_detail_id().as_str() != id2 { return; }

                    w.set_detail_title(detail.name.as_str().into());
                    w.set_detail_series_label(series_label.as_str().into());
                    w.set_detail_meta(meta.as_str().into());
                    w.set_detail_genres(genres.as_str().into());
                    w.set_detail_overview(overview.as_str().into());
                    w.set_detail_rating_label(rating_label.as_str().into());
                    w.set_detail_can_resume(resume_secs > 0.0);
                    w.set_detail_resume_label(fmt_resume_label(resume_secs).into());
                    w.set_detail_cast(ModelRc::new(VecModel::from(cast)));
                    w.set_detail_loading(false);

                    // Poster
                    if let Some(bytes) = poster_bytes {
                        if let Ok(img) = image::load_from_memory(&bytes) {
                            let rgba = img.to_rgba8();
                            let (ww, hh) = rgba.dimensions();
                            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                                rgba.as_raw(), ww, hh);
                            w.set_detail_poster(slint::Image::from_rgba8(buf));
                            w.set_detail_has_poster(true);
                        }
                    }

                    // Backdrop
                    if let Some(bytes) = backdrop_bytes {
                        if let Ok(img) = image::load_from_memory(&bytes) {
                            let rgba = img.to_rgba8();
                            let (bw, bh) = rgba.dimensions();
                            let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                                rgba.as_raw(), bw, bh);
                            w.set_detail_backdrop(slint::Image::from_rgba8(buf));
                            w.set_detail_has_backdrop(true);
                        }
                    }
                }).ok();
            });
        });
    }
    {
        let state_pd  = Arc::clone(&state);
        let ww        = window.as_weak();
        let video_pd  = Arc::clone(&video);
        let rt_handle = rt.handle().clone();
        window.on_play_detail(move || {
            let Some(w) = ww.upgrade() else { return };
            let id = w.get_detail_id().to_string();
            if id.is_empty() { return }
            w.set_show_detail(false);
            let s = state_pd.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            // Don't resume — play from start
            config.start_position_secs = None;
            let found_pd  = s.media_raw.iter().find(|i| i.id == id).cloned();
            let item_type = found_pd.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
            let series_id = found_pd.and_then(|i| i.series_id);
            let title = w.get_detail_title().to_string();
            drop(s);
            let play_url = client.direct_play_url(&id);
            info!("play_detail: {}", id);
            start_playback(play_url, id, &item_type, title, config, client, &video_pd, &ww, &rt_handle);
            video_pd.lock().unwrap().playing_series_id = series_id;
        });
    }
    {
        let state_rd  = Arc::clone(&state);
        let ww        = window.as_weak();
        let video_rd  = Arc::clone(&video);
        let rt_handle = rt.handle().clone();
        window.on_resume_detail(move || {
            let Some(w) = ww.upgrade() else { return };
            let id = w.get_detail_id().to_string();
            if id.is_empty() { return }
            w.set_show_detail(false);
            let s = state_rd.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let mut config = s.player_config();
            let found = s.media_raw.iter().find(|i| i.id == id).cloned();
            config.start_position_secs = found.as_ref().and_then(|i| i.resume_position_secs());
            let item_type = found.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
            let series_id = found.and_then(|i| i.series_id);
            let title = w.get_detail_title().to_string();
            drop(s);
            let play_url = client.direct_play_url(&id);
            info!("resume_detail: {} from {:?}s", id, config.start_position_secs);
            start_playback(play_url, id, &item_type, title, config, client, &video_rd, &ww, &rt_handle);
            video_rd.lock().unwrap().playing_series_id = series_id;
        });
    }
    {
        let ww = window.as_weak();
        window.on_close_detail(move || {
            if let Some(w) = ww.upgrade() {
                w.set_show_detail(false);
                w.set_detail_id("".into());
            }
        });
    }

    // ── series drill-down ─────────────────────────────────────────────────────
    {
        let state_os = Arc::clone(&state);
        let ww_os    = window.as_weak();
        let rth_os   = rt.handle().clone();
        window.on_open_series(move |id| {
            open_series_screen(id.to_string(), state_os.clone(), ww_os.clone(), rth_os.clone());
        });
    }
    {
        let state_ss = Arc::clone(&state);
        let ww_ss    = window.as_weak();
        let rth_ss   = rt.handle().clone();
        window.on_series_select_season(move |idx| {
            let idx = idx as usize;
            let s   = state_ss.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let series_id = s.series_open_id.clone();
            let Some(season_id) = s.series_season_ids.get(idx).cloned() else { return };
            drop(s);
            if let Some(w) = ww_ss.upgrade() {
                w.set_series_loading(true);
                w.set_series_episodes(ModelRc::new(VecModel::<EpisodeEntry>::default()));
                w.set_series_focused_ep(0);
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
                { state_ss2.lock().unwrap().series_episode_items = eps.clone(); }
                let raws: Vec<EpisodeRaw> = eps.iter().map(make_episode_raw).collect();
                let sid3 = sid2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww_ss2.upgrade() else { return };
                    if w.get_series_id().as_str() != sid3 { return; }
                    let entries: Vec<EpisodeEntry> = raws.into_iter().map(raw_to_entry).collect();
                    w.set_series_episodes(ModelRc::new(VecModel::from(entries)));
                    w.set_series_loading(false);
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
        window.on_play_series_episode(move |id| {
            let id = id.to_string();
            let s  = state_pe.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            let ep_item = s.series_episode_items.iter().find(|i| i.id == id).cloned();
            let mut config = s.player_config();
            config.start_position_secs = ep_item.as_ref().and_then(|i| i.resume_position_secs());
            let series_id = ep_item.as_ref().and_then(|i| i.series_id.clone());
            drop(s);
            if let Some(w) = ww_pe.upgrade() { w.set_show_series(false); }
            let play_url = client.direct_play_url(&id);
            let title    = ep_item.map(|i| i.display_name()).unwrap_or_else(|| id.clone());
            info!("play_series_episode: {}", id);
            start_playback(play_url, id, "Episode", title, config, client, &video_pe, &ww_pe, &rth_pe);
            video_pe.lock().unwrap().playing_series_id = series_id;
        });
    }
    {
        let state_cs = Arc::clone(&state);
        let ww_cs    = window.as_weak();
        window.on_close_series(move || {
            debug!("close_series");
            if let Some(w) = ww_cs.upgrade() {
                w.set_show_series(false);
                w.set_series_id("".into());
            }
            let mut s = state_cs.lock().unwrap();
            s.series_open_id.clear();
            s.series_season_ids.clear();
            s.series_episode_items.clear();
        });
    }

    // ── auto-advance cancel ───────────────────────────────────────────────────
    {
        let state_ca = Arc::clone(&state);
        let ww_ca    = window.as_weak();
        window.on_cancel_auto_advance(move || {
            state_ca.lock().unwrap().next_ep_pending = None;
            if let Some(w) = ww_ca.upgrade() {
                w.set_show_next_ep_banner(false);
            }
        });
    }

    // ── player controls ───────────────────────────────────────────────────────
    {
        let video5 = Arc::clone(&video);
        let ww     = window.as_weak();
        window.on_pause_play_toggle(move || {
            let vs = video5.lock().unwrap();
            if let Some(p) = vs.player.as_ref() { p.toggle_pause(); }
            drop(vs);
            if let Some(w) = ww.upgrade() {
                let now_paused = !w.get_is_paused();
                debug!("pause_play_toggle → {}", if now_paused { "paused" } else { "playing" });
                w.set_is_paused(now_paused);
            }
        });
    }
    {
        let video6 = Arc::clone(&video);
        window.on_seek_backward(move || {
            if let Some(p) = video6.lock().unwrap().player.as_ref() {
                debug!("seek_backward 10s");
                p.seek_backward(10.0);
            }
        });
    }
    {
        let video7 = Arc::clone(&video);
        window.on_seek_forward(move || {
            if let Some(p) = video7.lock().unwrap().player.as_ref() {
                debug!("seek_forward 10s");
                p.seek_forward(10.0);
            }
        });
    }
    {
        let video_sbl = Arc::clone(&video);
        window.on_seek_backward_long(move || {
            if let Some(p) = video_sbl.lock().unwrap().player.as_ref() {
                debug!("seek_backward 30s");
                p.seek_backward(30.0);
            }
        });
    }
    {
        let video_sfl = Arc::clone(&video);
        window.on_seek_forward_long(move || {
            if let Some(p) = video_sfl.lock().unwrap().player.as_ref() {
                debug!("seek_forward 30s");
                p.seek_forward(30.0);
            }
        });
    }
    {
        let video8 = Arc::clone(&video);
        window.on_stop_playback(move || {
            info!("stop_playback requested");
            if let Some(p) = video8.lock().unwrap().player.as_ref() { p.stop(); }
        });
    }
    {
        let video_seek = Arc::clone(&video);
        window.on_seek_to(move |ratio| {
            let vs = video_seek.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                let dur = p.get_duration();
                if dur > 0.0 {
                    let secs = ratio as f64 * dur;
                    debug!("seek_to: ratio={:.3} → {:.1}s / {:.1}s", ratio, secs, dur);
                    p.seek_to(secs);
                }
            }
        });
    }
    {
        let video_si = Arc::clone(&video);
        window.on_skip_intro(move || {
            let vs = video_si.lock().unwrap();
            if let (Some(ref ts), Some(p)) = (vs.intro_timestamps.as_ref(), vs.player.as_ref()) {
                info!("skip intro: seeking to {:.1}s", ts.intro_end);
                p.seek_to(ts.intro_end);
            }
        });
    }
    {
        let video_sub = Arc::clone(&video);
        window.on_select_sub(move |id| {
            if let Some(p) = video_sub.lock().unwrap().player.as_ref() {
                debug!("select subtitle track id={}", id);
                p.set_sub_track(id as i64);
            }
        });
    }
    {
        let video_aud = Arc::clone(&video);
        window.on_select_audio(move |id| {
            if let Some(p) = video_aud.lock().unwrap().player.as_ref() {
                debug!("select audio track id={}", id);
                p.set_audio_track(id as i64);
            }
        });
    }
    {
        let video_cps = Arc::clone(&video);
        let ww = window.as_weak();
        window.on_commit_panel_selection(move || {
            let Some(w) = ww.upgrade() else { return };
            let panel  = w.get_player_open_panel();
            let cursor = w.get_player_panel_cursor() as usize;
            let vs = video_cps.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                match panel {
                    1 => {
                        // Sub panel: cursor 0 = Off, 1+ = sub-tracks[cursor-1]
                        let id = if cursor == 0 {
                            0i32
                        } else {
                            w.get_sub_tracks().row_data(cursor - 1).map(|t| t.id).unwrap_or(0)
                        };
                        debug!("commit sub: cursor={} → id={}", cursor, id);
                        p.set_sub_track(id as i64);
                        w.set_current_sub_id(id);
                    }
                    2 => {
                        let id = w.get_audio_tracks().row_data(cursor).map(|t| t.id).unwrap_or(1);
                        debug!("commit audio: cursor={} → id={}", cursor, id);
                        p.set_audio_track(id as i64);
                        w.set_current_audio_id(id);
                    }
                    3 => {
                        let id = w.get_video_tracks().row_data(cursor).map(|t| t.id).unwrap_or(1);
                        debug!("commit video: cursor={} → id={}", cursor, id);
                        p.set_video_track(id as i64);
                        w.set_current_video_id(id);
                    }
                    _ => {}
                }
            }
        });
    }
    {
        let video_vol_up = Arc::clone(&video);
        window.on_volume_up(move || {
            if let Some(p) = video_vol_up.lock().unwrap().player.as_ref() { p.adjust_volume(5.0); }
        });
    }
    {
        let video_vol_dn = Arc::clone(&video);
        window.on_volume_down(move || {
            if let Some(p) = video_vol_dn.lock().unwrap().player.as_ref() { p.adjust_volume(-5.0); }
        });
    }
    {
        let video_sv = Arc::clone(&video);
        let ww = window.as_weak();
        window.on_show_controls(move || {
            if let Some(w) = ww.upgrade() { w.set_controls_visible(true); }
            video_sv.lock().unwrap().controls_idle_ticks = 0;
        });
    }
    {
        let video_vid = Arc::clone(&video);
        window.on_select_video(move |id| {
            if let Some(p) = video_vid.lock().unwrap().player.as_ref() {
                debug!("select video track id={}", id);
                p.set_video_track(id as i64);
            }
        });
    }
    {
        let ww = window.as_weak();
        window.on_resume_player(move || {
            let Some(w) = ww.upgrade() else { return };
            if w.get_has_background_player() {
                info!("resuming player to fullscreen");
                w.set_is_playing(true);
                w.set_has_background_player(false);
                w.set_video_behind_ui(false);
                w.set_controls_visible(true);
            }
        });
    }
    {
        let video_mute = Arc::clone(&video);
        window.on_mute_toggle(move || {
            if let Some(p) = video_mute.lock().unwrap().player.as_ref() {
                p.toggle_mute();
            }
        });
    }
    {
        let ww = window.as_weak();
        window.on_toggle_stats(move || {
            let Some(w) = ww.upgrade() else { return; };
            w.set_stats_visible(!w.get_stats_visible());
        });
    }
    {
        let ww = window.as_weak();
        window.on_minimize_player(move || {
            let Some(w) = ww.upgrade() else { return; };
            let behind = w.get_settings_video_behind();
            w.set_is_playing(false);
            w.set_has_background_player(true);
            w.set_video_behind_ui(behind);
            w.set_stats_visible(false);
        });
    }
    // ── settings changed ──────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        window.on_settings_changed(move || {
            let Some(w) = window_weak.upgrade() else { return; };
            { let mut s = state.lock().unwrap(); read_settings_from_window(&w, &mut s); }
            if let Some(mut cfg) = load_config() {
                let s = state.lock().unwrap();
                cfg.audio_spdif            = s.audio_spdif;
                cfg.hwdec                  = s.hwdec.clone();
                cfg.hwdec_image_format     = s.hwdec_image_format.clone();
                cfg.vf                     = s.vf.clone();
                cfg.gpu_api                = s.gpu_api.clone();
                cfg.video_sync             = s.video_sync.clone();
                cfg.opengl_early_flush     = s.opengl_early_flush;
                cfg.video_latency_hacks    = s.video_latency_hacks;
                cfg.interpolation          = s.interpolation;
                cfg.tscale                 = s.tscale.clone();
                cfg.tone_mapping           = s.tone_mapping.clone();
                cfg.target_colorspace_hint = s.target_colorspace_hint;
                cfg.deinterlace            = s.deinterlace;
                cfg.cache_size_mb          = s.cache_size_mb;
                cfg.video_behind           = s.video_behind;
                cfg.launch_fullscreen      = s.launch_fullscreen;
                let launch_fs = s.launch_fullscreen;
                drop(s);
                save_config(&cfg);
                w.window().set_fullscreen(launch_fs);
                info!("settings saved");
            }
        });
    }

    // ── fullscreen toggle (F key / F11) ──────────────────────────────────────
    {
        let window_weak = window.as_weak();
        window.on_toggle_fullscreen(move || {
            if let Some(w) = window_weak.upgrade() {
                let fs = w.window().is_fullscreen();
                w.window().set_fullscreen(!fs);
            }
        });
    }

    // ── sign-out ──────────────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        window.on_sign_out(move || {
            let _ = std::fs::remove_file(config_path());
            let _ = std::fs::remove_file(item_cache_path());
            let mut s = state.lock().unwrap();
            s.client = None;
            s.media_raw.clear();
            s.all_movies.clear();
            s.all_series.clear();
            s.filtered_items.clear();
            s.series_open_id.clear();
            s.series_season_ids.clear();
            s.series_episode_items.clear();
            drop(s);
            if let Some(w) = window_weak.upgrade() {
                w.set_show_login(true);
                w.set_active_nav(0);
                w.set_show_browse(false);
                w.set_server_url(ss(""));
            }
        });
    }

    window.on_quit(|| { slint::quit_event_loop().ok(); });

    window.invoke_grab_keyboard_focus();
    window.run()?;
    Ok(())
}
