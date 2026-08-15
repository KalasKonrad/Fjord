// ── fjord-app · config.rs ────────────────────────────────────────────────────
//   BoundedCache<V> FIFO cache (cap 40 by default), Serialize/Deserialize + Clone —
//                   Part 2/103 screen-open caches; freshness via WS invalidation +
//                   post-login refresh sweep, not a TTL (dropped in Phase 103);
//                   set_cap (raise-only) + recent_keys(n) added Phase 104 for the
//                   opt-in library prewarm; iter() -> (&str, &V) added in review
//                   (2026-07-11) for callers walking every entry without cloning;
//                   clear() added same day, called on sign-out — these caches hold
//                   per-user data with no user/server scoping; keys() (superseded
//                   by iter()) removed as dead code once its one remaining caller
//                   switched over; see doc comment above the type
//   ScreenCachesFile  on-disk snapshot of the six caches (screen_caches.json, Phase 103)
//   screen_caches_path/load_screen_caches/save_screen_caches  Phase 103 persistence I/O
//   RememberedTracks  { audio_lang, sub_lang } — one series' manually-picked S/A track languages
//   default_* fns   serde defaults for DeviceConfig/ProfileSettings string fields
//   sub_color_hex   ProfileSettings.sub_color display name ("White"/...) -> mpv hex color,
//                   "" = don't touch
//   vf_mpv_value    DeviceConfig.vf display label ("auto: nv12/p010"/"auto: yuv420p/...") -> raw
//                   mpv sentinel ("" / "auto"); deser_vf migrates a pre-relabel raw value forward
//   DeviceConfig    settings that describe THIS PHYSICAL BOX regardless of who's signed in —
//                   hwdec/vf/audio device/seek step/animation speed/log_level/launch_policy
//                   (Bonfire profile-picker launch policy, within one account) + default_profile_id;
//                   account_launch_policy/default_account_id (2026-08-14, 2-tier account/profile
//                   redesign — profile.rs's StartupGate doc comment has the full resolution order)
//                   are the identical 3-way shape one tier UP: which ACCOUNT (a plain login, or a
//                   whole Bonfire household) to resolve silently at startup, before ever touching
//                   which profile within it.
//   ProfileSettings settings that follow the SIGNED-IN PERSON — auth (server_url/user_id/token),
//                   subtitle/audio language, library sort, Seerr connection, Discover filters,
//                   skip_*_mode/_secs, trailer_quality. keyed by user_id (also
//                   Config.active_profile_id's lookup key). remember_login (2026-08-14, default
//                   true) — the real security gate the 2-tier redesign was built to close: false
//                   means this profile's ROOT account can never be silently resumed at startup
//                   regardless of account_launch_policy, always demanding a fresh password via
//                   StartupGate::RequireLogin — a plain (non-Bonfire) account has no PIN concept
//                   at all, so without this a stored token would be a built-in bypass around
//                   every PIN in a sibling Bonfire household on the same install.
//   Config          { device: DeviceConfig, profiles: Vec<ProfileSettings>, active_profile_id }
//                   (Bonfire Phase 1, 2026-08-08 — was one flat struct before this; see the
//                   device/profile split's own doc comment above DeviceConfig for the full "why").
//                   active()/active_mut() are the ONLY way other code should read/write a
//                   profile-scoped field. v1 (this commit) always has exactly one profile —
//                   no picker, no switching UI yet, deliberately zero behavior change from the
//                   old flat Config. skip_*_mode: "always-skip"|"ask"|"ask-timed"|"never-skip";
//                   skip_*_secs: auto-skip countdown (ask-timed); credits secs for Up Next banner.
//                   log_level: "error"|"warn"|"info"|"debug" — read once at startup before the
//                   tracing subscriber is built (main.rs); Settings→General row.
//                   seerr_enabled/seerr_url/seerr_auth_method/seerr_api_key/seerr_session_cookie —
//                   Seerr integration (Settings→Integrations), cleared on sign-out with the
//                   Jellyfin auth fields; token/seerr_api_key/seerr_session_cookie are encrypted
//                   at rest — see load_config/save_config and secrets.rs (looped over
//                   cfg.profiles now, not three flat fields — secrets.rs itself is unchanged).
//                   discover_filter_type/_genre_names/_sort/_min_rating/_min_year/_provider_ids
//                   (2026-07-18) — Discover screen's persisted filter selections; profile-scoped
//                   as of the Phase 1 split (a deliberate reversal of the old "not tied to Seerr
//                   connection state so not cleared on sign-out" — see ProfileSettings' own doc
//                   comment); genre persisted by name (stable across movie/TV id-space mismatch),
//                   provider by id (TMDB watch-provider ids are shared across movie/TV).
//   LegacyConfig/migrate_legacy_config  the pre-Phase-1 flat shape + its one-time migration into
//                   the device/profiles shape — see load_config's own doc comment
//   FjordState      runtime app state: config (auth + all settings, canonical),
//                   client, library vecs, filtered lists, series cache, keybindings.
//                   audio_devices: Vec<(name, description)> fetched at startup from mpv.
//                   system_fonts: Vec<(value, display)> fetched at startup via fc-list, same
//                     pattern as audio_devices — see fetch_system_fonts (main.rs).
//                   movie_collections: HashMap<movie_id, (boxset_id, boxset_name)> built in background.
//                   remembered_tracks: HashMap<series_id, RememberedTracks> — manual S/A panel picks,
//                     session-only, cleared on sign-out; see RememberedTracks doc comment.
//                   series_episode_cache: HashMap<season_id, Vec<MediaItem>> avoids re-fetching
//                     already-seen seasons; cleared when a new series is opened.
//                   series_season_generation: incremented on each season switch; async tasks compare
//                     on completion to discard stale results from rapid navigation.
//                   ws_abort: AbortHandle for the WebSocket reconnect task; abort on sign-out.
//                   item_detail_cache/similar_items_cache/boxset_items_cache/artist_albums_cache/
//                     person_filmography_cache/container_tracks_cache: BoundedCache<...> — screen-open
//                     caches keyed by item/container id (Part 2), shared across the 7 detail-style screens;
//                     persisted as one unit to screen_caches.json (Phase 103)
//                   person_tmdb_id_cache (2026-07-29, Deep Seerr integration) — Jellyfin person id ->
//                     TMDB person id (None = tried, no match); also persisted via ScreenCachesFile,
//                     since the fuzzy name-search fallback is comparatively expensive to repeat.
//                     person_other_work_cache — the row's own DiscoverCardMeta results, session-only
//                     (item_type is &'static str, can't round-trip through serde); local_person_by_tmdb_cache
//                     (2026-08-13) — TMDB person id -> matching local Jellyfin Person id (None = no
//                     match), session-only, cleared on sign-out/profile-switch only (not Seerr
//                     connect/disconnect — the mapping depends on the Jellyfin library, not Seerr)
//                   discover_watchlist_ids/discover_watchlist_fetched/discover_calendar_entries/
//                     seerr_discover_region (2026-07-18, Watchlist + Release Calendar) — Seerr-
//                     connection-scoped, cleared alongside discover_known_requests/
//                     seerr_streaming_region in clear_connection/commit_connection/sign-out; see
//                     discover.rs's own TOC header for the fetch/rebuild functions
//                   jellyfin_watchlist_ids (2026-07-20) — the resolved set of LOCAL Jellyfin
//                     ids on the Seerr watchlist (tmdb-id-keyed discover_watchlist_ids' own
//                     Jellyfin-id-keyed counterpart); real bug fix — item_to_card_item/
//                     items_to_model consult this directly at CardItem-construction time
//                     because a live model patch alone (context_menu::patch_watchlist_on_-
//                     jellyfin_models) gets silently wiped by the next screen rebuild, since
//                     MediaItem itself has no watchlist concept; kept authoritative by
//                     discover::resync_jellyfin_watchlist_stars (wholesale replace) + a per-
//                     toggle incremental update in discover_toggle_watchlist; cleared
//                     alongside discover_watchlist_ids in clear_connection/commit_connection/
//                     sign-out — see its own doc comment for the full story ("the watch list
//                     symbol do not show up on items i the library screens")
//                   screen_revalidate_last_run (2026-07-31, performance sweep) — rate-limits the
//                     7 screen "revalidate on cache hit" functions to once per 60s per item id
//                     (main.rs::should_revalidate); same missing-guard bug class as
//                     seerr_admin_last_refresh, cleared on sign-out
//                   pending_keybind_rebind: Option<keys::PendingKeybindRebind> (2026-08-07,
//                     Key Bindings rebind-collision confirm) — stashed (row, combo) while
//                     `show-keybinding-collision-confirm` is open; consumed by
//                     `on_keybinding_collision_confirmed`/`_cancelled` (main.rs); cleared on
//                     sign-out alongside the other transient Settings UI-flow flags
//                   Adding a setting: add to Config only — FjordState.config is the copy.
//                   movies_fetched/artists_fetched/albums_fetched/playlists_fetched: true after first network fetch (guards re-fetch)
//                   next_ep_pending moved to VideoState — cleared automatically on start_playback
//   path helpers    xdg_config_base, xdg_cache_base (shared), config_path, poster_cache_dir/path, backdrop_cache_dir/path,
//                   discover_poster_cache_dir/path (Seerr/TMDB posters — separate dir, no Jellyfin tag-revalidation concept), keybindings_path
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

// Translates a Config.sub_color display name ("White"/"Yellow"/...) into the
// mpv hex color it maps to — mirrors sub_lang display-name-to-code
// translation in playback.rs, kept out of PlayerConfig/fjord-player since
// that crate stays UI-preset-agnostic. "" (Default) and anything unknown
// both map to "" — meaning "don't touch sub-color at all", not "set to white".
pub(crate) fn sub_color_hex(name: &str) -> &str {
    match name {
        "White"  => "#FFFFFF",
        "Yellow" => "#FFFF00",
        "Cyan"   => "#00FFFF",
        "Green"  => "#00FF00",
        _        => "",
    }
}

/// Translates the Settings dropdown's display value for the `vf` row to the
/// raw mpv-facing sentinel/value `fjord_player::PlayerConfig` and `Player`
/// actually understand (same display-name-stores-directly idiom as
/// `sub_color_hex`, mirrored here since `SettingsDropdown` has no separate
/// label/value concept — its model entries ARE both). The two "auto" labels
/// map to the pre-existing `""` (native nv12/p010, no filter forced) and
/// `"auto"` (runtime yuv420p/yuv420p10le stride-fix, see `apply_auto_vf`)
/// sentinels `fjord-player` already checks for; every other value (the four
/// explicit `format=...` options, and any pre-relabel legacy config.json
/// value from before this dropdown was reworded) passes through unchanged.
pub(crate) fn vf_mpv_value(display: &str) -> String {
    match display {
        "auto: nv12/p010"          => String::new(),
        "auto: yuv420p/yuv420p10le" => "auto".to_string(),
        other                        => other.to_string(),
    }
}

pub(crate) fn default_audio_channels() -> String { "auto-safe".into() }
fn default_gapless() -> bool { true }
fn default_skip_fade_mute_passthrough() -> bool { true }
fn default_now_playing_auto_open() -> bool { true }
fn default_hwdec()        -> String { "auto".into()       }
pub(crate) fn default_video_sync()   -> String { "audio".into()      }
pub(crate) fn default_tscale()       -> String { "oversample".into() }
pub(crate) fn default_tone_mapping() -> String { "auto".into()       }
fn default_true()                    -> bool   { true                }
fn default_deinterlace()             -> String { "no".into()         }
fn default_vf()                      -> String { "auto: nv12/p010".into() }
fn default_skip_mode()               -> String { "ask".into()        }
fn default_log_level()               -> String { "info".into()       }
fn default_skip_secs()               -> u32    { 8                   }
fn default_credits_secs()            -> u32    { 30                  }
fn default_seek_step()               -> u32    { 10                  }
fn default_seek_step_long()          -> u32    { 30                  }
// Base duration (ms) of the skip-segment fade-to-black, before the
// settings-animation-speed multiplier is applied — matches the literal
// value this replaced (playback.rs's old hardcoded SKIP_FADE_MS const).
fn default_skip_fade_ms()            -> u32    { 200                 }
fn default_sub_pct()                 -> u32    { 100                 }
fn default_speed_pct()               -> u32    { 100                 }
// "Inter" = Fjord's own bundled default text font; "" = system default (no
// font-family override at all); anything else = that system font by name.
fn default_ui_font_family()          -> String { "Inter".into()      }
fn default_cache_secs()              -> u32    { 60                  }
fn default_cache_max_mb()            -> u32    { 500                 }
// Display-ready values stored directly in Config, same idiom as
// Config.sub_color/SUB_COLOR_MODEL ("White"/"Yellow" are both the stored
// value and the display string) — translated to an mpv ytdl-format string
// only at point of use (main.rs::trailer_ytdl_format), not here. Caps
// yt-dlp's resolution selection for Watch Trailer playback
// (Settings → Integrations → Trailer Quality).
fn default_trailer_quality()         -> String { "1080p".into()      }

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

// Migrate the raw sentinel/mpv-value strings this field stored before the
// vf dropdown was reworded to self-describing labels (2026-07-31) forward
// to their new display equivalent — `vf_mpv_value()` already treats these
// two old raw forms and their new labels as interchangeable for actual
// playback, but without this an install upgrading from before the reword
// would show the now-unrecognized old value as a blank "(none)" in the
// Settings dropdown until the user happened to touch it. The four explicit
// `format=...` values are unchanged by the reword and pass through as-is.
fn deser_vf<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match String::deserialize(d)?.as_str() {
        ""      => "auto: nv12/p010",
        "auto"  => "auto: yuv420p/yuv420p10le",
        other   => return Ok(other.to_string()),
    }.into())
}

// ── Config: device-scoped vs profile-scoped split (Bonfire Phase 1) ──────────
//
// A Bonfire sub-profile switch and a full sign-out/sign-in-as-someone-else
// are the same underlying event: a new Jellyfin user_id+token becomes
// active. Before this split, `Config` was one flat struct — every setting
// followed whichever account happened to be signed in on this install, so a
// second profile would silently inherit the first's subtitle language,
// library sort order, Seerr connection, etc. `DeviceConfig` holds settings
// that describe THIS PHYSICAL BOX regardless of who's using it (hwdec,
// audio device, seek step, animation speed, log level...); `ProfileSettings`
// holds everything that should follow the signed-in person instead (auth,
// subtitle/audio language, library sort, Seerr connection, Discover
// filters...). `Config.profiles: Vec<ProfileSettings>` + `active_profile_id`
// is deliberately a `Vec` even though v1 (this commit) only ever has exactly
// one entry and no UI to add a second — see the Bonfire integration plan for
// the full profile-switching design this sets up. `Config.active()`/
// `active_mut()` are the ONLY way any other code should read/write a
// profile-scoped field; every call site was updated to go through them as
// part of this same change (verified by the compiler — moving a field out
// of a flat struct into a nested one makes every remaining direct reference
// a compile error with an exact file:line, which is what actually drove this
// pass, not a manual audit).
//
// Field classification below mirrors the plan's own table, with two
// corrections made while writing this code against the real struct (not the
// plan's earlier draft of it): `now_playing_auto_open` is profile-scoped
// (pure per-viewer UX preference, its own doc comment below has no
// hardware-compatibility framing the way `gapless_audio`'s "kill switch"
// wording does); `cache_secs`/`cache_max_mb` stay device-scoped (about the
// box's own network path, not per-viewer taste).
//
// This whole restructuring is meant to be genuinely ZERO BEHAVIOR CHANGE —
// one profile behaves identically to the single flat Config that existed
// before it. Nothing here builds the profile PICKER, switching UI, or
// Bonfire API calls yet; those are later, separate commits in the same
// phase, deliberately kept out of this one so it stays small enough to
// bisect on its own if anything regresses.

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct DeviceConfig {
    #[serde(default)] pub device_id: String,

    #[serde(default)]                         pub audio_spdif:           bool,
    #[serde(default = "default_true")]        pub spdif_ac3:             bool,
    #[serde(default = "default_true")]        pub spdif_eac3:            bool,
    #[serde(default = "default_true")]        pub spdif_dts:             bool,
    #[serde(default = "default_true")]        pub spdif_dts_hd:          bool,
    #[serde(default = "default_true")]        pub spdif_truehd:          bool,
    #[serde(default = "default_hwdec")]       pub hwdec:                 String,
    #[serde(default = "default_vf", deserialize_with = "deser_vf")]
                                               pub vf:                    String,
    #[serde(default = "default_video_sync")]  pub video_sync:            String,
    #[serde(default)]                         pub opengl_early_flush:    bool,
    #[serde(default)]                         pub video_latency_hacks:   bool,
    #[serde(default)]                         pub interpolation:         bool,
    #[serde(default = "default_tscale")]      pub tscale:                String,
    #[serde(default = "default_tone_mapping")]pub tone_mapping:          String,
    #[serde(default)]                         pub target_colorspace_hint:bool,
    #[serde(default = "default_deinterlace", deserialize_with = "deser_deinterlace")]
                                              pub deinterlace:           String,
    // ── Network cache (Settings → Player → Buffering) ───────────────────────
    // cache_secs sets mpv's `cache-secs` directly (0 = mpv's own default,
    // which is enormous — ~3.6M seconds per the real mpv manual — so in
    // practice cache_max_mb is what actually binds). cache_max_mb sets
    // `demuxer-max-bytes`, the real byte ceiling mpv enforces by default
    // (150 MiB) regardless of cache_secs — confirmed 2026-08-09 against the
    // installed mpv 0.41.0 manual after a real HTPC network outage showed
    // the *previous* single `cache_size_mb` setting only ever adjusted
    // cache-secs via an arbitrary `×0.8` conversion and never touched
    // demuxer-max-bytes at all, meaning it could never actually raise the
    // real buffer past mpv's stock 150 MiB — see CLAUDE.md's "Playback
    // resilience" section for the full story. cache_max_mb's own 0 means
    // something different from cache_secs's — "Unlimited" (fjord-player's
    // `Player::new` raises demuxer-max-bytes to a large fixed ceiling
    // instead of leaving mpv's small 150 MiB stock default in place, which
    // would be the most RESTRICTIVE choice on the row, the opposite of
    // unlimited), for a user who wants cache_secs alone to govern.
    #[serde(default = "default_cache_secs")]   pub cache_secs:            u32,
    #[serde(default = "default_cache_max_mb")] pub cache_max_mb:          u32,
    #[serde(default)]                         pub video_behind:          bool,
    #[serde(default)]                         pub launch_fullscreen:     bool,
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
    #[serde(default)]                         pub alsa_irq_scheduling:   bool,
    // Whether the skip-segment fade (see skip_fade_ms below) mutes audio for
    // its whole duration when SPDIF passthrough is active — the closest
    // available analog to a fade there, since a raw bitstream can't be
    // volume-ramped like PCM (Settings → Audio → Passthrough; 2026-08-11
    // follow-up: "add a setting for mute during fade for audio passthrou so
    // i only can turn off that and not the video fade" — this only ever
    // gates the passthrough mute, never the video fade or the PCM ramp,
    // both of which stay unconditional regardless of this toggle).
    #[serde(default = "default_skip_fade_mute_passthrough")]
    pub skip_fade_mute_passthrough: bool,

    // ── Log level for fjord.log ("error"|"warn"|"info"|"debug") — read once at
    // startup before the tracing subscriber is built; changes apply on next launch.
    #[serde(default = "default_log_level")] pub log_level: String,

    // ── Player seek step (Settings → Player → Seeking) ──────────────────────
    #[serde(default = "default_seek_step")]      pub seek_step_secs:      u32,
    #[serde(default = "default_seek_step_long")] pub seek_step_long_secs: u32,

    // ── Skip-segment fade-to-black duration (Settings → Player → Seeking,
    // appended alongside seek_step for the same "no natural home, append at
    // the end" reason) — base ms before the settings-animation-speed
    // multiplier; 0 = instant, same hard cut as before this feature shipped.
    // Closer to "how this screen/setup feels" than personal viewer taste
    // (same reasoning already applied to seek_step_secs), so device- not
    // profile-scoped.
    #[serde(default = "default_skip_fade_ms")] pub skip_fade_ms: u32,

    // ── UI animation speed (Settings → UI) — multiplier percentages, 100 = mpv/
    // widgets.slint's original hand-tuned durations unchanged. Scroll is kept
    // separate from general Animation since it's a throughput property (how
    // fast you can move through content), not decoration.
    #[serde(default = "default_speed_pct")] pub scroll_speed_pct:    u32,
    #[serde(default = "default_speed_pct")] pub animation_speed_pct: u32,

    // ── Text font (Settings → UI) — "Inter" (bundled default), "" (system
    // default, no override), or any other font family installed on the host.
    #[serde(default = "default_ui_font_family")] pub ui_font_family: String,

    // ── Bonfire profile-picker launch policy (Phase 1, not yet wired to a
    // Settings row or the picker screen in this commit) — "always_ask" |
    // "remember_last" | "default". default_profile_id is that profile's
    // user_id, meaningful only when launch_policy == "default".
    #[serde(default = "default_launch_policy")] pub launch_policy: String,
    #[serde(default)] pub default_profile_id: String,

    // ── Account-tier picker (2026-08-14) — the identical launch-policy
    // shape one level up, mirroring launch_policy/default_profile_id
    // exactly: "always_ask" | "remember_last" | "default". Only actually
    // consulted once should_show_picker_at_startup finds 2+ distinct
    // ACCOUNTS (grouped by profile::account_root_id, not raw profile
    // count) — a single-account install (every existing one, today) never
    // reads either of these. default_account_id is that account's own
    // root user_id (the master's, or a standalone plain profile's own).
    #[serde(default = "default_launch_policy")] pub account_launch_policy: String,
    #[serde(default)] pub default_account_id: String,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            audio_spdif: false,
            spdif_ac3: true, spdif_eac3: true, spdif_dts: true, spdif_dts_hd: true, spdif_truehd: true,
            hwdec: default_hwdec(), vf: "auto: nv12/p010".into(), video_sync: default_video_sync(),
            opengl_early_flush: false, video_latency_hacks: false,
            interpolation: false, tscale: default_tscale(), tone_mapping: default_tone_mapping(),
            target_colorspace_hint: false, deinterlace: "no".into(),
            cache_secs: default_cache_secs(), cache_max_mb: default_cache_max_mb(),
            video_behind: false, launch_fullscreen: false,
            audio_device: String::new(), audio_device_passthrough: String::new(),
            audio_channels: default_audio_channels(),
            gapless_audio: true, alsa_irq_scheduling: false,
            skip_fade_mute_passthrough: default_skip_fade_mute_passthrough(),
            log_level: default_log_level(),
            seek_step_secs: default_seek_step(), seek_step_long_secs: default_seek_step_long(),
            skip_fade_ms: default_skip_fade_ms(),
            scroll_speed_pct: 100, animation_speed_pct: 100,
            ui_font_family: default_ui_font_family(),
            launch_policy: default_launch_policy(),
            default_profile_id: String::new(),
            account_launch_policy: default_launch_policy(),
            default_account_id: String::new(),
        }
    }
}

fn default_launch_policy() -> String { "always_ask".into() }

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ProfileSettings {
    // The profile's identity — a Jellyfin user id. Doubles as the key
    // `Config.active_profile_id` matches against; see `Config::active()`.
    #[serde(default)] pub user_id:    String,
    #[serde(default)] pub server_url: String,
    #[serde(default)] pub token:      String,

    // ── Profile identity (Bonfire Phase 1, step 6, 2026-08-09) — ProfilePickerScreen's
    // own avatar tile. display_name/avatar_initial default to the Jellyfin username's
    // own first letter on a normal/append login (profile.rs); a real Bonfire sub-profile
    // gets these overwritten from the plugin's own /list response the next time
    // sync_bonfire_subprofiles runs (after every successful master login).
    #[serde(default)] pub display_name:   String,
    // Hex string ("#4a90d9"), not a Slint `color` — this struct has to stay
    // plain-serde-serializable; profile.rs parses it (or picks a deterministic
    // fallback when empty) when building the Slint-facing ProfileTile.
    #[serde(default)] pub avatar_color:   String,
    #[serde(default)] pub avatar_initial: String,
    // True once discovered via a real bonfire_list_profiles() response — never
    // true for a profile that only ever came from do_login/append (normal
    // sign-in has no way to know it's talking to a Bonfire sub-profile; only
    // the MASTER's own /list call can tell us that).
    #[serde(default)] pub is_bonfire:     bool,
    #[serde(default)] pub master_user_id: String,
    // Cached from the same /list response — may go stale between syncs, same
    // caveat as every other cached-until-next-refresh field in this app.
    #[serde(default)] pub has_pin:        bool,
    // Account-tier picker (2026-08-14, live-reported design feedback: "if
    // there is no bonfire on the other server everyone can use that
    // accaunt as the session is saved... mabey add a setting to remember
    // login"). Only meaningful on an ACCOUNT-root entry (is_bonfire ==
    // false — a Bonfire sub-profile is switched into via the master's own
    // token, never its own stored one, so this flag has nothing to gate
    // for it). Default true — preserves today's actual behavior (every
    // account silently resumes via its stored token) for every existing
    // install and every newly-added account unless explicitly turned off;
    // when false, that account's own StartupGate/switch path must show a
    // fresh Login (password required) instead of ever silently resuming
    // via the stored token, regardless of what account/profile launch
    // policy would otherwise decide. Set via the "Remember this login"
    // checkbox on LoginScreen at Add-Account time (also the very first
    // login) — see should_show_picker_at_startup's own doc comment for
    // where this is actually enforced.
    #[serde(default = "default_true")] pub remember_login: bool,

    #[serde(default = "default_true")]         pub sub_enabled:           bool,
    #[serde(default)]                         pub sub_lang:              String,
    #[serde(default)]                         pub sub_lang2:             String,
    #[serde(default)]                         pub sub_type:              String,
    // ── Subtitle appearance — see the mpv sub-ass-override note on
    // sub_respect_ass_styling below; scale/pos apply to ASS subtitles
    // unconditionally (mpv's own default), color/background do not.
    #[serde(default = "default_sub_pct")]     pub sub_scale_pct:         u32,
    #[serde(default = "default_sub_pct")]     pub sub_pos_pct:           u32,
    // true (default) = don't touch mpv's sub-ass-override (its own default,
    // "scale" tier — embedded ASS styling is respected); false = force it,
    // so sub_color/sub_background below also apply to ASS-styled subtitles.
    #[serde(default = "default_true")]        pub sub_respect_ass_styling: bool,
    // Display name from a static preset table ("" | "White" | "Yellow" | ...),
    // NOT a raw mpv color string — mirrors sub_lang storing a display name
    // that's translated to an mpv value at point of use.
    #[serde(default)]                         pub sub_color:             String,
    #[serde(default)]                         pub sub_background:        bool,
    #[serde(default)]                         pub audio_lang:            String,
    // Auto-open the fullscreen Now Playing screen after ~30 s idle while music
    // plays. Fixed threshold in v1 — only the on/off is a setting. Profile-
    // scoped (2026-08-08, Bonfire Phase 1): pure per-viewer UX preference,
    // unlike gapless_audio's hardware-compatibility framing.
    #[serde(default = "default_now_playing_auto_open")]
    pub now_playing_auto_open: bool,

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

    // ── Seerr integration (Settings → Integrations) — cleared on sign-out
    // alongside server_url/user_id/token. seerr_auth_method is purely
    // informational (drives the "Connected via X" Settings subtitle);
    // seerr_api_key is populated only when method == "apikey",
    // seerr_session_cookie for the other three methods. Both encrypted at
    // rest — see secrets.rs and load_config/save_config above.
    #[serde(default)] pub seerr_enabled:         bool,
    #[serde(default)] pub seerr_url:              String,
    #[serde(default)] pub seerr_auth_method:      String,
    #[serde(default)] pub seerr_api_key:          String,
    #[serde(default)] pub seerr_session_cookie:   String,

    // ── Watch Trailer (Discover request-detail screen) ───────────────────────
    // "Best"|"1080p"|"720p"|"480p" — display-ready, mapped to an mpv
    // ytdl-format string by main.rs::trailer_ytdl_format.
    #[serde(default = "default_trailer_quality")] pub trailer_quality: String,

    // ── Discover filters (2026-07-18) ─────────────────────────────────────
    // "" / empty Vec / 0 all mean "no filter set" for their respective field,
    // matching this file's existing empty-string-means-unset convention
    // (sub_lang etc). discover_filter_type: "" (All) | "movie" | "tv".
    // discover_filter_sort: "" (Popularity, Seerr/TMDB's own default) |
    // "rating" | "newest" | "oldest" — an internal key, translated to the
    // correct per-media-type TMDB sortBy value at request time (discover.rs),
    // not the TMDB value itself, since the same internal key means a
    // different literal string for movies vs TV (primary_release_date.* vs
    // first_air_date.*). discover_filter_min_rating: 0.0 = Any, else the
    // bucket floor (6.0/7.0/8.0). discover_filter_min_year: 0 = Any, else
    // the bucket floor (2000/2010/2015/2020). Provider ids are TMDB ids as
    // selected in the Provider chip picker (real TMDB watch-provider ids are
    // shared across movie/TV, unlike genre ids — see DiscoverFilters' own
    // doc comment in fjord-seerr), ORed together at request time. Genre is
    // persisted by NAME, not id — movie/TV genre id spaces don't fully
    // overlap even for same-named genres, so a raw id alone is ambiguous
    // once discover_filter_type changes; name is the stable identity
    // `GenreItem` dedup already uses, re-resolved to whichever type-specific
    // id is actually needed at request time.
    //
    // Profile-scoped (2026-08-08, Bonfire Phase 1) — a deliberate reversal
    // of this block's own earlier "not tied to Seerr connection state so not
    // cleared on sign-out" note: these now reset to blank while no profile
    // is active and are restored when that profile is switched back to,
    // since Discover filters really are personal taste.
    #[serde(default)] pub discover_filter_type:          String,
    #[serde(default)] pub discover_filter_genre_names:   Vec<String>,
    #[serde(default)] pub discover_filter_sort:           String,
    #[serde(default)] pub discover_filter_min_rating:     f32,
    #[serde(default)] pub discover_filter_min_year:       u32,
    #[serde(default)] pub discover_filter_provider_ids:   Vec<i64>,

    // ── Request Options "remember last choice" (2026-08-12) ──────────────────
    // Direct request, matching Seerr's own web UI behavior ("seerr always
    // remember what you hade chosen last time so it shuld mirror it"):
    // Quality/Profile/Tags picked in the RequestOptionsOverlay modal are
    // remembered permanently and re-applied as the starting point the next
    // time the modal opens for a DIFFERENT item — not just within the same
    // item's session, which already worked for free (nothing ever reset
    // those fields between a Cancel and a reopen, or between toggling
    // Quality and back). Split by media type (movie vs tv) since Seerr's own
    // Radarr/Sonarr split means the two have entirely separate
    // profile/tag id spaces and a user may reasonably want a different
    // default tier for each; each bucket remembers BOTH tiers' own
    // profile+tags at once (not just whichever tier was actually
    // submitted), mirroring the existing session-only 2K/4K "alt" swap
    // (discover::set_quality) so toggling tiers in the modal doesn't lose
    // whatever was independently picked on the other one. Persisted only on
    // a successful Request/Save (submit_request/submit_edit_request), not
    // on every intermediate toggle — Cancel shouldn't overwrite what was
    // remembered before. Profile/tag ids that no longer exist on the server
    // (config changed) are simply not found when re-applied, falling back
    // to Default/unselected for that one row — no explicit migration needed.
    #[serde(default)] pub request_pref_movie: RequestPreference,
    #[serde(default)] pub request_pref_tv:    RequestPreference,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct RequestPreference {
    #[serde(default)] pub want_4k:        bool,
    // 0 = the synthetic "Default" row (no profile override) — same
    // convention request_detail_selected_profile_id already uses.
    #[serde(default)] pub profile_id_2k:  i32,
    #[serde(default)] pub profile_id_4k:  i32,
    #[serde(default)] pub tag_ids_2k:     Vec<i64>,
    #[serde(default)] pub tag_ids_4k:     Vec<i64>,
}

impl Default for ProfileSettings {
    fn default() -> Self {
        Self {
            user_id: String::new(), server_url: String::new(), token: String::new(),
            display_name: String::new(), avatar_color: String::new(), avatar_initial: String::new(),
            is_bonfire: false, master_user_id: String::new(), has_pin: false,
            remember_login: true,
            sub_enabled: true, sub_lang: String::new(), sub_lang2: String::new(),
            sub_type: String::new(), audio_lang: String::new(),
            sub_scale_pct: 100, sub_pos_pct: 100, sub_respect_ass_styling: true,
            sub_color: String::new(), sub_background: false,
            now_playing_auto_open: true,
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
            seerr_enabled: false,
            seerr_url: String::new(),
            seerr_auth_method: String::new(),
            seerr_api_key: String::new(),
            seerr_session_cookie: String::new(),
            trailer_quality: default_trailer_quality(),
            discover_filter_type: String::new(),
            discover_filter_genre_names: Vec::new(),
            discover_filter_sort: String::new(),
            discover_filter_min_rating: 0.0,
            discover_filter_min_year: 0,
            discover_filter_provider_ids: Vec::new(),
            request_pref_movie: RequestPreference::default(),
            request_pref_tv: RequestPreference::default(),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct Config {
    pub device: DeviceConfig,
    pub profiles: Vec<ProfileSettings>,
    pub active_profile_id: String,
}

impl Config {
    /// The only way any other code should read a profile-scoped setting.
    /// Falls back to the first profile if `active_profile_id` doesn't match
    /// any entry (shouldn't happen in normal operation — `Config::default()`
    /// and `migrate_legacy_config` both guarantee `profiles` is never empty
    /// and `active_profile_id` always names a real entry — but self-heals
    /// rather than panicking if `profiles` were ever hand-edited).
    pub fn active(&self) -> &ProfileSettings {
        self.profiles.iter().find(|p| p.user_id == self.active_profile_id)
            .or_else(|| self.profiles.first())
            .expect("Config.profiles is never empty — enforced by Config::default() and load_config's migration")
    }
    pub fn active_mut(&mut self) -> &mut ProfileSettings {
        let id = self.active_profile_id.clone();
        let pos = self.profiles.iter().position(|p| p.user_id == id).unwrap_or(0);
        self.profiles.get_mut(pos)
            .expect("Config.profiles is never empty — enforced by Config::default() and load_config's migration")
    }
}

impl Default for Config {
    fn default() -> Self {
        let profile = ProfileSettings::default();
        Self {
            active_profile_id: profile.user_id.clone(),
            device: DeviceConfig::default(),
            profiles: vec![profile],
        }
    }
}

/// The pre-Phase-1 flat shape every existing `config.json` is in — kept
/// solely so `load_config` can parse an old file and migrate it forward
/// (see `migrate_legacy_config` below). Field-for-field identical to the
/// `Config` struct this replaced, including the same inner-value migrations
/// (`deser_vf`/`deser_deinterlace`) for a doubly-old file. Deserialize only —
/// this is never constructed by hand or written back out in this shape.
#[derive(Deserialize)]
struct LegacyConfig {
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
    #[serde(default = "default_vf", deserialize_with = "deser_vf")]
                                               pub vf:                    String,
    #[serde(default = "default_video_sync")]  pub video_sync:            String,
    #[serde(default)]                         pub opengl_early_flush:    bool,
    #[serde(default)]                         pub video_latency_hacks:   bool,
    #[serde(default)]                         pub interpolation:         bool,
    #[serde(default = "default_tscale")]      pub tscale:                String,
    #[serde(default = "default_tone_mapping")]pub tone_mapping:          String,
    #[serde(default)]                         pub target_colorspace_hint:bool,
    #[serde(default = "default_deinterlace", deserialize_with = "deser_deinterlace")]
                                              pub deinterlace:           String,
    // Kept only so an old-shape file still deserializes cleanly — deliberately
    // no longer read by migrate_legacy_config, see its own comment.
    #[allow(dead_code)]
    #[serde(default)]                         pub cache_size_mb:         u32,
    #[serde(default)]                         pub video_behind:          bool,
    #[serde(default)]                         pub launch_fullscreen:     bool,
    #[serde(default = "default_true")]         pub sub_enabled:           bool,
    #[serde(default)]                         pub sub_lang:              String,
    #[serde(default)]                         pub sub_lang2:             String,
    #[serde(default)]                         pub sub_type:              String,
    #[serde(default = "default_sub_pct")]     pub sub_scale_pct:         u32,
    #[serde(default = "default_sub_pct")]     pub sub_pos_pct:           u32,
    #[serde(default = "default_true")]        pub sub_respect_ass_styling: bool,
    #[serde(default)]                         pub sub_color:             String,
    #[serde(default)]                         pub sub_background:        bool,
    #[serde(default)]                         pub audio_lang:            String,
    #[serde(default)]                         pub audio_device:          String,
    #[serde(default)]
    pub audio_device_passthrough: String,
    #[serde(default = "default_audio_channels")]
    pub audio_channels: String,
    #[serde(default = "default_gapless")]
    pub gapless_audio: bool,
    #[serde(default = "default_now_playing_auto_open")]
    pub now_playing_auto_open: bool,
    #[serde(default)]                         pub alsa_irq_scheduling:   bool,

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

    #[serde(default)] pub library_movies_sort:       u8,
    #[serde(default)] pub library_series_sort:       u8,
    #[serde(default)] pub library_collections_sort:  u8,
    #[serde(default)] pub library_artists_sort:      u8,
    #[serde(default)] pub library_albums_sort:       u8,
    #[serde(default)] pub library_playlists_sort:    u8,
    #[serde(default)] pub library_music_view:        u8,

    #[serde(default = "default_log_level")] pub log_level: String,

    #[serde(default = "default_seek_step")]      pub seek_step_secs:      u32,
    #[serde(default = "default_seek_step_long")] pub seek_step_long_secs: u32,

    #[serde(default = "default_speed_pct")] pub scroll_speed_pct:    u32,
    #[serde(default = "default_speed_pct")] pub animation_speed_pct: u32,

    #[serde(default = "default_ui_font_family")] pub ui_font_family: String,

    #[serde(default)] pub seerr_enabled:         bool,
    #[serde(default)] pub seerr_url:              String,
    #[serde(default)] pub seerr_auth_method:      String,
    #[serde(default)] pub seerr_api_key:          String,
    #[serde(default)] pub seerr_session_cookie:   String,

    #[serde(default = "default_trailer_quality")] pub trailer_quality: String,

    #[serde(default)] pub discover_filter_type:          String,
    #[serde(default)] pub discover_filter_genre_names:   Vec<String>,
    #[serde(default)] pub discover_filter_sort:           String,
    #[serde(default)] pub discover_filter_min_rating:     f32,
    #[serde(default)] pub discover_filter_min_year:       u32,
    #[serde(default)] pub discover_filter_provider_ids:   Vec<i64>,
}

/// Splits an old flat `LegacyConfig` into the new `DeviceConfig` +
/// single-entry `profiles` shape. Purely mechanical field reassignment —
/// see the device/profile split's own doc comment above `DeviceConfig` for
/// which field goes where and why.
fn migrate_legacy_config(l: LegacyConfig) -> Config {
    let device = DeviceConfig {
        device_id: l.device_id,
        audio_spdif: l.audio_spdif,
        spdif_ac3: l.spdif_ac3, spdif_eac3: l.spdif_eac3, spdif_dts: l.spdif_dts,
        spdif_dts_hd: l.spdif_dts_hd, spdif_truehd: l.spdif_truehd,
        hwdec: l.hwdec, vf: l.vf, video_sync: l.video_sync,
        opengl_early_flush: l.opengl_early_flush, video_latency_hacks: l.video_latency_hacks,
        interpolation: l.interpolation, tscale: l.tscale, tone_mapping: l.tone_mapping,
        target_colorspace_hint: l.target_colorspace_hint, deinterlace: l.deinterlace,
        // l.cache_size_mb deliberately dropped, not migrated — it was a
        // broken setting (see the doc comment on DeviceConfig's own
        // cache_secs/cache_max_mb fields) that only ever adjusted an mpv
        // option that rarely bound in practice, so there's no real user
        // intent worth preserving from it; every migrated install just
        // gets the new, actually-functional defaults instead.
        cache_secs: default_cache_secs(), cache_max_mb: default_cache_max_mb(),
        video_behind: l.video_behind, launch_fullscreen: l.launch_fullscreen,
        audio_device: l.audio_device, audio_device_passthrough: l.audio_device_passthrough,
        audio_channels: l.audio_channels, gapless_audio: l.gapless_audio,
        alsa_irq_scheduling: l.alsa_irq_scheduling,
        // No legacy equivalent — same treatment as skip_fade_ms just below.
        skip_fade_mute_passthrough: default_skip_fade_mute_passthrough(),
        log_level: l.log_level,
        seek_step_secs: l.seek_step_secs, seek_step_long_secs: l.seek_step_long_secs,
        // No legacy equivalent — skip-fade shipped after the last flat
        // config.json shape, same treatment as launch_policy/
        // default_profile_id just below.
        skip_fade_ms: default_skip_fade_ms(),
        scroll_speed_pct: l.scroll_speed_pct, animation_speed_pct: l.animation_speed_pct,
        ui_font_family: l.ui_font_family,
        launch_policy: default_launch_policy(),
        default_profile_id: String::new(),
        account_launch_policy: default_launch_policy(),
        default_account_id: String::new(),
    };
    let profile = ProfileSettings {
        user_id: l.user_id, server_url: l.server_url, token: l.token,
        // A pre-Bonfire flat config has no identity/avatar concept at all —
        // profile.rs's own tile-building already falls back gracefully
        // (empty display_name/avatar_initial derive from user_id) for
        // exactly this case, so leaving these blank here is correct, not
        // a gap to fill in.
        display_name: String::new(), avatar_color: String::new(), avatar_initial: String::new(),
        is_bonfire: false, master_user_id: String::new(), has_pin: false,
        remember_login: true,
        sub_enabled: l.sub_enabled, sub_lang: l.sub_lang, sub_lang2: l.sub_lang2, sub_type: l.sub_type,
        sub_scale_pct: l.sub_scale_pct, sub_pos_pct: l.sub_pos_pct,
        sub_respect_ass_styling: l.sub_respect_ass_styling,
        sub_color: l.sub_color, sub_background: l.sub_background,
        audio_lang: l.audio_lang, now_playing_auto_open: l.now_playing_auto_open,
        skip_intro_mode: l.skip_intro_mode, skip_intro_secs: l.skip_intro_secs,
        skip_recap_mode: l.skip_recap_mode, skip_recap_secs: l.skip_recap_secs,
        skip_preview_mode: l.skip_preview_mode, skip_preview_secs: l.skip_preview_secs,
        skip_commercial_mode: l.skip_commercial_mode, skip_commercial_secs: l.skip_commercial_secs,
        skip_credits_mode: l.skip_credits_mode, skip_credits_secs: l.skip_credits_secs,
        library_movies_sort: l.library_movies_sort, library_series_sort: l.library_series_sort,
        library_collections_sort: l.library_collections_sort, library_artists_sort: l.library_artists_sort,
        library_albums_sort: l.library_albums_sort, library_playlists_sort: l.library_playlists_sort,
        library_music_view: l.library_music_view,
        seerr_enabled: l.seerr_enabled, seerr_url: l.seerr_url, seerr_auth_method: l.seerr_auth_method,
        seerr_api_key: l.seerr_api_key, seerr_session_cookie: l.seerr_session_cookie,
        trailer_quality: l.trailer_quality,
        discover_filter_type: l.discover_filter_type,
        discover_filter_genre_names: l.discover_filter_genre_names,
        discover_filter_sort: l.discover_filter_sort,
        discover_filter_min_rating: l.discover_filter_min_rating,
        discover_filter_min_year: l.discover_filter_min_year,
        discover_filter_provider_ids: l.discover_filter_provider_ids,
        // No legacy equivalent — this feature shipped after the last flat
        // config.json shape, same treatment as skip_fade_ms/skip_fade_mute_
        // passthrough above on the DeviceConfig side.
        request_pref_movie: RequestPreference::default(),
        request_pref_tv: RequestPreference::default(),
    };
    let active_profile_id = profile.user_id.clone();
    Config { device, profiles: vec![profile], active_profile_id }
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
// Bonfire Phase 1 (cache namespacing, 2026-08-09): namespaced under the
// owning profile's user_id, same as the seven caches in home.rs — see that
// file's own module-header comment for the full "why explicit, not
// resolved internally" reasoning.
pub(crate) fn screen_caches_path(user_id: &str) -> std::path::PathBuf {
    xdg_cache_base().join("fjord").join("profiles").join(user_id).join("screen_caches.json")
}

/// One-time migration for existing installs: if this profile's namespaced
/// cache directory doesn't have a given file yet but the pre-Bonfire flat
/// `~/.cache/fjord/<filename>` location does, move it into place —
/// preserves the instant warm start instead of quietly downgrading every
/// existing single-profile install to a cold first launch the moment cache
/// namespacing ships. A nice-to-have, not required for correctness (a miss
/// here is just a normal, harmless network refetch) per the Bonfire plan's
/// own explicit framing. Safe to call on every `push_cached_data` run, not
/// just the first: after the first successful move, every file already
/// exists at its new path, so subsequent calls are just 8 cheap
/// `Path::exists` checks — no separate "ran once" flag needed.
pub(crate) fn migrate_flat_caches_to_profile(user_id: &str) {
    const CACHE_FILES: &[&str] = &[
        "home.json", "movies.json", "series.json", "collections.json",
        "artists.json", "albums.json", "playlists.json", "screen_caches.json",
    ];
    let old_base = xdg_cache_base().join("fjord");
    let new_dir  = old_base.join("profiles").join(user_id);
    for filename in CACHE_FILES {
        let old_path = old_base.join(filename);
        let new_path = new_dir.join(filename);
        if new_path.exists() || !old_path.exists() { continue; }
        if std::fs::create_dir_all(&new_dir).is_ok() {
            let _ = std::fs::rename(&old_path, &new_path);
        }
    }
}

// Discover (Seerr) posters — TMDB CDN images, not Jellyfin's own server, so
// kept in a separate directory rather than reusing poster_cache_dir/path
// (those are tag-revalidated against Jellyfin's ImageTags, a concept TMDB
// doesn't share). Keyed by "<mediaType>-<tmdbId>" since movie/tv id spaces
// can collide (see discover.rs).
pub(crate) fn discover_poster_cache_dir() -> std::path::PathBuf {
    xdg_cache_base().join("fjord").join("discover_posters")
}
pub(crate) fn discover_poster_cache_path(key: &str) -> std::path::PathBuf {
    discover_poster_cache_dir().join(key)
}

pub(crate) fn fmt_resume_label(secs: f64) -> String {
    let s = secs as u64;
    let h = s / 3600; let m = (s % 3600) / 60; let s = s % 60;
    if h > 0 { format!("Resume from {}:{:02}:{:02}", h, m, s) }
    else { format!("Resume from {}:{:02}", m, s) }
}

// Config's on-disk `token`/`seerr_api_key`/`seerr_session_cookie` are
// encrypted at rest (see secrets.rs) — decrypted here immediately after
// parsing so every other call site keeps reading plain strings from `Config`
// exactly as before. `device_id` itself stays plaintext (it's the key
// material, not a secret — Jellyfin ties auth to it, but it isn't a bearer
// credential on its own) and is what makes the derivation possible without a
// separate keyfile.
// Bonfire Phase 1: tries the current (device/profiles) shape first; on
// failure, falls back to the pre-Phase-1 flat `LegacyConfig` shape and
// migrates it forward, re-saving once so this fallback only ever runs on
// the very first post-upgrade launch. Every profile's token/seerr_api_key/
// seerr_session_cookie are decrypted here — same reasoning as before this
// split, just looped over `profiles` instead of three flat fields.
pub(crate) fn load_config() -> Option<Config> {
    let data = std::fs::read_to_string(config_path()).ok()?;
    let (mut cfg, migrated) = match serde_json::from_str::<Config>(&data) {
        Ok(c) => (c, false),
        Err(_) => {
            let legacy: LegacyConfig = serde_json::from_str(&data).ok()?;
            (migrate_legacy_config(legacy), true)
        }
    };
    if !cfg.device.device_id.is_empty() {
        let key = crate::secrets::derive_key(&cfg.device.device_id);
        for p in cfg.profiles.iter_mut() {
            p.token = crate::secrets::decrypt_field(&p.token, &key);
            p.seerr_api_key = crate::secrets::decrypt_field(&p.seerr_api_key, &key);
            p.seerr_session_cookie = crate::secrets::decrypt_field(&p.seerr_session_cookie, &key);
        }
    }
    // Self-heal a real corruption found live 2026-08-14: an older build's
    // `sync_bonfire_subprofiles` had no guard against Bonfire's `/list`
    // response including the calling MASTER account's own profile
    // alongside its real sub-profiles, so a master's own already-correct
    // entry could get silently overwritten to `is_bonfire: true,
    // master_user_id: <itself>` — a self-referencing state that's always
    // wrong (a profile can never legitimately be a Bonfire sub-profile of
    // itself) and made `switch_to_profile` treat clicking that profile's
    // own picker tile as a Bonfire switch INTO itself, which the server has
    // no reason to accept. The write side is fixed (that function now skips
    // its own id in the response), but this repairs any config.json that
    // was already corrupted by the old, unguarded version before this fix
    // shipped — same "self-heal forward, can't resurrect data already
    // dropped" precedent as the earlier keybindings.json migration.
    let mut repaired = false;
    for p in cfg.profiles.iter_mut() {
        if p.is_bonfire && p.master_user_id == p.user_id {
            tracing::warn!("load_config: repairing self-referencing Bonfire profile entry ({})", p.user_id);
            p.is_bonfire = false;
            p.master_user_id.clear();
            repaired = true;
        }
    }
    // Persist the migrated shape now, AFTER decrypting — `save_config`
    // re-encrypts from what it's given, so saving before decrypting would
    // encrypt an already-encrypted string.
    if migrated {
        tracing::info!("config.json migrated to the device/profiles shape (Bonfire Phase 1)");
        save_config(&cfg);
    } else if repaired {
        save_config(&cfg);
    }
    Some(cfg)
}

pub(crate) fn save_config(cfg: &Config) {
    let path = config_path();
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    // Encrypt a copy for the on-disk form — `cfg` itself (and every other
    // reader of it in memory) stays plaintext.
    let mut on_disk = cfg.clone();
    if !on_disk.device.device_id.is_empty() {
        let key = crate::secrets::derive_key(&on_disk.device.device_id);
        for p in on_disk.profiles.iter_mut() {
            p.token = crate::secrets::encrypt(&p.token, &key);
            p.seerr_api_key = crate::secrets::encrypt(&p.seerr_api_key, &key);
            p.seerr_session_cookie = crate::secrets::encrypt(&p.seerr_session_cookie, &key);
        }
    }
    if let Ok(json) = serde_json::to_string_pretty(&on_disk) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
    set_owner_only_permissions(&path);
}

// Defense in depth alongside encryption: config.json otherwise inherits the
// umask (typically 0644, world-readable). No downside to tightening it.
#[cfg(unix)]
fn set_owner_only_permissions(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
}
#[cfg(not(unix))]
fn set_owner_only_permissions(_path: &std::path::Path) {}

/// On-disk snapshot of the six screen-open caches (Phase 103) — one file since
/// they're always loaded/saved together as a unit, unlike movies.json/etc
/// which are independently refreshed per library type.
#[derive(Serialize, Deserialize)]
pub(crate) struct ScreenCachesFile {
    pub item_detail:        BoundedCache<MediaItem>,
    pub similar_items:      BoundedCache<Vec<MediaItem>>,
    pub boxset_items:       BoundedCache<Vec<MediaItem>>,
    pub artist_albums:      BoundedCache<Vec<MediaItem>>,
    pub person_filmography: BoundedCache<Vec<MediaItem>>,
    pub container_tracks:   BoundedCache<Vec<MediaItem>>,
    // Jellyfin person id -> TMDB person id, `None` meaning "already tried,
    // no confident match" (worth persisting since a miss still costs a
    // real fuzzy name-search fallback — Person "Other Work" row, 2026-07-29,
    // Deep Seerr integration). Unlike the other 5 caches above this one
    // holds a trivially-serializable Option<i64>, not a MediaItem — added
    // with a default fn (BoundedCache has no Default impl) so old
    // screen_caches.json files without this field still load.
    #[serde(default = "default_person_tmdb_id_cache")]
    pub person_tmdb_id: BoundedCache<Option<i64>>,
}

fn default_person_tmdb_id_cache() -> BoundedCache<Option<i64>> {
    BoundedCache::new(100)
}

pub(crate) fn load_screen_caches(user_id: &str) -> Option<ScreenCachesFile> {
    let data = std::fs::read_to_string(screen_caches_path(user_id)).ok()?;
    serde_json::from_str(&data).ok()
}

/// Snapshots the six caches out of `FjordState` (only needs a brief lock,
/// released before the actual file write) and writes them atomically.
/// `user_id` is the profile these caches belong to — passed explicitly by
/// the caller (captured before the lock, same as the caches themselves are
/// about to be) rather than re-read from `state.config.active()` here,
/// since this fn's own periodic-timer caller doesn't know whether a switch
/// happened since it was scheduled; the caller deciding "which profile am I
/// saving for" is the same discipline as every other namespaced cache.
pub(crate) fn save_screen_caches(state: &Arc<std::sync::Mutex<FjordState>>, user_id: &str) {
    let file = {
        let s = state.lock().unwrap();
        ScreenCachesFile {
            item_detail:        s.item_detail_cache.clone(),
            similar_items:      s.similar_items_cache.clone(),
            boxset_items:       s.boxset_items_cache.clone(),
            artist_albums:      s.artist_albums_cache.clone(),
            person_filmography: s.person_filmography_cache.clone(),
            container_tracks:   s.container_tracks_cache.clone(),
            person_tmdb_id:     s.person_tmdb_id_cache.clone(),
        }
    };
    let path = screen_caches_path(user_id);
    if let Some(parent) = path.parent() { let _ = std::fs::create_dir_all(parent); }
    if let Ok(json) = serde_json::to_string(&file) {
        let tmp = path.with_extension("json.tmp");
        if std::fs::write(&tmp, &json).is_ok() {
            let _ = std::fs::rename(&tmp, &path);
        }
    }
}

pub(crate) fn ensure_device_id(cfg: &mut Config) {
    if !cfg.device.device_id.is_empty() { return; }
    cfg.device.device_id = std::fs::read_to_string("/proc/sys/kernel/random/uuid")
        .unwrap_or_default()
        .trim()
        .to_string();
    if cfg.device.device_id.is_empty() {
        cfg.device.device_id = format!("fjord-{:016x}", std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos());
    }
    save_config(cfg);
    tracing::info!("generated device id: {}", cfg.device.device_id);
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

fn default_cap() -> usize { 40 }

/// FIFO cache: at most `cap` entries, oldest evicted first. Used to skip the
/// network round-trip for screen-open fetches (get_item_detail /
/// get_similar_items / etc) when an item was viewed recently — the goal is
/// "reopening something you just looked at shows instantly, no loading
/// spinner." Persisted to disk (`screen_caches.json`, Phase 103) so this
/// survives a restart; freshness is guarded by WS-driven invalidation
/// (`ws.rs`'s LibraryChanged/UserDataChanged handlers call `.remove()`/
/// `.insert()` on the matching cache as changes are reported) plus a
/// post-login background refresh sweep, not a TTL — an earlier version of
/// this cache used a 5-minute TTL, dropped once WS invalidation covered the
/// same freshness guarantee without discarding a still-valid entry early.
/// `cap` is genuinely persisted (not `#[serde(skip)]`, Phase 104 fix): the
/// opt-in library prewarm (`prewarm.rs`) raises it via `set_cap` to fit
/// however many items it actually populates — a skipped/reset-to-40 cap
/// silently evicted all but the last 40 of a 10,000+-item prewarm sweep
/// straight back down to nothing, confirmed via a real run's
/// `screen_caches.json` (thousands of requests made, 40 entries survived).
/// `default_cap()` only covers a JSON file predating this field.
///
/// Backing storage is `Arc<BoundedCacheInner<V>>`, not a plain struct —
/// real fix, 2026-08-01 (previously deferred as "disproportionate blast
/// radius" under the assumption a cheap clone would require touching every
/// call site across the 7 screens that use these caches; re-examined after
/// being asked directly why not do the full fix, and it turns out
/// `Arc::make_mut` gets the same result with zero call-site changes, since
/// it's entirely internal to this impl block). `BoundedCache::clone()`
/// (used by `save_screen_caches` to snapshot all six caches under a brief
/// lock before writing them to disk) is now O(1) — just bumps the Arc's
/// refcount — instead of a deep `HashMap`+`VecDeque` copy, which stopped
/// being free once the opt-in library prewarm (Phase 104) raises `cap` to
/// fit the whole library. Every mutating method below goes through
/// `Arc::make_mut`, Rust's standard clone-on-write primitive: when the Arc
/// is uniquely held (the overwhelmingly common case — nothing else has
/// cloned it), the mutation happens in place with zero extra cost, exactly
/// like before this change; only if something else (a save in flight) is
/// still holding a clone does the one mutation that collides with it pay a
/// real clone, once, after which the Arc is unique again and subsequent
/// mutations are back to free. Needs serde's `rc` feature (workspace
/// `Cargo.toml`) so `Arc<BoundedCacheInner<V>>` can derive
/// `Serialize`/`Deserialize` directly — it serializes the inner value as
/// if it were a plain field, which is exactly what's wanted here (this
/// cache is never actually shared across more than one live `Arc` clone
/// for longer than a save's own snapshot window, so there's no meaningful
/// "shared reference" semantic being lost on a round trip through JSON).
#[derive(Serialize, Deserialize, Clone, Default)]
struct BoundedCacheInner<V> {
    map:   std::collections::HashMap<String, V>,
    order: std::collections::VecDeque<String>,
    #[serde(default = "default_cap")]
    cap:   usize,
}

// `transparent`: without it, this would serialize as `{"inner": {...}}`
// instead of `BoundedCacheInner`'s own flat `{"map":...,"order":...,"cap":...}`
// shape — a real, live-caught regression risk, not a hypothetical one: a
// genuine 41MB `screen_caches.json` already exists on disk in the old flat
// shape (confirmed by reading it directly before adding this attribute),
// and every one of the six caches in `ScreenCachesFile` embeds a
// `BoundedCache<V>` the same way. `transparent` keeps the on-disk format
// byte-for-byte identical to before this whole Arc/COW change — the `Arc`
// wrapping is purely an in-memory implementation detail now, invisible on
// both sides of a JSON round trip.
#[derive(Serialize, Deserialize, Clone)]
#[serde(transparent)]
pub(crate) struct BoundedCache<V> {
    inner: std::sync::Arc<BoundedCacheInner<V>>,
}

impl<V: Clone> BoundedCache<V> {
    pub(crate) fn new(cap: usize) -> Self {
        Self { inner: std::sync::Arc::new(BoundedCacheInner { map: Default::default(), order: Default::default(), cap }) }
    }
    pub(crate) fn get(&self, key: &str) -> Option<V> {
        self.inner.map.get(key).cloned()
    }
    /// Raises `cap` if `min_cap` is larger than the current value; never lowers
    /// it. Called by the prewarm sweep before inserting, sized to the number of
    /// items it's about to populate, so nothing gets evicted mid-sweep.
    pub(crate) fn set_cap(&mut self, min_cap: usize) {
        if min_cap > self.inner.cap {
            std::sync::Arc::make_mut(&mut self.inner).cap = min_cap;
        }
    }
    pub(crate) fn insert(&mut self, key: String, value: V) {
        let inner = std::sync::Arc::make_mut(&mut self.inner);
        if !inner.map.contains_key(&key) {
            inner.order.push_back(key.clone());
            if inner.order.len() > inner.cap {
                if let Some(oldest) = inner.order.pop_front() {
                    inner.map.remove(&oldest);
                }
            }
        }
        inner.map.insert(key, value);
    }
    pub(crate) fn remove(&mut self, key: &str) {
        let inner = std::sync::Arc::make_mut(&mut self.inner);
        inner.map.remove(key);
        inner.order.retain(|k| k != key);
    }
    /// Drop every entry (cap is left unchanged). Used on sign-out — these six
    /// caches hold per-user UserData (played/favorite) keyed only by item id,
    /// with no user/server scoping, so a second account signing in on the same
    /// install would otherwise see the first account's watched-state on any
    /// item cached before sign-out, silently, since a cache hit skips the
    /// network fetch that would have corrected it.
    pub(crate) fn clear(&mut self) {
        let inner = std::sync::Arc::make_mut(&mut self.inner);
        inner.map.clear();
        inner.order.clear();
    }
    /// Borrowed (key, value) pairs, no cloning — for callers that need to read
    /// every entry (e.g. deriving referenced ids for cache cleanup) without
    /// paying for a `keys()` + `get()` double lookup that clones every value
    /// twice over (once inside `get`, once again for the caller's own use).
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&str, &V)> {
        self.inner.map.iter().map(|(k, v)| (k.as_str(), v))
    }
    /// Up to the `n` most recently inserted/touched keys. Used by the ambient
    /// post-login background refresh (`main.rs::spawn_screen_cache_refresh`,
    /// Phase 103), which must stay cheap and unprompted-safe regardless of how
    /// large `cap` has grown via the opt-in prewarm sweep (Phase 104) — that
    /// sweep can fill the cache with the whole library, but the ambient one
    /// should still only ever revalidate a small "recently used" slice, or a
    /// prewarmed library would repeat the prewarm's full cost on every login.
    pub(crate) fn recent_keys(&self, n: usize) -> Vec<String> {
        let len = self.inner.order.len();
        self.inner.order.iter().skip(len.saturating_sub(n)).cloned().collect()
    }
}

/// A manually-picked audio/subtitle language remembered for one series —
/// see `FjordState::remembered_tracks`. Stores the mpv track `lang` code
/// (e.g. "eng"), not a numeric track id, since ids are only meaningful
/// within the mpv instance/file they came from and don't carry across
/// episodes; the next episode's own track list is matched by language the
/// same way Config.sub_lang/audio_lang already are.
#[derive(Default, Clone)]
pub(crate) struct RememberedTracks {
    pub audio_lang: Option<String>,
    pub sub_lang:   Option<String>,
}

// ── app state (library + settings) ───────────────────────────────────────────

pub(crate) struct FjordState {
    pub config:               Config,          // authoritative settings + auth; saved on change
    pub client:               Option<Arc<JellyfinClient>>,
    // PIN digits typed so far in ProfilePickerScreen's PIN-entry sub-state
    // (Bonfire Phase 1, step 6, 2026-08-09) — deliberately never round-tripped
    // through a Slint string property (see AppState.profile-pin-len's own
    // doc comment in app_state.slint); cleared on every fresh PIN-entry open
    // and on a successful/failed switch attempt alike.
    pub profile_pin_buffer:   String,
    // Same discipline, for ProfileEditScreen's two PIN pads (Bonfire Phase
    // 2, 2026-08-09) — the profile's own new/changed PIN, and the
    // household master's own confirmation PIN. Two separate buffers, not
    // one reused for both: they're semantically different values (one
    // profile's new PIN vs. the account authorizing the change), and
    // conflating them would risk sending the wrong one to Bonfire's API.
    pub profile_edit_pin_buffer:        String,
    pub profile_edit_master_pin_buffer: String,
    // The last bonfire_list_profiles() result ManageProfilesScreen showed —
    // kept around purely so selecting a tile can resolve back to the full
    // BonfireProfile (parental rating, tags, enabled libraries, etc.) it
    // needs to pre-fill ProfileEditScreen with, without a second fetch.
    pub manage_profiles_cache:          Vec<fjord_api::models::BonfireProfile>,
    // Plugin names installed on the server (Bonfire Phase 1, 2026-08-09) —
    // fetched once per login/auto-login (GET /Plugins) alongside the
    // existing home-data/series/system-info join. Two consumers: Bonfire's
    // own gate (a fast presence check before ever calling
    // /plugins/profiles/list) and, later, Intro Skipper detection (which
    // today only ever infers presence per-episode via a 404). Keyed by
    // plugin NAME, not GUID — deliberately, since exact plugin GUIDs
    // couldn't be verified against a real server in this sandboxed
    // environment, unlike most other API surfaces this project checks
    // directly. Session-scoped, cleared by reset_session_state.
    pub available_plugins:    std::collections::HashSet<String>,
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
    // True once the unfiltered (query="") Browse All list has been built this
    // session — mirrors movies_fetched/discover_landing_fetched's own "fetch
    // once, not on every arrival" shape. Real gap this closes: sidebar_nav
    // landing on nav=5 used to unconditionally rebuild the ~800-item Slint
    // list model every single arrival, even a quick pass-through — Discover's
    // landing rows already had this exact guard, Browse All never did.
    // Invalidated (not by this flag directly, but the same effect) whenever
    // all_movies/all_series change via a WS LibraryChanged event, so the list
    // doesn't go stale for the rest of the session — see ws.rs.
    pub browse_populated:     bool,
    pub series_open_id:         String,
    pub series_season_ids:      Vec<String>,
    pub series_episode_items:   Vec<MediaItem>,
    pub series_episode_cache:   std::collections::HashMap<String, Vec<MediaItem>>,
    pub series_season_generation: u64,
    pub last_nw_mov_refresh:    Option<Instant>,
    pub last_nw_tv_refresh:   Option<Instant>,
    pub audio_devices:        Vec<(String, String)>,  // (mpv name, description)
    pub system_fonts:         Vec<(String, String)>,  // (value, display) — see fetch_system_fonts
    pub movie_collections:    std::collections::HashMap<String, (String, String)>, // movie_id → (boxset_id, boxset_name)
    // Per-series audio/subtitle language remembered from a manual S/A panel
    // pick (controls.rs::on_commit_panel_selection writes; playback.rs's
    // track auto-select reads, preferring it over Config.sub_lang/audio_lang
    // for that series only). In-memory only — session-scoped like
    // series_episode_cache, not persisted to disk; cleared on sign-out.
    pub remembered_tracks:    std::collections::HashMap<String, RememberedTracks>,
    // A rebind capture (Key Bindings screen) that collided with an existing
    // binding for a DIFFERENT action, awaiting the user's confirm/cancel on
    // the resulting dialog (2026-08-08) — keys.rs::rebind_action/
    // dispatch_keybinding_nav. None the rest of the time; in-memory only,
    // never persisted (the collision is resolved or abandoned within the
    // same session before it would ever matter).
    pub pending_keybind_rebind: Option<crate::keys::PendingKeybindRebind>,
    pub ws_abort:             Option<tokio::task::AbortHandle>, // abort to stop the WS reconnect loop on sign-out
    // Screen-open caches (Part 2, see BoundedCache doc comment above). Keyed by
    // item id (or the relevant container id — boxset/artist/person/album/playlist).
    pub item_detail_cache:        BoundedCache<MediaItem>,       // get_item_detail — shared by all 7 screens
    pub similar_items_cache:      BoundedCache<Vec<MediaItem>>,  // get_similar_items — detail.rs + series.rs
    pub boxset_items_cache:       BoundedCache<Vec<MediaItem>>,  // get_boxset_items — detail.rs + collection.rs
    pub artist_albums_cache:      BoundedCache<Vec<MediaItem>>,  // get_artist_albums — artist.rs
    pub person_filmography_cache: BoundedCache<Vec<MediaItem>>,  // get_person_filmography — person.rs
    pub container_tracks_cache:   BoundedCache<Vec<MediaItem>>,  // get_album_tracks / get_playlist_items — album.rs
    // Jellyfin person id -> TMDB person id (None = already tried, no match) —
    // Person "Other Work" row, 2026-07-29 (Deep Seerr integration). Persisted
    // via ScreenCachesFile (see its own doc comment) unlike the session-only
    // cache below, since the fuzzy name-search fallback is comparatively
    // expensive and worth not repeating every restart.
    pub person_tmdb_id_cache: BoundedCache<Option<i64>>,
    // Person "Other Work" row results, keyed by resolved TMDB person id —
    // session-only (not part of ScreenCachesFile): DiscoverCardMeta.item_type
    // is a &'static str, which genuinely can't round-trip through serde into
    // an owned struct field, and these are "discovery" results that are fine
    // to refetch after a restart anyway (2026-07-29, Deep Seerr integration).
    // Keeps poster_path alongside each meta (same shape `build_filtered_metas`
    // returns) so a same-session cache hit can still re-fetch/redisplay
    // posters, not just the text data.
    pub person_other_work_cache: BoundedCache<Vec<(crate::discover::DiscoverCardMeta, Option<String>)>>,
    // TMDB person id (as string) -> matching LOCAL Jellyfin Person id, if any
    // (None = searched, no confident match) — 2026-08-13, opening person
    // detail from a Discover-sourced cast member (RequestDetailScreen's
    // CastRow). Session-only, same reasoning as person_other_work_cache
    // above: cheap to redo (one name search + at most one detail fetch) and
    // Jellyfin's own library contents can change what a fresh search would
    // find, so not worth persisting stale answers across restarts.
    pub local_person_by_tmdb_cache: BoundedCache<Option<String>>,
    // Opt-in one-time library prewarm progress (Phase 104) — read by a 1s
    // AppState-updating timer (main.rs::wire_prewarm_progress_timer), written
    // by prewarm.rs's two spawn_*_prewarm functions.
    pub prewarm_metadata_running: bool,
    pub prewarm_metadata_total:   usize,
    pub prewarm_metadata_done:    usize,
    pub prewarm_metadata_summary: String,
    pub prewarm_image_running:    bool,
    pub prewarm_image_total:      usize,
    pub prewarm_image_done:       usize,
    pub prewarm_image_summary:    String,
    // Seerr integration (Settings → Integrations, discover.rs) — built from
    // Config.seerr_* at startup (if enabled + a valid cookie/key is present)
    // and rebuilt after every successful ConnectSeerrScreen flow. None means
    // "not connected"; a 401 from any call (session-auth only, API keys don't
    // expire) also resets this to None — see discover.rs's re-auth handling.
    pub seerr_client: Option<Arc<fjord_seerr::SeerrClient>>,
    // Guards the Discover landing rows (Trending/Popular/Upcoming) so they're
    // fetched once per session on first arrival, not on every nav switch back
    // to the tab. Reset to false on sign-out and whenever the Seerr
    // connection is cleared/reconnected (a different server may have a
    // different catalog).
    pub discover_landing_fetched: bool,
    // Search-result pagination (Seerr/TMDB commonly has hundreds of pages for
    // a common word — Fjord only ever fetched page 1, capping results far
    // below what Seerr's own web UI shows for the same query). `page` is the
    // last page successfully committed to `discover-results` (0 = no search
    // yet this generation); `total_pages` is from that page's own response.
    // `loading_more` is an in-flight guard against duplicate fetches from
    // holding Down. Reset (page/total_pages, not loading_more — an in-flight
    // fetch is discarded via the shared discover_gen check, not raced) at the
    // top of every fresh `spawn_discover_search` call, same as the results
    // model itself.
    pub discover_search_page: u32,
    pub discover_search_total_pages: u32,
    pub discover_search_loading_more: bool,
    // Full unfiltered fetch history for the CURRENT search query (page-
    // appended, same order as discover-results before any client-side
    // filter/sort is applied) — genre_ids/vote_average never make it onto
    // CardItem (never displayed), so this is the only place they're kept.
    // discover.rs::apply_search_filters reads this to rebuild
    // discover-results whenever a filter changes; cleared/rebuilt on every
    // NEW search alongside discover_search_page.
    pub discover_search_metas: Vec<crate::discover::DiscoverCardMeta>,
    // Same idea as discover_search_metas, but for the filtered-browse view
    // (query empty, ≥1 filter active) — accumulated across every fetched
    // page so a page-2+ load can re-sort the FULL set by sort_key, not just
    // the newly-fetched page's own batch. Real bug fixed 2026-07-31 (code
    // review): merge_filtered_metas only ever sorted each page in isolation;
    // the commit closure then plain-appended it onto the already-rendered
    // rows with no re-sort, so Type=All + any sort visibly broke order
    // across a page boundary the moment a later page's top item outranked
    // an earlier page's tail item. Reset on every fresh page-1 fetch.
    pub discover_filtered_metas: Vec<crate::discover::FilteredRowItem>,
    // Real bug fixed 2026-07-18: search results and 5 of the 6 landing rows
    // never carried real request state at all (request_id/pending/mine were
    // always zeroed for them — only the Requested row itself had it), so
    // their context menu offered "Request" instead of "Edit/Cancel/View
    // Request" for an item that was, in fact, already requested. Populated
    // from the DiscoverCardMeta list fetch_requested_row already builds
    // (no new network call) whenever the Requested row is (re)fetched, and
    // consulted to patch matching cards built elsewhere in discover.rs.
    // Keyed by (item_type, tmdb-id-as-string) - the exact same key shape
    // ensure_discover_landing's own requested_keys dedup set already uses.
    // Only catches requests within fetch_requested_row's own ~20-per-type
    // cap (a much older request might still show stale until it ages into
    // that window) - a deliberate, cheap tradeoff over a full uncapped
    // GET /request sweep, confirmed with the user before implementing.
    pub discover_known_requests: std::collections::HashMap<(&'static str, String), crate::discover::KnownRequest>,
    // Watchlist + Release Calendar (2026-07-18). discover_watchlist_ids
    // mirrors discover_known_requests' own (item_type, tmdb-id) key shape —
    // populated by ensure_discover_watchlist/refresh_watchlist, consulted
    // by patch_known_request_state's sibling to set CardItem.on-watchlist.
    // discover_calendar_entries is the built "Coming Up" row's data (see
    // discover.rs::CalendarEntry/build_calendar_entries), rebuilt whenever
    // the watchlist or requested-row set changes. seerr_discover_region
    // mirrors seerr_streaming_region's own cache shape exactly, just for
    // the DIFFERENT discoverRegion user setting Seerr's own frontend uses
    // for release-date display specifically (not the same region as
    // "Currently Streaming On" — confirmed from Seerr's real source).
    pub discover_watchlist_ids: std::collections::HashSet<(&'static str, String)>,
    // Real gap fixed 2026-07-20, live-reported ("the watch list symbol do
    // not show up on items i the library screens"): the star badge needs
    // to be TRUE at CardItem-construction time for local Jellyfin cards, not
    // just live-patched onto an already-rendered model — a live patch alone
    // (context_menu::patch_watchlist_on_jellyfin_models) gets silently wiped
    // the next time that screen's model is rebuilt from MediaItem data
    // (item_to_card_item/items_to_model always default on_watchlist to
    // false, since MediaItem itself has no watchlist concept — it's
    // Jellyfin-only data), which happens routinely (grid open, sort/filter,
    // WS delta sync, a background network refresh). This is the resolved
    // set of LOCAL Jellyfin ids currently on the Seerr watchlist — the
    // Jellyfin-id-keyed counterpart to discover_watchlist_ids (tmdb-id-
    // keyed) — kept authoritative by resync_jellyfin_watchlist_stars
    // (replaces wholesale, not add-only) and updated incrementally by
    // discover_toggle_watchlist's own single-item success handler;
    // item_to_card_item/items_to_model consult it directly so every
    // rebuild gets the right value the first time, with no separate patch
    // pass needed afterward.
    pub jellyfin_watchlist_ids: std::collections::HashSet<String>,
    // Generation guard for resync_jellyfin_watchlist_stars (2026-07-22, code
    // review finding): the resync is spawned independently from 4 separate
    // trigger points (the watchlist fetch itself, push_cached_data,
    // spawn_auto_login's fresh-series landing, spawn_movies_list_fetch's
    // completion) with no ordering between them — without this, whichever
    // call's mem::replace happened to land LAST won regardless of which one
    // actually computed the more complete result, so a call that started
    // earlier (and had a less-populated all_movies/all_series to scan) could
    // finish after a later, more-complete call and silently overwrite it,
    // un-starring genuinely-still-watchlisted cards via its own stale
    // "removed" diff. Each call bumps this and captures its own value before
    // scanning; only the call that's still the highest-numbered one by the
    // time it's ready to write actually writes — an older call that finds a
    // newer one has already started skips its own write outright rather than
    // clobbering, the same single-writer-wins idiom this codebase already
    // uses for stale-async-result guards elsewhere (e.g. discover_gen).
    pub jellyfin_watchlist_resync_seq: u64,
    // Rate-limits the 7 screen-open "revalidate on cache hit" functions
    // (collection.rs/detail.rs/series.rs/season.rs/artist.rs/person.rs/
    // album.rs' spawn_X_revalidate) to at most once per REVALIDATE_COOLDOWN
    // per item id (main.rs::should_revalidate) — same missing-guard bug class
    // as seerr_admin_last_refresh's own cooldown, fixed once already this
    // same day. Plain HashMap, not BoundedCache: Instant isn't Serialize and
    // this never needs to persist across restarts. Cleared on sign-out.
    pub screen_revalidate_last_run: std::collections::HashMap<String, Instant>,
    // Guards the watchlist-id fetch (ensure_discover_watchlist) the same
    // way discover_landing_fetched guards the landing rows — once per
    // session, reset on disconnect/reconnect/sign-out.
    pub discover_watchlist_fetched: bool,
    pub discover_calendar_entries: Vec<crate::discover::CalendarEntry>,
    pub seerr_discover_region: Option<String>,
    // Discover filters (2026-07-18). Guards the one-per-session genre +
    // watch-provider list fetch, same shape as discover_landing_fetched;
    // reset alongside it. Raw lists cached so switching the Type filter
    // (All/Movies/TV) can rebuild the Genre/Provider chip models locally
    // without a re-fetch.
    pub discover_filter_options_fetched: bool,
    pub seerr_genres_movie: Vec<fjord_seerr::Genre>,
    pub seerr_genres_tv: Vec<fjord_seerr::Genre>,
    pub seerr_providers_movie: Vec<fjord_seerr::WatchProviderDetail>,
    pub seerr_providers_tv: Vec<fjord_seerr::WatchProviderDetail>,
    // Filtered-browse pagination — mirrors discover_search_page/
    // total_pages/loading_more above exactly, just for the query-empty
    // filtered-browse view instead of a text search. Kept as separate
    // fields rather than reused, since the two views can't be active at
    // the same time but do need independent state when switching back and
    // forth (a search you were paginating through shouldn't lose its place
    // just because you toggled a filter and back).
    pub discover_filtered_page: u32,
    pub discover_filtered_total_pages_movie: u32,
    pub discover_filtered_total_pages_tv: u32,
    pub discover_filtered_loading_more: bool,
    // `Some(region)` once resolved (the connected user's own `streamingRegion`
    // preference, empty falls back to "US" — see discover.rs's
    // `resolve_streaming_region`), used to pick which entry of
    // MovieDetails/TvDetails.watch_providers to show as "Currently
    // Streaming On." Fetched once per connection, not once per item; also
    // updated on a successful Settings -> Integrations -> Streaming Region
    // write so the Discover panel picks up a change immediately. Reset
    // alongside discover_landing_fetched (same "different server may mean a
    // different region" reasoning).
    pub seerr_streaming_region: Option<String>,
    // (iso_3166_1, "English Name (US)") pairs — Settings -> Integrations ->
    // Streaming Region dropdown's display list, fetched once per connection
    // (GET /watchproviders/regions) the same way `system_fonts` is fetched
    // once at startup, just gated on a live Seerr connection existing first
    // instead of being always-available like a local `fc-list` query.
    pub seerr_regions: Vec<(String, String)>,
    // (iso_639_1, "English Name (en)") pairs — TMDB's full language list
    // (GET /languages), shared by BOTH Settings -> Integrations -> Display
    // Language and Discover Language dropdowns (2026-07-17) — see
    // fjord_seerr::Language's own doc comment for why one fetched list
    // backs both rather than hardcoding Seerr's separate, smaller
    // UI-translation locale set for Display Language.
    pub seerr_languages: Vec<(String, String)>,
    // The connected user's own `locale` (Display Language — Seerr's default
    // TMDB `language` query param for movie/tv/search calls whenever Fjord
    // doesn't pass one explicitly, confirmed from source; genuinely affects
    // what language titles/overviews come back in). `""` = "Default
    // (English)" (Seerr's own admin-configured fallback). `None` before the
    // first fetch resolves.
    pub seerr_locale: Option<String>,
    // The connected user's own `originalLanguage` (Discover Language —
    // filters Discover/trending/search results by TMDB original language).
    // `"all"` (the literal sentinel Seerr's own frontend sends, NOT an
    // empty string — see the write-handler doc comment) = "Default (All
    // Languages)", no filter. `None` before the first fetch resolves.
    pub seerr_original_language: Option<String>,
    // The connected Seerr account's own user id and `MANAGE_REQUESTS`
    // permission bit, fetched alongside the other seerr_* settings above
    // (piggybacks on the same `get_current_user` call already made there —
    // no new round trip). `seerr_user_id` drives the Discover context
    // menu's Edit/Cancel Request ownership check (`requested_by_me`);
    // `seerr_is_admin` gates Approve/Decline. Both `None`/`false` before
    // the first fetch resolves or when not connected.
    pub seerr_user_id: Option<i64>,
    pub seerr_is_admin: bool,
    // `MANAGE_BLOCKLIST` permission bit — a genuinely separate permission
    // from `MANAGE_REQUESTS`/`ADMIN` (see `fjord_seerr::User::
    // can_manage_blocklist`'s own doc comment) — fetched from the same
    // already-in-flight `get_current_user` call as `seerr_is_admin`, zero
    // extra network cost. Gates the Discover context menu's Blocklist row,
    // RequestDetailScreen's Blocklist button, CollectionScreen's bulk
    // blocklist button, and the Settings -> Integrations -> Manage
    // Blocklist row. `false` before the first fetch resolves or when not
    // connected. 2026-08-06, Seerr Blocklist support.
    pub seerr_can_manage_blocklist: bool,
    // Manage Blocklist screen's own pagination cursor (blocklist.rs) — a
    // plain `skip` offset into `GET /blocklist`, reset to 0 every time the
    // screen opens (not connection-scoped like discover_search_page, since
    // this screen has no equivalent "stays open across a query change"
    // concern). blocklist_total_results (page_info.results, the true total
    // across every page) is what load_more checks against to know whether
    // another page exists; blocklist_loading_more guards a double-fetch,
    // same shape as discover_search_loading_more. 2026-08-06, Seerr
    // Blocklist support.
    pub blocklist_skip: u32,
    pub blocklist_total_results: u32,
    pub blocklist_loading_more: bool,
    // Rate-limits `discover::refresh_seerr_admin_status` — that function was
    // unconditionally re-firing a real `GET /auth/me` round trip on EVERY
    // single arrival at the Discover sidebar tab, no guard at all (deliberate
    // at the time, so a server-side permission change mid-session would be
    // picked up on the next visit). Live-reported HTPC hitch, 2026-07-31: a
    // user holding Down/Up to rapidly cycle the sidebar passes through
    // Discover (nav==6) many times a minute — each pass fired its own
    // network request, and a burst of these completing out of order (worse
    // under the higher latency/lower thread-pool headroom of a lower-end
    // machine) queued up `invoke_from_event_loop` closures that visibly
    // collided with the next real keypress, the same mechanism already
    // documented for the Browse All rebuild hitch just above. `None` before
    // the first refresh.
    pub seerr_admin_last_refresh: Option<Instant>,
    // Whether `yt-dlp` was found on `PATH` at startup (`main.rs::
    // detect_yt_dlp`) — gates Watch Trailer button visibility. A pure
    // local-machine fact, not tied to Seerr connection state, not reset on
    // sign-out/disconnect.
    pub yt_dlp_available: bool,
}

impl FjordState {
    pub(crate) fn new() -> Self {
        Self {
            config: Config::default(),
            client: None, available_plugins: std::collections::HashSet::new(),
            profile_pin_buffer: String::new(),
            profile_edit_pin_buffer: String::new(), profile_edit_master_pin_buffer: String::new(),
            manage_profiles_cache: vec![],
            keybindings: load_keybindings(),
            all_movies: vec![], all_series: vec![], all_collections: vec![], all_artists: vec![], all_albums: vec![],
            all_playlists: vec![],
            movies_fetched: false, collections_fetched: false, artists_fetched: false, albums_fetched: false,
            playlists_fetched: false, filtered_items: vec![], browse_populated: false,
            series_open_id: String::new(), series_season_ids: vec![], series_episode_items: vec![],
            series_episode_cache: std::collections::HashMap::new(), series_season_generation: 0,
            last_nw_mov_refresh: None,
            last_nw_tv_refresh: None,
            audio_devices: vec![],
            system_fonts: vec![],
            movie_collections: std::collections::HashMap::new(),
            remembered_tracks: std::collections::HashMap::new(),
            pending_keybind_rebind: None,
            ws_abort: None,
            item_detail_cache:        BoundedCache::new(40),
            similar_items_cache:      BoundedCache::new(40),
            boxset_items_cache:       BoundedCache::new(40),
            artist_albums_cache:      BoundedCache::new(40),
            person_filmography_cache: BoundedCache::new(40),
            container_tracks_cache:   BoundedCache::new(40),
            person_tmdb_id_cache:       BoundedCache::new(100),
            person_other_work_cache:    BoundedCache::new(40),
            local_person_by_tmdb_cache: BoundedCache::new(100),
            prewarm_metadata_running: false,
            prewarm_metadata_total:   0,
            prewarm_metadata_done:    0,
            prewarm_metadata_summary: String::new(),
            prewarm_image_running:    false,
            prewarm_image_total:      0,
            prewarm_image_done:       0,
            prewarm_image_summary:    String::new(),
            seerr_client: None,
            discover_landing_fetched: false,
            discover_search_page: 0,
            discover_search_total_pages: 0,
            discover_search_loading_more: false,
            discover_search_metas: Vec::new(),
            discover_filtered_metas: Vec::new(),
            discover_known_requests: std::collections::HashMap::new(),
            discover_watchlist_ids: std::collections::HashSet::new(),
            jellyfin_watchlist_ids: std::collections::HashSet::new(),
            jellyfin_watchlist_resync_seq: 0,
            screen_revalidate_last_run: std::collections::HashMap::new(),
            discover_watchlist_fetched: false,
            discover_calendar_entries: Vec::new(),
            seerr_discover_region: None,
            discover_filter_options_fetched: false,
            seerr_genres_movie: Vec::new(),
            seerr_genres_tv: Vec::new(),
            seerr_providers_movie: Vec::new(),
            seerr_providers_tv: Vec::new(),
            discover_filtered_page: 0,
            discover_filtered_total_pages_movie: 0,
            discover_filtered_total_pages_tv: 0,
            discover_filtered_loading_more: false,
            seerr_streaming_region: None,
            seerr_regions: Vec::new(),
            seerr_languages: Vec::new(),
            seerr_locale: None,
            seerr_original_language: None,
            seerr_user_id: None,
            seerr_is_admin: false,
            seerr_can_manage_blocklist: false,
            blocklist_skip: 0,
            blocklist_total_results: 0,
            blocklist_loading_more: false,
            seerr_admin_last_refresh: None,
            yt_dlp_available: false,
        }
    }

    pub(crate) fn player_config(&self) -> PlayerConfig {
        let c  = &self.config.device;
        let cp = self.config.active();
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
            vf:                     vf_mpv_value(&c.vf),
            video_sync:             c.video_sync.clone(),
            opengl_early_flush:     c.opengl_early_flush,
            video_latency_hacks:    c.video_latency_hacks,
            interpolation:          c.interpolation,
            tscale:                 c.tscale.clone(),
            tone_mapping:           c.tone_mapping.clone(),
            target_colorspace_hint: c.target_colorspace_hint,
            deinterlace:            c.deinterlace.clone(),
            cache_secs:             c.cache_secs,
            cache_max_mb:           c.cache_max_mb,
            start_position_secs:    None,
            sub_scale:              cp.sub_scale_pct as f64 / 100.0,
            sub_pos:                cp.sub_pos_pct as i64,
            sub_respect_ass_styling: cp.sub_respect_ass_styling,
            sub_color:              sub_color_hex(&cp.sub_color).to_string(),
            sub_background:         cp.sub_background,
            ytdl_format:            None, // trailer-only; set by the caller (main.rs) when playing one
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

#[cfg(test)]
mod tests {
    use super::*;

    // A real pre-Phase-1 flat config.json (minus token/seerr fields, which
    // would be genuine encrypted ciphertext here — those round-trip through
    // decrypt_field's own established "treat undecryptable value as already
    // plaintext" fallback either way, so a plain "" is enough to exercise the
    // shape migration itself). Field selection: a few explicit non-default
    // values in each of the device/profile buckets, everything else omitted
    // to also exercise every #[serde(default)] on LegacyConfig at once.
    const OLD_SHAPE: &str = r#"{
        "server_url": "https://jellyfin.example.com",
        "user_id": "abc123",
        "token": "",
        "device_id": "device-xyz",
        "hwdec": "nvdec-copy",
        "audio_device": "pipewire/my-speakers",
        "sub_lang": "English",
        "library_movies_sort": 2,
        "seerr_enabled": true,
        "seerr_url": "https://seerr.example.com",
        "discover_filter_type": "movie"
    }"#;

    #[test]
    fn migrates_legacy_flat_shape() {
        let legacy: LegacyConfig = serde_json::from_str(OLD_SHAPE)
            .expect("OLD_SHAPE should parse as the pre-Phase-1 flat shape");
        let cfg = migrate_legacy_config(legacy);

        // Device-scoped fields landed on `device`.
        assert_eq!(cfg.device.device_id, "device-xyz");
        assert_eq!(cfg.device.hwdec, "nvdec-copy");
        assert_eq!(cfg.device.audio_device, "pipewire/my-speakers");

        // Profile-scoped fields landed on the single migrated profile, and
        // active_profile_id correctly names it.
        assert_eq!(cfg.profiles.len(), 1);
        assert_eq!(cfg.active_profile_id, "abc123");
        let p = cfg.active();
        assert_eq!(p.user_id, "abc123");
        assert_eq!(p.server_url, "https://jellyfin.example.com");
        assert_eq!(p.sub_lang, "English");
        assert_eq!(p.library_movies_sort, 2);
        assert!(p.seerr_enabled);
        assert_eq!(p.seerr_url, "https://seerr.example.com");
        assert_eq!(p.discover_filter_type, "movie");

        // Fields omitted from OLD_SHAPE landed on their real defaults, not
        // zeroed — confirms LegacyConfig's own #[serde(default = ...)]
        // attributes (copied from the original flat Config) still work.
        assert_eq!(cfg.device.video_sync, default_video_sync());
        assert!(p.sub_enabled); // default_true
        assert_eq!(p.skip_intro_mode, "ask");
    }

    #[test]
    fn new_shape_round_trips_without_migration() {
        let cfg = Config::default();
        let json = serde_json::to_string(&cfg).unwrap();
        let reparsed: Config = serde_json::from_str(&json)
            .expect("a freshly-serialized new-shape Config must parse back as Config directly, no migration needed");
        assert_eq!(reparsed.active_profile_id, cfg.active_profile_id);
        assert_eq!(reparsed.profiles.len(), cfg.profiles.len());
    }

    #[test]
    fn old_shape_does_not_parse_directly_as_new_config() {
        // This is the branch condition load_config's migration depends on:
        // an old-shape file must fail to parse as the new Config so the
        // fallback to LegacyConfig actually triggers.
        assert!(serde_json::from_str::<Config>(OLD_SHAPE).is_err());
    }
}
