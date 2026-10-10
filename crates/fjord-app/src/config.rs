// ── fjord-app · config.rs ────────────────────────────────────────────────────
//   BoundedCache<V> FIFO cache (default cap 40), Serialize/Deserialize, O(1) Clone (Arc + make_mut);
//                   set_cap (raise-only, for the prewarm), recent_keys(n), iter(), clear() (sign-out);
//                   freshness via WS invalidation + a post-login refresh, not a TTL
//   ScreenCachesFile  on-disk snapshot of the screen-open caches (screen_caches.json)
//   screen_caches_path/load_screen_caches/save_screen_caches  its persistence I/O (per profile)
//   RememberedTracks  { audio_lang, sub_lang } — one series' manually picked track languages
//   default_* fns   serde defaults for DeviceConfig/ProfileSettings fields
//   sub_color_hex   ProfileSettings.sub_color display name → mpv hex colour ("" = don't touch)
//   vf_mpv_value / deser_vf  DeviceConfig.vf display label → raw mpv sentinel ("" / "auto");
//                   deser_vf migrates old raw values to the labels
//   DeviceConfig    settings of THIS BOX, whoever is signed in: hwdec/vf/audio device/seek step/
//                   animation speed/log_level/cache, launch_policy + default_profile_id (profile
//                   tier), account_launch_policy + default_account_id (account tier),
//                   onscreen_keyboard_enabled, display_sync_* (opt-in; display_sync_trailers),
//                   separate_video_surface (default on), video_own_buffers (opt-in, per run),
//                   video_dither_off (test aid)
//   ProfileSettings settings that follow the PERSON, keyed by user_id: auth (server_url/user_id/
//                   token), subtitle/audio language, library sort, skip_*_mode/_secs, trailer
//                   quality, Seerr connection (seerr_enabled/_url/_auth_method/_api_key/
//                   _session_cookie), Discover filters (genres by name, providers by id), Request
//                   Options' remembered choices; remember_login (false → never resumed silently,
//                   StartupGate::RequireLogin); Bonfire: is_bonfire/master_user_id/has_pin/
//                   lockout_minutes, is_group_account/synced_via (group accounts, see
//                   profile.rs::is_true_master), bonfire_linked_roots
//   Config          { device, profiles, active_profile_id } — active()/active_mut() are the ONLY
//                   way to read/write a profile-scoped field. Adding a setting: add it to Config
//                   only (FjordState.config is the copy). token/seerr_api_key/seerr_session_cookie
//                   are encrypted at rest (load_config/save_config, secrets.rs)
//   LegacyConfig/migrate_legacy_config  the old flat shape + its one-time migration
//   repair_bonfire_profile_corruption  self-heals profiles written by an old, unguarded Bonfire sync
//   FjordState      runtime state (never persisted as a whole): config (canonical), client,
//                   library vecs + *_fetched / movie_posters_loaded guards, filtered lists,
//                   series/episode caches + series_season_generation, keybindings,
//                   audio_devices / system_fonts (startup fetches), movie_collections,
//                   remembered_tracks, ws_abort, ws_connected / ws_last_keepalive_at (stall-recovery
//                   budget), PIN buffers, live_requires_pin, available_plugins, the screen-open
//                   BoundedCaches (persisted together) + person caches, Seerr state
//                   (client, regions, languages, permissions, watchlist/calendar/known requests,
//                   jellyfin_watchlist_ids + its resync generation), screen_revalidate_last_run,
//                   pending_keybind_rebind, trailer_playable, display_sync_current_mode.
//                   Idle tracking lives in activity::ActivityClock, not here. Every transient field
//                   is reset in reset_session_state (session.rs)
//   path helpers    xdg_config_base, xdg_cache_base, config_path, poster/backdrop cache dir/path,
//                   discover_poster_cache_dir/path (TMDB posters, separate dir), keybindings_path
//   safe_cache_name server-provided id → cache file/folder name only if it's 32 hex; the cache
//                   path helpers return None otherwise (unit-tested)
//   write_private   owner-only (0600) file write from the first byte (config.json's temp file)
//   config I/O      load_config, save_config (one save at a time), ensure_device_id — every failure
//                   is logged, including a config.json that matches neither shape
//   keybindings I/O load_keybindings, save_keybindings
//   fmt_resume_label  resume position as "1h 23m 45s"
//   upsert_media_item  replace-by-id or append; WS delta-sync merge helper
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;
use std::time::Instant;

use fjord_api::{JellyfinClient, models::MediaItem};
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
        "White" => "#FFFFFF",
        "Yellow" => "#FFFF00",
        "Cyan" => "#00FFFF",
        "Green" => "#00FF00",
        _ => "",
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
        "auto: nv12/p010" => String::new(),
        "auto: yuv420p/yuv420p10le" => "auto".to_string(),
        other => other.to_string(),
    }
}

pub(crate) fn default_audio_channels() -> String {
    "auto-safe".into()
}
fn default_gapless() -> bool {
    true
}
fn default_skip_fade_mute_passthrough() -> bool {
    true
}
fn default_now_playing_auto_open() -> bool {
    true
}
fn default_hwdec() -> String {
    "auto".into()
}
pub(crate) fn default_video_sync() -> String {
    "audio".into()
}
pub(crate) fn default_tscale() -> String {
    "oversample".into()
}
pub(crate) fn default_tone_mapping() -> String {
    "auto".into()
}
fn default_true() -> bool {
    true
}
fn default_deinterlace() -> String {
    "no".into()
}
fn default_vf() -> String {
    "auto: nv12/p010".into()
}
fn default_skip_mode() -> String {
    "ask".into()
}
fn default_log_level() -> String {
    "info".into()
}
fn default_skip_secs() -> u32 {
    8
}
fn default_credits_secs() -> u32 {
    30
}
fn default_seek_step() -> u32 {
    10
}
fn default_seek_step_long() -> u32 {
    30
}
// Base duration (ms) of the skip-segment fade-to-black, before the
// settings-animation-speed multiplier is applied — matches the literal
// value this replaced (playback.rs's old hardcoded SKIP_FADE_MS const).
fn default_skip_fade_ms() -> u32 {
    200
}
fn default_sub_pct() -> u32 {
    100
}
fn default_speed_pct() -> u32 {
    100
}
// "Inter" = Fjord's own bundled default text font; "" = system default (no
// font-family override at all); anything else = that system font by name.
fn default_ui_font_family() -> String {
    "Inter".into()
}
fn default_cache_secs() -> u32 {
    60
}
fn default_cache_max_mb() -> u32 {
    500
}
// Display-ready values stored directly in Config, same idiom as
// Config.sub_color/SUB_COLOR_MODEL ("White"/"Yellow" are both the stored
// value and the display string) — translated to an mpv ytdl-format string
// only at point of use (main.rs::trailer_ytdl_format), not here. Caps
// yt-dlp's resolution selection for Watch Trailer playback
// (Settings → Integrations → Trailer Quality).
fn default_trailer_quality() -> String {
    "1080p".into()
}

// Migrate old bool (false/true) stored by earlier versions to "no"/"yes".
// Option<> wrapper accepts JSON null without error (maps to "no").
fn deser_deinterlace<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum BoolOrStr {
        Bool(bool),
        Str(String),
    }
    Ok(match Option::<BoolOrStr>::deserialize(d)? {
        Some(BoolOrStr::Bool(b)) => if b { "yes" } else { "no" }.into(),
        Some(BoolOrStr::Str(s)) => s,
        None => "no".into(),
    })
}

// Migrates the raw sentinel values this field stored before the vf dropdown got
// self-describing labels to those labels, so an upgraded install doesn't show a
// blank "(none)" in Settings. `vf_mpv_value()` treats old and new forms alike for
// playback; the four explicit `format=...` values pass through unchanged.
fn deser_vf<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(match String::deserialize(d)?.as_str() {
        "" => "auto: nv12/p010",
        "auto" => "auto: yuv420p/yuv420p10le",
        other => return Ok(other.to_string()),
    }
    .into())
}

// ── Config: device-scoped vs. profile-scoped ──────────────────────────────────
//
// A Bonfire sub-profile switch and a sign-in as someone else are the same event: a
// new Jellyfin user_id + token becomes active. `DeviceConfig` holds what describes
// THIS BOX whoever uses it (hwdec, audio device, seek step, animation speed, log
// level, cache_secs/cache_max_mb — the box's network path …); `ProfileSettings`
// holds what follows the person (auth, subtitle/audio language, library sort, Seerr
// connection, Discover filters, now_playing_auto_open …). `Config.profiles` +
// `active_profile_id` select the active one; `Config::active()`/`active_mut()` are
// the ONLY way other code reads/writes a profile-scoped field.

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct DeviceConfig {
    #[serde(default)]
    pub device_id: String,

    #[serde(default)]
    pub audio_spdif: bool,
    #[serde(default = "default_true")]
    pub spdif_ac3: bool,
    #[serde(default = "default_true")]
    pub spdif_eac3: bool,
    #[serde(default = "default_true")]
    pub spdif_dts: bool,
    #[serde(default = "default_true")]
    pub spdif_dts_hd: bool,
    #[serde(default = "default_true")]
    pub spdif_truehd: bool,
    #[serde(default = "default_hwdec")]
    pub hwdec: String,
    #[serde(default = "default_vf", deserialize_with = "deser_vf")]
    pub vf: String,
    #[serde(default = "default_video_sync")]
    pub video_sync: String,
    #[serde(default)]
    pub opengl_early_flush: bool,
    #[serde(default)]
    pub video_latency_hacks: bool,
    #[serde(default)]
    pub interpolation: bool,
    #[serde(default = "default_tscale")]
    pub tscale: String,
    #[serde(default = "default_tone_mapping")]
    pub tone_mapping: String,
    #[serde(default)]
    pub target_colorspace_hint: bool,
    // HDR Stage 5 (2026-10-05): video on its own Wayland subsurface (keeps
    // menus/OSD in correct colours with HDR passthrough). Default on; off =
    // the old in-window path, for platforms where it performs better.
    #[serde(default = "default_true")]
    pub separate_video_surface: bool,
    // 10-bit video plane (2026-10-08): Fjord's own GBM/dmabuf buffers for the
    // subsurface. Opt-in (was automatic where EGL has no 10-bit window, until
    // NVIDIA's source showed Pascal sends 8 bpc over HDMI anyway). Read once
    // per run when the subsurface is set up.
    #[serde(default)]
    pub video_own_buffers: bool,
    // Test aid (2026-10-08): mpv dither-depth=no, to compare 8- vs 10-bit
    // output on a gradient (dithering hides the difference).
    #[serde(default)]
    pub video_dither_off: bool,
    #[serde(
        default = "default_deinterlace",
        deserialize_with = "deser_deinterlace"
    )]
    pub deinterlace: String,
    // ── Network cache (Settings → Player → Buffering) ───────────────────────────
    // cache_secs → mpv `cache-secs` (0 = mpv's default, ~3.6M s, so cache_max_mb is what
    // binds in practice). cache_max_mb → `demuxer-max-bytes`, the byte ceiling mpv
    // enforces (stock 150 MiB) regardless of cache_secs; its 0 means "Unlimited"
    // (`Player::new` raises demuxer-max-bytes to a large fixed ceiling) for letting
    // cache_secs alone govern. Background: DEVLOG → "Playback resilience".
    #[serde(default = "default_cache_secs")]
    pub cache_secs: u32,
    #[serde(default = "default_cache_max_mb")]
    pub cache_max_mb: u32,
    #[serde(default)]
    pub video_behind: bool,
    #[serde(default)]
    pub launch_fullscreen: bool,
    #[serde(default)]
    pub audio_device: String,
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
    #[serde(default)]
    pub alsa_irq_scheduling: bool,
    // Whether the skip-segment fade (skip_fade_ms) mutes audio during SPDIF passthrough
    // — a raw bitstream can't be volume-ramped like PCM, so muting is the closest analog.
    // Gates only that mute; the video fade and the PCM ramp always apply.
    #[serde(default = "default_skip_fade_mute_passthrough")]
    pub skip_fade_mute_passthrough: bool,

    // ── Log level for fjord.log ("error"|"warn"|"info"|"debug") — read once at
    // startup before the tracing subscriber is built; changes apply on next launch.
    #[serde(default = "default_log_level")]
    pub log_level: String,

    // ── Player seek step (Settings → Player → Seeking) ──────────────────────
    #[serde(default = "default_seek_step")]
    pub seek_step_secs: u32,
    #[serde(default = "default_seek_step_long")]
    pub seek_step_long_secs: u32,

    // ── Skip-segment fade-to-black duration (Settings → Player → Seeking) ──────────
    // Base ms before the settings-animation-speed multiplier; 0 = an instant cut.
    // Device-scoped like seek_step_secs (about the setup, not personal taste).
    #[serde(default = "default_skip_fade_ms")]
    pub skip_fade_ms: u32,

    // ── UI animation speed (Settings → UI) — multiplier percentages, 100 = mpv/
    // widgets.slint's original hand-tuned durations unchanged. Scroll is kept
    // separate from general Animation since it's a throughput property (how
    // fast you can move through content), not decoration.
    #[serde(default = "default_speed_pct")]
    pub scroll_speed_pct: u32,
    #[serde(default = "default_speed_pct")]
    pub animation_speed_pct: u32,

    // ── Text font (Settings → UI) — "Inter" (bundled default), "" (system
    // default, no override), or any other font family installed on the host.
    #[serde(default = "default_ui_font_family")]
    pub ui_font_family: String,

    // ── On-screen alphanumeric keyboard (Settings → UI) ─────────────────────────
    // Default ON: Fjord must be fully usable with no physical keyboard (remote/D-pad).
    // Device-scoped — whether a keyboard is attached is a hardware fact. Checked by the
    // openers (AppState.open-onscreen-keyboard / keys::open_onscreen_keyboard — nothing
    // opens when off), every QwertyKeyboard mount, and keys.rs's keyboard gate.
    #[serde(default = "default_true")]
    pub onscreen_keyboard_enabled: bool,

    // ── Profile-picker launch policy (Settings → Profiles) ──────────────────────
    // "always_ask" | "remember_last" | "default"; default_profile_id is that profile's
    // user_id, used only with "default".
    #[serde(default = "default_launch_policy")]
    pub launch_policy: String,
    #[serde(default)]
    pub default_profile_id: String,

    // ── Account-tier launch policy ───────────────────────────────────────────────
    // The same three values one tier up (account_launch_policy / default_account_id =
    // the account's root user_id). Only consulted when should_show_picker_at_startup
    // finds 2+ accounts (grouped by profile::account_root_id).
    #[serde(default = "default_launch_policy")]
    pub account_launch_policy: String,
    #[serde(default)]
    pub default_account_id: String,

    // ── display_sync — resolution/refresh-rate/HDR/WCG matched to the source ────
    // Device-scoped (which output exists and its modes are hardware facts), opt-in.
    // Design in display_sync.rs's module doc and DEVLOG → display_sync, including its
    // ordering against the HDR Stage 3 hook.
    #[serde(default)]
    pub display_sync_enabled: bool,
    // Not auto-detected at runtime: pre-filled once at startup while still empty, and
    // only when `display_sync::list_output_names()` finds exactly one output; an
    // explicit setting from then on ("first connected output" picks wrong as soon as
    // there's a second one). Editable via Settings' dynamic dropdown.
    #[serde(default)]
    pub display_sync_screen_name: String,
    #[serde(default = "default_display_sync_resolution")]
    pub display_sync_default_resolution: String,
    #[serde(default = "default_display_sync_hz")]
    pub display_sync_default_hz: String,
    // Always applied alongside every mode switch, matching the proven
    // external script's own `run_kscreen` — dropping this would leave KDE's
    // desktop scale wherever it was for the previous resolution, a real,
    // visible regression versus what the script already does today.
    #[serde(default = "default_scale")]
    pub display_sync_scale_4k: String,
    #[serde(default = "default_scale")]
    pub display_sync_scale_1080p: String,
    // When false, resolution is always pinned to display_sync_default_resolution
    // (never switches to 4K regardless of source width) and only refresh
    // rate varies by cadence — see display_sync::compute_target_mode's own
    // doc comment for why this is defined as "pinned," not "leave whatever
    // KDE is currently at alone" (the latter has no mechanism behind it).
    #[serde(default = "default_true")]
    pub display_sync_sync_resolution: bool,
    // When false, refresh rate stays pinned to display_sync_default_hz and
    // only resolution varies (4K vs. the configured default).
    #[serde(default = "default_true")]
    pub display_sync_sync_refresh_rate: bool,
    // "fallback" (default, matches the proven script exactly — 4K content
    // at a non-film/NTSC/PAL framerate drops to the default 1080p
    // resolution@59.94) or "stay_4k" (pick the closest supported Hz at 4K
    // instead). Only consulted when display_sync_sync_resolution is true.
    #[serde(default = "default_4k_odd_fps_mode")]
    pub display_sync_4k_odd_fps_mode: String,
    // "yes" (HDR on exactly when the source is HDR) | "no" (never) |
    // "always". No "manual" value — display_sync_enabled=false already IS
    // "never touch HDR/WCG", making a 4th value redundant.
    #[serde(default = "default_display_sync_hdr_mode")]
    pub display_sync_hdr_mode: String,
    // "auto" (follows HDR state, matching the proven script's own default) |
    // "yes" | "no".
    #[serde(default = "default_display_sync_wcg_mode")]
    pub display_sync_wcg_mode: String,
    // Whether Watch Trailer also switches the display (2026-10-04, user
    // request: off by default — a YouTube trailer isn't worth a mode switch
    // and a TV resync, and it often comes in an odd 4K/24p format).
    #[serde(default)]
    pub display_sync_trailers: bool,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            device_id: String::new(),
            audio_spdif: false,
            spdif_ac3: true,
            spdif_eac3: true,
            spdif_dts: true,
            spdif_dts_hd: true,
            spdif_truehd: true,
            hwdec: default_hwdec(),
            vf: "auto: nv12/p010".into(),
            video_sync: default_video_sync(),
            opengl_early_flush: false,
            video_latency_hacks: false,
            interpolation: false,
            tscale: default_tscale(),
            tone_mapping: default_tone_mapping(),
            target_colorspace_hint: false,
            separate_video_surface: true,
            video_own_buffers: false,
            video_dither_off: false,
            deinterlace: "no".into(),
            cache_secs: default_cache_secs(),
            cache_max_mb: default_cache_max_mb(),
            video_behind: false,
            launch_fullscreen: false,
            audio_device: String::new(),
            audio_device_passthrough: String::new(),
            audio_channels: default_audio_channels(),
            gapless_audio: true,
            alsa_irq_scheduling: false,
            skip_fade_mute_passthrough: default_skip_fade_mute_passthrough(),
            log_level: default_log_level(),
            seek_step_secs: default_seek_step(),
            seek_step_long_secs: default_seek_step_long(),
            skip_fade_ms: default_skip_fade_ms(),
            scroll_speed_pct: 100,
            animation_speed_pct: 100,
            ui_font_family: default_ui_font_family(),
            onscreen_keyboard_enabled: true,
            launch_policy: default_launch_policy(),
            default_profile_id: String::new(),
            account_launch_policy: default_launch_policy(),
            default_account_id: String::new(),
            display_sync_enabled: false,
            display_sync_screen_name: String::new(),
            display_sync_default_resolution: default_display_sync_resolution(),
            display_sync_default_hz: default_display_sync_hz(),
            display_sync_scale_4k: default_scale(),
            display_sync_scale_1080p: default_scale(),
            display_sync_sync_resolution: true,
            display_sync_sync_refresh_rate: true,
            display_sync_4k_odd_fps_mode: default_4k_odd_fps_mode(),
            display_sync_hdr_mode: default_display_sync_hdr_mode(),
            display_sync_wcg_mode: default_display_sync_wcg_mode(),
            display_sync_trailers: false,
        }
    }
}

fn default_launch_policy() -> String {
    "always_ask".into()
}
fn default_display_sync_resolution() -> String {
    "1920x1080".into()
}
fn default_display_sync_hz() -> String {
    "59.94".into()
}
fn default_scale() -> String {
    "1.0".into()
}
fn default_4k_odd_fps_mode() -> String {
    "fallback".into()
}
fn default_display_sync_hdr_mode() -> String {
    "yes".into()
}
fn default_display_sync_wcg_mode() -> String {
    "auto".into()
}

#[derive(Clone, Serialize, Deserialize)]
pub(crate) struct ProfileSettings {
    // The profile's identity — a Jellyfin user id. Doubles as the key
    // `Config.active_profile_id` matches against; see `Config::active()`.
    #[serde(default)]
    pub user_id: String,
    #[serde(default)]
    pub server_url: String,
    #[serde(default)]
    pub token: String,

    // ── Profile identity (Bonfire Phase 1, step 6, 2026-08-09) — ProfilePickerScreen's
    // own avatar tile. display_name/avatar_initial default to the Jellyfin username's
    // own first letter on a normal/append login (profile.rs); a real Bonfire sub-profile
    // gets these overwritten from the plugin's own /list response the next time
    // sync_bonfire_subprofiles runs (after every successful master login).
    #[serde(default)]
    pub display_name: String,
    // Hex string ("#4a90d9"), not a Slint `color` — this struct has to stay
    // plain-serde-serializable; profile.rs parses it (or picks a deterministic
    // fallback when empty) when building the Slint-facing ProfileTile.
    #[serde(default)]
    pub avatar_color: String,
    #[serde(default)]
    pub avatar_initial: String,
    // True once discovered via a real bonfire_list_profiles() response — never
    // true for a profile that only ever came from do_login/append (normal
    // sign-in has no way to know it's talking to a Bonfire sub-profile; only
    // the MASTER's own /list call can tell us that).
    #[serde(default)]
    pub is_bonfire: bool,
    #[serde(default)]
    pub master_user_id: String,
    // True only for a /list entry that is ANOTHER master's own account (Bonfire
    // `is_master: true`), reached through a group the calling master joined or owns —
    // never for a sub-profile. Then `master_user_id` stays EMPTY (never self-referencing —
    // see `repair_bonfire_profile_corruption`), `account_root_id()` roots the entry to
    // itself, and `synced_via` records who discovered it (switch auth, prune scope).
    #[serde(default)]
    pub is_group_account: bool,
    // Bonfire Phase 5 — which of *my own* saved master accounts' `/list`
    // call most recently reported this entry. For a genuine sub-profile
    // this coincides with `master_user_id` (both are the calling master's
    // own id); for a group account it's the only field left recording who
    // discovered it, since `master_user_id` is empty for that case. Used by
    // `sync_bonfire_subprofiles`'s prune step and by `switch_to_profile`'s
    // bonfire branch to resolve which locally-known account to authenticate
    // a switch INTO a group account with (there's no independently-stored
    // token for someone else's account — the switch is always authenticated
    // via whichever of my own accounts joined/owns the group).
    #[serde(default)]
    pub synced_via: String,
    // Cached from the same /list response — may go stale between syncs, same
    // caveat as every other cached-until-next-refresh field in this app.
    #[serde(default)]
    pub has_pin: bool,
    // Minutes idle before this profile locks back to the picker; 0 = off. Bonfire only
    // declares it (developer-api.md: clients enforce it, the server doesn't). Cached from
    // /list like has_pin (same staleness), refreshed in sync_bonfire_subprofiles and the
    // self-edit save. Only acts when has_pin is also true — profile::wire_idle_lock_timer.
    #[serde(default)]
    pub lockout_minutes: i64,
    // OTHER account roots my latest /list sync reported as linked to my account, stored
    // only on the syncing session's own root entry. Independent of is_bonfire /
    // is_group_account (those encode auth authority, not membership): an account can be
    // independently known AND linked. Used by profile::linked_account_roots for the
    // picker's extra sections.
    #[serde(default)]
    pub bonfire_linked_roots: Vec<String>,
    // Only meaningful on an account-root entry (a sub-profile is switched into with its
    // master's token). Default true: the account resumes silently from its stored token.
    // False: the startup gate and picker switches show Login (password) instead, whatever
    // the launch policies say (should_show_picker_at_startup,
    // profile::account_requires_login). Set by "Remember this login" on LoginScreen and in
    // Settings. Inert for a group account (never set false; it never auto-resumes anyway).
    #[serde(default = "default_true")]
    pub remember_login: bool,

    #[serde(default = "default_true")]
    pub sub_enabled: bool,
    #[serde(default)]
    pub sub_lang: String,
    #[serde(default)]
    pub sub_lang2: String,
    #[serde(default)]
    pub sub_type: String,
    // ── Subtitle appearance — see the mpv sub-ass-override note on
    // sub_respect_ass_styling below; scale/pos apply to ASS subtitles
    // unconditionally (mpv's own default), color/background do not.
    #[serde(default = "default_sub_pct")]
    pub sub_scale_pct: u32,
    #[serde(default = "default_sub_pct")]
    pub sub_pos_pct: u32,
    // true (default) = don't touch mpv's sub-ass-override (its own default,
    // "scale" tier — embedded ASS styling is respected); false = force it,
    // so sub_color/sub_background below also apply to ASS-styled subtitles.
    #[serde(default = "default_true")]
    pub sub_respect_ass_styling: bool,
    // Display name from a static preset table ("" | "White" | "Yellow" | ...),
    // NOT a raw mpv color string — mirrors sub_lang storing a display name
    // that's translated to an mpv value at point of use.
    #[serde(default)]
    pub sub_color: String,
    #[serde(default)]
    pub sub_background: bool,
    #[serde(default)]
    pub audio_lang: String,
    // Auto-open the fullscreen Now Playing screen after ~30 s idle while music
    // plays. Fixed threshold in v1 — only the on/off is a setting. Profile-
    // scoped (2026-08-08, Bonfire Phase 1): pure per-viewer UX preference,
    // unlike gapless_audio's hardware-compatibility framing.
    #[serde(default = "default_now_playing_auto_open")]
    pub now_playing_auto_open: bool,

    // ── Intro Skipper skip modes ─────────────────────────────────────────────
    // "always-skip" | "ask" | "ask-timed" | "never-skip"  (Intro/Recap/Preview/Commercial)
    // "always-skip" | "ask" | "never-skip"                 (Credits)
    #[serde(default = "default_skip_mode")]
    pub skip_intro_mode: String,
    #[serde(default = "default_skip_secs")]
    pub skip_intro_secs: u32,
    #[serde(default = "default_skip_mode")]
    pub skip_recap_mode: String,
    #[serde(default = "default_skip_secs")]
    pub skip_recap_secs: u32,
    #[serde(default = "default_skip_mode")]
    pub skip_preview_mode: String,
    #[serde(default = "default_skip_secs")]
    pub skip_preview_secs: u32,
    #[serde(default = "default_skip_mode")]
    pub skip_commercial_mode: String,
    #[serde(default = "default_skip_secs")]
    pub skip_commercial_secs: u32,
    #[serde(default = "default_skip_mode")]
    pub skip_credits_mode: String,
    #[serde(default = "default_credits_secs")]
    pub skip_credits_secs: u32,

    // ── Library sort (0=NameAZ 1=NameZA 2=YearDesc 3=YearAsc 4=Random) ─────────
    #[serde(default)]
    pub library_movies_sort: u8,
    #[serde(default)]
    pub library_series_sort: u8,
    #[serde(default)]
    pub library_collections_sort: u8,
    #[serde(default)]
    pub library_artists_sort: u8,
    #[serde(default)]
    pub library_albums_sort: u8,
    #[serde(default)]
    pub library_playlists_sort: u8,

    // ── Music library view (0=Artists, 1=Albums, 2=Playlists) ────────────────
    #[serde(default)]
    pub library_music_view: u8,

    // ── Seerr integration (Settings → Integrations) — cleared on sign-out
    // alongside server_url/user_id/token. seerr_auth_method is purely
    // informational (drives the "Connected via X" Settings subtitle);
    // seerr_api_key is populated only when method == "apikey",
    // seerr_session_cookie for the other three methods. Both encrypted at
    // rest — see secrets.rs and load_config/save_config above.
    #[serde(default)]
    pub seerr_enabled: bool,
    #[serde(default)]
    pub seerr_url: String,
    #[serde(default)]
    pub seerr_auth_method: String,
    #[serde(default)]
    pub seerr_api_key: String,
    #[serde(default)]
    pub seerr_session_cookie: String,

    // ── Watch Trailer (Discover request-detail screen) ───────────────────────
    // "Best"|"1080p"|"720p"|"480p" — display-ready, mapped to an mpv
    // ytdl-format string by main.rs::trailer_ytdl_format.
    #[serde(default = "default_trailer_quality")]
    pub trailer_quality: String,

    // ── Discover filters ────────────────────────────────────────────────────────
    // "" / empty Vec / 0 = no filter. discover_filter_type: "" (All) | "movie" | "tv".
    // discover_filter_sort: "" (popularity) | "rating" | "newest" | "oldest" — an internal
    // key, mapped to the per-media-type TMDB sortBy at request time (the date fields
    // differ between movies and TV). min_rating: 0.0 = Any, else 6/7/8. min_year: 0 =
    // Any, else 2000/2010/2015/2020. Providers: TMDB watch-provider ids (shared by movie
    // and TV), ORed. Genres by NAME, not id — movie/TV genre ids differ even for the same
    // name; re-resolved to the type's id at request time. Profile-scoped (personal taste).
    #[serde(default)]
    pub discover_filter_type: String,
    #[serde(default)]
    pub discover_filter_genre_names: Vec<String>,
    #[serde(default)]
    pub discover_filter_sort: String,
    #[serde(default)]
    pub discover_filter_min_rating: f32,
    #[serde(default)]
    pub discover_filter_min_year: u32,
    #[serde(default)]
    pub discover_filter_provider_ids: Vec<i64>,

    // ── Request Options: remember the last choice ────────────────────────────────
    // Like Seerr's web UI: Quality/Profile/Tags picked in the Request Options modal are
    // remembered and pre-selected next time, for any item. Separate buckets for movie
    // and tv (Radarr/Sonarr have separate profile/tag ids), each keeping BOTH tiers'
    // profile+tags (like the modal's 2K/4K "alt" swap, discover::set_quality). Saved only
    // on a successful Request/Save (submit_request/submit_edit_request), never on Cancel.
    // Ids that no longer exist on the server just aren't found and fall back to
    // Default/unselected.
    #[serde(default)]
    pub request_pref_movie: RequestPreference,
    #[serde(default)]
    pub request_pref_tv: RequestPreference,
}

#[derive(Serialize, Deserialize, Clone, Default)]
pub(crate) struct RequestPreference {
    #[serde(default)]
    pub want_4k: bool,
    // 0 = the synthetic "Default" row (no profile override) — same
    // convention request_detail_selected_profile_id already uses.
    #[serde(default)]
    pub profile_id_2k: i32,
    #[serde(default)]
    pub profile_id_4k: i32,
    #[serde(default)]
    pub tag_ids_2k: Vec<i64>,
    #[serde(default)]
    pub tag_ids_4k: Vec<i64>,
}

impl Default for ProfileSettings {
    fn default() -> Self {
        Self {
            user_id: String::new(),
            server_url: String::new(),
            token: String::new(),
            display_name: String::new(),
            avatar_color: String::new(),
            avatar_initial: String::new(),
            is_bonfire: false,
            master_user_id: String::new(),
            has_pin: false,
            is_group_account: false,
            synced_via: String::new(),
            lockout_minutes: 0,
            bonfire_linked_roots: Vec::new(),
            remember_login: true,
            sub_enabled: true,
            sub_lang: String::new(),
            sub_lang2: String::new(),
            sub_type: String::new(),
            audio_lang: String::new(),
            sub_scale_pct: 100,
            sub_pos_pct: 100,
            sub_respect_ass_styling: true,
            sub_color: String::new(),
            sub_background: false,
            now_playing_auto_open: true,
            skip_intro_mode: default_skip_mode(),
            skip_intro_secs: 8,
            skip_recap_mode: default_skip_mode(),
            skip_recap_secs: 8,
            skip_preview_mode: default_skip_mode(),
            skip_preview_secs: 8,
            skip_commercial_mode: default_skip_mode(),
            skip_commercial_secs: 8,
            skip_credits_mode: default_skip_mode(),
            skip_credits_secs: 30,
            library_movies_sort: 0,
            library_series_sort: 0,
            library_collections_sort: 0,
            library_artists_sort: 0,
            library_albums_sort: 0,
            library_playlists_sort: 0,
            library_music_view: 0,
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
        let pos = self
            .profiles
            .iter()
            .position(|p| p.user_id == id)
            .unwrap_or(0);
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
    pub user_id: String,
    pub token: String,
    #[serde(default)]
    pub device_id: String,

    #[serde(default)]
    pub audio_spdif: bool,
    #[serde(default = "default_true")]
    pub spdif_ac3: bool,
    #[serde(default = "default_true")]
    pub spdif_eac3: bool,
    #[serde(default = "default_true")]
    pub spdif_dts: bool,
    #[serde(default = "default_true")]
    pub spdif_dts_hd: bool,
    #[serde(default = "default_true")]
    pub spdif_truehd: bool,
    #[serde(default = "default_hwdec")]
    pub hwdec: String,
    #[serde(default = "default_vf", deserialize_with = "deser_vf")]
    pub vf: String,
    #[serde(default = "default_video_sync")]
    pub video_sync: String,
    #[serde(default)]
    pub opengl_early_flush: bool,
    #[serde(default)]
    pub video_latency_hacks: bool,
    #[serde(default)]
    pub interpolation: bool,
    #[serde(default = "default_tscale")]
    pub tscale: String,
    #[serde(default = "default_tone_mapping")]
    pub tone_mapping: String,
    #[serde(default)]
    pub target_colorspace_hint: bool,
    #[serde(
        default = "default_deinterlace",
        deserialize_with = "deser_deinterlace"
    )]
    pub deinterlace: String,
    // Kept only so an old-shape file still deserializes cleanly — deliberately
    // no longer read by migrate_legacy_config, see its own comment.
    #[allow(dead_code)]
    #[serde(default)]
    pub cache_size_mb: u32,
    #[serde(default)]
    pub video_behind: bool,
    #[serde(default)]
    pub launch_fullscreen: bool,
    #[serde(default = "default_true")]
    pub sub_enabled: bool,
    #[serde(default)]
    pub sub_lang: String,
    #[serde(default)]
    pub sub_lang2: String,
    #[serde(default)]
    pub sub_type: String,
    #[serde(default = "default_sub_pct")]
    pub sub_scale_pct: u32,
    #[serde(default = "default_sub_pct")]
    pub sub_pos_pct: u32,
    #[serde(default = "default_true")]
    pub sub_respect_ass_styling: bool,
    #[serde(default)]
    pub sub_color: String,
    #[serde(default)]
    pub sub_background: bool,
    #[serde(default)]
    pub audio_lang: String,
    #[serde(default)]
    pub audio_device: String,
    #[serde(default)]
    pub audio_device_passthrough: String,
    #[serde(default = "default_audio_channels")]
    pub audio_channels: String,
    #[serde(default = "default_gapless")]
    pub gapless_audio: bool,
    #[serde(default = "default_now_playing_auto_open")]
    pub now_playing_auto_open: bool,
    #[serde(default)]
    pub alsa_irq_scheduling: bool,

    #[serde(default = "default_skip_mode")]
    pub skip_intro_mode: String,
    #[serde(default = "default_skip_secs")]
    pub skip_intro_secs: u32,
    #[serde(default = "default_skip_mode")]
    pub skip_recap_mode: String,
    #[serde(default = "default_skip_secs")]
    pub skip_recap_secs: u32,
    #[serde(default = "default_skip_mode")]
    pub skip_preview_mode: String,
    #[serde(default = "default_skip_secs")]
    pub skip_preview_secs: u32,
    #[serde(default = "default_skip_mode")]
    pub skip_commercial_mode: String,
    #[serde(default = "default_skip_secs")]
    pub skip_commercial_secs: u32,
    #[serde(default = "default_skip_mode")]
    pub skip_credits_mode: String,
    #[serde(default = "default_credits_secs")]
    pub skip_credits_secs: u32,

    #[serde(default)]
    pub library_movies_sort: u8,
    #[serde(default)]
    pub library_series_sort: u8,
    #[serde(default)]
    pub library_collections_sort: u8,
    #[serde(default)]
    pub library_artists_sort: u8,
    #[serde(default)]
    pub library_albums_sort: u8,
    #[serde(default)]
    pub library_playlists_sort: u8,
    #[serde(default)]
    pub library_music_view: u8,

    #[serde(default = "default_log_level")]
    pub log_level: String,

    #[serde(default = "default_seek_step")]
    pub seek_step_secs: u32,
    #[serde(default = "default_seek_step_long")]
    pub seek_step_long_secs: u32,

    #[serde(default = "default_speed_pct")]
    pub scroll_speed_pct: u32,
    #[serde(default = "default_speed_pct")]
    pub animation_speed_pct: u32,

    #[serde(default = "default_ui_font_family")]
    pub ui_font_family: String,

    #[serde(default)]
    pub seerr_enabled: bool,
    #[serde(default)]
    pub seerr_url: String,
    #[serde(default)]
    pub seerr_auth_method: String,
    #[serde(default)]
    pub seerr_api_key: String,
    #[serde(default)]
    pub seerr_session_cookie: String,

    #[serde(default = "default_trailer_quality")]
    pub trailer_quality: String,

    #[serde(default)]
    pub discover_filter_type: String,
    #[serde(default)]
    pub discover_filter_genre_names: Vec<String>,
    #[serde(default)]
    pub discover_filter_sort: String,
    #[serde(default)]
    pub discover_filter_min_rating: f32,
    #[serde(default)]
    pub discover_filter_min_year: u32,
    #[serde(default)]
    pub discover_filter_provider_ids: Vec<i64>,
}

/// Splits an old flat `LegacyConfig` into the new `DeviceConfig` +
/// single-entry `profiles` shape. Purely mechanical field reassignment —
/// see the device/profile split's own doc comment above `DeviceConfig` for
/// which field goes where and why.
fn migrate_legacy_config(l: LegacyConfig) -> Config {
    let device = DeviceConfig {
        device_id: l.device_id,
        audio_spdif: l.audio_spdif,
        spdif_ac3: l.spdif_ac3,
        spdif_eac3: l.spdif_eac3,
        spdif_dts: l.spdif_dts,
        spdif_dts_hd: l.spdif_dts_hd,
        spdif_truehd: l.spdif_truehd,
        hwdec: l.hwdec,
        vf: l.vf,
        video_sync: l.video_sync,
        opengl_early_flush: l.opengl_early_flush,
        video_latency_hacks: l.video_latency_hacks,
        interpolation: l.interpolation,
        tscale: l.tscale,
        tone_mapping: l.tone_mapping,
        target_colorspace_hint: l.target_colorspace_hint,
        separate_video_surface: true,
        video_own_buffers: false,
        video_dither_off: false,
        deinterlace: l.deinterlace,
        // l.cache_size_mb deliberately dropped, not migrated — it was a
        // broken setting (see the doc comment on DeviceConfig's own
        // cache_secs/cache_max_mb fields) that only ever adjusted an mpv
        // option that rarely bound in practice, so there's no real user
        // intent worth preserving from it; every migrated install just
        // gets the new, actually-functional defaults instead.
        cache_secs: default_cache_secs(),
        cache_max_mb: default_cache_max_mb(),
        video_behind: l.video_behind,
        launch_fullscreen: l.launch_fullscreen,
        audio_device: l.audio_device,
        audio_device_passthrough: l.audio_device_passthrough,
        audio_channels: l.audio_channels,
        gapless_audio: l.gapless_audio,
        alsa_irq_scheduling: l.alsa_irq_scheduling,
        // No legacy equivalent — same treatment as skip_fade_ms just below.
        skip_fade_mute_passthrough: default_skip_fade_mute_passthrough(),
        log_level: l.log_level,
        seek_step_secs: l.seek_step_secs,
        seek_step_long_secs: l.seek_step_long_secs,
        // No legacy equivalent — skip-fade shipped after the last flat
        // config.json shape, same treatment as launch_policy/
        // default_profile_id just below.
        skip_fade_ms: default_skip_fade_ms(),
        scroll_speed_pct: l.scroll_speed_pct,
        animation_speed_pct: l.animation_speed_pct,
        ui_font_family: l.ui_font_family,
        // No legacy equivalent — this feature shipped well after the last
        // flat config.json shape, same treatment as skip_fade_ms/
        // launch_policy above/below. Defaults on regardless of migration
        // path, matching every fresh install.
        onscreen_keyboard_enabled: true,
        launch_policy: default_launch_policy(),
        default_profile_id: String::new(),
        account_launch_policy: default_launch_policy(),
        default_account_id: String::new(),
        // No legacy equivalent — display_sync shipped well after the last
        // flat config.json shape, same treatment as the fields just above.
        // Defaults regardless of migration path, matching every fresh
        // install — off, and every other field at its own module default.
        display_sync_enabled: false,
        display_sync_screen_name: String::new(),
        display_sync_default_resolution: default_display_sync_resolution(),
        display_sync_default_hz: default_display_sync_hz(),
        display_sync_scale_4k: default_scale(),
        display_sync_scale_1080p: default_scale(),
        display_sync_sync_resolution: true,
        display_sync_sync_refresh_rate: true,
        display_sync_4k_odd_fps_mode: default_4k_odd_fps_mode(),
        display_sync_hdr_mode: default_display_sync_hdr_mode(),
        display_sync_wcg_mode: default_display_sync_wcg_mode(),
        display_sync_trailers: false,
    };
    let profile = ProfileSettings {
        user_id: l.user_id,
        server_url: l.server_url,
        token: l.token,
        // A pre-Bonfire flat config has no identity/avatar concept at all —
        // profile.rs's own tile-building already falls back gracefully
        // (empty display_name/avatar_initial derive from user_id) for
        // exactly this case, so leaving these blank here is correct, not
        // a gap to fill in.
        display_name: String::new(),
        avatar_color: String::new(),
        avatar_initial: String::new(),
        is_bonfire: false,
        master_user_id: String::new(),
        has_pin: false,
        // No legacy equivalent — Bonfire Phase 5 (cross-household groups)
        // shipped well after the last flat config.json shape; a pre-Bonfire
        // flat config has no group concept at all, so both are inert.
        is_group_account: false,
        synced_via: String::new(),
        // No legacy equivalent — Bonfire's own lockoutMinutes field shipped
        // well after the last flat config.json shape, same treatment as
        // has_pin/is_bonfire above. A pre-Bonfire flat config has no PIN
        // concept at all, so this is inert (never acted on without has_pin)
        // regardless.
        lockout_minutes: 0,
        // No legacy equivalent — Bonfire cross-household groups shipped
        // well after the last flat config.json shape; nothing to migrate.
        bonfire_linked_roots: Vec::new(),
        remember_login: true,
        sub_enabled: l.sub_enabled,
        sub_lang: l.sub_lang,
        sub_lang2: l.sub_lang2,
        sub_type: l.sub_type,
        sub_scale_pct: l.sub_scale_pct,
        sub_pos_pct: l.sub_pos_pct,
        sub_respect_ass_styling: l.sub_respect_ass_styling,
        sub_color: l.sub_color,
        sub_background: l.sub_background,
        audio_lang: l.audio_lang,
        now_playing_auto_open: l.now_playing_auto_open,
        skip_intro_mode: l.skip_intro_mode,
        skip_intro_secs: l.skip_intro_secs,
        skip_recap_mode: l.skip_recap_mode,
        skip_recap_secs: l.skip_recap_secs,
        skip_preview_mode: l.skip_preview_mode,
        skip_preview_secs: l.skip_preview_secs,
        skip_commercial_mode: l.skip_commercial_mode,
        skip_commercial_secs: l.skip_commercial_secs,
        skip_credits_mode: l.skip_credits_mode,
        skip_credits_secs: l.skip_credits_secs,
        library_movies_sort: l.library_movies_sort,
        library_series_sort: l.library_series_sort,
        library_collections_sort: l.library_collections_sort,
        library_artists_sort: l.library_artists_sort,
        library_albums_sort: l.library_albums_sort,
        library_playlists_sort: l.library_playlists_sort,
        library_music_view: l.library_music_view,
        seerr_enabled: l.seerr_enabled,
        seerr_url: l.seerr_url,
        seerr_auth_method: l.seerr_auth_method,
        seerr_api_key: l.seerr_api_key,
        seerr_session_cookie: l.seerr_session_cookie,
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
    Config {
        device,
        profiles: vec![profile],
        active_profile_id,
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
/// A server-provided id as a cache file or folder name — None unless it is
/// exactly 32 hex characters (Jellyfin's id format; every existing cache
/// entry has it). Anything else could leave the cache folder: `../` climbs
/// out, and an absolute path replaces the base in `Path::join` — a
/// malicious server, or anyone altering plain-HTTP traffic, could then
/// write or delete any file the user can (2026-10-09 security review).
pub(crate) fn safe_cache_name(id: &str) -> Option<&str> {
    (id.len() == 32 && id.bytes().all(|b| b.is_ascii_hexdigit())).then_some(id)
}

/// None when `item_id` isn't a valid cache name (see safe_cache_name) —
/// the image is then fetched but not cached.
pub(crate) fn poster_cache_path(item_id: &str) -> Option<std::path::PathBuf> {
    Some(poster_cache_dir().join(safe_cache_name(item_id)?))
}
pub(crate) fn backdrop_cache_path(item_id: &str) -> Option<std::path::PathBuf> {
    Some(backdrop_cache_dir().join(safe_cache_name(item_id)?))
}
// Bonfire Phase 1 (cache namespacing, 2026-08-09): namespaced under the
// owning profile's user_id, same as the seven caches in home.rs — see that
// file's own module-header comment for the full "why explicit, not
// resolved internally" reasoning.
pub(crate) fn screen_caches_path(user_id: &str) -> Option<std::path::PathBuf> {
    Some(
        xdg_cache_base()
            .join("fjord")
            .join("profiles")
            .join(safe_cache_name(user_id)?)
            .join("screen_caches.json"),
    )
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
        "home.json",
        "movies.json",
        "series.json",
        "collections.json",
        "artists.json",
        "albums.json",
        "playlists.json",
        "screen_caches.json",
    ];
    let Some(user_id) = safe_cache_name(user_id) else {
        return;
    };
    let old_base = xdg_cache_base().join("fjord");
    let new_dir = old_base.join("profiles").join(user_id);
    for filename in CACHE_FILES {
        let old_path = old_base.join(filename);
        let new_path = new_dir.join(filename);
        if new_path.exists() || !old_path.exists() {
            continue;
        }
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
/// None unless `key` is 1–64 characters of `a-z`, `0-9` and `-` (the keys
/// are built from TMDB's numeric ids; checked anyway, see safe_cache_name).
pub(crate) fn discover_poster_cache_path(key: &str) -> Option<std::path::PathBuf> {
    let ok = (1..=64).contains(&key.len())
        && key
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
    ok.then(|| discover_poster_cache_dir().join(key))
}

pub(crate) fn fmt_resume_label(secs: f64) -> String {
    let s = secs as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let s = s % 60;
    if h > 0 {
        format!("Resume from {}:{:02}:{:02}", h, m, s)
    } else {
        format!("Resume from {}:{:02}", m, s)
    }
}

// Self-heals `Config.profiles` against two corruption patterns written by an older,
// unguarded `sync_bonfire_subprofiles` (it used the CALLING session's `user_id` as
// `master_user_id` for everything /list returned; it now refuses to run unless the
// caller is a true master). Repairs only already-corrupted config.json files; returns
// whether anything changed (the caller re-saves).
//
// **Self-reference:** a master rewritten to `is_bonfire: true, master_user_id: <itself>`
// (a switch to it then fails) → restored to a plain account.
//
// **Re-parented under a sub-profile:** a `master_user_id` pointing at another Bonfire
// profile (a sub-profile is never another's master) is re-pointed at its chain's
// root, following up to 5 hops.
//
// **Cycles** (A → B → A, when /list also returned the real master): the data can't
// tell which node is the real master, so only nodes whose own walk loops back to
// themselves are demoted to standalone accounts; profiles pointing into the cycle
// are left alone. That interim state works, and the next real login as the true
// master re-derives the household from /list (ground truth).
fn repair_bonfire_profile_corruption(profiles: &mut [ProfileSettings]) -> bool {
    let mut repaired = false;
    for p in profiles.iter_mut() {
        if p.is_bonfire && p.master_user_id == p.user_id {
            tracing::warn!(
                "load_config: repairing self-referencing Bonfire profile entry ({})",
                p.user_id
            );
            p.is_bonfire = false;
            p.master_user_id.clear();
            repaired = true;
        }
    }
    let snapshot = profiles.to_vec();
    for p in profiles.iter_mut() {
        if !p.is_bonfire || p.master_user_id.is_empty() {
            continue;
        }
        let Some(direct_master) = snapshot.iter().find(|m| m.user_id == p.master_user_id) else {
            continue;
        };
        if !direct_master.is_bonfire {
            continue;
        } // already correct — master is a genuine root
        let mut chain: Vec<&str> = vec![p.user_id.as_str(), direct_master.user_id.as_str()];
        let mut cursor = direct_master;
        let mut resolved = None;
        let mut cycle_includes_self = false;
        for _ in 0..5 {
            let Some(next) = snapshot.iter().find(|m| m.user_id == cursor.master_user_id) else {
                break;
            };
            if !next.is_bonfire {
                resolved = Some(next.user_id.clone());
                break;
            }
            if chain.contains(&next.user_id.as_str()) {
                cycle_includes_self = next.user_id == p.user_id;
                break;
            }
            chain.push(next.user_id.as_str());
            cursor = next;
        }
        if let Some(real_master) = resolved {
            tracing::warn!(
                "load_config: repairing Bonfire profile {} — was reparented under sub-profile {}, restoring real master {}",
                p.user_id,
                p.master_user_id,
                real_master
            );
            p.master_user_id = real_master;
            repaired = true;
        } else if cycle_includes_self {
            tracing::warn!(
                "load_config: profile {} is part of a Bonfire master_user_id CYCLE (master {}) with no real root reachable — demoting to a standalone account; the real household tree self-heals on the true master's next genuine login",
                p.user_id,
                p.master_user_id
            );
            p.is_bonfire = false;
            p.master_user_id.clear();
            repaired = true;
        }
    }
    repaired
}

// Loads config.json: the current (device/profiles) shape first, else the old flat
// `LegacyConfig`, migrated forward and re-saved once. Every profile's token /
// seerr_api_key / seerr_session_cookie is decrypted here (encrypted at rest, see
// secrets.rs), so the rest of the app reads plain strings. `device_id` stays
// plaintext: it's the key material, not a bearer credential.
pub(crate) fn load_config() -> Option<Config> {
    let data = std::fs::read_to_string(config_path()).ok()?;
    let (mut cfg, migrated) = match serde_json::from_str::<Config>(&data) {
        Ok(c) => (c, false),
        Err(new_err) => match serde_json::from_str::<LegacyConfig>(&data) {
            Ok(legacy) => (migrate_legacy_config(legacy), true),
            Err(legacy_err) => {
                // Neither shape parses (crash mid-write, bad manual edit, disk error): log it —
                // otherwise it looks like a fresh install and the user lands on Login with
                // everything gone and nothing in the log.
                tracing::error!(
                    "load_config: config.json is neither valid new-shape ({new_err:#}) nor valid legacy-shape ({legacy_err:#}) — treating as absent, all settings/session will reset"
                );
                return None;
            }
        },
    };
    if !cfg.device.device_id.is_empty() {
        let key = crate::secrets::derive_key(&cfg.device.device_id);
        for p in cfg.profiles.iter_mut() {
            p.token = crate::secrets::decrypt_field(&p.token, &key);
            p.seerr_api_key = crate::secrets::decrypt_field(&p.seerr_api_key, &key);
            p.seerr_session_cookie = crate::secrets::decrypt_field(&p.seerr_session_cookie, &key);
        }
    }
    let repaired = repair_bonfire_profile_corruption(&mut cfg.profiles);
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
    // One save at a time (2026-10-04): saves run from the UI thread and from
    // background tasks, and all of them write the same `config.json.tmp`.
    // Two overlapping saves → the second rename found the temp file already
    // moved ("rename … failed: No such file or directory — settings NOT
    // saved", seen at startup on both the dev machine and the HTPC). This
    // lock is never held while taking any other lock, so it can't deadlock.
    static SAVE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _save_guard = SAVE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let path = config_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
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
    // Every failure below is logged: this is the one persistence path (settings,
    // sign-out, profile switch) — a silent failure looks like settings resetting.
    match serde_json::to_string_pretty(&on_disk) {
        Ok(json) => {
            let tmp = path.with_extension("json.tmp");
            match write_private(&tmp, json.as_bytes()) {
                Ok(()) => {
                    if let Err(e) = std::fs::rename(&tmp, &path) {
                        tracing::error!(
                            "save_config: rename {tmp:?} -> {path:?} failed: {e:#} — settings NOT saved"
                        );
                    }
                }
                Err(e) => tracing::error!(
                    "save_config: write to {tmp:?} failed: {e:#} — settings NOT saved"
                ),
            }
        }
        Err(e) => tracing::error!("save_config: serialization failed: {e:#} — settings NOT saved"),
    }
    set_owner_only_permissions(&path);
}

/// Writes `data` to `path`, readable by the owner only from the first byte on
/// (writing with the default umask and chmod-ing after the rename would leave
/// config.json, which holds the tokens, world-readable for a moment). Re-tightens an
/// existing file too: a temp file left by a crash keeps its old mode when reopened.
pub(crate) fn write_private(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    f.write_all(data)
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
    pub item_detail: BoundedCache<MediaItem>,
    pub similar_items: BoundedCache<Vec<MediaItem>>,
    pub boxset_items: BoundedCache<Vec<MediaItem>>,
    pub artist_albums: BoundedCache<Vec<MediaItem>>,
    pub person_filmography: BoundedCache<Vec<MediaItem>>,
    pub container_tracks: BoundedCache<Vec<MediaItem>>,
    // Jellyfin person id → TMDB person id (Person "Other Work" row); `None` = already
    // tried, no confident match — worth persisting, a miss costs a fuzzy name search.
    // Has a default fn (BoundedCache has no Default) so older screen_caches.json files
    // without this field still load.
    #[serde(default = "default_person_tmdb_id_cache")]
    pub person_tmdb_id: BoundedCache<Option<i64>>,
}

fn default_person_tmdb_id_cache() -> BoundedCache<Option<i64>> {
    BoundedCache::new(100)
}

pub(crate) fn load_screen_caches(user_id: &str) -> Option<ScreenCachesFile> {
    let data = std::fs::read_to_string(screen_caches_path(user_id)?).ok()?;
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
            item_detail: s.item_detail_cache.clone(),
            similar_items: s.similar_items_cache.clone(),
            boxset_items: s.boxset_items_cache.clone(),
            artist_albums: s.artist_albums_cache.clone(),
            person_filmography: s.person_filmography_cache.clone(),
            container_tracks: s.container_tracks_cache.clone(),
            person_tmdb_id: s.person_tmdb_id_cache.clone(),
        }
    };
    let Some(path) = screen_caches_path(user_id) else {
        tracing::warn!("save_screen_caches: not a valid user id for a cache folder — not saved");
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 2026-08-28 logging audit — same silent-failure shape save_config had
    // (this file can reach ~1.3MB after a library prewarm, and this runs
    // on a 60s repeating timer for the whole session).
    match serde_json::to_string(&file) {
        Ok(json) => {
            let tmp = path.with_extension("json.tmp");
            match std::fs::write(&tmp, &json) {
                Ok(()) => {
                    if let Err(e) = std::fs::rename(&tmp, &path) {
                        tracing::error!(
                            "save_screen_caches: rename {tmp:?} -> {path:?} failed: {e:#}"
                        );
                    }
                }
                Err(e) => tracing::error!("save_screen_caches: write to {tmp:?} failed: {e:#}"),
            }
        }
        Err(e) => tracing::error!("save_screen_caches: serialization failed: {e:#}"),
    }
}

pub(crate) fn ensure_device_id(cfg: &mut Config) {
    if !cfg.device.device_id.is_empty() {
        return;
    }
    cfg.device.device_id = std::fs::read_to_string("/proc/sys/kernel/random/uuid")
        .unwrap_or_default()
        .trim()
        .to_string();
    if cfg.device.device_id.is_empty() {
        cfg.device.device_id = format!(
            "fjord-{:016x}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        );
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
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    // 2026-08-28 logging audit — same silent-failure shape save_config
    // had: a rebind that silently failed to persist would read as "my
    // rebind didn't save" with nothing in the log to explain why.
    match serde_json::to_string_pretty(kb) {
        Ok(json) => {
            if let Err(e) = std::fs::write(&path, json) {
                tracing::error!("save_keybindings: write to {path:?} failed: {e:#}");
            }
        }
        Err(e) => tracing::error!("save_keybindings: serialization failed: {e:#}"),
    }
}

// ── screen-open caches (Part 2 of the loading-consolidation plan) ────────────

fn default_cap() -> usize {
    40
}

/// FIFO cache: at most `cap` entries, oldest evicted first — reopening something you
/// just looked at shows instantly. Persisted (`screen_caches.json`); kept fresh by
/// ws.rs's LibraryChanged/UserDataChanged handlers (remove/insert) plus a post-login
/// background refresh, not a TTL. `cap` is persisted too: the opt-in library prewarm
/// (prewarm.rs) raises it via `set_cap` to fit the whole library; `default_cap()` only
/// covers files predating the field.
///
/// Storage is `Arc<BoundedCacheInner<V>>` with `Arc::make_mut` in every mutating
/// method (clone-on-write): `clone()` — used by `save_screen_caches` to snapshot all
/// caches under a brief lock — is O(1), and a mutation only pays a real copy while a
/// save still holds a clone. Needs serde's `rc` feature (workspace Cargo.toml).
#[derive(Serialize, Deserialize, Clone, Default)]
struct BoundedCacheInner<V> {
    map: std::collections::HashMap<String, V>,
    order: std::collections::VecDeque<String>,
    #[serde(default = "default_cap")]
    cap: usize,
}

// `transparent`: serializes as BoundedCacheInner's flat {map, order, cap} shape, not
// {"inner": …}, so existing screen_caches.json files (the six caches in
// ScreenCachesFile embed it) keep the exact same format.
#[derive(Serialize, Deserialize, Clone)]
#[serde(transparent)]
pub(crate) struct BoundedCache<V> {
    inner: std::sync::Arc<BoundedCacheInner<V>>,
}

impl<V: Clone> BoundedCache<V> {
    pub(crate) fn new(cap: usize) -> Self {
        Self {
            inner: std::sync::Arc::new(BoundedCacheInner {
                map: Default::default(),
                order: Default::default(),
                cap,
            }),
        }
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
            if inner.order.len() > inner.cap
                && let Some(oldest) = inner.order.pop_front()
            {
                inner.map.remove(&oldest);
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
    /// Up to the `n` most recently inserted/touched keys — for the post-login background
    /// refresh (`startup::spawn_screen_cache_refresh`), which must stay a small "recently
    /// used" slice even when the prewarm has filled the cache with the whole library.
    pub(crate) fn recent_keys(&self, n: usize) -> Vec<String> {
        let len = self.inner.order.len();
        self.inner
            .order
            .iter()
            .skip(len.saturating_sub(n))
            .cloned()
            .collect()
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
    pub sub_lang: Option<String>,
}

// ── app state (library + settings) ───────────────────────────────────────────

pub(crate) struct FjordState {
    pub config: Config, // authoritative settings + auth; saved on change
    pub client: Option<Arc<JellyfinClient>>,
    // PIN digits typed in ProfilePickerScreen's PIN entry — never stored in a Slint
    // property (see AppState.profile-pin-len); cleared on every PIN-entry open and after
    // every switch attempt.
    pub profile_pin_buffer: String,
    // Same discipline for ProfileEditScreen's two PIN pads: the profile's new PIN and the
    // master's confirmation PIN. Two buffers so the wrong value can never be sent to
    // Bonfire.
    pub profile_edit_pin_buffer: String,
    pub profile_edit_master_pin_buffer: String,
    // The last bonfire_list_profiles() result ManageProfilesScreen showed —
    // kept around purely so selecting a tile can resolve back to the full
    // BonfireProfile (parental rating, tags, enabled libraries, etc.) it
    // needs to pre-fill ProfileEditScreen with, without a second fetch.
    pub manage_profiles_cache: Vec<fjord_api::models::BonfireProfile>,
    // Session-only, never persisted. `ProfileSettings.has_pin` is only as fresh as the
    // last sync; Bonfire's `bypassPinOnLocalNetwork` can make the live `requires_pin`
    // false (this device is on the LAN) while has_pin stays true. Captured from
    // sync_bonfire_subprofiles' /list loop; one flat map (LAN bypass depends on this
    // device's network position, not the syncing account). Every "show a PIN pad?"
    // decision prefers it over has_pin and falls back to has_pin when nothing was
    // captured yet.
    pub live_requires_pin: std::collections::HashMap<String, bool>,
    // Plugin names installed on the server, fetched once per login (GET /Plugins) —
    // Bonfire's quick presence check before /plugins/profiles/list. By NAME, not GUID
    // (GUIDs weren't verifiable here). Session-scoped, cleared by reset_session_state.
    pub available_plugins: std::collections::HashSet<String>,
    pub keybindings: Keybindings,
    pub all_movies: Vec<MediaItem>,
    pub all_series: Vec<MediaItem>,
    pub all_collections: Vec<MediaItem>,
    pub all_artists: Vec<MediaItem>,
    pub all_albums: Vec<MediaItem>,
    pub all_playlists: Vec<MediaItem>,
    pub movies_fetched: bool,
    // The Movies grid's posters were loaded for the current all_movies (reset
    // with movies_fetched). Discover fetches the list without posters, so the
    // grid can't rely on movies_fetched alone (2026-10-10).
    pub movie_posters_loaded: bool,
    pub collections_fetched: bool,
    pub artists_fetched: bool,
    pub albums_fetched: bool,
    pub playlists_fetched: bool,
    pub filtered_items: Vec<MediaItem>,
    // True once the unfiltered (query "") Browse All list has been built this session —
    // arriving at nav 5 doesn't rebuild the ~800-item model each time (like
    // movies_fetched / discover_landing_fetched). A WS LibraryChanged that changes
    // all_movies/all_series invalidates it (ws.rs).
    pub browse_populated: bool,
    pub series_open_id: String,
    pub series_season_ids: Vec<String>,
    pub series_episode_items: Vec<MediaItem>,
    pub series_episode_cache: std::collections::HashMap<String, Vec<MediaItem>>,
    pub series_season_generation: u64,
    pub last_nw_mov_refresh: Option<Instant>,
    pub last_nw_tv_refresh: Option<Instant>,
    pub audio_devices: Vec<(String, String)>, // (mpv name, description)
    pub display_sync_outputs: Vec<(String, String)>, // (kscreen-doctor connector name, display label incl. "(Primary)")
    pub system_fonts: Vec<(String, String)>,         // (value, display) — see fetch_system_fonts
    pub movie_collections: std::collections::HashMap<String, (String, String)>, // movie_id → (boxset_id, boxset_name)
    // Per-series audio/subtitle language remembered from a manual S/A panel
    // pick (controls.rs::on_commit_panel_selection writes; playback.rs's
    // track auto-select reads, preferring it over Config.sub_lang/audio_lang
    // for that series only). In-memory only — session-scoped like
    // series_episode_cache, not persisted to disk; cleared on sign-out.
    pub remembered_tracks: std::collections::HashMap<String, RememberedTracks>,
    // A rebind capture that collided with another action's binding, waiting for the
    // confirm/cancel dialog (keys::rebind_action / dispatch_keybinding_nav). In memory
    // only.
    pub pending_keybind_rebind: Option<crate::keys::PendingKeybindRebind>,
    pub ws_abort: Option<tokio::task::AbortHandle>, // abort to stop the WS reconnect loop on sign-out
    // Live connection-health signal: the WebSocket's 30 s keep-alive proves the server
    // is reachable independently of a stalled stream (e.g. a spun-down library disk
    // blocks server-side reads, not the WS). Stall recovery (wire_mpv_timer) uses it to
    // pick a LONG retry budget (connection healthy — be patient) or a SHORT one (status
    // unknown — report a broken network quickly). ws.rs sets ws_connected true after
    // connect and false the moment the read loop ends; ws_last_keepalive_at is stamped
    // on every keep-alive ack.
    pub ws_connected: bool,
    pub ws_last_keepalive_at: Option<Instant>,
    // Screen-open caches (Part 2, see BoundedCache doc comment above). Keyed by
    // item id (or the relevant container id — boxset/artist/person/album/playlist).
    pub item_detail_cache: BoundedCache<MediaItem>, // get_item_detail — shared by all 7 screens
    pub similar_items_cache: BoundedCache<Vec<MediaItem>>, // get_similar_items — detail.rs + series.rs
    pub boxset_items_cache: BoundedCache<Vec<MediaItem>>, // get_boxset_items — detail.rs + collection.rs
    pub artist_albums_cache: BoundedCache<Vec<MediaItem>>, // get_artist_albums — artist.rs
    pub person_filmography_cache: BoundedCache<Vec<MediaItem>>, // get_person_filmography — person.rs
    pub container_tracks_cache: BoundedCache<Vec<MediaItem>>, // get_album_tracks / get_playlist_items — album.rs
    // Jellyfin person id → TMDB person id (None = no match) for the Person "Other Work"
    // row. Persisted via ScreenCachesFile — the fuzzy name-search fallback is expensive.
    pub person_tmdb_id_cache: BoundedCache<Option<i64>>,
    // Person "Other Work" results by TMDB person id — session-only (not in
    // ScreenCachesFile): DiscoverCardMeta.item_type is a &'static str that can't
    // round-trip through serde, and discovery results are fine to refetch. Keeps
    // poster_path with each meta so a cache hit can still show posters.
    pub person_other_work_cache:
        BoundedCache<Vec<(crate::discover::DiscoverCardMeta, Option<String>)>>,
    // TMDB person id → matching local Jellyfin Person id (None = no confident match),
    // for opening a Discover cast member (RequestDetailScreen's CastRow). Session-only:
    // cheap to redo, and the library can change what a search finds.
    pub local_person_by_tmdb_cache: BoundedCache<Option<String>>,
    // In-flight marker for open_person_from_discover: the TMDB person id being resolved.
    // Repeat presses on the same cast member are ignored until it resolves and the
    // screen opens (otherwise each press ran a full resolve + open chain;
    // open_person_screen_tmdb's own guard can't see a chain still inside
    // resolve_local_person). Not persisted.
    pub person_discover_resolving: Option<i64>,
    // Opt-in one-time library prewarm progress (Phase 104) — read by a 1s
    // AppState-updating timer (main.rs::wire_prewarm_progress_timer), written
    // by prewarm.rs's two spawn_*_prewarm functions.
    pub prewarm_metadata_running: bool,
    pub prewarm_metadata_total: usize,
    pub prewarm_metadata_done: usize,
    pub prewarm_metadata_summary: String,
    pub prewarm_image_running: bool,
    pub prewarm_image_total: usize,
    pub prewarm_image_done: usize,
    pub prewarm_image_summary: String,
    // Built from Config.seerr_* at startup (enabled + valid key/cookie) and after every
    // successful Connect Seerr flow. None = not connected; a 401 (session auth only —
    // API keys don't expire) resets it to None (discover's re-auth handling).
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
    // All pages of the filtered-browse view (query empty, ≥1 filter), so a page-2+ load
    // re-sorts the FULL set by sort_key — sorting each page alone breaks the order across
    // page boundaries. Reset on every fresh page-1 fetch.
    pub discover_filtered_metas: Vec<crate::discover::FilteredRowItem>,
    // Request state for cards outside the Requested row: (item_type, tmdb id) → request,
    // filled from fetch_requested_row's list (no extra call) and used to patch search
    // results and the other landing rows, so their context menu offers Edit/Cancel/View
    // Request. Covers fetch_requested_row's ~20-per-type window only — a cheap tradeoff
    // over an uncapped GET /request sweep.
    pub discover_known_requests:
        std::collections::HashMap<(&'static str, String), crate::discover::KnownRequest>,
    // discover_watchlist_ids: (item_type, tmdb id) like discover_known_requests — filled by
    // ensure_discover_watchlist/refresh_watchlist, sets CardItem.on-watchlist.
    // discover_calendar_entries: the "Coming Up" row's data (build_calendar_entries),
    // rebuilt when the watchlist or requested set changes. seerr_discover_region: like
    // seerr_streaming_region but for Seerr's separate discoverRegion setting (release
    // dates), not the streaming region.
    pub discover_watchlist_ids: std::collections::HashSet<(&'static str, String)>,
    // LOCAL Jellyfin ids currently on the Seerr watchlist (the Jellyfin-id counterpart of
    // discover_watchlist_ids). item_to_card_item/items_to_model read it, so the watchlist
    // star is right every time a model is rebuilt from MediaItems (grid open, sort,
    // filter, WS sync, refresh) — a live patch alone would be wiped by the next rebuild.
    // Replaced wholesale by resync_jellyfin_watchlist_stars, updated per item by
    // discover_toggle_watchlist.
    pub jellyfin_watchlist_ids: std::collections::HashSet<String>,
    // Generation guard for resync_jellyfin_watchlist_stars, which starts from 4
    // independent triggers (watchlist fetch, push_cached_data, auto-login's series
    // landing, spawn_movies_list_fetch). Each call bumps it and only writes if it's
    // still the newest when ready, so an older call scanning less-complete data can't
    // overwrite a newer result and un-star still-watchlisted cards.
    pub jellyfin_watchlist_resync_seq: u64,
    // Rate-limits the 7 screens' "revalidate on cache hit" (spawn_*_revalidate) to once
    // per REVALIDATE_COOLDOWN per item id (should_revalidate). A plain HashMap (Instant
    // isn't Serialize, nothing to persist); cleared on sign-out.
    pub screen_revalidate_last_run: std::collections::HashMap<String, Instant>,
    // Guards the watchlist-id fetch (ensure_discover_watchlist) the same
    // way discover_landing_fetched guards the landing rows — once per
    // session, reset on disconnect/reconnect/sign-out.
    pub discover_watchlist_fetched: bool,
    pub discover_calendar_entries: Vec<crate::discover::CalendarEntry>,
    pub seerr_discover_region: Option<String>,
    // Guards the once-per-session genre + watch-provider list fetch (like
    // discover_landing_fetched, reset with it). The raw lists are kept so switching
    // Type (All/Movies/TV) rebuilds the chip models without a re-fetch.
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
    // (iso_639_1, "English Name (en)") pairs — TMDB's full language list (GET /languages),
    // shared by the Display Language and Discover Language dropdowns (see
    // fjord_seerr::Language for why not Seerr's smaller UI-locale set).
    pub seerr_languages: Vec<(String, String)>,
    // The connected user's `locale` (Display Language) — Seerr's default TMDB `language`
    // param whenever Fjord passes none; it changes the language of titles/overviews.
    // "" = "Default (English)". None until fetched.
    pub seerr_locale: Option<String>,
    // The connected user's `originalLanguage` (Discover Language — filters results by
    // TMDB original language). "all" (Seerr's own sentinel, not "") = "Default (All
    // Languages)". None until fetched.
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
    // MANAGE_BLOCKLIST — separate from MANAGE_REQUESTS/ADMIN (fjord_seerr::User::
    // can_manage_blocklist), read from the same get_current_user call as seerr_is_admin.
    // Gates the Discover context menu's Blocklist row, RequestDetailScreen's Blocklist
    // button, CollectionScreen's bulk blocklist and Settings → Manage Blocklist. False
    // until fetched or when not connected.
    pub seerr_can_manage_blocklist: bool,
    // Jellyfin's server-admin flag (Policy.IsAdministrator — what Bonfire's admin/*
    // endpoints check server-side). Never persisted; re-fetched on every session start
    // (startup::spawn_jellyfin_admin_check). Gates Settings → Profiles → "Bonfire Admin".
    pub jellyfin_is_server_admin: bool,
    // Manage Blocklist pagination (blocklist.rs): `skip` offset into GET /blocklist,
    // reset to 0 on every open. blocklist_total_results (page_info.results) tells
    // load_more whether another page exists; blocklist_loading_more prevents a
    // double fetch.
    pub blocklist_skip: u32,
    pub blocklist_total_results: u32,
    pub blocklist_loading_more: bool,
    // Rate-limits discover::refresh_seerr_admin_status (a GET /auth/me) — without it,
    // holding Down/Up through the sidebar fired one request per pass over Discover, and
    // their late UI updates collided with the next key press (a visible hitch on the
    // HTPC). None before the first refresh.
    pub seerr_admin_last_refresh: Option<Instant>,
    // Whether `yt-dlp` was found on `PATH` at startup (`main.rs::
    // detect_yt_dlp`) — gates Watch Trailer button visibility. A pure
    // local-machine fact, not tied to Seerr connection state, not reset on
    // sign-out/disconnect.
    pub yt_dlp_available: bool,
    // Trailer check (discover::start_trailer_check): YouTube URL → plays or not.
    // Session-only, cleared in reset_session_state. request_detail_trailers = the
    // candidates of the RequestDetail screen showing, so a failed play can re-check the rest.
    pub trailer_playable: std::collections::HashMap<String, bool>,
    pub request_detail_trailers: Vec<String>,
    // What's currently applied to the physical output, so a same-mode item (back-to-back
    // episodes) doesn't re-switch and pay the ~3 s settle again. None = nothing applied
    // this session. Not reset on sign-out/profile switch — a fact about the display.
    pub display_sync_current_mode: Option<(String, String)>,
    pub display_sync_current_hdr: Option<bool>,
}

impl FjordState {
    pub(crate) fn new() -> Self {
        Self {
            config: Config::default(),
            client: None,
            available_plugins: std::collections::HashSet::new(),
            profile_pin_buffer: String::new(),
            profile_edit_pin_buffer: String::new(),
            profile_edit_master_pin_buffer: String::new(),
            manage_profiles_cache: vec![],
            live_requires_pin: std::collections::HashMap::new(),
            keybindings: load_keybindings(),
            all_movies: vec![],
            all_series: vec![],
            all_collections: vec![],
            all_artists: vec![],
            all_albums: vec![],
            all_playlists: vec![],
            movies_fetched: false,
            movie_posters_loaded: false,
            collections_fetched: false,
            artists_fetched: false,
            albums_fetched: false,
            playlists_fetched: false,
            filtered_items: vec![],
            browse_populated: false,
            series_open_id: String::new(),
            series_season_ids: vec![],
            series_episode_items: vec![],
            series_episode_cache: std::collections::HashMap::new(),
            series_season_generation: 0,
            last_nw_mov_refresh: None,
            last_nw_tv_refresh: None,
            audio_devices: vec![],
            display_sync_outputs: vec![],
            system_fonts: vec![],
            movie_collections: std::collections::HashMap::new(),
            remembered_tracks: std::collections::HashMap::new(),
            pending_keybind_rebind: None,
            ws_abort: None,
            ws_connected: false,
            ws_last_keepalive_at: None,
            item_detail_cache: BoundedCache::new(40),
            similar_items_cache: BoundedCache::new(40),
            boxset_items_cache: BoundedCache::new(40),
            artist_albums_cache: BoundedCache::new(40),
            person_filmography_cache: BoundedCache::new(40),
            container_tracks_cache: BoundedCache::new(40),
            person_tmdb_id_cache: BoundedCache::new(100),
            person_other_work_cache: BoundedCache::new(40),
            local_person_by_tmdb_cache: BoundedCache::new(100),
            person_discover_resolving: None,
            prewarm_metadata_running: false,
            prewarm_metadata_total: 0,
            prewarm_metadata_done: 0,
            prewarm_metadata_summary: String::new(),
            prewarm_image_running: false,
            prewarm_image_total: 0,
            prewarm_image_done: 0,
            prewarm_image_summary: String::new(),
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
            jellyfin_is_server_admin: false,
            blocklist_skip: 0,
            blocklist_total_results: 0,
            blocklist_loading_more: false,
            seerr_admin_last_refresh: None,
            yt_dlp_available: false,
            trailer_playable: std::collections::HashMap::new(),
            request_detail_trailers: Vec::new(),
            display_sync_current_mode: None,
            display_sync_current_hdr: None,
        }
    }

    pub(crate) fn player_config(&self) -> PlayerConfig {
        let c = &self.config.device;
        let cp = self.config.active();
        PlayerConfig {
            audio_device: c.audio_device.clone(),
            audio_device_passthrough: c.audio_device_passthrough.clone(),
            audio_channels: c.audio_channels.clone(),
            audio_spdif_formats: if c.audio_spdif {
                let mut f = Vec::new();
                if c.spdif_ac3 {
                    f.push("ac3");
                }
                if c.spdif_eac3 {
                    f.push("eac3");
                }
                if c.spdif_dts {
                    f.push("dts");
                }
                if c.spdif_dts_hd {
                    f.push("dts-hd");
                }
                if c.spdif_truehd {
                    f.push("truehd");
                }
                f.join(",")
            } else {
                String::new()
            },
            hwdec: c.hwdec.clone(),
            vf: vf_mpv_value(&c.vf),
            video_sync: c.video_sync.clone(),
            opengl_early_flush: c.opengl_early_flush,
            video_latency_hacks: c.video_latency_hacks,
            dither_off: c.video_dither_off,
            interpolation: c.interpolation,
            tscale: c.tscale.clone(),
            tone_mapping: c.tone_mapping.clone(),
            target_colorspace_hint: c.target_colorspace_hint,
            deinterlace: c.deinterlace.clone(),
            cache_secs: c.cache_secs,
            cache_max_mb: c.cache_max_mb,
            start_position_secs: None,
            sub_scale: cp.sub_scale_pct as f64 / 100.0,
            sub_pos: cp.sub_pos_pct as i64,
            sub_respect_ass_styling: cp.sub_respect_ass_styling,
            sub_color: sub_color_hex(&cp.sub_color).to_string(),
            sub_background: cp.sub_background,
            ytdl_format: None, // trailer-only; set by the caller (main.rs) when playing one
        }
    }

    // Update user state (played / is_favorite) in all canonical Rust-side vecs.
    // Call this before patching Slint models so any model rebuild reads correct data.
    pub(crate) fn update_item_user_state(
        &mut self,
        id: &str,
        played: Option<bool>,
        fav: Option<bool>,
    ) {
        let patch = |item: &mut MediaItem| {
            if item.id == id {
                if let Some(p) = played {
                    item.user_data.played = p;
                }
                if let Some(f) = fav {
                    item.user_data.is_favorite = f;
                }
            }
        };
        for list in [
            &mut self.all_movies,
            &mut self.all_series,
            &mut self.all_collections,
            &mut self.all_artists,
            &mut self.all_albums,
            &mut self.all_playlists,
            &mut self.filtered_items,
            &mut self.series_episode_items,
        ] {
            for item in list.iter_mut() {
                patch(item);
            }
        }
        for eps in self.series_episode_cache.values_mut() {
            for item in eps.iter_mut() {
                patch(item);
            }
        }
    }
}

/// Replace-if-present-else-append by id. Used by the WS LibraryChanged/UserDataChanged
/// delta-sync path to merge added/updated items into a cached list without a full re-fetch.
pub(crate) fn upsert_media_item(list: &mut Vec<MediaItem>, item: MediaItem) {
    match list.iter_mut().find(|i| i.id == item.id) {
        Some(existing) => *existing = item,
        None => list.push(item),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn private_files_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("fjord-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("secret.json");
        // A leftover file with the default mode gets tightened too.
        std::fs::write(&p, b"old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        write_private(&p, b"new").unwrap();
        assert_eq!(
            std::fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(std::fs::read(&p).unwrap(), b"new");
        let fresh = dir.join("fresh.json");
        write_private(&fresh, b"x").unwrap();
        assert_eq!(
            std::fs::metadata(&fresh).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cache_names_must_be_jellyfin_ids() {
        assert_eq!(
            safe_cache_name("91aecc28b97a57839f4836ba15b6e04b"),
            Some("91aecc28b97a57839f4836ba15b6e04b")
        );
        assert_eq!(
            safe_cache_name("91AECC28B97A57839F4836BA15B6E04B"),
            Some("91AECC28B97A57839F4836BA15B6E04B")
        );
        for bad in [
            "",
            "../../.bashrc",
            "/home/user/.config/autostart/x.desktop",
            "a/b",
            "91aecc28-b97a-5783-9f48-36ba15b6e04b", // dashed GUID form
            "91aecc28b97a57839f4836ba15b6e04",
            "91aecc28b97a57839f4836ba15b6e04bb",
            "91aecc28b97a57839f4836ba15b6e04g",
            "..\\..\\x",
            "91aecc28b97a57839f4836ba15b6e0/.",
        ] {
            assert_eq!(safe_cache_name(bad), None, "{bad:?}");
            assert!(
                poster_cache_path(bad).is_none() && backdrop_cache_path(bad).is_none(),
                "{bad:?}"
            );
        }
        assert!(
            poster_cache_path("91aecc28b97a57839f4836ba15b6e04b")
                .unwrap()
                .ends_with("posters/91aecc28b97a57839f4836ba15b6e04b")
        );
        // Discover keys: lowercase, digits, dashes only.
        assert!(discover_poster_cache_path("movie-12345").is_some());
        assert!(discover_poster_cache_path("season-missing-1399-2").is_some());
        for bad in ["", "../x", "/abs", "Movie-1", "a b", &"x".repeat(65)] {
            assert!(discover_poster_cache_path(bad).is_none(), "{bad:?}");
        }
    }

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

    // Regression test for the cycle repair in repair_bonfire_profile_corruption
    // (synthetic ids).
    #[test]
    fn repairs_bonfire_master_user_id_cycle() {
        let mut profiles = vec![
            ProfileSettings {
                user_id: "root".into(),
                display_name: "Root".into(),
                is_bonfire: true,
                master_user_id: "sub-a".into(),
                ..Default::default()
            },
            ProfileSettings {
                user_id: "sub-a".into(),
                display_name: "SubA".into(),
                is_bonfire: true,
                master_user_id: "root".into(),
                ..Default::default()
            },
            ProfileSettings {
                user_id: "sub-b".into(),
                display_name: "SubB".into(),
                is_bonfire: true,
                master_user_id: "sub-a".into(),
                ..Default::default()
            },
            ProfileSettings {
                user_id: "other".into(),
                display_name: "Other".into(),
                is_bonfire: false,
                master_user_id: "".into(),
                ..Default::default()
            },
        ];
        assert!(repair_bonfire_profile_corruption(&mut profiles));
        let find = |id: &str| profiles.iter().find(|p| p.user_id == id).unwrap();
        // Both cyclic nodes demoted to standalone plain accounts.
        assert!(!find("root").is_bonfire);
        assert_eq!(find("root").master_user_id, "");
        assert!(!find("sub-a").is_bonfire);
        assert_eq!(find("sub-a").master_user_id, "");
        // A sibling that merely points INTO the cycle is left untouched —
        // it self-heals once the real master's own next login re-derives
        // the whole tree via sync_bonfire_subprofiles.
        assert!(find("sub-b").is_bonfire);
        assert_eq!(find("sub-b").master_user_id, "sub-a");
        // An unrelated plain account is untouched.
        assert!(!find("other").is_bonfire);
    }

    // A legitimate group account (`is_group_account: true`, EMPTY `master_user_id`) must
    // never be mistaken for the self-referencing corruption
    // `repair_bonfire_profile_corruption` fixes — a self-referencing master_user_id
    // would be stripped on the next load.
    #[test]
    fn leaves_group_account_entries_untouched() {
        let mut profiles = vec![
            ProfileSettings {
                user_id: "me".into(),
                display_name: "Me".into(),
                is_bonfire: false,
                ..Default::default()
            },
            ProfileSettings {
                user_id: "friend-master".into(),
                display_name: "Friend".into(),
                is_bonfire: true,
                is_group_account: true,
                master_user_id: String::new(),
                synced_via: "me".into(),
                ..Default::default()
            },
            // A genuine sub-profile of "me", for good measure — confirms the
            // repair pass still treats it normally alongside a group account.
            ProfileSettings {
                user_id: "kid".into(),
                display_name: "Kid".into(),
                is_bonfire: true,
                master_user_id: "me".into(),
                ..Default::default()
            },
        ];
        assert!(!repair_bonfire_profile_corruption(&mut profiles));
        let find = |id: &str| profiles.iter().find(|p| p.user_id == id).unwrap();
        assert!(find("friend-master").is_bonfire);
        assert!(find("friend-master").is_group_account);
        assert_eq!(find("friend-master").master_user_id, "");
        assert!(find("kid").is_bonfire);
        assert_eq!(find("kid").master_user_id, "me");
    }

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
