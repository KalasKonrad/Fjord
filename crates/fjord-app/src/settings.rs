// ── fjord-app · settings.rs ───────────────────────────────────────────────────
//   Design       every section and row has a stable string key ("video",
//                "general.launch_fullscreen"); AppState.settings-section/settings-focused are
//                strings ("" = nothing selected).
//                Which rows exist is decided twice — by settings.slint's `if` conditions and by
//                section_row_keys() here — and the two must be kept in sync by hand; a mismatch
//                only hides or skips that one row (identity doesn't depend on position).
//   Sections     SECTION_PLAYBACK, SECTION_LANGUAGE, SECTION_DISPLAY, SECTION_AUDIO, SECTION_UI
//                ("Interface"), SECTION_PROFILES, SECTION_INTEGRATIONS, SECTION_KEYBINDINGS,
//                SECTION_ADVANCED (expert switches); ALL_SECTIONS is the sidebar order (0.5.1
//                regrouping — row keys keep their old prefixes)
//   Row keys     the const block below, "<section>.<field>", grouped by section (bespoke button
//                rows — Sign Out, Add Account, Connect/Disconnect, Manage Blocklist, Prewarm —
//                use the same shape)
//   SECTION_ROWS / row_shown / section_row_keys  every row per section in on-screen order (a
//                test checks it against settings.slint), the visibility conditions, and the
//                rows on screen now — the source of truth for Up/Down stepping
//                for Up/Down stepping
//   set_focused / set_section / row_focused  focus helpers (row_focused = mouse click path)
//   dispatch_settings  keyboard: section list → rows / key bindings; Enter opens a dropdown
//                popup (Up/Down/Enter/Esc inside it); confirm dialogs checked first
//   Choice / CHOICES / choice(key)  every fixed-choice dropdown: (stored value, label) options +
//                window get/set — drives row labels, the popup, Right-cycling; wire_choice_rows
//                answers settings.slint's `choice: true` rows (setting-options/-label/-choose)
//   is_dynamic_dropdown(key)  the 12 rows whose options come from AppState at runtime: default
//                profile/account (from Config), audio/passthrough device, font, Seerr streaming
//                region/display language/discover language/discover region, display-sync
//                screen/default resolution/default Hz (kscreen-doctor, per selected screen)
//   open_dropdown_popup / apply_dropdown_selection  the keyboard dropdown popup (dynamic lists, or
//                a Choice)
//   settings_row_action(key, g)  Confirm/Right on a row: cycle a Choice forward, flip a toggle,
//                cycle a dynamic dropdown, or run a button
//   wire_device_lists      audio/passthrough device and font lists (fetched once) + their dropdowns
//   wire_profile_defaults  default profile / account dropdowns (local, no network)
//   wire_copy_settings     "Copy settings from…": source picked → confirm → copy_settings_from +
//                          dashboard_rows into the config (no window mirror) +
//                          apply_profile_settings_to_window + settings-changed (save, live-apply, log)
//   wire_regions           Seerr streaming/discover region + display/discover language
//                          (GET-mutate-POST of the user's general settings)
//   wire_settings_changed  settings-changed (persist + live-apply), dropdown mouse pick, row focus
//   DEVICE_BINDS / PROFILE_BINDS (bind!)  every setting mirrored config ↔ window, one line each
//                (load / store, profile ones also copy — "Copy settings from…"); TOGGLES  the on/off
//                rows (Enter/Right flips, only while shown)
//   apply_profile_settings_to_window  PROFILE_BINDS → window + dashboard row orders; font_desc =
//                   the Text font dropdown's label (launch, profile switch, Copy settings from…)
//   apply_settings_to_window ↔ read_settings_from_window  the binds + the special cases
//                (device descriptions, speed floats, audio channels, seerr_enabled)
//   settings_snapshot / settings_diff  the settings-changed handler logs which settings changed
//                (text values by name only)
//   fetch_audio_devices / fetch_system_fonts  startup fetches for the device and font dropdowns
//                (duplicate device descriptions get a backend or name suffix)
// ─────────────────────────────────────────────────────────────────────────────

use crate::MainWindow;
use crate::config::{self, FjordState};
use crate::keys::Action;
use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use tracing::debug;

// ── Sections (sidebar order) ────────────────────────────────────────────────
// Grouped by what you want to change (0.5.1). Row keys keep their old prefixes ("video.hwdec" now
// lives in Advanced): a key is only an identity, nothing derives the section from it.
pub(crate) const SECTION_PLAYBACK: &str = "playback";
pub(crate) const SECTION_LANGUAGE: &str = "language";
pub(crate) const SECTION_DISPLAY: &str = "display";
pub(crate) const SECTION_AUDIO: &str = "audio";
pub(crate) const SECTION_UI: &str = "ui";
pub(crate) const SECTION_PROFILES: &str = "profiles";
pub(crate) const SECTION_INTEGRATIONS: &str = "integrations";
pub(crate) const SECTION_KEYBINDINGS: &str = "keybindings";
pub(crate) const SECTION_ADVANCED: &str = "advanced";

const ALL_SECTIONS: &[&str] = &[
    SECTION_PLAYBACK,
    SECTION_LANGUAGE,
    SECTION_DISPLAY,
    SECTION_AUDIO,
    SECTION_UI,
    SECTION_PROFILES,
    SECTION_INTEGRATIONS,
    SECTION_KEYBINDINGS,
    SECTION_ADVANCED,
];

// ── General section rows ──────────────────────────────────────────────────────
const GEN_LAUNCH_FULLSCREEN: &str = "general.launch_fullscreen";
const GEN_VIDEO_BEHIND: &str = "general.video_behind";
const GEN_LOG_LEVEL: &str = "general.log_level";
const GEN_PREWARM_METADATA: &str = "general.prewarm_metadata";
const GEN_PREWARM_IMAGES: &str = "general.prewarm_images";

// ── Profiles section rows (Phase 0 — shell; Phase 1 step 7 adds the
// launch-policy rows; Phase 2 adds Manage Profiles) ─────────────────────────
const PROF_LAUNCH_POLICY: &str = "profiles.launch_policy";
const PROF_DEFAULT_PROFILE: &str = "profiles.default_profile"; // virtual — only when launch_policy == "default"
// virtual — only when the active profile is itself a master account; a
// Bonfire sub-profile can't manage siblings (bonfire_list_profiles' own
// "all profiles under THIS master account" semantics — see profile_edit.rs).
const PROF_MANAGE_PROFILES: &str = "profiles.manage_profiles";
// Copy another local profile's settings (the "Profile"-tagged rows) — 0.5.1. Only when there is one.
const PROF_COPY_SETTINGS: &str = "profiles.copy_settings";
// Account-tier launch policy: the profile-level shape one tier up, appended after the
// profile rows.
const PROF_ACCOUNT_LAUNCH_POLICY: &str = "profiles.account_launch_policy";
const PROF_DEFAULT_ACCOUNT: &str = "profiles.default_account"; // virtual — only when account_launch_policy == "default"
// Always visible: the picker's "+ Add Account" tile only shows once there are two
// accounts, so this is the way to add the second.
const PROF_ADD_ACCOUNT: &str = "profiles.add_account";
// A toggle with its own handler (profile::on_remember_login_toggle): OFF is immediate, ON
// opens the confirm-password modal (see settings-remember-login in app_state.slint).
const PROF_REMEMBER_LOGIN: &str = "profiles.remember_login";
const PROF_SIGN_OUT: &str = "profiles.sign_out";
// Bonfire Group: same gate as Manage Profiles (settings-is-master-profile — also true while
// impersonating a foreign group account, see profile::is_true_master).
const PROF_BONFIRE_GROUP: &str = "profiles.bonfire_group";
// Gated on jellyfin-is-server-admin, not settings-is-master-profile: the plugin's
// mappings/reset-pin/set-limit/audit-log endpoints authorize against Jellyfin's
// Policy.IsAdministrator, regardless of Bonfire household ownership.
const PROF_BONFIRE_ADMIN: &str = "profiles.bonfire_admin";

// ── Video section rows ────────────────────────────────────────────────────────
const VID_HWDEC: &str = "video.hwdec";
const VID_VF: &str = "video.vf";
const VID_DEINTERLACE: &str = "video.deinterlace";
const VID_VIDEO_SYNC: &str = "video.video_sync";
const VID_INTERPOLATION: &str = "video.interpolation";
const VID_TSCALE: &str = "video.tscale"; // virtual — only when interpolation is on
const VID_TARGET_COLORSPACE: &str = "video.target_colorspace";
const VID_SEPARATE_VIDEO_SURFACE: &str = "video.separate_video_surface";
const VID_OWN_BUFFERS: &str = "video.video_own_buffers";
const VID_DITHER_OFF: &str = "video.video_dither_off"; // test aid, 2026-10-08
const VID_TONE_MAPPING: &str = "video.tone_mapping"; // always visible (2026-08-15 — was
// virtual, hidden while HDR passthrough
// was on; see section_row_keys's own
// comment for why that was wrong)
const VID_OPENGL_EARLY_FLUSH: &str = "video.opengl_early_flush";
const VID_VIDEO_LATENCY_HACKS: &str = "video.video_latency_hacks"; // virtual — only when video-sync == display-resample

// Display sync (resolution/refresh-rate/HDR/WCG matched to the source, see
// display_sync.rs). Every row below the master toggle is virtual — see section_row_keys'
// SECTION_DISPLAY arm.
const VID_DISPLAY_SYNC_ENABLED: &str = "video.display_sync_enabled";
const VID_DISPLAY_SYNC_SCREEN: &str = "video.display_sync_screen"; // dynamic dropdown
const VID_DISPLAY_SYNC_SYNC_RESOLUTION: &str = "video.display_sync_sync_resolution";
const VID_DISPLAY_SYNC_SYNC_REFRESH_RATE: &str = "video.display_sync_sync_refresh_rate";
const VID_DISPLAY_SYNC_DEFAULT_RESOLUTION: &str = "video.display_sync_default_resolution";
const VID_DISPLAY_SYNC_DEFAULT_HZ: &str = "video.display_sync_default_hz";
const VID_DISPLAY_SYNC_SCALE_4K: &str = "video.display_sync_scale_4k";
const VID_DISPLAY_SYNC_SCALE_1080P: &str = "video.display_sync_scale_1080p";
// Only consulted when VID_DISPLAY_SYNC_SYNC_RESOLUTION is true — see
// compute_target_mode's own doc comment for why (structurally inapplicable
// otherwise, not just hidden for tidiness).
const VID_DISPLAY_SYNC_4K_ODD_FPS_MODE: &str = "video.display_sync_4k_odd_fps_mode";
const VID_DISPLAY_SYNC_HDR_MODE: &str = "video.display_sync_hdr_mode";
const VID_DISPLAY_SYNC_WCG_MODE: &str = "video.display_sync_wcg_mode";
const VID_DISPLAY_SYNC_TRAILERS: &str = "video.display_sync_trailers";

// ── Audio section rows ────────────────────────────────────────────────────────
const AUD_AUDIO_DEVICE: &str = "audio.device";
const AUD_CHANNELS: &str = "audio.channels";
const AUD_SPDIF: &str = "audio.spdif";
const AUD_SPDIF_AC3: &str = "audio.spdif_ac3";
const AUD_SPDIF_EAC3: &str = "audio.spdif_eac3";
const AUD_SPDIF_DTS: &str = "audio.spdif_dts";
const AUD_SPDIF_DTS_HD: &str = "audio.spdif_dts_hd";
const AUD_SPDIF_TRUEHD: &str = "audio.spdif_truehd";
const AUD_PASSTHROUGH_DEVICE: &str = "audio.passthrough_device"; // hidden when SPDIF off
const AUD_ALSA_IRQ: &str = "audio.alsa_irq"; // virtual — hidden when SPDIF off or non-PipeWire device
const AUD_SKIP_FADE_MUTE: &str = "audio.skip_fade_mute"; // virtual — hidden when SPDIF off
const AUD_AUDIO_LANG: &str = "audio.lang";
const AUD_GAPLESS: &str = "audio.gapless";
const AUD_NOW_PLAYING_AUTO_OPEN: &str = "audio.now_playing_auto_open_secs"; // per profile; timer.rs
const AUD_SCREENSAVER: &str = "audio.screensaver_mins"; // per profile; screensaver.rs

// ── Player (config) section rows ──────────────────────────────────────────────
const PLY_SUB_ENABLED: &str = "player.sub_enabled";
const PLY_SUB_LANG: &str = "player.sub_lang";
const PLY_SUB_LANG2: &str = "player.sub_lang2";
const PLY_SUB_TYPE: &str = "player.sub_type";
const PLY_SUB_SCALE: &str = "player.sub_scale";
const PLY_SUB_POS: &str = "player.sub_pos";
const PLY_SUB_RESPECT_ASS: &str = "player.sub_respect_ass";
const PLY_SUB_COLOR: &str = "player.sub_color"; // virtual — only when !sub_respect_ass_styling
const PLY_SUB_BACKGROUND: &str = "player.sub_background_pct"; // virtual — only when !sub_respect_ass_styling
const PLY_CACHE_SECS: &str = "player.cache_secs";
const PLY_CACHE_MAX_MB: &str = "player.cache_max_mb";
const PLY_INTRO_MODE: &str = "player.intro_mode";
const PLY_INTRO_SECS: &str = "player.intro_secs"; // virtual — only when intro_mode == "ask-timed"
const PLY_RECAP_MODE: &str = "player.recap_mode";
const PLY_RECAP_SECS: &str = "player.recap_secs"; // virtual
const PLY_PREVIEW_MODE: &str = "player.preview_mode";
const PLY_PREVIEW_SECS: &str = "player.preview_secs"; // virtual
const PLY_COMMERCIAL_MODE: &str = "player.commercial_mode";
const PLY_COMMERCIAL_SECS: &str = "player.commercial_secs"; // virtual
const PLY_CREDITS_MODE: &str = "player.credits_mode";
const PLY_CREDITS_SECS: &str = "player.credits_secs"; // virtual — only when credits_mode == "ask"
const PLY_SEEK_STEP: &str = "player.seek_step";
const PLY_SEEK_STEP_LONG: &str = "player.seek_step_long";
const PLY_SKIP_FADE_MS: &str = "player.skip_fade_ms";
const PLY_CONTROLS_HIDE: &str = "player.controls_hide_secs"; // per profile; timer.rs
const PLY_TRAILER_QUALITY: &str = "player.trailer_quality"; // every YouTube trailer; was in Integrations

// ── UI section rows ───────────────────────────────────────────────────────────
const UI_CARD_SIZE: &str = "ui.card_size"; // per profile, unlike the other UI rows
const UI_TEXT_SIZE: &str = "ui.text_size"; // per profile, like card size
const UI_DASHBOARD_ROWS: &str = "ui.dashboard_rows"; // per profile; opens dashboard_rows.rs's editor
const UI_DASHBOARD_HERO: &str = "ui.dashboard_hero"; // per profile, a dropdown; hero.rs
// Focus highlight, per profile (AppState.focus-*).
const UI_FOCUS_COLOR: &str = "ui.focus_color";
const UI_FOCUS_WIDTH: &str = "ui.focus_width";
const UI_FOCUS_STYLE: &str = "ui.focus_style";
const UI_FOCUS_GLOW: &str = "ui.focus_glow"; // virtual — only when focus_style isn't "line"
const UI_FOCUS_GLOW_ON: &str = "ui.focus_glow_on"; // virtual, like UI_FOCUS_GLOW
const UI_FOCUS_OPACITY: &str = "ui.focus_opacity";
const UI_FOCUS_ZOOM: &str = "ui.focus_zoom";
const UI_SCROLL_SPEED: &str = "ui.scroll_speed";
const UI_ANIMATION_SPEED: &str = "ui.animation_speed";
const UI_FONT_FAMILY: &str = "ui.font_family";
const UI_ONSCREEN_KEYBOARD: &str = "ui.onscreen_keyboard"; // 2026-08-27, default on
const UI_TOAST_SECS: &str = "ui.toast_secs"; // per profile; main.slint's toast timer

// ── Integrations section rows ─────────────────────────────────────────────────
const INT_SEERR_ENABLED: &str = "integrations.seerr_enabled";
const INT_SEERR_CONNECT: &str = "integrations.seerr_connect"; // "Connect Seerr" / "Disconnect"
const INT_STREAMING_REGION: &str = "integrations.streaming_region";
const INT_DISPLAY_LANGUAGE: &str = "integrations.display_language";
const INT_DISCOVER_LANGUAGE: &str = "integrations.discover_language";
const INT_DISCOVER_REGION: &str = "integrations.discover_region";
const INT_MANAGE_BLOCKLIST: &str = "integrations.manage_blocklist";

// ── Row existence per section (replaces the old duplicated skip-logic) ────────

/// Every Settings row, per section, in on-screen order (Up/Down follow it); `row_shown` hides the
/// conditional ones. settings.slint draws the same rows in the same order with the same
/// conditions — the test `section_rows_match_settings_slint` checks rows and order.
const SECTION_ROWS: &[(&str, &[&str])] = &[
    (
        SECTION_PLAYBACK,
        &[
            PLY_INTRO_MODE,
            PLY_INTRO_SECS,
            PLY_RECAP_MODE,
            PLY_RECAP_SECS,
            PLY_PREVIEW_MODE,
            PLY_PREVIEW_SECS,
            PLY_COMMERCIAL_MODE,
            PLY_COMMERCIAL_SECS,
            PLY_CREDITS_MODE,
            PLY_CREDITS_SECS,
            PLY_SKIP_FADE_MS,
            PLY_SEEK_STEP,
            PLY_SEEK_STEP_LONG,
            PLY_CONTROLS_HIDE,
            PLY_TRAILER_QUALITY,
        ],
    ),
    (
        SECTION_LANGUAGE,
        &[
            AUD_AUDIO_LANG,
            PLY_SUB_ENABLED,
            PLY_SUB_LANG,
            PLY_SUB_LANG2,
            PLY_SUB_TYPE,
            PLY_SUB_SCALE,
            PLY_SUB_POS,
            PLY_SUB_RESPECT_ASS,
            PLY_SUB_COLOR,
            PLY_SUB_BACKGROUND,
        ],
    ),
    (
        SECTION_DISPLAY,
        &[
            GEN_LAUNCH_FULLSCREEN,
            VID_TARGET_COLORSPACE,
            VID_SEPARATE_VIDEO_SURFACE,
            VID_TONE_MAPPING,
            VID_DISPLAY_SYNC_ENABLED,
            VID_DISPLAY_SYNC_SCREEN,
            VID_DISPLAY_SYNC_SYNC_RESOLUTION,
            VID_DISPLAY_SYNC_SYNC_REFRESH_RATE,
            VID_DISPLAY_SYNC_DEFAULT_RESOLUTION,
            VID_DISPLAY_SYNC_DEFAULT_HZ,
            VID_DISPLAY_SYNC_SCALE_4K,
            VID_DISPLAY_SYNC_SCALE_1080P,
            VID_DISPLAY_SYNC_4K_ODD_FPS_MODE,
            VID_DISPLAY_SYNC_HDR_MODE,
            VID_DISPLAY_SYNC_WCG_MODE,
            VID_DISPLAY_SYNC_TRAILERS,
        ],
    ),
    (
        SECTION_AUDIO,
        &[
            AUD_AUDIO_DEVICE,
            AUD_CHANNELS,
            AUD_SPDIF,
            AUD_SPDIF_AC3,
            AUD_SPDIF_EAC3,
            AUD_SPDIF_DTS,
            AUD_SPDIF_DTS_HD,
            AUD_SPDIF_TRUEHD,
            AUD_PASSTHROUGH_DEVICE,
            AUD_ALSA_IRQ,
            AUD_SKIP_FADE_MUTE,
            AUD_GAPLESS,
            AUD_NOW_PLAYING_AUTO_OPEN,
            AUD_SCREENSAVER,
        ],
    ),
    (
        SECTION_UI,
        &[
            UI_CARD_SIZE,
            UI_TEXT_SIZE,
            UI_DASHBOARD_HERO,
            UI_DASHBOARD_ROWS,
            UI_FOCUS_COLOR,
            UI_FOCUS_WIDTH,
            UI_FOCUS_STYLE,
            UI_FOCUS_GLOW,
            UI_FOCUS_GLOW_ON,
            UI_FOCUS_OPACITY,
            UI_FOCUS_ZOOM,
            GEN_VIDEO_BEHIND,
            UI_FONT_FAMILY,
            UI_ONSCREEN_KEYBOARD,
            UI_TOAST_SECS,
            UI_SCROLL_SPEED,
            UI_ANIMATION_SPEED,
        ],
    ),
    (
        SECTION_PROFILES,
        &[
            PROF_LAUNCH_POLICY,
            PROF_DEFAULT_PROFILE,
            PROF_MANAGE_PROFILES,
            PROF_COPY_SETTINGS,
            PROF_ACCOUNT_LAUNCH_POLICY,
            PROF_DEFAULT_ACCOUNT,
            PROF_ADD_ACCOUNT,
            PROF_REMEMBER_LOGIN,
            PROF_SIGN_OUT,
            PROF_BONFIRE_GROUP,
            PROF_BONFIRE_ADMIN,
        ],
    ),
    (
        SECTION_INTEGRATIONS,
        &[
            INT_SEERR_ENABLED,
            INT_SEERR_CONNECT,
            INT_STREAMING_REGION,
            INT_DISPLAY_LANGUAGE,
            INT_DISCOVER_LANGUAGE,
            INT_DISCOVER_REGION,
            INT_MANAGE_BLOCKLIST,
        ],
    ),
    (
        SECTION_ADVANCED,
        &[
            VID_HWDEC,
            VID_VF,
            VID_DEINTERLACE,
            VID_VIDEO_SYNC,
            VID_INTERPOLATION,
            VID_TSCALE,
            VID_OPENGL_EARLY_FLUSH,
            VID_VIDEO_LATENCY_HACKS,
            VID_OWN_BUFFERS,
            VID_DITHER_OFF,
            PLY_CACHE_SECS,
            PLY_CACHE_MAX_MB,
            GEN_PREWARM_METADATA,
            GEN_PREWARM_IMAGES,
            GEN_LOG_LEVEL,
        ],
    ),
];

/// Whether a row is on screen right now — the same conditions as settings.slint's `if`s.
fn row_shown(key: &str, g: &crate::AppState<'_>) -> bool {
    let subs = g.get_settings_sub_enabled();
    let sync = g.get_settings_display_sync_enabled();
    let spdif = g.get_settings_audio_spdif();
    let seerr = g.get_settings_seerr_enabled();
    let seerr_connected = seerr && g.get_seerr_connected();
    match key {
        PLY_INTRO_SECS => g.get_settings_skip_intro_mode().as_str() == "ask-timed",
        PLY_RECAP_SECS => g.get_settings_skip_recap_mode().as_str() == "ask-timed",
        PLY_PREVIEW_SECS => g.get_settings_skip_preview_mode().as_str() == "ask-timed",
        PLY_COMMERCIAL_SECS => g.get_settings_skip_commercial_mode().as_str() == "ask-timed",
        PLY_CREDITS_SECS => g.get_settings_skip_credits_mode().as_str() == "ask",
        PLY_SUB_LANG | PLY_SUB_LANG2 | PLY_SUB_TYPE | PLY_SUB_SCALE | PLY_SUB_POS
        | PLY_SUB_RESPECT_ASS => subs,
        PLY_SUB_COLOR | PLY_SUB_BACKGROUND => subs && !g.get_settings_sub_respect_ass_styling(),
        // HDR Stage 5: only on Wayland, where it does anything.
        VID_SEPARATE_VIDEO_SURFACE => g.get_is_wayland(),
        UI_FOCUS_GLOW | UI_FOCUS_GLOW_ON => g.get_settings_focus_style().as_str() != "line",
        VID_DISPLAY_SYNC_SCREEN
        | VID_DISPLAY_SYNC_SYNC_RESOLUTION
        | VID_DISPLAY_SYNC_SYNC_REFRESH_RATE
        | VID_DISPLAY_SYNC_DEFAULT_RESOLUTION
        | VID_DISPLAY_SYNC_DEFAULT_HZ
        | VID_DISPLAY_SYNC_SCALE_4K
        | VID_DISPLAY_SYNC_SCALE_1080P
        | VID_DISPLAY_SYNC_HDR_MODE
        | VID_DISPLAY_SYNC_WCG_MODE
        | VID_DISPLAY_SYNC_TRAILERS => sync,
        VID_DISPLAY_SYNC_4K_ODD_FPS_MODE => sync && g.get_settings_display_sync_sync_resolution(),
        AUD_SPDIF_AC3
        | AUD_SPDIF_EAC3
        | AUD_SPDIF_DTS
        | AUD_SPDIF_DTS_HD
        | AUD_SPDIF_TRUEHD
        | AUD_PASSTHROUGH_DEVICE
        | AUD_SKIP_FADE_MUTE => spdif,
        AUD_ALSA_IRQ => spdif && g.get_settings_device_is_pipewire(),
        PROF_DEFAULT_PROFILE => g.get_settings_launch_policy().as_str() == "default",
        PROF_MANAGE_PROFILES | PROF_BONFIRE_GROUP => g.get_settings_is_master_profile(),
        PROF_COPY_SETTINGS => g.get_settings_copy_sources_display().row_count() > 0,
        PROF_DEFAULT_ACCOUNT => g.get_settings_account_launch_policy().as_str() == "default",
        PROF_BONFIRE_ADMIN => g.get_jellyfin_is_server_admin(),
        INT_SEERR_CONNECT => seerr,
        INT_STREAMING_REGION
        | INT_DISPLAY_LANGUAGE
        | INT_DISCOVER_LANGUAGE
        | INT_DISCOVER_REGION => seerr_connected,
        INT_MANAGE_BLOCKLIST => seerr_connected && g.get_seerr_can_manage_blocklist(),
        // vf only works around NVDEC's stride corruption (see CLAUDE.md): only with NVDEC chosen
        // explicitly — "auto" counts as not NVDEC.
        VID_VF => matches!(g.get_settings_hwdec().as_str(), "nvdec" | "nvdec-copy"),
        VID_TSCALE => g.get_settings_interpolation(),
        VID_VIDEO_LATENCY_HACKS => g.get_settings_video_sync().as_str() == "display-resample",
        // 10-bit video plane: only with the separate video surface (Wayland).
        VID_OWN_BUFFERS => g.get_is_wayland() && g.get_settings_separate_video_surface(),
        _ => true,
    }
}

fn section_row_keys(section: &str, g: &crate::AppState<'_>) -> Vec<&'static str> {
    SECTION_ROWS
        .iter()
        .find(|(s, _)| *s == section)
        .map(|(_, keys)| keys.iter().copied().filter(|k| row_shown(k, g)).collect())
        .unwrap_or_default()
}

// ── Main dispatch ─────────────────────────────────────────────────────────────

// Sets settings-focused AND its companion approximate-scroll-position index
// (settings.slint's kb-y) together, so the two can never drift apart —
// every keyboard-driven focus change in this file goes through this, and
// main.rs's on_settings_row_focused (the mouse-click path) calls the same
// pair via section_row_keys + this same idea.
fn set_focused(g: &crate::AppState<'_>, key: &str, visual_index: i32) {
    debug!("settings: focused -> {key} (visual_index={visual_index})");
    g.set_settings_focused(key.into());
    g.set_settings_focused_visual_index(visual_index);
}

fn set_section(g: &crate::AppState<'_>, section: &str) {
    debug!("settings: section -> {section}");
    g.set_settings_section(section.into());
}

// Mouse click on a SettingsRow (widgets.slint's AppState.settings-row-focused
// callback, wired in main.rs) — same set_focused pairing as the keyboard
// path above, just resolving the visual index from the row's key instead of
// stepping from a known current position.
pub(crate) fn row_focused(g: &crate::AppState<'_>, key: &str) {
    // A keyboard-opened dropdown has no backdrop, so rows behind it stay clickable; close it on
    // a row click, or the next Confirm would apply the old popup's cursor to the new row.
    if g.get_settings_dropdown_open() {
        g.set_settings_dropdown_open(false);
    }
    let rows = section_row_keys(g.get_settings_section().as_str(), g);
    let idx = rows.iter().position(|&k| k == key).unwrap_or(0) as i32;
    debug!(
        "settings: mouse click on {key} (resolved index={idx} of {} visible rows)",
        rows.len()
    );
    set_focused(g, key, idx);
}

pub(crate) fn dispatch_settings(action: &Action, g: &crate::AppState<'_>) -> Option<bool> {
    // "Copy settings from…" confirmation: like Disconnect Seerr below — only reachable from
    // Settings, swallows all Settings input while open.
    if g.get_show_copy_settings_confirm() {
        match action {
            Action::Left => g.set_copy_settings_confirm_focused(0),
            Action::Right => g.set_copy_settings_confirm_focused(1),
            Action::Confirm => {
                if g.get_copy_settings_confirm_focused() == 1 {
                    g.invoke_copy_settings_confirmed();
                }
                g.set_show_copy_settings_confirm(false);
            }
            Action::Back => g.set_show_copy_settings_confirm(false),
            _ => {}
        }
        return Some(true);
    }
    // Disconnect Seerr confirmation (see show-seerr-disconnect-confirm in app_state.slint):
    // checked first, swallows all Settings input while open. Settings is its only trigger, so
    // it needs no global dialog or keys.rs tier (unlike Sign Out / Cancel Request).
    if g.get_show_seerr_disconnect_confirm() {
        match action {
            Action::Left => g.set_seerr_disconnect_confirm_focused(0),
            Action::Right => g.set_seerr_disconnect_confirm_focused(1),
            Action::Confirm => {
                if g.get_seerr_disconnect_confirm_focused() == 1 {
                    g.invoke_seerr_disconnect();
                }
                g.set_show_seerr_disconnect_confirm(false);
            }
            Action::Back => g.set_show_seerr_disconnect_confirm(false),
            _ => {}
        }
        return Some(true);
    }

    let sf = g.get_settings_focused();
    let ss = g.get_settings_section();
    debug!(
        "settings: dispatch action={action:?} section={:?} focused={:?}",
        ss.as_str(),
        sf.as_str()
    );

    // ── Dropdown popup open: intercept all input for in-popup navigation ──────
    if g.get_settings_dropdown_open() {
        if ss.as_str().is_empty() || sf.as_str().is_empty() {
            g.set_settings_dropdown_open(false);
            return None;
        }
        let model_len = g.get_settings_dropdown_model().row_count() as i32;
        let cursor = g.get_settings_dropdown_cursor();
        match action {
            Action::Down => {
                g.set_settings_dropdown_cursor((cursor + 1).min(model_len - 1));
            }
            Action::Up => {
                g.set_settings_dropdown_cursor((cursor - 1).max(0));
            }
            Action::Confirm => {
                if cursor >= 0 && cursor < model_len {
                    apply_dropdown_selection(sf.as_str(), cursor, g);
                }
                g.set_settings_dropdown_open(false);
            }
            Action::Back | Action::Left => {
                g.set_settings_dropdown_open(false);
            }
            _ => {}
        }
        return Some(true);
    }

    if !sf.as_str().is_empty() {
        // ── Right pane: row navigation ────────────────────────────────────
        let rows = section_row_keys(ss.as_str(), g);
        let idx = rows.iter().position(|&k| k == sf.as_str());
        if matches!(action, Action::Up | Action::Down) {
            debug!(
                "settings: {ss} has {} visible row(s), current idx={idx:?}, rows={rows:?}",
                rows.len()
            );
        }
        match action {
            Action::Down => {
                match idx {
                    Some(i) if i + 1 < rows.len() => set_focused(g, rows[i + 1], (i + 1) as i32),
                    None => {
                        if let Some(&first) = rows.first() {
                            set_focused(g, first, 0);
                        }
                    }
                    _ => {}
                }
                Some(true)
            }
            Action::Up => {
                match idx {
                    Some(0) => g.set_settings_focused("".into()),
                    Some(i) => set_focused(g, rows[i - 1], (i - 1) as i32),
                    None => {
                        if let Some(&first) = rows.first() {
                            set_focused(g, first, 0);
                        }
                    }
                }
                Some(true)
            }
            Action::Back | Action::Left => {
                g.set_settings_focused("".into());
                Some(true)
            }
            Action::Confirm => {
                // Like Up/Down, self-heal when `sf` points at a row that's no longer shown (a mouse
                // toggle can hide rows) instead of acting on an invisible row.
                let Some(_) = idx else {
                    if let Some(&first) = rows.first() {
                        set_focused(g, first, 0);
                    }
                    return Some(true);
                };
                // Dropdown rows: show overlay with cursor on current value.
                // Toggle/button/action rows: activate directly.
                if is_dropdown_row(sf.as_str()) {
                    open_dropdown_popup(sf.as_str(), g);
                } else {
                    settings_row_action(sf.as_str(), g);
                }
                Some(true)
            }
            Action::Right => {
                let Some(_) = idx else {
                    if let Some(&first) = rows.first() {
                        set_focused(g, first, 0);
                    }
                    return Some(true);
                };
                settings_row_action(sf.as_str(), g);
                Some(true)
            }
            _ => None,
        }
    } else if !ss.as_str().is_empty() {
        // ── Left pane: section list navigation ───────────────────────────
        let idx = ALL_SECTIONS
            .iter()
            .position(|&s| s == ss.as_str())
            .unwrap_or(0);
        match action {
            Action::Down => {
                if idx + 1 < ALL_SECTIONS.len() {
                    set_section(g, ALL_SECTIONS[idx + 1]);
                }
                Some(true)
            }
            Action::Up => {
                if idx > 0 {
                    set_section(g, ALL_SECTIONS[idx - 1]);
                }
                Some(true)
            }
            Action::Right | Action::Confirm => {
                if ss.as_str() == SECTION_KEYBINDINGS {
                    g.set_keybinding_focused(0);
                } else if let Some(&first) = section_row_keys(ss.as_str(), g).first() {
                    set_focused(g, first, 0);
                }
                Some(true)
            }
            Action::Back | Action::Left => {
                debug!("settings: exit to sidebar");
                g.set_settings_section("".into());
                Some(true)
            }
            _ => None,
        }
    } else {
        // ── Sidebar (ss == ""): Right/Enter enters left pane ─────────────
        match action {
            Action::Right | Action::Confirm => {
                set_section(g, SECTION_PLAYBACK);
                Some(true)
            }
            _ => None,
        }
    }
}

// ── Fixed-choice settings (CHOICES) ───────────────────────────────────────────
// Every Settings dropdown with a fixed option list is one Choice: its options as (stored value,
// label) and how its window property is read and written. Everything else derives from it — the
// label shown in the row (settings.slint `choice: true` → setting-label), the keyboard list
// (open_dropdown_popup / apply_dropdown_selection), Right-cycling (settings_row_action). Adding
// one: a key const, an entry here, a SettingsRow { choice: true; value: … } row, the config
// field. Stored-value notes: "" = Default / Any (languages, subtitle type and color — sub_color
// and trailer_quality store display names, translated at the point of use); cache "0" =
// unlimited (mpv.rs raises the byte ceiling); skip fade "0" = an instant cut; speed % is a
// duration multiplier (bigger = slower). Dynamic lists (devices, fonts, regions…) are separate:
// is_dynamic_dropdown.

/// How a fixed-choice setting is held in the window: a string or an int property.
enum Field {
    Str(
        fn(&crate::AppState<'_>) -> SharedString,
        fn(&crate::AppState<'_>, SharedString),
    ),
    Int(
        fn(&crate::AppState<'_>) -> i32,
        fn(&crate::AppState<'_>, i32),
    ),
}

struct Choice {
    key: &'static str,
    options: &'static [(&'static str, &'static str)],
    field: Field,
}

impl Choice {
    fn value(&self, g: &crate::AppState<'_>) -> String {
        match self.field {
            Field::Str(get, _) => get(g).to_string(),
            Field::Int(get, _) => get(g).to_string(),
        }
    }
    fn index_of(&self, value: &str) -> Option<usize> {
        self.options.iter().position(|(v, _)| *v == value)
    }
    /// The label for a stored value (the value itself when it isn't one of the options).
    fn label(&self, value: &str) -> String {
        self.index_of(value)
            .map(|i| self.options[i].1.to_string())
            .unwrap_or_else(|| value.to_string())
    }
    /// Store option `i` and save (settings-changed).
    fn set(&self, g: &crate::AppState<'_>, i: usize) {
        let Some((value, _)) = self.options.get(i) else {
            return;
        };
        match self.field {
            Field::Str(_, set) => set(g, (*value).into()),
            Field::Int(_, set) => match value.parse() {
                Ok(n) => set(g, n),
                Err(_) => return,
            },
        }
        g.invoke_settings_changed();
    }
}

fn choice(key: &str) -> Option<&'static Choice> {
    CHOICES.iter().find(|c| c.key == key)
}

const SKIP_MODE_4_OPTIONS: &[(&str, &str)] = &[
    ("always-skip", "Always skip"),
    ("ask", "Ask"),
    ("ask-timed", "Ask (timed)"),
    ("never-skip", "Never skip"),
];
const SKIP_SECS_OPTIONS: &[(&str, &str)] = &[
    ("3", "3"),
    ("5", "5"),
    ("8", "8"),
    ("10", "10"),
    ("15", "15"),
    ("20", "20"),
    ("30", "30"),
];
const SKIP_MODE_3_OPTIONS: &[(&str, &str)] = &[
    ("always-skip", "Always skip"),
    ("ask", "Ask"),
    ("never-skip", "Never skip"),
];
const NOW_PLAYING_SECS_OPTIONS: &[(&str, &str)] = &[
    ("0", "Off"),
    ("15", "15 s"),
    ("30", "30 s"),
    ("60", "1 min"),
    ("120", "2 min"),
    ("300", "5 min"),
];
const SUB_BACKGROUND_OPTIONS: &[(&str, &str)] = &[
    ("0", "Off"),
    ("25", "Light"),
    ("50", "Medium"),
    ("75", "Dark"),
    ("100", "Solid"),
];
const CONTROLS_HIDE_SECS_OPTIONS: &[(&str, &str)] =
    &[("2", "2 s"), ("3", "3 s"), ("5", "5 s"), ("10", "10 s")];
const TOAST_SECS_OPTIONS: &[(&str, &str)] =
    &[("3", "3 s"), ("4", "4 s"), ("6", "6 s"), ("10", "10 s")];
const SCREENSAVER_MINS_OPTIONS: &[(&str, &str)] = &[
    ("0", "Off"),
    ("1", "1 min"),
    ("2", "2 min"),
    ("5", "5 min"),
    ("10", "10 min"),
    ("15", "15 min"),
    ("30", "30 min"),
];
const CREDITS_SECS_OPTIONS: &[(&str, &str)] = &[
    ("10", "10"),
    ("15", "15"),
    ("20", "20"),
    ("30", "30"),
    ("45", "45"),
    ("60", "60"),
];
const SKIP_FADE_MS_OPTIONS: &[(&str, &str)] = &[
    ("0", "Off (instant)"),
    ("100", "100"),
    ("150", "150"),
    ("200", "200"),
    ("300", "300"),
    ("400", "400"),
    ("500", "500"),
    ("750", "750"),
    ("1000", "1000"),
];
const SEEK_STEP_OPTIONS: &[(&str, &str)] = &[
    ("5", "5"),
    ("10", "10"),
    ("15", "15"),
    ("20", "20"),
    ("30", "30"),
];
const SEEK_STEP_LONG_OPTIONS: &[(&str, &str)] = &[
    ("15", "15"),
    ("30", "30"),
    ("45", "45"),
    ("60", "60"),
    ("120", "120"),
];
const TRAILER_QUALITY_OPTIONS: &[(&str, &str)] = &[
    ("Best", "Best"),
    ("1080p", "1080p"),
    ("720p", "720p"),
    ("480p", "480p"),
];
const LANG_OPTIONS: &[(&str, &str)] = &[
    ("", "Default"),
    ("English", "English"),
    ("German", "German"),
    ("French", "French"),
    ("Japanese", "Japanese"),
    ("Spanish", "Spanish"),
    ("Italian", "Italian"),
    ("Portuguese", "Portuguese"),
    ("Russian", "Russian"),
    ("Korean", "Korean"),
    ("Chinese", "Chinese"),
    ("Dutch", "Dutch"),
    ("Swedish", "Swedish"),
    ("Polish", "Polish"),
    ("Czech", "Czech"),
    ("Arabic", "Arabic"),
    ("Turkish", "Turkish"),
    ("Finnish", "Finnish"),
    ("Danish", "Danish"),
    ("Norwegian", "Norwegian"),
];
const SUB_TYPE_OPTIONS: &[(&str, &str)] = &[
    ("", "Any"),
    ("Normal", "Normal"),
    ("Forced", "Forced"),
    ("Hearing Impaired", "Hearing Impaired"),
];
const SUB_SCALE_OPTIONS: &[(&str, &str)] = &[
    ("50", "50"),
    ("75", "75"),
    ("100", "100"),
    ("125", "125"),
    ("150", "150"),
    ("175", "175"),
    ("200", "200"),
];
const SUB_POS_OPTIONS: &[(&str, &str)] = &[
    ("50", "50"),
    ("60", "60"),
    ("70", "70"),
    ("80", "80"),
    ("90", "90"),
    ("95", "95"),
    ("100", "100"),
    ("110", "110"),
    ("120", "120"),
    ("130", "130"),
    ("140", "140"),
    ("150", "150"),
];
const SUB_COLOR_OPTIONS: &[(&str, &str)] = &[
    ("", "Default"),
    ("White", "White"),
    ("Yellow", "Yellow"),
    ("Cyan", "Cyan"),
    ("Green", "Green"),
];
const TONE_MAPPING_OPTIONS: &[(&str, &str)] = &[
    ("auto", "auto"),
    ("hable", "hable"),
    ("bt.2390", "bt.2390"),
    ("reinhard", "reinhard"),
    ("mobius", "mobius"),
    ("clip", "clip"),
    ("gamma", "gamma"),
    ("linear", "linear"),
];
const DISPLAY_SYNC_SCALE_OPTIONS: &[(&str, &str)] = &[
    ("1.0", "1.0"),
    ("1.25", "1.25"),
    ("1.5", "1.5"),
    ("1.75", "1.75"),
    ("2.0", "2.0"),
];
const DISPLAY_SYNC_4K_ODD_FPS_OPTIONS: &[(&str, &str)] = &[
    ("fallback", "Fallback to default resolution"),
    ("stay_4k", "Stay at 4K"),
];
const DISPLAY_SYNC_HDR_MODE_OPTIONS: &[(&str, &str)] = &[
    ("yes", "Match source"),
    ("no", "Never"),
    ("always", "Always"),
];
const DISPLAY_SYNC_WCG_MODE_OPTIONS: &[(&str, &str)] =
    &[("auto", "Follow HDR"), ("yes", "Always"), ("no", "Never")];
const AUDIO_CHANNELS_OPTIONS: &[(&str, &str)] = &[
    ("auto-safe", "auto-safe"),
    ("auto", "auto"),
    ("stereo", "stereo"),
    ("5.1", "5.1"),
    ("7.1", "7.1"),
    ("7.1,5.1,stereo", "7.1,5.1,stereo"),
];
const CARD_SIZE_OPTIONS: &[(&str, &str)] = &[
    ("small", "Small"),
    ("normal", "Normal"),
    ("large", "Large"),
    ("extra-large", "Extra large"),
    ("huge", "Huge"),
];
const HERO_STYLE_OPTIONS: &[(&str, &str)] = &[
    ("off", "Off"),
    ("band", "Band at the top"),
    ("background", "Background"),
];
const FOCUS_COLOR_OPTIONS: &[(&str, &str)] = &[
    ("white", "White"),
    ("blue", "Accent blue"),
    ("gold", "Gold"),
    ("green", "Green"),
    ("red", "Red"),
];
const FOCUS_WIDTH_OPTIONS: &[(&str, &str)] = &[
    ("2", "Thin (2px)"),
    ("3", "Medium (3px)"),
    ("4", "Thick (4px)"),
    ("6", "Extra thick (6px)"),
];
const FOCUS_STYLE_OPTIONS: &[(&str, &str)] = &[
    ("line", "Line"),
    ("glow", "Glow"),
    ("both", "Line and glow"),
];
const FOCUS_GLOW_OPTIONS: &[(&str, &str)] = &[
    ("faint", "Faint"),
    ("very-soft", "Very soft"),
    ("subtle", "Subtle"),
    ("soft", "Soft"),
    ("medium", "Medium"),
    ("strong", "Strong"),
    ("very-strong", "Very strong"),
];
const FOCUS_GLOW_ON_OPTIONS: &[(&str, &str)] = &[
    ("pictures", "Pictures and buttons"),
    ("sidebar", "Pictures, buttons and sidebar"),
    ("everything", "Everything"),
];
const FOCUS_OPACITY_OPTIONS: &[(&str, &str)] =
    &[("solid", "Solid"), ("see-through", "See-through")];
const FOCUS_ZOOM_OPTIONS: &[(&str, &str)] = &[
    ("0", "Off"),
    ("5", "5%"),
    ("10", "10%"),
    ("15", "15%"),
    ("20", "20%"),
    ("25", "25%"),
    ("30", "30%"),
];
const SPEED_PCT_OPTIONS: &[(&str, &str)] = &[
    ("0", "0"),
    ("25", "25"),
    ("50", "50"),
    ("75", "75"),
    ("100", "100"),
    ("150", "150"),
    ("200", "200"),
    ("300", "300"),
    ("400", "400"),
    ("500", "500"),
];
const LAUNCH_POLICY_OPTIONS: &[(&str, &str)] = &[
    ("always_ask", "Always ask"),
    ("remember_last", "Remember last"),
    ("default", "Default profile"),
];
const ACCOUNT_LAUNCH_POLICY_OPTIONS: &[(&str, &str)] = &[
    ("always_ask", "Always ask"),
    ("remember_last", "Remember last"),
    ("default", "Default account"),
];
const HWDEC_OPTIONS: &[(&str, &str)] = &[
    ("auto", "auto"),
    ("vulkan", "vulkan"),
    ("vulkan-copy", "vulkan-copy"),
    ("nvdec", "nvdec"),
    ("nvdec-copy", "nvdec-copy"),
    ("vaapi", "vaapi"),
    ("vaapi-copy", "vaapi-copy"),
    ("vdpau", "vdpau"),
    ("vdpau-copy", "vdpau-copy"),
    ("none", "none"),
];
const VF_OPTIONS: &[(&str, &str)] = &[
    ("auto: nv12/p010", "auto: nv12/p010"),
    ("auto: yuv420p/yuv420p10le", "auto: yuv420p/yuv420p10le"),
    ("format=yuv420p", "format=yuv420p"),
    ("format=yuv420p10le", "format=yuv420p10le"),
    ("format=nv12", "format=nv12"),
    ("format=p010", "format=p010"),
];
const DEINTERLACE_OPTIONS: &[(&str, &str)] = &[("no", "no"), ("auto", "auto"), ("yes", "yes")];
const VIDEO_SYNC_OPTIONS: &[(&str, &str)] = &[
    ("audio", "audio"),
    ("display-resample", "display-resample"),
    ("display-vdrop", "display-vdrop"),
    ("display-adrop", "display-adrop"),
    ("desync", "desync"),
];
const TSCALE_OPTIONS: &[(&str, &str)] = &[
    ("oversample", "oversample"),
    ("catmull_rom", "catmull_rom"),
    ("mitchell", "mitchell"),
    ("gaussian", "gaussian"),
    ("bicubic", "bicubic"),
];
const CACHE_SECS_OPTIONS: &[(&str, &str)] = &[
    ("0", "Unlimited"),
    ("10", "10"),
    ("30", "30"),
    ("60", "60"),
    ("120", "120"),
    ("300", "300"),
];
const CACHE_MAX_MB_OPTIONS: &[(&str, &str)] = &[
    ("0", "Unlimited"),
    ("150", "150"),
    ("300", "300"),
    ("500", "500"),
    ("1000", "1000"),
    ("2000", "2000"),
];
const LOG_LEVEL_OPTIONS: &[(&str, &str)] = &[
    ("error", "error"),
    ("warn", "warn"),
    ("info", "info"),
    ("debug", "debug"),
];

static CHOICES: &[Choice] = &[
    Choice {
        key: PLY_INTRO_MODE,
        options: SKIP_MODE_4_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_skip_intro_mode(),
            |g, v| g.set_settings_skip_intro_mode(v),
        ),
    },
    Choice {
        key: PLY_INTRO_SECS,
        options: SKIP_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_skip_intro_secs(),
            |g, v| g.set_settings_skip_intro_secs(v),
        ),
    },
    Choice {
        key: PLY_RECAP_MODE,
        options: SKIP_MODE_4_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_skip_recap_mode(),
            |g, v| g.set_settings_skip_recap_mode(v),
        ),
    },
    Choice {
        key: PLY_RECAP_SECS,
        options: SKIP_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_skip_recap_secs(),
            |g, v| g.set_settings_skip_recap_secs(v),
        ),
    },
    Choice {
        key: PLY_PREVIEW_MODE,
        options: SKIP_MODE_4_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_skip_preview_mode(),
            |g, v| g.set_settings_skip_preview_mode(v),
        ),
    },
    Choice {
        key: PLY_PREVIEW_SECS,
        options: SKIP_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_skip_preview_secs(),
            |g, v| g.set_settings_skip_preview_secs(v),
        ),
    },
    Choice {
        key: PLY_COMMERCIAL_MODE,
        options: SKIP_MODE_4_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_skip_commercial_mode(),
            |g, v| g.set_settings_skip_commercial_mode(v),
        ),
    },
    Choice {
        key: PLY_COMMERCIAL_SECS,
        options: SKIP_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_skip_commercial_secs(),
            |g, v| g.set_settings_skip_commercial_secs(v),
        ),
    },
    Choice {
        key: PLY_CREDITS_MODE,
        options: SKIP_MODE_3_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_skip_credits_mode(),
            |g, v| g.set_settings_skip_credits_mode(v),
        ),
    },
    Choice {
        key: AUD_NOW_PLAYING_AUTO_OPEN,
        options: NOW_PLAYING_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_now_playing_auto_open_secs(),
            |g, v| g.set_settings_now_playing_auto_open_secs(v),
        ),
    },
    Choice {
        key: PLY_SUB_BACKGROUND,
        options: SUB_BACKGROUND_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_sub_background_pct(),
            |g, v| g.set_settings_sub_background_pct(v),
        ),
    },
    Choice {
        key: PLY_CONTROLS_HIDE,
        options: CONTROLS_HIDE_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_controls_hide_secs(),
            |g, v| g.set_settings_controls_hide_secs(v),
        ),
    },
    Choice {
        key: UI_TOAST_SECS,
        options: TOAST_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_toast_secs(),
            |g, v| g.set_settings_toast_secs(v),
        ),
    },
    Choice {
        key: AUD_SCREENSAVER,
        options: SCREENSAVER_MINS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_screensaver_mins(),
            |g, v| g.set_settings_screensaver_mins(v),
        ),
    },
    Choice {
        key: PLY_CREDITS_SECS,
        options: CREDITS_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_skip_credits_secs(),
            |g, v| g.set_settings_skip_credits_secs(v),
        ),
    },
    Choice {
        key: PLY_SKIP_FADE_MS,
        options: SKIP_FADE_MS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_skip_fade_ms(),
            |g, v| g.set_settings_skip_fade_ms(v),
        ),
    },
    Choice {
        key: PLY_SEEK_STEP,
        options: SEEK_STEP_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_seek_step_secs(),
            |g, v| g.set_settings_seek_step_secs(v),
        ),
    },
    Choice {
        key: PLY_SEEK_STEP_LONG,
        options: SEEK_STEP_LONG_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_seek_step_long_secs(),
            |g, v| g.set_settings_seek_step_long_secs(v),
        ),
    },
    Choice {
        key: PLY_TRAILER_QUALITY,
        options: TRAILER_QUALITY_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_trailer_quality(),
            |g, v| g.set_settings_trailer_quality(v),
        ),
    },
    Choice {
        key: AUD_AUDIO_LANG,
        options: LANG_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_audio_lang(),
            |g, v| g.set_settings_audio_lang(v),
        ),
    },
    Choice {
        key: PLY_SUB_LANG,
        options: LANG_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_sub_lang(),
            |g, v| g.set_settings_sub_lang(v),
        ),
    },
    Choice {
        key: PLY_SUB_LANG2,
        options: LANG_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_sub_lang2(),
            |g, v| g.set_settings_sub_lang2(v),
        ),
    },
    Choice {
        key: PLY_SUB_TYPE,
        options: SUB_TYPE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_sub_type(),
            |g, v| g.set_settings_sub_type(v),
        ),
    },
    Choice {
        key: PLY_SUB_SCALE,
        options: SUB_SCALE_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_sub_scale_pct(),
            |g, v| g.set_settings_sub_scale_pct(v),
        ),
    },
    Choice {
        key: PLY_SUB_POS,
        options: SUB_POS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_sub_pos_pct(),
            |g, v| g.set_settings_sub_pos_pct(v),
        ),
    },
    Choice {
        key: PLY_SUB_COLOR,
        options: SUB_COLOR_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_sub_color(),
            |g, v| g.set_settings_sub_color(v),
        ),
    },
    Choice {
        key: VID_TONE_MAPPING,
        options: TONE_MAPPING_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_tone_mapping(),
            |g, v| g.set_settings_tone_mapping(v),
        ),
    },
    Choice {
        key: VID_DISPLAY_SYNC_SCALE_4K,
        options: DISPLAY_SYNC_SCALE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_display_sync_scale_4k(),
            |g, v| g.set_settings_display_sync_scale_4k(v),
        ),
    },
    Choice {
        key: VID_DISPLAY_SYNC_SCALE_1080P,
        options: DISPLAY_SYNC_SCALE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_display_sync_scale_1080p(),
            |g, v| g.set_settings_display_sync_scale_1080p(v),
        ),
    },
    Choice {
        key: VID_DISPLAY_SYNC_4K_ODD_FPS_MODE,
        options: DISPLAY_SYNC_4K_ODD_FPS_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_display_sync_4k_odd_fps_mode(),
            |g, v| g.set_settings_display_sync_4k_odd_fps_mode(v),
        ),
    },
    Choice {
        key: VID_DISPLAY_SYNC_HDR_MODE,
        options: DISPLAY_SYNC_HDR_MODE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_display_sync_hdr_mode(),
            |g, v| g.set_settings_display_sync_hdr_mode(v),
        ),
    },
    Choice {
        key: VID_DISPLAY_SYNC_WCG_MODE,
        options: DISPLAY_SYNC_WCG_MODE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_display_sync_wcg_mode(),
            |g, v| g.set_settings_display_sync_wcg_mode(v),
        ),
    },
    Choice {
        key: AUD_CHANNELS,
        options: AUDIO_CHANNELS_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_audio_channels(),
            |g, v| g.set_settings_audio_channels(v),
        ),
    },
    Choice {
        key: UI_CARD_SIZE,
        options: CARD_SIZE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_card_size(),
            |g, v| g.set_settings_card_size(v),
        ),
    },
    Choice {
        key: UI_TEXT_SIZE,
        options: CARD_SIZE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_text_size(),
            |g, v| g.set_settings_text_size(v),
        ),
    },
    Choice {
        key: UI_DASHBOARD_HERO,
        options: HERO_STYLE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_dashboard_hero(),
            |g, v| g.set_settings_dashboard_hero(v),
        ),
    },
    Choice {
        key: UI_FOCUS_COLOR,
        options: FOCUS_COLOR_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_focus_color(),
            |g, v| g.set_settings_focus_color(v),
        ),
    },
    Choice {
        key: UI_FOCUS_WIDTH,
        options: FOCUS_WIDTH_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_focus_width(),
            |g, v| g.set_settings_focus_width(v),
        ),
    },
    Choice {
        key: UI_FOCUS_STYLE,
        options: FOCUS_STYLE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_focus_style(),
            |g, v| g.set_settings_focus_style(v),
        ),
    },
    Choice {
        key: UI_FOCUS_GLOW,
        options: FOCUS_GLOW_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_focus_glow(),
            |g, v| g.set_settings_focus_glow(v),
        ),
    },
    Choice {
        key: UI_FOCUS_GLOW_ON,
        options: FOCUS_GLOW_ON_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_focus_glow_on(),
            |g, v| g.set_settings_focus_glow_on(v),
        ),
    },
    Choice {
        key: UI_FOCUS_OPACITY,
        options: FOCUS_OPACITY_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_focus_opacity(),
            |g, v| g.set_settings_focus_opacity(v),
        ),
    },
    Choice {
        key: UI_FOCUS_ZOOM,
        options: FOCUS_ZOOM_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_focus_zoom(),
            |g, v| g.set_settings_focus_zoom(v),
        ),
    },
    Choice {
        key: UI_SCROLL_SPEED,
        options: SPEED_PCT_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_scroll_speed_pct(),
            |g, v| g.set_settings_scroll_speed_pct(v),
        ),
    },
    Choice {
        key: UI_ANIMATION_SPEED,
        options: SPEED_PCT_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_animation_speed_pct(),
            |g, v| g.set_settings_animation_speed_pct(v),
        ),
    },
    Choice {
        key: PROF_LAUNCH_POLICY,
        options: LAUNCH_POLICY_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_launch_policy(),
            |g, v| g.set_settings_launch_policy(v),
        ),
    },
    Choice {
        key: PROF_ACCOUNT_LAUNCH_POLICY,
        options: ACCOUNT_LAUNCH_POLICY_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_account_launch_policy(),
            |g, v| g.set_settings_account_launch_policy(v),
        ),
    },
    Choice {
        key: VID_HWDEC,
        options: HWDEC_OPTIONS,
        field: Field::Str(|g| g.get_settings_hwdec(), |g, v| g.set_settings_hwdec(v)),
    },
    Choice {
        key: VID_VF,
        options: VF_OPTIONS,
        field: Field::Str(|g| g.get_settings_vf(), |g, v| g.set_settings_vf(v)),
    },
    Choice {
        key: VID_DEINTERLACE,
        options: DEINTERLACE_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_deinterlace(),
            |g, v| g.set_settings_deinterlace(v),
        ),
    },
    Choice {
        key: VID_VIDEO_SYNC,
        options: VIDEO_SYNC_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_video_sync(),
            |g, v| g.set_settings_video_sync(v),
        ),
    },
    Choice {
        key: VID_TSCALE,
        options: TSCALE_OPTIONS,
        field: Field::Str(|g| g.get_settings_tscale(), |g, v| g.set_settings_tscale(v)),
    },
    Choice {
        key: PLY_CACHE_SECS,
        options: CACHE_SECS_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_cache_secs(),
            |g, v| g.set_settings_cache_secs(v),
        ),
    },
    Choice {
        key: PLY_CACHE_MAX_MB,
        options: CACHE_MAX_MB_OPTIONS,
        field: Field::Int(
            |g| g.get_settings_cache_max_mb(),
            |g, v| g.set_settings_cache_max_mb(v),
        ),
    },
    Choice {
        key: GEN_LOG_LEVEL,
        options: LOG_LEVEL_OPTIONS,
        field: Field::Str(
            |g| g.get_settings_log_level(),
            |g, v| g.set_settings_log_level(v),
        ),
    },
];

// ── Fixed-choice rows in Slint: options, current label, pick by label ──────────
/// settings.slint's `SettingsRow { choice: true; value: … }` draws its dropdown from CHOICES.
pub(crate) fn wire_choice_rows(window: &MainWindow) {
    let g = crate::AppState::get(window);
    g.on_setting_options(|key| {
        let labels: Vec<SharedString> = choice(&key)
            .map(|c| {
                c.options
                    .iter()
                    .map(|(_, l)| SharedString::from(*l))
                    .collect()
            })
            .unwrap_or_default();
        ModelRc::new(VecModel::from(labels))
    });
    g.on_setting_label(|key, value| match choice(&key) {
        Some(c) => c.label(&value).into(),
        None => value,
    });
    let ww = window.as_weak();
    g.on_setting_choose(move |key, label| {
        let Some(w) = ww.upgrade() else { return };
        if let Some(c) = choice(&key)
            && let Some(i) = c.options.iter().position(|(_, l)| *l == label.as_str())
        {
            debug!("settings: {key} → {} (mouse)", c.options[i].0);
            c.set(&crate::AppState::get(&w), i);
        }
    });
}

// Rows whose option list + current display value come from an AppState
// property populated by an async fetch (mpv --audio-device=help, fc-list,
// Seerr's own settings/regions/languages endpoints, or kscreen-doctor)
// rather than a fixed compile-time list.
fn is_dynamic_dropdown(key: &str) -> bool {
    matches!(
        key,
        PROF_DEFAULT_PROFILE
            | PROF_DEFAULT_ACCOUNT
            | PROF_COPY_SETTINGS
            | AUD_AUDIO_DEVICE
            | AUD_PASSTHROUGH_DEVICE
            | UI_FONT_FAMILY
            | INT_STREAMING_REGION
            | INT_DISPLAY_LANGUAGE
            | INT_DISCOVER_LANGUAGE
            | INT_DISCOVER_REGION
            | VID_DISPLAY_SYNC_SCREEN
            | VID_DISPLAY_SYNC_DEFAULT_RESOLUTION
            | VID_DISPLAY_SYNC_DEFAULT_HZ
    )
}

fn is_dropdown_row(key: &str) -> bool {
    is_dynamic_dropdown(key) || choice(key).is_some()
}

// Populates settings-dropdown-model/-cursor/-display and opens the popup.
// Used by keyboard Confirm on a dropdown row (dispatch_settings above) and by
// the click-to-open path on a dropdown row's SettingsDropdown mouse handler.
pub(crate) fn open_dropdown_popup(key: &str, g: &crate::AppState<'_>) {
    // Dynamic dropdowns: display list + current desc live on AppState
    // properties populated by an async fetch, not a fixed compile-time list.
    let dynamic: Option<(ModelRc<SharedString>, SharedString)> = match key {
        PROF_DEFAULT_PROFILE => Some((
            g.get_settings_default_profile_display(),
            g.get_settings_default_profile_desc(),
        )),
        // No "current" source: the list always opens on its first entry.
        PROF_COPY_SETTINGS => Some((g.get_settings_copy_sources_display(), SharedString::new())),
        PROF_DEFAULT_ACCOUNT => Some((
            g.get_settings_default_account_display(),
            g.get_settings_default_account_desc(),
        )),
        AUD_AUDIO_DEVICE => Some((
            g.get_settings_audio_device_display(),
            g.get_settings_audio_device_desc(),
        )),
        AUD_PASSTHROUGH_DEVICE => Some((
            g.get_settings_audio_device_display(),
            g.get_settings_passthrough_device_desc(),
        )),
        UI_FONT_FAMILY => Some((
            g.get_settings_font_family_display(),
            g.get_settings_font_family_desc(),
        )),
        INT_STREAMING_REGION => Some((
            g.get_settings_streaming_region_display(),
            g.get_settings_streaming_region_desc(),
        )),
        INT_DISPLAY_LANGUAGE => Some((
            g.get_settings_display_language_display(),
            g.get_settings_display_language_desc(),
        )),
        INT_DISCOVER_LANGUAGE => Some((
            g.get_settings_discover_language_display(),
            g.get_settings_discover_language_desc(),
        )),
        // Discover Region reuses Streaming Region's own already-fetched
        // list — the two settings share one region catalog, just a
        // different desc value for which one's currently set.
        INT_DISCOVER_REGION => Some((
            g.get_settings_streaming_region_display(),
            g.get_settings_discover_region_desc(),
        )),
        // The output's desc is its annotated label ("HDMI-A-2 (Primary)"); name↔desc is looked up
        // in FjordState.display_sync_outputs by display_sync.rs's on_display_sync_screen_selected,
        // like audio-device/font-family. Resolution/Hz descs are the values themselves.
        VID_DISPLAY_SYNC_SCREEN => Some((
            g.get_settings_display_sync_screen_options(),
            g.get_settings_display_sync_screen_desc(),
        )),
        VID_DISPLAY_SYNC_DEFAULT_RESOLUTION => Some((
            g.get_settings_display_sync_resolution_options(),
            g.get_settings_display_sync_default_resolution(),
        )),
        VID_DISPLAY_SYNC_DEFAULT_HZ => Some((
            g.get_settings_display_sync_hz_options(),
            g.get_settings_display_sync_default_hz(),
        )),
        _ => None,
    };
    if let Some((display, current_desc)) = dynamic {
        let n = display.row_count();
        let current_desc = current_desc.to_string();
        let cursor = (0..n)
            .find(|&i| display.row_data(i).map(|s| s.to_string()) == Some(current_desc.clone()))
            .unwrap_or(0) as i32;
        let items: Vec<SharedString> = (0..n).filter_map(|i| display.row_data(i)).collect();
        let current_display = items.get(cursor as usize).cloned().unwrap_or_default();
        debug!(
            "settings: open dynamic dropdown {key} ({n} options, cursor={cursor}, current={current_display})"
        );
        g.set_settings_dropdown_model(ModelRc::new(VecModel::from(items)));
        g.set_settings_dropdown_display(current_display);
        g.set_settings_dropdown_cursor(cursor);
        g.set_settings_dropdown_open(true);
        return;
    }
    let Some(c) = choice(key) else {
        debug!(
            "settings: open_dropdown_popup({key}) — no model, not a dropdown row (bug if this fires)"
        );
        return;
    };
    let current = c.value(g);
    let cursor = c.index_of(&current).unwrap_or(0) as i32;
    let display_items: Vec<SharedString> = c.options.iter().map(|(_, l)| (*l).into()).collect();
    debug!(
        "settings: open static dropdown {key} ({} options, cursor={cursor}, current={current})",
        c.options.len()
    );
    g.set_settings_dropdown_model(ModelRc::new(VecModel::from(display_items)));
    g.set_settings_dropdown_display(c.label(&current).into());
    g.set_settings_dropdown_cursor(cursor);
    g.set_settings_dropdown_open(true);
}

pub(crate) fn apply_dropdown_selection(key: &str, cursor: i32, g: &crate::AppState<'_>) {
    debug!("settings: apply_dropdown_selection {key} cursor={cursor}");
    match key {
        PROF_COPY_SETTINGS => {
            if let Some(label) = g
                .get_settings_copy_sources_display()
                .row_data(cursor as usize)
            {
                g.invoke_copy_settings_source_selected(label);
            }
            return;
        }
        PROF_DEFAULT_PROFILE => {
            let display = g.get_settings_default_profile_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_default_profile_selected(desc);
            }
            return;
        }
        PROF_DEFAULT_ACCOUNT => {
            let display = g.get_settings_default_account_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_default_account_selected(desc);
            }
            return;
        }
        AUD_AUDIO_DEVICE | AUD_PASSTHROUGH_DEVICE => {
            let display = g.get_settings_audio_device_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                if key == AUD_AUDIO_DEVICE {
                    g.invoke_audio_device_selected(desc);
                } else {
                    g.invoke_passthrough_device_selected(desc);
                }
            }
            return;
        }
        UI_FONT_FAMILY => {
            let display = g.get_settings_font_family_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_font_family_selected(desc);
            }
            return;
        }
        INT_STREAMING_REGION => {
            let display = g.get_settings_streaming_region_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_streaming_region_selected(desc);
            }
            return;
        }
        INT_DISPLAY_LANGUAGE => {
            let display = g.get_settings_display_language_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_display_language_selected(desc);
            }
            return;
        }
        INT_DISCOVER_LANGUAGE => {
            let display = g.get_settings_discover_language_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_discover_language_selected(desc);
            }
            return;
        }
        INT_DISCOVER_REGION => {
            let display = g.get_settings_streaming_region_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_discover_region_selected(desc);
            }
            return;
        }
        VID_DISPLAY_SYNC_SCREEN => {
            let display = g.get_settings_display_sync_screen_options();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_display_sync_screen_selected(desc);
            }
            return;
        }
        VID_DISPLAY_SYNC_DEFAULT_RESOLUTION => {
            let display = g.get_settings_display_sync_resolution_options();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_display_sync_resolution_selected(desc);
            }
            return;
        }
        VID_DISPLAY_SYNC_DEFAULT_HZ => {
            let display = g.get_settings_display_sync_hz_options();
            if let Some(desc) = display.row_data(cursor as usize) {
                g.invoke_display_sync_hz_selected(desc);
            }
            return;
        }
        _ => {}
    }
    if let Some(c) = choice(key) {
        c.set(g, cursor.max(0) as usize);
    }
}

// ── Settings ↔ config mirroring ───────────────────────────────────────────────
// Every setting kept both in the config and in a `settings-*` window property is one line below:
// apply_settings_to_window / apply_profile_settings_to_window load them, read_settings_from_window
// stores them, ProfileSettings::copy_settings_from copies PROFILE_BINDS (= the Settings rows
// tagged "Profile"). What isn't a plain copy stays explicit in those functions (device
// descriptions, the speed floats, audio channels' "auto-safe", seerr_enabled, dashboard rows).

/// A device-wide setting: DeviceConfig field ↔ window property.
pub(crate) struct DeviceBind {
    load: fn(&config::DeviceConfig, &crate::AppState<'_>),
    store: fn(&mut config::DeviceConfig, &crate::AppState<'_>),
}

/// A per-profile setting: ProfileSettings field ↔ window property, and how "Copy settings from…"
/// copies it.
pub(crate) struct ProfileBind {
    load: fn(&config::ProfileSettings, &crate::AppState<'_>),
    store: fn(&mut config::ProfileSettings, &crate::AppState<'_>),
    pub(crate) copy: fn(&mut config::ProfileSettings, &config::ProfileSettings),
}

/// `bind!(Device|Profile, config field, window getter, window setter, bool|str|u32)`.
macro_rules! bind {
    (@load $f:ident, $set:ident, bool) => { |c, g| g.$set(c.$f) };
    (@load $f:ident, $set:ident, str) => { |c, g| g.$set(c.$f.as_str().into()) };
    (@load $f:ident, $set:ident, u32) => { |c, g| g.$set(c.$f as i32) };
    (@store $f:ident, $get:ident, bool) => { |c, g| c.$f = g.$get() };
    (@store $f:ident, $get:ident, str) => { |c, g| c.$f = g.$get().to_string() };
    (@store $f:ident, $get:ident, u32) => { |c, g| c.$f = g.$get().max(0) as u32 };
    (Device, $f:ident, $get:ident, $set:ident, $k:ident) => {
        DeviceBind { load: bind!(@load $f, $set, $k), store: bind!(@store $f, $get, $k) }
    };
    (Profile, $f:ident, $get:ident, $set:ident, $k:ident) => {
        ProfileBind {
            load: bind!(@load $f, $set, $k),
            store: bind!(@store $f, $get, $k),
            copy: |d, s| d.$f = s.$f.clone(),
        }
    };
}

static DEVICE_BINDS: &[DeviceBind] = &[
    bind!(
        Device,
        audio_spdif,
        get_settings_audio_spdif,
        set_settings_audio_spdif,
        bool
    ),
    bind!(
        Device,
        spdif_ac3,
        get_settings_spdif_ac3,
        set_settings_spdif_ac3,
        bool
    ),
    bind!(
        Device,
        spdif_eac3,
        get_settings_spdif_eac3,
        set_settings_spdif_eac3,
        bool
    ),
    bind!(
        Device,
        spdif_dts,
        get_settings_spdif_dts,
        set_settings_spdif_dts,
        bool
    ),
    bind!(
        Device,
        spdif_dts_hd,
        get_settings_spdif_dts_hd,
        set_settings_spdif_dts_hd,
        bool
    ),
    bind!(
        Device,
        spdif_truehd,
        get_settings_spdif_truehd,
        set_settings_spdif_truehd,
        bool
    ),
    bind!(Device, hwdec, get_settings_hwdec, set_settings_hwdec, str),
    bind!(Device, vf, get_settings_vf, set_settings_vf, str),
    bind!(
        Device,
        video_sync,
        get_settings_video_sync,
        set_settings_video_sync,
        str
    ),
    bind!(
        Device,
        opengl_early_flush,
        get_settings_opengl_early_flush,
        set_settings_opengl_early_flush,
        bool
    ),
    bind!(
        Device,
        video_latency_hacks,
        get_settings_video_latency_hacks,
        set_settings_video_latency_hacks,
        bool
    ),
    bind!(
        Device,
        interpolation,
        get_settings_interpolation,
        set_settings_interpolation,
        bool
    ),
    bind!(
        Device,
        tscale,
        get_settings_tscale,
        set_settings_tscale,
        str
    ),
    bind!(
        Device,
        tone_mapping,
        get_settings_tone_mapping,
        set_settings_tone_mapping,
        str
    ),
    bind!(
        Device,
        target_colorspace_hint,
        get_settings_target_colorspace_hint,
        set_settings_target_colorspace_hint,
        bool
    ),
    bind!(
        Device,
        separate_video_surface,
        get_settings_separate_video_surface,
        set_settings_separate_video_surface,
        bool
    ),
    bind!(
        Device,
        video_own_buffers,
        get_settings_video_own_buffers,
        set_settings_video_own_buffers,
        bool
    ),
    bind!(
        Device,
        video_dither_off,
        get_settings_video_dither_off,
        set_settings_video_dither_off,
        bool
    ),
    bind!(
        Device,
        deinterlace,
        get_settings_deinterlace,
        set_settings_deinterlace,
        str
    ),
    bind!(
        Device,
        cache_secs,
        get_settings_cache_secs,
        set_settings_cache_secs,
        u32
    ),
    bind!(
        Device,
        cache_max_mb,
        get_settings_cache_max_mb,
        set_settings_cache_max_mb,
        u32
    ),
    bind!(
        Device,
        launch_fullscreen,
        get_settings_launch_fullscreen,
        set_settings_launch_fullscreen,
        bool
    ),
    bind!(
        Device,
        log_level,
        get_settings_log_level,
        set_settings_log_level,
        str
    ),
    bind!(
        Device,
        audio_device,
        get_settings_audio_device,
        set_settings_audio_device,
        str
    ),
    bind!(
        Device,
        audio_device_passthrough,
        get_settings_passthrough_device,
        set_settings_passthrough_device,
        str
    ),
    bind!(
        Device,
        gapless_audio,
        get_settings_gapless_audio,
        set_settings_gapless_audio,
        bool
    ),
    bind!(
        Device,
        alsa_irq_scheduling,
        get_settings_alsa_irq_scheduling,
        set_settings_alsa_irq_scheduling,
        bool
    ),
    bind!(
        Device,
        skip_fade_mute_passthrough,
        get_settings_skip_fade_mute_passthrough,
        set_settings_skip_fade_mute_passthrough,
        bool
    ),
    bind!(
        Device,
        launch_policy,
        get_settings_launch_policy,
        set_settings_launch_policy,
        str
    ),
    bind!(
        Device,
        default_profile_id,
        get_settings_default_profile_id,
        set_settings_default_profile_id,
        str
    ),
    bind!(
        Device,
        account_launch_policy,
        get_settings_account_launch_policy,
        set_settings_account_launch_policy,
        str
    ),
    bind!(
        Device,
        default_account_id,
        get_settings_default_account_id,
        set_settings_default_account_id,
        str
    ),
    bind!(
        Device,
        display_sync_enabled,
        get_settings_display_sync_enabled,
        set_settings_display_sync_enabled,
        bool
    ),
    bind!(
        Device,
        display_sync_trailers,
        get_settings_display_sync_trailers,
        set_settings_display_sync_trailers,
        bool
    ),
    bind!(
        Device,
        display_sync_screen_name,
        get_settings_display_sync_screen_name,
        set_settings_display_sync_screen_name,
        str
    ),
    bind!(
        Device,
        display_sync_default_resolution,
        get_settings_display_sync_default_resolution,
        set_settings_display_sync_default_resolution,
        str
    ),
    bind!(
        Device,
        display_sync_default_hz,
        get_settings_display_sync_default_hz,
        set_settings_display_sync_default_hz,
        str
    ),
    bind!(
        Device,
        display_sync_scale_4k,
        get_settings_display_sync_scale_4k,
        set_settings_display_sync_scale_4k,
        str
    ),
    bind!(
        Device,
        display_sync_scale_1080p,
        get_settings_display_sync_scale_1080p,
        set_settings_display_sync_scale_1080p,
        str
    ),
    bind!(
        Device,
        display_sync_sync_resolution,
        get_settings_display_sync_sync_resolution,
        set_settings_display_sync_sync_resolution,
        bool
    ),
    bind!(
        Device,
        display_sync_sync_refresh_rate,
        get_settings_display_sync_sync_refresh_rate,
        set_settings_display_sync_sync_refresh_rate,
        bool
    ),
    bind!(
        Device,
        display_sync_4k_odd_fps_mode,
        get_settings_display_sync_4k_odd_fps_mode,
        set_settings_display_sync_4k_odd_fps_mode,
        str
    ),
    bind!(
        Device,
        display_sync_hdr_mode,
        get_settings_display_sync_hdr_mode,
        set_settings_display_sync_hdr_mode,
        str
    ),
    bind!(
        Device,
        display_sync_wcg_mode,
        get_settings_display_sync_wcg_mode,
        set_settings_display_sync_wcg_mode,
        str
    ),
];

pub(crate) static PROFILE_BINDS: &[ProfileBind] = &[
    bind!(
        Profile,
        sub_enabled,
        get_settings_sub_enabled,
        set_settings_sub_enabled,
        bool
    ),
    bind!(
        Profile,
        sub_lang,
        get_settings_sub_lang,
        set_settings_sub_lang,
        str
    ),
    bind!(
        Profile,
        sub_lang2,
        get_settings_sub_lang2,
        set_settings_sub_lang2,
        str
    ),
    bind!(
        Profile,
        sub_type,
        get_settings_sub_type,
        set_settings_sub_type,
        str
    ),
    bind!(
        Profile,
        sub_scale_pct,
        get_settings_sub_scale_pct,
        set_settings_sub_scale_pct,
        u32
    ),
    bind!(
        Profile,
        sub_pos_pct,
        get_settings_sub_pos_pct,
        set_settings_sub_pos_pct,
        u32
    ),
    bind!(
        Profile,
        sub_respect_ass_styling,
        get_settings_sub_respect_ass_styling,
        set_settings_sub_respect_ass_styling,
        bool
    ),
    bind!(
        Profile,
        sub_color,
        get_settings_sub_color,
        set_settings_sub_color,
        str
    ),
    bind!(
        Profile,
        sub_background_pct,
        get_settings_sub_background_pct,
        set_settings_sub_background_pct,
        u32
    ),
    bind!(
        Profile,
        audio_lang,
        get_settings_audio_lang,
        set_settings_audio_lang,
        str
    ),
    bind!(
        Profile,
        now_playing_auto_open_secs,
        get_settings_now_playing_auto_open_secs,
        set_settings_now_playing_auto_open_secs,
        u32
    ),
    bind!(
        Profile,
        skip_intro_mode,
        get_settings_skip_intro_mode,
        set_settings_skip_intro_mode,
        str
    ),
    bind!(
        Profile,
        skip_intro_secs,
        get_settings_skip_intro_secs,
        set_settings_skip_intro_secs,
        u32
    ),
    bind!(
        Profile,
        skip_recap_mode,
        get_settings_skip_recap_mode,
        set_settings_skip_recap_mode,
        str
    ),
    bind!(
        Profile,
        skip_recap_secs,
        get_settings_skip_recap_secs,
        set_settings_skip_recap_secs,
        u32
    ),
    bind!(
        Profile,
        skip_preview_mode,
        get_settings_skip_preview_mode,
        set_settings_skip_preview_mode,
        str
    ),
    bind!(
        Profile,
        skip_preview_secs,
        get_settings_skip_preview_secs,
        set_settings_skip_preview_secs,
        u32
    ),
    bind!(
        Profile,
        skip_commercial_mode,
        get_settings_skip_commercial_mode,
        set_settings_skip_commercial_mode,
        str
    ),
    bind!(
        Profile,
        skip_commercial_secs,
        get_settings_skip_commercial_secs,
        set_settings_skip_commercial_secs,
        u32
    ),
    bind!(
        Profile,
        skip_credits_mode,
        get_settings_skip_credits_mode,
        set_settings_skip_credits_mode,
        str
    ),
    bind!(
        Profile,
        card_size,
        get_settings_card_size,
        set_settings_card_size,
        str
    ),
    bind!(
        Profile,
        text_size,
        get_settings_text_size,
        set_settings_text_size,
        str
    ),
    bind!(
        Profile,
        dashboard_hero,
        get_settings_dashboard_hero,
        set_settings_dashboard_hero,
        str
    ),
    bind!(
        Profile,
        focus_color,
        get_settings_focus_color,
        set_settings_focus_color,
        str
    ),
    bind!(
        Profile,
        focus_width,
        get_settings_focus_width,
        set_settings_focus_width,
        str
    ),
    bind!(
        Profile,
        focus_style,
        get_settings_focus_style,
        set_settings_focus_style,
        str
    ),
    bind!(
        Profile,
        focus_glow,
        get_settings_focus_glow,
        set_settings_focus_glow,
        str
    ),
    bind!(
        Profile,
        focus_glow_on,
        get_settings_focus_glow_on,
        set_settings_focus_glow_on,
        str
    ),
    bind!(
        Profile,
        focus_opacity,
        get_settings_focus_opacity,
        set_settings_focus_opacity,
        str
    ),
    bind!(
        Profile,
        focus_zoom,
        get_settings_focus_zoom,
        set_settings_focus_zoom,
        str
    ),
    bind!(
        Profile,
        skip_credits_secs,
        get_settings_skip_credits_secs,
        set_settings_skip_credits_secs,
        u32
    ),
    bind!(
        Profile,
        screensaver_mins,
        get_settings_screensaver_mins,
        set_settings_screensaver_mins,
        u32
    ),
    bind!(
        Profile,
        controls_hide_secs,
        get_settings_controls_hide_secs,
        set_settings_controls_hide_secs,
        u32
    ),
    bind!(
        Profile,
        toast_secs,
        get_settings_toast_secs,
        set_settings_toast_secs,
        u32
    ),
    bind!(
        Profile,
        trailer_quality,
        get_settings_trailer_quality,
        set_settings_trailer_quality,
        str
    ),
    bind!(
        Profile,
        video_behind,
        get_settings_video_behind,
        set_settings_video_behind,
        bool
    ),
    bind!(
        Profile,
        seek_step_secs,
        get_settings_seek_step_secs,
        set_settings_seek_step_secs,
        u32
    ),
    bind!(
        Profile,
        seek_step_long_secs,
        get_settings_seek_step_long_secs,
        set_settings_seek_step_long_secs,
        u32
    ),
    bind!(
        Profile,
        skip_fade_ms,
        get_settings_skip_fade_ms,
        set_settings_skip_fade_ms,
        u32
    ),
    bind!(
        Profile,
        scroll_speed_pct,
        get_settings_scroll_speed_pct,
        set_settings_scroll_speed_pct,
        u32
    ),
    bind!(
        Profile,
        animation_speed_pct,
        get_settings_animation_speed_pct,
        set_settings_animation_speed_pct,
        u32
    ),
    bind!(
        Profile,
        ui_font_family,
        get_settings_font_family,
        set_settings_font_family,
        str
    ),
    bind!(
        Profile,
        onscreen_keyboard_enabled,
        get_settings_onscreen_keyboard_enabled,
        set_settings_onscreen_keyboard_enabled,
        bool
    ),
];

/// The on/off rows: Enter/Right flips the property and saves (settings_row_action).
type Toggle = (
    &'static str,
    fn(&crate::AppState<'_>) -> bool,
    fn(&crate::AppState<'_>, bool),
);
static TOGGLES: &[Toggle] = &[
    (
        GEN_LAUNCH_FULLSCREEN,
        |g| g.get_settings_launch_fullscreen(),
        |g, v| g.set_settings_launch_fullscreen(v),
    ),
    (
        GEN_VIDEO_BEHIND,
        |g| g.get_settings_video_behind(),
        |g, v| g.set_settings_video_behind(v),
    ),
    (
        VID_INTERPOLATION,
        |g| g.get_settings_interpolation(),
        |g, v| g.set_settings_interpolation(v),
    ),
    (
        VID_TARGET_COLORSPACE,
        |g| g.get_settings_target_colorspace_hint(),
        |g, v| g.set_settings_target_colorspace_hint(v),
    ),
    (
        VID_SEPARATE_VIDEO_SURFACE,
        |g| g.get_settings_separate_video_surface(),
        |g, v| g.set_settings_separate_video_surface(v),
    ),
    (
        VID_OWN_BUFFERS,
        |g| g.get_settings_video_own_buffers(),
        |g, v| g.set_settings_video_own_buffers(v),
    ),
    (
        VID_DITHER_OFF,
        |g| g.get_settings_video_dither_off(),
        |g, v| g.set_settings_video_dither_off(v),
    ),
    (
        VID_OPENGL_EARLY_FLUSH,
        |g| g.get_settings_opengl_early_flush(),
        |g, v| g.set_settings_opengl_early_flush(v),
    ),
    (
        VID_VIDEO_LATENCY_HACKS,
        |g| g.get_settings_video_latency_hacks(),
        |g, v| g.set_settings_video_latency_hacks(v),
    ),
    (
        VID_DISPLAY_SYNC_ENABLED,
        |g| g.get_settings_display_sync_enabled(),
        |g, v| g.set_settings_display_sync_enabled(v),
    ),
    (
        VID_DISPLAY_SYNC_TRAILERS,
        |g| g.get_settings_display_sync_trailers(),
        |g, v| g.set_settings_display_sync_trailers(v),
    ),
    (
        AUD_SPDIF,
        |g| g.get_settings_audio_spdif(),
        |g, v| g.set_settings_audio_spdif(v),
    ),
    (
        AUD_SPDIF_AC3,
        |g| g.get_settings_spdif_ac3(),
        |g, v| g.set_settings_spdif_ac3(v),
    ),
    (
        AUD_SPDIF_EAC3,
        |g| g.get_settings_spdif_eac3(),
        |g, v| g.set_settings_spdif_eac3(v),
    ),
    (
        AUD_SPDIF_DTS,
        |g| g.get_settings_spdif_dts(),
        |g, v| g.set_settings_spdif_dts(v),
    ),
    (
        AUD_SPDIF_DTS_HD,
        |g| g.get_settings_spdif_dts_hd(),
        |g, v| g.set_settings_spdif_dts_hd(v),
    ),
    (
        AUD_SPDIF_TRUEHD,
        |g| g.get_settings_spdif_truehd(),
        |g, v| g.set_settings_spdif_truehd(v),
    ),
    (
        AUD_ALSA_IRQ,
        |g| g.get_settings_alsa_irq_scheduling(),
        |g, v| g.set_settings_alsa_irq_scheduling(v),
    ),
    (
        AUD_SKIP_FADE_MUTE,
        |g| g.get_settings_skip_fade_mute_passthrough(),
        |g, v| g.set_settings_skip_fade_mute_passthrough(v),
    ),
    (
        AUD_GAPLESS,
        |g| g.get_settings_gapless_audio(),
        |g, v| g.set_settings_gapless_audio(v),
    ),
    (
        UI_ONSCREEN_KEYBOARD,
        |g| g.get_settings_onscreen_keyboard_enabled(),
        |g, v| g.set_settings_onscreen_keyboard_enabled(v),
    ),
    (
        PLY_SUB_ENABLED,
        |g| g.get_settings_sub_enabled(),
        |g, v| g.set_settings_sub_enabled(v),
    ),
    (
        PLY_SUB_RESPECT_ASS,
        |g| g.get_settings_sub_respect_ass_styling(),
        |g, v| g.set_settings_sub_respect_ass_styling(v),
    ),
    (
        INT_SEERR_ENABLED,
        |g| g.get_settings_seerr_enabled(),
        |g, v| g.set_settings_seerr_enabled(v),
    ),
    (
        VID_DISPLAY_SYNC_SYNC_RESOLUTION,
        |g| g.get_settings_display_sync_sync_resolution(),
        |g, v| g.set_settings_display_sync_sync_resolution(v),
    ),
    (
        VID_DISPLAY_SYNC_SYNC_REFRESH_RATE,
        |g| g.get_settings_display_sync_sync_refresh_rate(),
        |g, v| g.set_settings_display_sync_sync_refresh_rate(v),
    ),
];

// ── Per-row action (Confirm on a non-dropdown row, or Right on any row) ───────

fn settings_row_action(key: &str, g: &crate::AppState<'_>) {
    debug!("settings: row_action {key}");
    fn cycle_dynamic(display: ModelRc<SharedString>, current_desc: &str) -> Option<SharedString> {
        let n = display.row_count();
        if n == 0 {
            return None;
        }
        let idx = (0..n)
            .find(|&i| display.row_data(i).map(|s| s.to_string()) == Some(current_desc.to_string()))
            .unwrap_or(0);
        display.row_data((idx + 1) % n)
    }

    // A fixed-choice row: the next option (after the last, the first).
    if let Some(c) = choice(key) {
        let next = c
            .index_of(&c.value(g))
            .map_or(0, |i| (i + 1) % c.options.len());
        c.set(g, next);
        return;
    }
    // An on/off row (only while it's on screen — e.g. latency hacks needs display-resample).
    if let Some((_, get, set)) = TOGGLES.iter().find(|(k, _, _)| *k == key) {
        if row_shown(key, g) {
            set(g, !get(g));
            g.invoke_settings_changed();
        }
        return;
    }
    match key {
        GEN_PREWARM_METADATA => g.invoke_prewarm_metadata(),
        UI_DASHBOARD_ROWS => g.invoke_dash_rows_open(),
        GEN_PREWARM_IMAGES => g.invoke_prewarm_images(),

        PROF_DEFAULT_PROFILE => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_default_profile_display(),
                g.get_settings_default_profile_desc().as_str(),
            ) {
                g.invoke_default_profile_selected(desc);
            }
        }
        PROF_MANAGE_PROFILES => g.invoke_open_manage_profiles(),
        // Right opens the list (cycling would ask to copy from each profile in turn).
        PROF_COPY_SETTINGS => open_dropdown_popup(key, g),
        PROF_DEFAULT_ACCOUNT => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_default_account_display(),
                g.get_settings_default_account_desc().as_str(),
            ) {
                g.invoke_default_account_selected(desc);
            }
        }
        PROF_ADD_ACCOUNT => g.invoke_settings_add_account(),
        PROF_REMEMBER_LOGIN => g.invoke_settings_remember_login_toggle(),
        PROF_SIGN_OUT => {
            // Opens the global (main.slint) Sign Out confirmation — the same one the sidebar
            // quick-menu and OfflineScreen's "Change Server" open (see show-sign-out-confirm).
            g.set_sign_out_confirm_focused(0);
            g.set_show_sign_out_confirm(true);
        }
        PROF_BONFIRE_GROUP => g.invoke_open_bonfire_group(),
        PROF_BONFIRE_ADMIN => g.invoke_open_bonfire_admin(),

        VID_DISPLAY_SYNC_SCREEN => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_display_sync_screen_options(),
                g.get_settings_display_sync_screen_desc().as_str(),
            ) {
                g.invoke_display_sync_screen_selected(desc);
            }
        }
        VID_DISPLAY_SYNC_DEFAULT_RESOLUTION => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_display_sync_resolution_options(),
                g.get_settings_display_sync_default_resolution().as_str(),
            ) {
                g.invoke_display_sync_resolution_selected(desc);
            }
        }
        VID_DISPLAY_SYNC_DEFAULT_HZ => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_display_sync_hz_options(),
                g.get_settings_display_sync_default_hz().as_str(),
            ) {
                g.invoke_display_sync_hz_selected(desc);
            }
        }

        AUD_AUDIO_DEVICE => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_audio_device_display(),
                g.get_settings_audio_device_desc().as_str(),
            ) {
                g.invoke_audio_device_selected(desc);
            }
        }
        AUD_PASSTHROUGH_DEVICE => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_audio_device_display(),
                g.get_settings_passthrough_device_desc().as_str(),
            ) {
                g.invoke_passthrough_device_selected(desc);
            }
        }

        UI_FONT_FAMILY => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_font_family_display(),
                g.get_settings_font_family_desc().as_str(),
            ) {
                g.invoke_font_family_selected(desc);
            }
        }

        INT_SEERR_CONNECT => {
            // Confirmation dialog, 2026-08-22 — see show-seerr-disconnect-
            // confirm's own doc comment in app_state.slint. Only the
            // Disconnect direction is gated; connecting isn't destructive.
            if g.get_seerr_connected() {
                g.set_seerr_disconnect_confirm_focused(0);
                g.set_show_seerr_disconnect_confirm(true);
            } else {
                g.invoke_open_connect_seerr();
            }
        }
        INT_STREAMING_REGION => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_streaming_region_display(),
                g.get_settings_streaming_region_desc().as_str(),
            ) {
                g.invoke_streaming_region_selected(desc);
            }
        }
        INT_DISPLAY_LANGUAGE => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_display_language_display(),
                g.get_settings_display_language_desc().as_str(),
            ) {
                g.invoke_display_language_selected(desc);
            }
        }
        INT_DISCOVER_LANGUAGE => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_discover_language_display(),
                g.get_settings_discover_language_desc().as_str(),
            ) {
                g.invoke_discover_language_selected(desc);
            }
        }
        INT_DISCOVER_REGION => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_streaming_region_display(),
                g.get_settings_discover_region_desc().as_str(),
            ) {
                g.invoke_discover_region_selected(desc);
            }
        }
        // Plain button row (2026-08-06, Seerr Blocklist support) — same
        // shape as INT_SEERR_CONNECT above, no dropdown to cycle.
        INT_MANAGE_BLOCKLIST => g.invoke_open_blocklist(),

        _ => {}
    }
}

// ── wire_device_lists (moved from main(), 0.5.0 step 3) ──────────────────
/// Wires audio/passthrough device and font lists (fetched once) + their dropdowns:
/// audio_device_selected, passthrough_device_selected, font_family_selected.
pub(crate) fn wire_device_lists(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── audio device list: fetch once at startup ─────────────────────────────
    {
        let state_ad = Arc::clone(&state);
        let ww_ad = window.as_weak();
        let (cfg_device, cfg_pt_device) = {
            let s = state.lock().unwrap();
            (
                s.config.device.audio_device.clone(),
                s.config.device.audio_device_passthrough.clone(),
            )
        };
        rt.spawn(async move {
            let devices = tokio::task::spawn_blocking(fetch_audio_devices)
                .await
                .unwrap_or_default();
            state_ad.lock().unwrap().audio_devices = devices.clone();
            let display: Vec<slint::SharedString> = devices
                .iter()
                .map(|(_, d)| slint::SharedString::from(d.as_str()))
                .collect();
            let desc = devices
                .iter()
                .find(|(n, _)| n.as_str() == cfg_device.as_str())
                .map(|(_, d)| d.as_str())
                .unwrap_or(if cfg_device.is_empty() {
                    ""
                } else {
                    cfg_device.as_str()
                })
                .to_string();
            let pt_desc = devices
                .iter()
                .find(|(n, _)| n.as_str() == cfg_pt_device.as_str())
                .map(|(_, d)| d.as_str())
                .unwrap_or(if cfg_pt_device.is_empty() {
                    ""
                } else {
                    cfg_pt_device.as_str()
                })
                .to_string();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww_ad.upgrade() {
                    let g = AppState::get(&w);
                    g.set_settings_audio_device_display(slint::ModelRc::new(
                        slint::VecModel::from(display),
                    ));
                    if !desc.is_empty() {
                        g.set_settings_audio_device_desc(slint::SharedString::from(desc.as_str()));
                    }
                    if !pt_desc.is_empty() {
                        g.set_settings_passthrough_device_desc(slint::SharedString::from(
                            pt_desc.as_str(),
                        ));
                    }
                }
            });
        });
    }

    // ── audio device selected callback ────────────────────────────────────────
    {
        let state_ad = Arc::clone(&state);
        let ww_ad = window.as_weak();
        AppState::get(&window).on_audio_device_selected(move |desc| {
            let name = {
                let s = state_ad.lock().unwrap();
                s.audio_devices
                    .iter()
                    .find(|(_, d)| d.as_str() == desc.as_str())
                    .map(|(n, _)| n.clone())
                    .unwrap_or_else(|| "auto".to_string())
            };
            if let Some(w) = ww_ad.upgrade() {
                let g = AppState::get(&w);
                g.set_settings_audio_device(slint::SharedString::from(name.as_str()));
                let pt = g.get_settings_passthrough_device().to_string();
                let effective = if pt.is_empty() {
                    name.as_str()
                } else {
                    pt.as_str()
                };
                g.set_settings_device_is_pipewire(pipewire_fix::is_pipewire_device(effective));
                g.set_settings_audio_device_desc(desc);
                g.invoke_settings_changed();
            }
        });
    }

    // ── passthrough device selected callback ─────────────────────────────────
    {
        let state_pd = Arc::clone(&state);
        let ww_pd = window.as_weak();
        AppState::get(&window).on_passthrough_device_selected(move |desc| {
            let name = {
                let s = state_pd.lock().unwrap();
                s.audio_devices
                    .iter()
                    .find(|(_, d)| d.as_str() == desc.as_str())
                    .map(|(n, _)| n.clone())
                    .unwrap_or_else(|| "auto".to_string())
            };
            if let Some(w) = ww_pd.upgrade() {
                let g = AppState::get(&w);
                // "auto" means "same as audio output" here — store empty so
                // start_playback falls back to the normal device.
                let (store, show_desc) = if name == "auto" {
                    (String::new(), slint::SharedString::default())
                } else {
                    (name.clone(), desc)
                };
                g.set_settings_passthrough_device(slint::SharedString::from(store.as_str()));
                g.set_settings_passthrough_device_desc(show_desc);
                let effective = if store.is_empty() {
                    g.get_settings_audio_device().to_string()
                } else {
                    store
                };
                g.set_settings_device_is_pipewire(pipewire_fix::is_pipewire_device(&effective));
                g.invoke_settings_changed();
            }
        });
    }

    // ── system font list: fetch once at startup ───────────────────────────────
    // settings-font-family (the value MainWindow.font-family actually binds
    // to) is already set synchronously from Config in apply_settings_to_window
    // at launch — this only populates the dropdown's display list/label, which
    // can safely lag behind by however long fc-list takes.
    {
        let state_fd = Arc::clone(&state);
        let ww_fd = window.as_weak();
        let cfg_font = state.lock().unwrap().config.active().ui_font_family.clone();
        rt.spawn(async move {
            let fonts = tokio::task::spawn_blocking(fetch_system_fonts)
                .await
                .unwrap_or_default();
            state_fd.lock().unwrap().system_fonts = fonts.clone();
            let display: Vec<slint::SharedString> = fonts
                .iter()
                .map(|(_, d)| slint::SharedString::from(d.as_str()))
                .collect();
            let desc = fonts
                .iter()
                .find(|(v, _)| v.as_str() == cfg_font.as_str())
                .map(|(_, d)| d.clone())
                .unwrap_or_else(|| "Inter (Fjord default)".to_string());
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww_fd.upgrade() {
                    let g = AppState::get(&w);
                    g.set_settings_font_family_display(slint::ModelRc::new(slint::VecModel::from(
                        display,
                    )));
                    g.set_settings_font_family_desc(slint::SharedString::from(desc.as_str()));
                }
            });
        });
    }

    // ── font family selected callback ─────────────────────────────────────────
    {
        let state_ff = Arc::clone(&state);
        let ww_ff = window.as_weak();
        AppState::get(&window).on_font_family_selected(move |desc| {
            let value = {
                let s = state_ff.lock().unwrap();
                s.system_fonts
                    .iter()
                    .find(|(_, d)| d.as_str() == desc.as_str())
                    .map(|(v, _)| v.clone())
                    .unwrap_or_else(|| "Inter".to_string())
            };
            if let Some(w) = ww_ff.upgrade() {
                let g = AppState::get(&w);
                g.set_settings_font_family(slint::SharedString::from(value.as_str()));
                g.set_settings_font_family_desc(desc);
                g.invoke_settings_changed();
            }
        });
    }
}

// ── wire_profile_defaults (moved from main(), 0.5.0 step 3) ──────────────
/// Wires default profile / account dropdowns: default_profile_selected, default_account_selected.
pub(crate) fn wire_profile_defaults(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── default profile selected callback ───────────────────────────────────────
    // Local only, like font-family. Resolves the label back to a user_id (duplicate labels:
    // first match, see refresh_profile_settings_dropdown); on_settings_changed persists it.
    {
        let state_dp = Arc::clone(&state);
        let ww_dp = window.as_weak();
        AppState::get(&window).on_default_profile_selected(move |desc| {
            let Some(w) = ww_dp.upgrade() else { return };
            let g = AppState::get(&w);
            let user_id = {
                let s = state_dp.lock().unwrap();
                // Scoped to the current Default Account, like the dropdown's options — a same-named
                // profile under another account must not match.
                let account_id = s.config.device.default_account_id.clone();
                s.config
                    .profiles
                    .iter()
                    .find(|p| {
                        let label = if p.display_name.is_empty() {
                            p.user_id.as_str()
                        } else {
                            p.display_name.as_str()
                        };
                        label == desc.as_str() && profile::account_root_id(p) == account_id
                    })
                    .map(|p| p.user_id.clone())
                    .unwrap_or_default()
            };
            g.set_settings_default_profile_id(ss(&user_id));
            g.set_settings_default_profile_desc(desc);
            g.invoke_settings_changed();
        });
    }

    // ── default account selected callback ───────────────────────────────────────
    // Account-tier mirror of default-profile-selected: local, label → root_id via
    // group_into_accounts.
    {
        let state_da = Arc::clone(&state);
        let ww_da = window.as_weak();
        AppState::get(&window).on_default_account_selected(move |desc| {
            let Some(w) = ww_da.upgrade() else { return };
            let g = AppState::get(&w);
            let root_id = {
                let s = state_da.lock().unwrap();
                profile::group_into_accounts(&s.config.profiles)
                    .into_iter()
                    .find(|a| {
                        let label = a
                            .profiles
                            .first()
                            .map(|p| {
                                if p.display_name.is_empty() {
                                    p.user_id.clone()
                                } else {
                                    p.display_name.clone()
                                }
                            })
                            .unwrap_or_default();
                        label == desc.as_str()
                    })
                    .map(|a| a.root_id)
                    .unwrap_or_default()
            };
            g.set_settings_default_account_id(ss(&root_id));
            g.set_settings_default_account_desc(desc);
            // Re-scope Default Profile's options to the just-picked account right away (display
            // only — invoke_settings_changed below does the real Config write).
            {
                let mut cfg = state_da.lock().unwrap().config.clone();
                cfg.device.default_account_id = root_id;
                profile::refresh_profile_settings_dropdown(&g, &cfg);
            }
            g.invoke_settings_changed();
        });
    }
}

// ── wire_copy_settings (0.5.1) ───────────────────────────────────────────
/// Settings → Profiles → "Copy settings from…": picking a source opens the confirm dialog; Copy
/// copies the source's "Profile"-tagged settings into the active profile — local only (no PIN,
/// no server call: they're preferences in this device's config) — by pushing them to the window
/// and invoking settings-changed, so saving, live-applying (subtitle style) and the change log
/// go the usual way.
pub(crate) fn wire_copy_settings(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
) {
    use crate::*;
    {
        let state_sel = Arc::clone(state);
        let ww = window.as_weak();
        AppState::get(window).on_copy_settings_source_selected(move |label| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let user_id = {
                let s = state_sel.lock().unwrap();
                profile::copy_settings_sources(&s.config)
                    .into_iter()
                    .find(|(l, _)| l.as_str() == label.as_str())
                    .map(|(_, id)| id)
            };
            let Some(user_id) = user_id else {
                warn!("copy settings: no profile for {label:?} (list changed?)");
                return;
            };
            debug!("copy settings: source picked — {label} ({user_id}), asking to confirm");
            g.set_copy_settings_source_id(ss(&user_id));
            g.set_copy_settings_source_label(label);
            g.set_copy_settings_confirm_focused(0);
            g.set_show_copy_settings_confirm(true);
        });
    }
    {
        let state_ok = Arc::clone(state);
        let ww = window.as_weak();
        AppState::get(window).on_copy_settings_confirmed(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let source_id = g.get_copy_settings_source_id().to_string();
            let label = g.get_copy_settings_source_label().to_string();
            let merged = {
                let mut s = state_ok.lock().unwrap();
                let Some(src) = s.config.profiles.iter().find(|p| p.user_id == source_id) else {
                    drop(s);
                    warn!("copy settings: source profile {source_id} is gone");
                    show_toast(
                        ww.clone(),
                        format!("Couldn't copy: {label} is no longer on this device"),
                    );
                    return;
                };
                let mut merged = s.config.active().clone();
                merged.copy_settings_from(src);
                // Not mirrored in a settings-* property, so read_settings_from_window (run by
                // settings-changed below) wouldn't carry it: write it to the config directly.
                s.config.active_mut().dashboard_rows = merged.dashboard_rows.clone();
                g.set_settings_font_family_desc(ss(&font_desc(
                    &s.system_fonts,
                    &merged.ui_font_family,
                )));
                merged
            };
            apply_profile_settings_to_window(&g, &merged);
            g.invoke_settings_changed();
            info!(
                "copy settings: copied the profile settings of {source_id} into the active profile"
            );
            show_toast(ww.clone(), format!("Settings copied from {label}"));
        });
    }
}

// ── wire_regions (moved from main(), 0.5.0 step 3) ───────────────────────
/// Wires streaming/discover region + display/discover language dropdowns:
/// streaming_region_selected, discover_region_selected, display_language_selected,
/// discover_language_selected.
pub(crate) fn wire_regions(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── streaming region selected callback ────────────────────────────────────────
    // A Seerr round trip: GET the user's general settings, change streamingRegion, POST the
    // whole object (a bare field would blank the rest, see UserGeneralSettings). AppState and
    // the resolve_streaming_region cache update only on success.
    {
        let state_sr = Arc::clone(&state);
        let ww_sr = window.as_weak();
        let rt_sr = rt.handle().clone();
        AppState::get(&window).on_streaming_region_selected(move |desc| {
            let (client, code) = {
                let s = state_sr.lock().unwrap();
                let Some(client) = s.seerr_client.clone() else {
                    return;
                };
                let Some((code, _)) = s
                    .seerr_regions
                    .iter()
                    .find(|(_, d)| d.as_str() == desc.as_str())
                else {
                    return;
                };
                (client, code.clone())
            };
            let state2 = Arc::clone(&state_sr);
            let ww2 = ww_sr.clone();
            rt_sr.spawn(async move {
                let result: anyhow::Result<()> = async {
                    let user = client.get_current_user().await?;
                    let mut settings = client.get_user_settings(user.id).await?;
                    settings.streaming_region = Some(code.clone());
                    client.update_user_settings(user.id, &settings).await
                }
                .await;
                match result {
                    Ok(()) => {
                        state2.lock().unwrap().seerr_streaming_region = Some(code);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_settings_streaming_region_desc(desc);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("settings: couldn't update streaming region: {e:#}");
                        show_toast(
                            ww2,
                            format!("Couldn't update streaming region — {}", short_error(&e)),
                        );
                    }
                }
            });
        });
    }

    // ── discover region selected callback ────────────────────────────────────────
    // Same GET-mutate-POST, for discoverRegion; reuses seerr_regions for the code lookup.
    {
        let state_dr = Arc::clone(&state);
        let ww_dr = window.as_weak();
        let rt_dr = rt.handle().clone();
        AppState::get(&window).on_discover_region_selected(move |desc| {
            let (client, code) = {
                let s = state_dr.lock().unwrap();
                let Some(client) = s.seerr_client.clone() else {
                    return;
                };
                let Some((code, _)) = s
                    .seerr_regions
                    .iter()
                    .find(|(_, d)| d.as_str() == desc.as_str())
                else {
                    return;
                };
                (client, code.clone())
            };
            let state2 = Arc::clone(&state_dr);
            let ww2 = ww_dr.clone();
            rt_dr.spawn(async move {
                let result: anyhow::Result<()> = async {
                    let user = client.get_current_user().await?;
                    let mut settings = client.get_user_settings(user.id).await?;
                    settings.discover_region = Some(code.clone());
                    client.update_user_settings(user.id, &settings).await
                }
                .await;
                match result {
                    Ok(()) => {
                        state2.lock().unwrap().seerr_discover_region = Some(code);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_settings_discover_region_desc(desc);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("settings: couldn't update discover region: {e:#}");
                        show_toast(
                            ww2,
                            format!("Couldn't update discover region — {}", short_error(&e)),
                        );
                    }
                }
            });
        });
    }

    // ── display language selected callback ─────────────────────────────────────
    // Same GET-mutate-POST shape as streaming region above. "Default
    // (English)" writes an empty locale — Seerr's own admin-configured
    // fallback applies server-side (see UserGeneralSettings' doc comment).
    {
        let state_dl = Arc::clone(&state);
        let ww_dl = window.as_weak();
        let rt_dl = rt.handle().clone();
        AppState::get(&window).on_display_language_selected(move |desc| {
            let (client, code) = {
                let s = state_dl.lock().unwrap();
                let Some(client) = s.seerr_client.clone() else {
                    return;
                };
                let code = if desc.as_str() == "Default (English)" {
                    String::new()
                } else {
                    let Some((code, _)) = s
                        .seerr_languages
                        .iter()
                        .find(|(_, d)| d.as_str() == desc.as_str())
                    else {
                        return;
                    };
                    code.clone()
                };
                (client, code)
            };
            let state2 = Arc::clone(&state_dl);
            let ww2 = ww_dl.clone();
            rt_dl.spawn(async move {
                let result: anyhow::Result<()> = async {
                    let user = client.get_current_user().await?;
                    let mut settings = client.get_user_settings(user.id).await?;
                    settings.locale = Some(code.clone());
                    client.update_user_settings(user.id, &settings).await
                }
                .await;
                match result {
                    Ok(()) => {
                        state2.lock().unwrap().seerr_locale = Some(code);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_settings_display_language_desc(desc);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("settings: couldn't update display language: {e:#}");
                        show_toast(
                            ww2,
                            format!("Couldn't update display language — {}", short_error(&e)),
                        );
                    }
                }
            });
        });
    }

    // ── discover language selected callback ─────────────────────────────────────
    // Same shape again. "Default (All Languages)" writes the literal
    // sentinel `"all"` — NOT an empty string, since Seerr's own
    // createTmdbWithRegionLanguage treats an empty originalLanguage as
    // "fall through to the server admin's own default," not "no filter"
    // (see spawn_seerr_settings_fetch's doc comment).
    {
        let state_dg = Arc::clone(&state);
        let ww_dg = window.as_weak();
        let rt_dg = rt.handle().clone();
        AppState::get(&window).on_discover_language_selected(move |desc| {
            let (client, code) = {
                let s = state_dg.lock().unwrap();
                let Some(client) = s.seerr_client.clone() else {
                    return;
                };
                let code = if desc.as_str() == "Default (All Languages)" {
                    "all".to_string()
                } else {
                    let Some((code, _)) = s
                        .seerr_languages
                        .iter()
                        .find(|(_, d)| d.as_str() == desc.as_str())
                    else {
                        return;
                    };
                    code.clone()
                };
                (client, code)
            };
            let state2 = Arc::clone(&state_dg);
            let ww2 = ww_dg.clone();
            rt_dg.spawn(async move {
                let result: anyhow::Result<()> = async {
                    let user = client.get_current_user().await?;
                    let mut settings = client.get_user_settings(user.id).await?;
                    settings.original_language = Some(code.clone());
                    client.update_user_settings(user.id, &settings).await
                }
                .await;
                match result {
                    Ok(()) => {
                        state2.lock().unwrap().seerr_original_language = Some(code);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_settings_discover_language_desc(desc);
                            }
                        });
                    }
                    Err(e) => {
                        warn!("settings: couldn't update discover language: {e:#}");
                        show_toast(
                            ww2,
                            format!("Couldn't update discover language — {}", short_error(&e)),
                        );
                    }
                }
            });
        });
    }
}

// ── wire_settings_changed (moved from main(), 0.5.0 step 3) ──────────────
/// Wires settings-changed, dropdown mouse pick, settings row focus: settings_changed,
/// dropdown_pick, profile_edit_dropdown_pick, settings_row_focused.
pub(crate) fn wire_settings_changed(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    video: &std::sync::Arc<std::sync::Mutex<crate::playback::VideoState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    let video = std::sync::Arc::clone(video);
    // ── settings changed ──────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let video = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_settings_changed(move || {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            let mut s = state.lock().unwrap();
            // Diagnostics (2026-10-08, a Slint panic right after a settings
            // change on the HTPC): which settings this change touched.
            let before = settings_snapshot(&s.config);
            read_settings_from_window(&w, &mut s);
            let changed = settings_diff(&before, &settings_snapshot(&s.config));
            debug!(
                "settings changed: {}",
                if changed.is_empty() {
                    "nothing".to_string()
                } else {
                    changed.join(", ")
                }
            );
            // Live-apply the seerr-enabled toggle: build_seerr_client returns None when disabled,
            // so this tears the client down or rebuilds it from the saved credentials (no
            // reconnect), and pushing seerr-connected/-label hides or shows every Seerr-gated row
            // right away.
            s.seerr_client = seerr_auth::build_seerr_client(s.config.active());
            seerr_auth::push_seerr_status(&AppState::get(&w), s.config.active());
            let launch_fs = s.config.device.launch_fullscreen;
            let irq_enable = s.config.device.audio_spdif
                && s.config.device.alsa_irq_scheduling
                && pipewire_fix::is_pipewire_device(
                    if s.config.device.audio_device_passthrough.is_empty() {
                        &s.config.device.audio_device
                    } else {
                        &s.config.device.audio_device_passthrough
                    },
                );
            // Subtitle appearance applies live to a currently-playing video —
            // no restart needed, mirrors the existing sub-delay/audio-delay
            // live-adjust UX. See fjord-player's Player::set_sub_style.
            let sub_scale = s.config.active().sub_scale_pct as f64 / 100.0;
            let sub_pos = s.config.active().sub_pos_pct as i64;
            let sub_respect_ass = s.config.active().sub_respect_ass_styling;
            let sub_color = sub_color_hex(&s.config.active().sub_color).to_string();
            let sub_background_pct = s.config.active().sub_background_pct;
            let cfg = s.config.clone();
            drop(s);
            save_config(&cfg);
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                p.set_sub_style(
                    sub_scale,
                    sub_pos,
                    sub_respect_ass,
                    &sub_color,
                    sub_background_pct,
                );
            }
            w.window().set_fullscreen(launch_fs);
            rt_handle.spawn_blocking(move || pipewire_fix::apply_alsa_irq_scheduling(irq_enable));
            info!("settings saved");
        });
    }

    // ── keyboard dropdown: mouse pick on overlay ─────────────────────────────
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_dropdown_pick(move || {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            let g = AppState::get(&w);
            let sf = g.get_settings_focused();
            let cursor = g.get_settings_dropdown_cursor();
            crate::settings::apply_dropdown_selection(sf.as_str(), cursor, &g);
            g.set_settings_dropdown_open(false);
        });
        // ProfileEditScreen's own screen-local dropdown overlay (Max
        // parental rating / Auto-lock) — same mouse-pick shape as the
        // Settings one right above.
        let window_weak2 = window.as_weak();
        AppState::get(&window).on_profile_edit_dropdown_pick(move || {
            let Some(w) = window_weak2.upgrade() else {
                return;
            };
            let g = AppState::get(&w);
            let cursor = g.get_profile_edit_dropdown_cursor();
            crate::profile_edit::apply_profile_edit_dropdown_selection(&g, cursor);
            g.set_profile_edit_dropdown_open(false);
        });
    }

    // ── settings row focused (mouse click on a SettingsRow) ──────────────────
    // Routes through the same set_focused pairing dispatch_settings's own
    // keyboard nav uses, so settings-focused-visual-index (settings.slint's
    // scroll-to-view) never goes stale after a mouse click.
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_settings_row_focused(move |key| {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            let g = AppState::get(&w);
            crate::settings::row_focused(&g, key.as_str());
        });
    }
}

/// Pushes a profile's own settings — the rows tagged "Profile", exactly the fields
/// `ProfileSettings::copy_settings_from` copies — to the window. Used by apply_settings_to_window
/// and by "Copy settings from…" (which then invokes settings-changed to save and live-apply).
pub(crate) fn apply_profile_settings_to_window(
    g: &crate::AppState<'_>,
    cp: &config::ProfileSettings,
) {
    for b in PROFILE_BINDS {
        (b.load)(cp, g);
    }
    crate::dashboard_rows::push_row_orders(g, cp);
}

/// The Text font dropdown's label for `family` — system_fonts (fc-list, fetched in the
/// background) may still be empty at launch, so the common cases have their own labels.
fn font_desc(system_fonts: &[(String, String)], family: &str) -> String {
    match system_fonts.iter().find(|(v, _)| v == family) {
        Some((_, d)) => d.clone(),
        None if family == "Inter" => "Inter (Fjord default)".into(),
        None if family.is_empty() => "System default".into(),
        None => family.to_string(),
    }
}

pub(crate) fn apply_settings_to_window(w: &MainWindow, s: &FjordState) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    let g = AppState::get(w);
    let c = &s.config.device;
    let cp = s.config.active();
    for b in DEVICE_BINDS {
        (b.load)(c, &g);
    }
    let dev_desc = s
        .audio_devices
        .iter()
        .find(|(n, _)| n == &c.audio_device)
        .map(|(_, d)| d.as_str())
        .unwrap_or(if c.audio_device.is_empty() {
            ""
        } else {
            c.audio_device.as_str()
        })
        .to_string();
    g.set_settings_audio_device_desc(ss(&dev_desc));
    let pt_desc = s
        .audio_devices
        .iter()
        .find(|(n, _)| n == &c.audio_device_passthrough)
        .map(|(_, d)| d.as_str())
        .unwrap_or(if c.audio_device_passthrough.is_empty() {
            ""
        } else {
            c.audio_device_passthrough.as_str()
        })
        .to_string();
    g.set_settings_passthrough_device_desc(ss(&pt_desc));
    // The IRQ fix targets the device passthrough actually plays on.
    let effective = if c.audio_device_passthrough.is_empty() {
        &c.audio_device
    } else {
        &c.audio_device_passthrough
    };
    g.set_settings_device_is_pipewire(pipewire_fix::is_pipewire_device(effective));
    g.set_settings_audio_channels(ss(if c.audio_channels.is_empty() {
        "auto-safe"
    } else {
        &c.audio_channels
    }));
    // The settings tagged "Profile" (also what "Copy settings from…" pushes) — the text font
    // among them, so MainWindow.font-family is right from the first frame.
    apply_profile_settings_to_window(&g, cp);
    g.set_settings_font_family_desc(ss(&font_desc(&s.system_fonts, &cp.ui_font_family)));
    profile::refresh_profile_settings_dropdown(&g, &s.config);
    profile::refresh_account_settings_dropdown(&g, &s.config);
    // is_true_master, not !is_bonfire: a session impersonating a foreign group account has
    // is_bonfire set but is fully privileged for that account (Manage Profiles / Bonfire Group).
    g.set_settings_is_master_profile(profile::is_true_master(s.config.active()));
    {
        let root_id = profile::account_root_id(s.config.active()).to_string();
        let remember = s
            .config
            .profiles
            .iter()
            .find(|p| p.user_id == root_id)
            .is_none_or(|p| p.remember_login); // no matching entry shouldn't happen; default to the field's own true
        g.set_settings_remember_login(remember);
    }
    g.set_settings_seerr_enabled(cp.seerr_enabled);
    seerr_auth::push_seerr_status(&g, cp);
}

/// The device settings and the active profile's settings as JSON objects,
/// for settings_diff.
pub(crate) fn settings_snapshot(c: &config::Config) -> [serde_json::Value; 2] {
    [
        serde_json::to_value(&c.device).unwrap_or_default(),
        serde_json::to_value(c.active()).unwrap_or_default(),
    ]
}

/// Field names that differ between two settings_snapshot()s — with old → new
/// for switches and numbers; text fields by name only (they can hold
/// credentials).
pub(crate) fn settings_diff(
    before: &[serde_json::Value; 2],
    after: &[serde_json::Value; 2],
) -> Vec<String> {
    use serde_json::Value;
    let mut out = Vec::new();
    for (b, a) in before.iter().zip(after) {
        let (Some(b), Some(a)) = (b.as_object(), a.as_object()) else {
            continue;
        };
        for (key, new) in a {
            let old = b.get(key).unwrap_or(&Value::Null);
            if old == new {
                continue;
            }
            out.push(match (old, new) {
                (Value::Bool(_) | Value::Number(_), Value::Bool(_) | Value::Number(_)) => {
                    format!("{key}: {old} → {new}")
                }
                _ => format!("{key} (changed)"),
            });
        }
    }
    out
}

pub(crate) fn read_settings_from_window(w: &MainWindow, s: &mut FjordState) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    let g = AppState::get(w);
    let c = &mut s.config.device;
    for b in DEVICE_BINDS {
        (b.store)(c, &g);
    }
    c.audio_channels = g.get_settings_audio_channels().to_string();

    let cp = s.config.active_mut();
    for b in PROFILE_BINDS {
        (b.store)(cp, &g);
    }
    cp.seerr_enabled = g.get_settings_seerr_enabled();
}

// ── audio device discovery ────────────────────────────────────────────────────

pub(crate) fn fetch_audio_devices() -> Vec<(String, String)> {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    let out = std::process::Command::new("mpv")
        .args(["--no-config", "--audio-device=help"])
        .output();
    let Ok(out) = out else {
        return vec![("auto".into(), "Autoselect device".into())];
    };
    let raw = String::from_utf8_lossy(&out.stdout);
    let text = if raw.trim().is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        raw.into_owned()
    };
    let mut devices = vec![("auto".into(), "Autoselect device".into())];
    for line in text.lines() {
        let line = line.trim();
        if !line.starts_with('\'') {
            continue;
        }
        let Some(end_q) = line[1..].find('\'') else {
            continue;
        };
        let name = line[1..end_q + 1].to_string();
        if name == "auto" {
            continue;
        }
        let rest = line[end_q + 2..].trim();
        let desc = if rest.starts_with('(') && rest.ends_with(')') {
            rest[1..rest.len() - 1].to_string()
        } else {
            name.clone()
        };
        devices.push((name, desc));
    }
    // Settings selects audio devices by description string, so duplicates made the second
    // entry unselectable. The same device can appear under two backends with one description
    // (pipewire/… and pulse/…): suffix duplicates with the backend. Config stores the device
    // NAME, so this is display-only.
    let mut counts: HashMap<String, usize> = HashMap::new();
    for (_, desc) in &devices {
        *counts.entry(desc.clone()).or_insert(0) += 1;
    }
    for (name, desc) in devices.iter_mut() {
        if counts.get(desc.as_str()).copied().unwrap_or(0) > 1 {
            let backend = name.split('/').next().unwrap_or(name.as_str());
            *desc = format!("{desc} [{backend}]");
        }
    }
    // Still colliding after the backend suffix (two devices with one description under the
    // same backend): fall back to the raw device name, which is unique.
    let mut counts2: HashMap<String, usize> = HashMap::new();
    for (_, desc) in &devices {
        *counts2.entry(desc.clone()).or_insert(0) += 1;
    }
    for (name, desc) in devices.iter_mut() {
        if counts2.get(desc.as_str()).copied().unwrap_or(0) > 1 {
            *desc = format!("{desc} ({name})");
        }
    }
    devices
}

// ── system font discovery ─────────────────────────────────────────────────────
// Same pattern as fetch_audio_devices above, but via fc-list instead of mpv.
// Returns (value, display) pairs: "Inter" (Fjord's bundled default) and ""
// (system default, no font-family override) are pinned first, followed by
// every distinct font family installed on the host, alphabetically. A family
// with multiple locale/weight aliases on one fc-list line ("Noto Sans
// Malayalam,Noto Sans Malayalam Light") only keeps the first — the rest are
// just alternate names for the same family, not separate fonts.
pub(crate) fn fetch_system_fonts() -> Vec<(String, String)> {
    let out = std::process::Command::new("fc-list")
        .args([":", "family"])
        .output();
    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    if let Ok(out) = out {
        let text = String::from_utf8_lossy(&out.stdout);
        for line in text.lines() {
            if let Some(name) = line.split(',').next() {
                let name = name.trim();
                if !name.is_empty() {
                    names.insert(name.to_string());
                }
            }
        }
    }
    let mut fonts = vec![
        ("Inter".to_string(), "Inter (Fjord default)".to_string()),
        (String::new(), "System default".to_string()),
    ];
    fonts.extend(names.into_iter().map(|n| (n.clone(), n)));
    fonts
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every field different from its default (bools flipped, text "t-<field>", numbers moved by a
    /// small per-field step); other kinds left alone.
    fn changed<T: serde::Serialize + serde::de::DeserializeOwned + Default>()
    -> (T, serde_json::Value) {
        let mut v = serde_json::to_value(T::default()).unwrap();
        for (i, (k, x)) in v.as_object_mut().unwrap().iter_mut().enumerate() {
            let new = match &*x {
                serde_json::Value::Bool(b) => (!b).into(),
                serde_json::Value::String(_) => format!("t-{k}").into(),
                // Small steps: some fields are u8.
                serde_json::Value::Number(n) if n.is_u64() => {
                    let n = n.as_u64().unwrap();
                    let step = 1 + i as u64 % 50;
                    (if n < 200 { n + step } else { n - step }).into()
                }
                other => other.clone(),
            };
            *x = new;
        }
        (serde_json::from_value(v.clone()).unwrap(), v)
    }

    /// Fields of `out` that came back as in `sent`; panics on one that is neither sent nor default
    /// (a bind writing the wrong field).
    fn round_tripped(
        out: serde_json::Value,
        sent: &serde_json::Value,
        default: serde_json::Value,
    ) -> usize {
        let mut n = 0;
        for (k, v) in out.as_object().unwrap() {
            if v == &sent[k] && v != &default[k] {
                n += 1;
            } else {
                assert_eq!(v, &default[k], "{k} came back as something else");
            }
        }
        n
    }

    #[test]
    fn glow_levels_at_a_3px_ring() {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::MainWindow::new().unwrap();
        let g = crate::AppState::get(&window);
        g.set_settings_focus_width("3".into());
        g.set_settings_focus_style("glow".into());
        assert_eq!(g.get_settings_focus_glow().as_str(), "soft", "default");
        for (level, layers) in [
            ("faint", [6.0, 0.0, 0.0]),
            ("very-soft", [10.0, 0.0, 0.0]),
            ("subtle", [13.0, 0.0, 0.0]),
            ("soft", [17.0, 0.0, 0.0]),
            ("medium", [8.0, 24.0, 0.0]),
            ("strong", [6.0, 16.0, 32.0]),
            ("very-strong", [9.0, 24.0, 48.0]),
            ("bogus", [17.0, 0.0, 0.0]),
        ] {
            g.set_settings_focus_glow(level.into());
            assert_eq!(
                [g.get_glow_1(), g.get_glow_2(), g.get_glow_3()],
                layers,
                "{level}"
            );
        }
        g.set_settings_focus_glow("faint".into());
        assert!(g.get_glow_color().alpha() < g.get_focus_color().alpha());
        g.set_settings_focus_glow("soft".into());
        assert_eq!(g.get_glow_color(), g.get_focus_color());
    }

    #[test]
    fn settings_round_trip_through_the_window() {
        i_slint_backend_testing::init_no_event_loop();
        let window = crate::MainWindow::new().unwrap();
        let g = crate::AppState::get(&window);

        let (dev, dev_json): (config::DeviceConfig, _) = changed();
        for b in DEVICE_BINDS {
            (b.load)(&dev, &g);
        }
        let mut dev_out = config::DeviceConfig::default();
        for b in DEVICE_BINDS {
            (b.store)(&mut dev_out, &g);
        }
        let dev_default = serde_json::to_value(config::DeviceConfig::default()).unwrap();
        let n = round_tripped(
            serde_json::to_value(dev_out).unwrap(),
            &dev_json,
            dev_default,
        );
        assert_eq!(
            n,
            DEVICE_BINDS.len(),
            "every device bind round-trips its own field"
        );

        let (prof, prof_json): (config::ProfileSettings, _) = changed();
        for b in PROFILE_BINDS {
            (b.load)(&prof, &g);
        }
        let mut prof_out = config::ProfileSettings::default();
        for b in PROFILE_BINDS {
            (b.store)(&mut prof_out, &g);
        }
        let prof_default = serde_json::to_value(config::ProfileSettings::default()).unwrap();
        let n = round_tripped(
            serde_json::to_value(&prof_out).unwrap(),
            &prof_json,
            prof_default,
        );
        assert_eq!(
            n,
            PROFILE_BINDS.len(),
            "every profile bind round-trips its own field"
        );

        // Copy settings from… copies exactly the same fields.
        let mut copy = config::ProfileSettings::default();
        copy.copy_settings_from(&prof);
        let prof_default = serde_json::to_value(config::ProfileSettings::default()).unwrap();
        let copied = round_tripped(
            serde_json::to_value(&copy).unwrap(),
            &prof_json,
            prof_default,
        );
        assert_eq!(
            copied,
            PROFILE_BINDS.len(),
            "copy_settings_from copies the profile binds"
        );
    }

    #[test]
    fn choices_are_well_formed() {
        let mut keys = HashSet::new();
        for c in CHOICES {
            assert!(keys.insert(c.key), "{} is in CHOICES twice", c.key);
            assert!(!c.options.is_empty(), "{} has no options", c.key);
            let values: HashSet<_> = c.options.iter().map(|(v, _)| v).collect();
            let labels: HashSet<_> = c.options.iter().map(|(_, l)| l).collect();
            assert_eq!(values.len(), c.options.len(), "{}: duplicate value", c.key);
            // A mouse pick is matched by its label.
            assert_eq!(labels.len(), c.options.len(), "{}: duplicate label", c.key);
            if let Field::Int(..) = c.field {
                for (v, _) in c.options {
                    assert!(v.parse::<i32>().is_ok(), "{}: {v:?} isn't a number", c.key);
                }
            }
            assert!(
                !is_dynamic_dropdown(c.key),
                "{} is also a dynamic dropdown",
                c.key
            );
        }
    }

    #[test]
    fn section_rows_match_settings_slint() {
        // Each section's block in settings.slint lists the same rows in the same order as
        // SECTION_ROWS (rows are `key: "…"` on a SettingsRow, or `settings-focused == "…"` on the
        // custom ones in Profiles/Integrations) — Up/Down would otherwise skip or misorder rows.
        let slint = include_str!("../ui/settings.slint");
        let marker = "AppState.settings-section == \"";
        for (section, keys) in SECTION_ROWS {
            let start = slint
                .find(&format!("{marker}{section}\""))
                .unwrap_or_else(|| panic!("no block for section {section}"));
            let rest = &slint[start + marker.len()..];
            let block = &rest[..rest.find(marker).unwrap_or(rest.len())];
            let mut found: Vec<&str> = Vec::new();
            let mut i = 0;
            while i < block.len() {
                let next_key = block[i..].find("key: \"").map(|p| (p, "key: \""));
                let next_focus = block[i..]
                    .find("settings-focused == \"")
                    .map(|p| (p, "settings-focused == \""));
                let Some((p, pat)) = [next_key, next_focus].into_iter().flatten().min() else {
                    break;
                };
                let from = i + p + pat.len();
                let end = from + block[from..].find('"').unwrap();
                let k = &block[from..end];
                if !found.contains(&k) {
                    found.push(k);
                }
                i = end;
            }
            assert_eq!(
                &found, keys,
                "section {section}: settings.slint rows differ"
            );
        }
    }

    #[test]
    fn every_choice_has_its_settings_row() {
        // The Slint row must say `choice: true` right after its key, or it draws no dropdown.
        let slint = include_str!("../ui/settings.slint");
        for c in CHOICES {
            let row = format!("key: \"{}\";", c.key);
            let at = slint
                .find(&row)
                .unwrap_or_else(|| panic!("no Settings row for {}", c.key));
            let after = slint[at + row.len()..].trim_start();
            assert!(
                after.starts_with("choice: true;"),
                "{}'s row isn't a choice row",
                c.key
            );
        }
    }
}
