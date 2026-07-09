// ── fjord-app · config.rs ────────────────────────────────────────────────────
//   BoundedCache<V> FIFO+TTL cache (cap 40, ttl 5min) — Part 2 screen-open caches,
//                   see doc comment above the type
//   default_* fns   serde defaults for Config string fields
//   Config          persisted JSON: server, user, token, device_id, all settings;
//                   skip_*_mode: "always-skip"|"ask"|"ask-timed"|"never-skip";
//                   skip_*_secs: auto-skip countdown (ask-timed); credits secs for Up Next banner
//                   log_level: "error"|"warn"|"info"|"debug" — read once at startup
//                   before the tracing subscriber is built (main.rs); Settings→General row
//   FjordState      runtime app state: config (auth + all settings, canonical),
//                   client, library vecs, filtered lists, series cache, keybindings.
//                   audio_devices: Vec<(name, description)> fetched at startup from mpv.
//                   movie_collections: HashMap<movie_id, (boxset_id, boxset_name)> built in background.
//                   series_episode_cache: HashMap<season_id, Vec<MediaItem>> avoids re-fetching
//                     already-seen seasons; cleared when a new series is opened.
//                   series_season_generation: incremented on each season switch; async tasks compare
//                     on completion to discard stale results from rapid navigation.
//                   ws_abort: AbortHandle for the WebSocket reconnect task; abort on sign-out.
//                   item_detail_cache/similar_items_cache/boxset_items_cache/artist_albums_cache/
//                     person_filmography_cache/container_tracks_cache: BoundedCache<...> — screen-open
//                     caches keyed by item/container id (Part 2), shared across the 7 detail-style screens
//                   Adding a setting: add to Config only — FjordState.config is the copy.
//                   movies_fetched/artists_fetched/albums_fetched/playlists_fetched: true after first network fetch (guards re-fetch)
//                   next_ep_pending moved to VideoState — cleared automatically on start_playback
//   path helpers    xdg_config_base, xdg_cache_base (shared), config_path, poster_cache_dir/path, backdrop_cache_dir/path, keybindings_path
//   config I/O      load_config, save_config, ensure_device_id
//   keybindings I/O load_keybindings, save_keybindings
//   fmt_resume_label  format resume position as "1h 23m 45s"
//   upsert_media_item  replace-by-id-if-present-else-append; WS delta-sync merge helper
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;
use std::time::Instant;

use fjord_api::{models::MediaItem, JellyfinClient};
use fjord_player::PlayerConfig;
use serde::{Deserialize, Serialize};

use crate::keys::{Keybindings, default_keybindings};

pub(crate) fn default_audio_channels() -> String { "auto-safe".into() }
fn default_gapless() -> bool { true }
fn default_now_playing_auto_open() -> bool { true }
fn default_hwdec()        -> String { "auto".into()       }
pub(crate) fn default_video_sync()   -> String { "audio".into()      }
pub(crate) fn default_tscale()       -> String { "oversample".into() }
pub(crate) fn default_tone_mapping() -> String { "auto".into()       }
fn default_true()                    -> bool   { true                }
fn default_deinterlace()             -> String { "no".into()         }
fn default_skip_mode()               -> String { "ask".into()        }
fn default_log_level()               -> String { "info".into()       }
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
    #[serde(default)]                         pub sub_type:              String,
    #[serde(default)]                         pub audio_lang:            String,
    #[serde(default)]                         pub audio_device:          String,
    // Separate output for video while SPDIF passthrough is on ("" = same as
    // audio_device). Music always plays on audio_device.
    #[serde(default)]
    pub audio_device_passthrough: String,
    // mpv --audio-channels: "auto-safe" (mpv default, may downmix multichannel
    // PCM to stereo on direct ALSA devices), "auto", fixed layout, or a
    // negotiation list like "7.1,5.1,stereo".
    #[serde(default = "default_audio_channels")]
    pub audio_channels: String,
    // Gapless music playback: preload the next audio track into the same mpv
    // instance so album transitions have no gap. Kill switch in Settings→Audio.
    #[serde(default = "default_gapless")]
    pub gapless_audio: bool,
    // Auto-open the fullscreen Now Playing screen after ~30 s idle while music
    // plays. Fixed threshold in v1 — only the on/off is a setting.
    #[serde(default = "default_now_playing_auto_open")]
    pub now_playing_auto_open: bool,
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

    // ── Library sort (0=NameAZ 1=NameZA 2=YearDesc 3=YearAsc 4=Random) ─────────
    #[serde(default)] pub library_movies_sort:       u8,
    #[serde(default)] pub library_series_sort:       u8,
    #[serde(default)] pub library_collections_sort:  u8,
    #[serde(default)] pub library_artists_sort:      u8,
    #[serde(default)] pub library_albums_sort:       u8,
    #[serde(default)] pub library_playlists_sort:    u8,

    // ── Music library view (0=Artists, 1=Albums, 2=Playlists) ────────────────
    #[serde(default)] pub library_music_view:        u8,

    // ── Log level for fjord.log ("error"|"warn"|"info"|"debug") — read once at
    // startup before the tracing subscriber is built; changes apply on next launch.
    #[serde(default = "default_log_level")] pub log_level: String,
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
            sub_enabled: true, sub_lang: String::new(), sub_lang2: String::new(), sub_type: String::new(), audio_lang: String::new(),
            audio_device: String::new(),
            audio_device_passthrough: String::new(),
            audio_channels: default_audio_channels(),
            gapless_audio: true,
            now_playing_auto_open: true,
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
            library_movies_sort:       0,
            library_series_sort:       0,
            library_collections_sort:  0,
            library_artists_sort:      0,
            library_albums_sort:       0,
            library_playlists_sort:    0,
            library_music_view:        0,
            log_level:    default_log_level(),
            hwdec:        default_hwdec(),
            video_sync:   default_video_sync(),
            tscale:       default_tscale(),
            tone_mapping: default_tone_mapping(),
            vf:           String::new(),
        }
    }
}

fn home_dir() -> std::path::PathBuf {
    std::env::var("HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            tracing::error!("$HOME is not set — config/cache paths will be relative to CWD");
            std::path::PathBuf::from(".")
        })
}

pub(crate) fn xdg_config_base() -> std::path::PathBuf {
    std::env::var("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".config"))
}

pub(crate) fn xdg_cache_base() -> std::path::PathBuf {
    std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| home_dir().join(".cache"))
}

pub(crate) fn config_path() -> std::path::PathBuf {
    xdg_config_base().join("fjord").join("config.json")
}

pub(crate) fn poster_cache_dir() -> std::path::PathBuf {
    xdg_cache_base().join("fjord").join("posters")
}
pub(crate) fn backdrop_cache_dir() -> std::path::PathBuf {
    xdg_cache_base().join("fjord").join("backdrops")
}
pub(crate) fn poster_cache_path(item_id: &str) -> std::path::PathBuf {
    poster_cache_dir().join(item_id)
}
pub(crate) fn backdrop_cache_path(item_id: &str) -> std::path::PathBuf {
    backdrop_cache_dir().join(item_id)
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
    if let Ok(json) = serde_json::to_string_pretty(cfg) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
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
    xdg_config_base().join("fjord").join("keybindings.json")
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

// ── screen-open caches (Part 2 of the loading-consolidation plan) ────────────

/// FIFO + TTL cache: at most `cap` entries, oldest evicted first; an entry older
/// than `ttl` is treated as a miss. Used to skip the network round-trip for
/// screen-open fetches (get_item_detail / get_similar_items / etc) when an item
/// was viewed recently in this session — the goal is "reopening something you
/// just looked at shows instantly, no loading spinner." TTL (not precise
/// invalidation on every played/favorite mutation) is the correctness net: a
/// stale hit self-heals within a few minutes rather than needing every mutation
/// call site across 7 screens to remember to purge the right cache — the same
/// trade-off this session's flash-bug hunt kept re-learning the hard way about
/// "refresh in the background" paths, so v1 deliberately keeps this simple.
pub(crate) struct BoundedCache<V> {
    map:   std::collections::HashMap<String, (V, Instant)>,
    order: std::collections::VecDeque<String>,
    cap:   usize,
    ttl:   std::time::Duration,
}

impl<V: Clone> BoundedCache<V> {
    pub(crate) fn new(cap: usize, ttl: std::time::Duration) -> Self {
        Self { map: Default::default(), order: Default::default(), cap, ttl }
    }
    pub(crate) fn get(&self, key: &str) -> Option<V> {
        self.map.get(key)
            .filter(|(_, t)| t.elapsed() < self.ttl)
            .map(|(v, _)| v.clone())
    }
    pub(crate) fn insert(&mut self, key: String, value: V) {
        if !self.map.contains_key(&key) {
            self.order.push_back(key.clone());
            if self.order.len() > self.cap {
                if let Some(oldest) = self.order.pop_front() {
                    self.map.remove(&oldest);
                }
            }
        }
        self.map.insert(key, (value, Instant::now()));
    }
}

// ── app state (library + settings) ───────────────────────────────────────────

pub(crate) struct FjordState {
    pub config:               Config,          // authoritative settings + auth; saved on change
    pub client:               Option<Arc<JellyfinClient>>,
    pub keybindings:          Keybindings,
    pub all_movies:           Vec<MediaItem>,
    pub all_series:           Vec<MediaItem>,
    pub all_collections:      Vec<MediaItem>,
    pub all_artists:          Vec<MediaItem>,
    pub all_albums:           Vec<MediaItem>,
    pub all_playlists:        Vec<MediaItem>,
    pub movies_fetched:       bool,
    pub collections_fetched:  bool,
    pub artists_fetched:      bool,
    pub albums_fetched:       bool,
    pub playlists_fetched:    bool,
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
    pub ws_abort:             Option<tokio::task::AbortHandle>, // abort to stop the WS reconnect loop on sign-out
    // Screen-open caches (Part 2, see BoundedCache doc comment above). Keyed by
    // item id (or the relevant container id — boxset/artist/person/album/playlist).
    pub item_detail_cache:        BoundedCache<MediaItem>,       // get_item_detail — shared by all 7 screens
    pub similar_items_cache:      BoundedCache<Vec<MediaItem>>,  // get_similar_items — detail.rs + series.rs
    pub boxset_items_cache:       BoundedCache<Vec<MediaItem>>,  // get_boxset_items — detail.rs + collection.rs
    pub artist_albums_cache:      BoundedCache<Vec<MediaItem>>,  // get_artist_albums — artist.rs
    pub person_filmography_cache: BoundedCache<Vec<MediaItem>>,  // get_person_filmography — person.rs
    pub container_tracks_cache:   BoundedCache<Vec<MediaItem>>,  // get_album_tracks / get_playlist_items — album.rs
}

impl FjordState {
    pub(crate) fn new() -> Self {
        Self {
            config: Config::default(),
            client: None, keybindings: load_keybindings(),
            all_movies: vec![], all_series: vec![], all_collections: vec![], all_artists: vec![], all_albums: vec![],
            all_playlists: vec![],
            movies_fetched: false, collections_fetched: false, artists_fetched: false, albums_fetched: false,
            playlists_fetched: false, filtered_items: vec![],
            series_open_id: String::new(), series_season_ids: vec![], series_episode_items: vec![],
            series_episode_cache: std::collections::HashMap::new(), series_season_generation: 0,
            last_nw_mov_refresh: None,
            last_nw_tv_refresh: None,
            audio_devices: vec![],
            movie_collections: std::collections::HashMap::new(),
            ws_abort: None,
            item_detail_cache:        BoundedCache::new(40, std::time::Duration::from_secs(300)),
            similar_items_cache:      BoundedCache::new(40, std::time::Duration::from_secs(300)),
            boxset_items_cache:       BoundedCache::new(40, std::time::Duration::from_secs(300)),
            artist_albums_cache:      BoundedCache::new(40, std::time::Duration::from_secs(300)),
            person_filmography_cache: BoundedCache::new(40, std::time::Duration::from_secs(300)),
            container_tracks_cache:   BoundedCache::new(40, std::time::Duration::from_secs(300)),
        }
    }

    pub(crate) fn player_config(&self) -> PlayerConfig {
        let c = &self.config;
        PlayerConfig {
            audio_device:            c.audio_device.clone(),
            audio_device_passthrough: c.audio_device_passthrough.clone(),
            audio_channels:           c.audio_channels.clone(),
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
        let patch = |item: &mut MediaItem| {
            if item.id == id {
                if let Some(p) = played { item.user_data.played      = p; }
                if let Some(f) = fav    { item.user_data.is_favorite = f; }
            }
        };
        for list in [
            &mut self.all_movies, &mut self.all_series, &mut self.all_collections,
            &mut self.all_artists, &mut self.all_albums, &mut self.all_playlists,
            &mut self.filtered_items, &mut self.series_episode_items,
        ] {
            for item in list.iter_mut() { patch(item); }
        }
        for eps in self.series_episode_cache.values_mut() {
            for item in eps.iter_mut() { patch(item); }
        }
    }

}

/// Replace-if-present-else-append by id. Used by the WS LibraryChanged/UserDataChanged
/// delta-sync path to merge added/updated items into a cached list without a full re-fetch.
pub(crate) fn upsert_media_item(list: &mut Vec<MediaItem>, item: MediaItem) {
    match list.iter_mut().find(|i| i.id == item.id) {
        Some(existing) => *existing = item,
        None           => list.push(item),
    }
}
