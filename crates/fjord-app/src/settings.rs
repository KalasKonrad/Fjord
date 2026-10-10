// ── fjord-app · settings.rs ───────────────────────────────────────────────────
//   Design (Phase 0, 2026-08-07, Bonfire prep — full data-driven rewrite,
//   zero intended behavior change)  Every section and every row now has a
//   stable string KEY ("general.launch_fullscreen", "video", ...) instead of
//   a positional int. `AppState.settings-section`/`settings-focused` are now
//   `string` (see app_state.slint) — "" is the sentinel for "nothing
//   selected" (was -1). Row *existence* for a section is computed by
//   `section_row_keys()`, one function per section that pushes each row's
//   key only when its visibility condition holds — this REPLACES the old
//   ~15 hand-duplicated skip-blocks that used to live inside dispatch_settings's
//   Up AND Down arms separately (two copies of every condition, easy to let
//   drift). Up/Down now just step through whatever section_row_keys()
//   returns; Confirm/Right resolve the row directly via key match. A row
//   being hidden is controlled ONLY by settings.slint's own `if` condition on
//   the Slint side and ONLY by section_row_keys()'s matching condition on the
//   Rust side — these two are still independently maintained (Slint can't
//   call Rust to ask "is this visible", and Rust has no visibility into
//   Slint's render tree), so a row's condition must still be kept in sync by
//   hand between the two files, same as before — but now there is exactly
//   ONE place per direction, not two, and the row's IDENTITY no longer
//   depends on getting that in sync (a row with a wrong/missing visibility
//   condition just doesn't appear or doesn't get skipped — it can no longer
//   silently push every LATER row's numbering off by one).
//   Sections     SECTION_GENERAL, SECTION_PROFILES (Sign Out moved here from
//                General; Bonfire Phase 1 step 7 added the launch-policy
//                rows — profiles.launch_policy + the virtual
//                profiles.default_profile dynamic dropdown; 2026-08-14's 2-tier
//                account/profile redesign added the identical pair one tier up —
//                profiles.account_launch_policy + the virtual profiles.default_account
//                — plus profiles.add_account, a plain button row: always available
//                regardless of how many accounts exist, since the account picker's own
//                "+ Add Account" tile only ever shows once 2+ accounts already do),
//                SECTION_VIDEO, SECTION_AUDIO,
//                SECTION_PLAYER_CFG, SECTION_KEYBINDINGS, SECTION_UI,
//                SECTION_INTEGRATIONS — ALL_SECTIONS is the sidebar order.
//   Row keys     see the const block below, grouped by section; each key is
//                "<section>.<field>" except the few bespoke action/button
//                rows (Sign Out, Connect/Disconnect, Manage Blocklist,
//                Prewarm ×2), which follow the same naming shape.
//   section_row_keys(section, g) -> Vec<&'static str>   ordered, visible-only
//                row list for a section — the single source of truth for
//                Up/Down bounds and stepping.
//   dispatch_settings   keyboard nav (three-state: sidebar → left pane →
//                right pane / keybindings; Enter opens dropdown popup;
//                Up/Down/Enter/Esc navigate popup).
//   dropdown_model(key)         static model strings for a row (None for
//                toggle/button/action rows and for the 12 dynamic-dropdown
//                rows, whose list comes from an AppState property instead).
//   is_dynamic_dropdown(key)    the 12 rows whose list/current-desc are
//                fetched at runtime (Default Profile/Default Account — built
//                from Config.profiles itself, not a network fetch — audio
//                device ×2, font family, Seerr streaming region/display
//                language/discover language/discover region, and display_sync's
//                VID_SEPARATE_VIDEO_SURFACE (HDR Stage 5) is only listed on Wayland (is-wayland);
//                VID_OWN_BUFFERS (10-bit video plane, 2026-10-08) under it while it's on;
//                VID_DITHER_OFF (test aid, 2026-10-08) always, right after them
//                own output/default-resolution/default-hz ×3 — kscreen-doctor,
//                re-fetched whenever the selected output changes) rather than
//                a fixed compile-time list.
//   open_dropdown_popup(key, g)   populates settings-dropdown-model/-cursor/
//                -display and opens the popup — used by both keyboard
//                Confirm and mouse click-to-open (main.rs wires the latter
//                through the same call via on_dropdown_pick/settings-row
//                click paths that already resolve to Confirm-equivalent).
//   current_value_str / display_val / apply_dropdown_selection   same shape
//                as before, re-keyed from (section, row) to a flat key.
//   settings_row_action(key, g)   per-row Confirm/Right activation — toggle
//                flip, dropdown cycle-forward-by-one, or button/action
//                invoke. `forward` was dropped: the old parameter was always
//                called with `true` from both of its two call sites, so the
//                backward branch was dead code — confirmed by reading every
//                call site before removing it.
//   wire_device_lists      callbacks moved from main() (0.5.0 step 3): audio/passthrough device and font lists (fetched once) + their dropdowns
//   wire_profile_defaults  callbacks moved from main() (0.5.0 step 3): default profile / account dropdowns
//   wire_regions           callbacks moved from main() (0.5.0 step 3): streaming/discover region + display/discover language dropdowns
//   wire_settings_changed  callbacks moved from main() (0.5.0 step 3): settings-changed, dropdown mouse pick, settings row focus
//   settings helpers     apply_settings_to_window ↔ read_settings_from_window;
//                        settings_snapshot/settings_diff — the settings-changed handler logs
//                        which settings changed (debug; text values by name only)
//   fetch_audio_devices / fetch_system_fonts  startup fetches for the audio-device and font dropdowns
// ─────────────────────────────────────────────────────────────────────────────

use crate::MainWindow;
use crate::config::{self, FjordState};
use crate::keys::Action;
use slint::{Model, ModelRc, SharedString, VecModel};
use tracing::debug;

// ── Sections (sidebar order) ────────────────────────────────────────────────
pub(crate) const SECTION_GENERAL: &str = "general";
pub(crate) const SECTION_PROFILES: &str = "profiles";
pub(crate) const SECTION_VIDEO: &str = "video";
pub(crate) const SECTION_AUDIO: &str = "audio";
pub(crate) const SECTION_PLAYER_CFG: &str = "player";
pub(crate) const SECTION_KEYBINDINGS: &str = "keybindings";
pub(crate) const SECTION_UI: &str = "ui";
pub(crate) const SECTION_INTEGRATIONS: &str = "integrations";

const ALL_SECTIONS: &[&str] = &[
    SECTION_GENERAL,
    SECTION_PROFILES,
    SECTION_VIDEO,
    SECTION_AUDIO,
    SECTION_PLAYER_CFG,
    SECTION_KEYBINDINGS,
    SECTION_UI,
    SECTION_INTEGRATIONS,
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
// 2026-08-14, the 2-tier account/profile redesign — the identical
// launch-policy shape one tier up, appended after the existing
// profile-level rows (not inserted before them) per this codebase's own
// "append, don't insert" convention for exactly this reason.
const PROF_ACCOUNT_LAUNCH_POLICY: &str = "profiles.account_launch_policy";
const PROF_DEFAULT_ACCOUNT: &str = "profiles.default_account"; // virtual — only when account_launch_policy == "default"
// Always visible, regardless of how many accounts already exist — the
// picker's own "+ Add Account" tile only shows once there's a 2nd one to
// switch between; this is the actual way to go from 1 to 2 in the first
// place.
const PROF_ADD_ACCOUNT: &str = "profiles.add_account";
// Live-questioned 2026-08-17 ("no why to change this on the accaunt
// without sinign out and in again") — see app_state.slint's own doc
// comment on settings-remember-login for the full design. A toggle, not a
// dropdown: OFF is immediate, ON opens the confirm-password modal instead
// of flipping directly (handled entirely in profile::on_remember_login_toggle,
// not the generic toggle-row shape most other bool rows use).
const PROF_REMEMBER_LOGIN: &str = "profiles.remember_login";
const PROF_SIGN_OUT: &str = "profiles.sign_out";
// Bonfire Phase 5 (cross-household groups, 2026-08-29) — appended at the
// end of the section, not sandwiched next to Manage Profiles, matching
// this section's own "append, don't insert" precedent (see
// PROF_ACCOUNT_LAUNCH_POLICY's comment above). Same gate as Manage
// Profiles (settings-is-master-profile — now correctly true while
// impersonating a foreign group account too, see profile.rs::is_true_master).
const PROF_BONFIRE_GROUP: &str = "profiles.bonfire_group";
// Bonfire Phase 6 (admin actions, 2026-09-04) — gated on
// jellyfin-is-server-admin, NOT settings-is-master-profile: the
// mappings/reset-pin/set-limit/audit-log endpoints all authorize against
// Jellyfin's own core Policy.IsAdministrator, confirmed against the real
// plugin controller source — a Bonfire household master with no server
// admin rights should never see this row, and a genuine server admin who
// happens to run no Bonfire household of their own still should.
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

// display_sync (2026-09-18) — native resolution/refresh-rate/HDR/WCG matched
// to source. See CLAUDE.md's own display_sync section and display_sync.rs's
// module doc comment for the full design. Every row below the master toggle
// is virtual (hidden while the toggle is off) — see section_row_keys' own
// SECTION_VIDEO arm.
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
const AUD_NOW_PLAYING_AUTO_OPEN: &str = "audio.now_playing_auto_open";

// ── Player (config) section rows ──────────────────────────────────────────────
const PLY_SUB_ENABLED: &str = "player.sub_enabled";
const PLY_SUB_LANG: &str = "player.sub_lang";
const PLY_SUB_LANG2: &str = "player.sub_lang2";
const PLY_SUB_TYPE: &str = "player.sub_type";
const PLY_SUB_SCALE: &str = "player.sub_scale";
const PLY_SUB_POS: &str = "player.sub_pos";
const PLY_SUB_RESPECT_ASS: &str = "player.sub_respect_ass";
const PLY_SUB_COLOR: &str = "player.sub_color"; // virtual — only when !sub_respect_ass_styling
const PLY_SUB_BACKGROUND: &str = "player.sub_background"; // virtual — only when !sub_respect_ass_styling
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

// ── UI section rows ───────────────────────────────────────────────────────────
const UI_SCROLL_SPEED: &str = "ui.scroll_speed";
const UI_ANIMATION_SPEED: &str = "ui.animation_speed";
const UI_FONT_FAMILY: &str = "ui.font_family";
const UI_ONSCREEN_KEYBOARD: &str = "ui.onscreen_keyboard"; // 2026-08-27, default on

// ── Integrations section rows ─────────────────────────────────────────────────
const INT_SEERR_ENABLED: &str = "integrations.seerr_enabled";
const INT_SEERR_CONNECT: &str = "integrations.seerr_connect"; // "Connect Seerr" / "Disconnect"
const INT_STREAMING_REGION: &str = "integrations.streaming_region";
const INT_TRAILER_QUALITY: &str = "integrations.trailer_quality";
const INT_DISPLAY_LANGUAGE: &str = "integrations.display_language";
const INT_DISCOVER_LANGUAGE: &str = "integrations.discover_language";
const INT_DISCOVER_REGION: &str = "integrations.discover_region";
const INT_MANAGE_BLOCKLIST: &str = "integrations.manage_blocklist";

// ── Row existence per section (replaces the old duplicated skip-logic) ────────

fn section_row_keys(section: &str, g: &crate::AppState<'_>) -> Vec<&'static str> {
    match section {
        SECTION_GENERAL => vec![
            GEN_LAUNCH_FULLSCREEN,
            GEN_VIDEO_BEHIND,
            GEN_LOG_LEVEL,
            GEN_PREWARM_METADATA,
            GEN_PREWARM_IMAGES,
        ],
        SECTION_PROFILES => {
            let mut rows = vec![PROF_LAUNCH_POLICY];
            if g.get_settings_launch_policy().as_str() == "default" {
                rows.push(PROF_DEFAULT_PROFILE);
            }
            if g.get_settings_is_master_profile() {
                rows.push(PROF_MANAGE_PROFILES);
            }
            rows.push(PROF_ACCOUNT_LAUNCH_POLICY);
            if g.get_settings_account_launch_policy().as_str() == "default" {
                rows.push(PROF_DEFAULT_ACCOUNT);
            }
            rows.push(PROF_ADD_ACCOUNT);
            rows.push(PROF_REMEMBER_LOGIN);
            rows.push(PROF_SIGN_OUT);
            if g.get_settings_is_master_profile() {
                rows.push(PROF_BONFIRE_GROUP);
            }
            if g.get_jellyfin_is_server_admin() {
                rows.push(PROF_BONFIRE_ADMIN);
            }
            rows
        }
        SECTION_VIDEO => {
            let mut rows = vec![VID_HWDEC];
            // vf exists solely to fix NVDEC's own stride-corruption bug (see
            // CLAUDE.md's "NVIDIA legacy Wayland: NVDEC stride corruption")
            // — with any other decoder there's no stride mismatch for it to
            // correct, so it does nothing useful. `auto` is deliberately
            // treated as "not committed to NVDEC" and hidden too (2026-08-08,
            // direct user choice) rather than shown just in case it resolves
            // to nvdec at runtime.
            if matches!(g.get_settings_hwdec().as_str(), "nvdec" | "nvdec-copy") {
                rows.push(VID_VF);
            }
            rows.extend([VID_DEINTERLACE, VID_VIDEO_SYNC, VID_INTERPOLATION]);
            if g.get_settings_interpolation() {
                rows.push(VID_TSCALE);
            }
            rows.push(VID_TARGET_COLORSPACE);
            // HDR Stage 5 (2026-10-05): only on Wayland, where it does anything.
            if g.get_is_wayland() {
                rows.push(VID_SEPARATE_VIDEO_SURFACE);
                // 10-bit video plane (2026-10-08), indented under it.
                if g.get_settings_separate_video_surface() {
                    rows.push(VID_OWN_BUFFERS);
                }
            }
            rows.push(VID_DITHER_OFF);
            // Always visible now (2026-08-15, live-reported: "the tonemap setting shuld
            // not be gateded byt the hdr hint setting") — was hidden whenever HDR
            // passthrough was on, on the assumption tone-mapping never runs in that
            // state. That assumption is wrong: mpv's own target-colorspace-hint-strict
            // (default on) falls back to tone-mapping whenever the compositor doesn't
            // actually accept the hint, using this exact stored value (fjord-player's
            // Player::new sets --tone-mapping and --target-colorspace-hint as two fully
            // independent mpv options, never gated on each other) — so hiding the row
            // left no way to pick which curve backs that fallback.
            rows.push(VID_TONE_MAPPING);
            rows.push(VID_OPENGL_EARLY_FLUSH);
            if g.get_settings_video_sync().as_str() == "display-resample" {
                rows.push(VID_VIDEO_LATENCY_HACKS);
            }
            rows.push(VID_DISPLAY_SYNC_ENABLED);
            if g.get_settings_display_sync_enabled() {
                rows.push(VID_DISPLAY_SYNC_SCREEN);
                rows.push(VID_DISPLAY_SYNC_SYNC_RESOLUTION);
                rows.push(VID_DISPLAY_SYNC_SYNC_REFRESH_RATE);
                rows.push(VID_DISPLAY_SYNC_DEFAULT_RESOLUTION);
                rows.push(VID_DISPLAY_SYNC_DEFAULT_HZ);
                rows.push(VID_DISPLAY_SYNC_SCALE_4K);
                rows.push(VID_DISPLAY_SYNC_SCALE_1080P);
                if g.get_settings_display_sync_sync_resolution() {
                    rows.push(VID_DISPLAY_SYNC_4K_ODD_FPS_MODE);
                }
                rows.push(VID_DISPLAY_SYNC_HDR_MODE);
                rows.push(VID_DISPLAY_SYNC_WCG_MODE);
                rows.push(VID_DISPLAY_SYNC_TRAILERS);
            }
            rows
        }
        SECTION_AUDIO => {
            let mut rows = vec![AUD_AUDIO_DEVICE, AUD_CHANNELS, AUD_SPDIF];
            if g.get_settings_audio_spdif() {
                rows.extend([
                    AUD_SPDIF_AC3,
                    AUD_SPDIF_EAC3,
                    AUD_SPDIF_DTS,
                    AUD_SPDIF_DTS_HD,
                    AUD_SPDIF_TRUEHD,
                    AUD_PASSTHROUGH_DEVICE,
                ]);
                if g.get_settings_device_is_pipewire() {
                    rows.push(AUD_ALSA_IRQ);
                }
                rows.push(AUD_SKIP_FADE_MUTE);
            }
            rows.extend([AUD_AUDIO_LANG, AUD_GAPLESS, AUD_NOW_PLAYING_AUTO_OPEN]);
            rows
        }
        SECTION_PLAYER_CFG => {
            let mut rows = vec![PLY_SUB_ENABLED];
            if g.get_settings_sub_enabled() {
                rows.extend([
                    PLY_SUB_LANG,
                    PLY_SUB_LANG2,
                    PLY_SUB_TYPE,
                    PLY_SUB_SCALE,
                    PLY_SUB_POS,
                    PLY_SUB_RESPECT_ASS,
                ]);
                if !g.get_settings_sub_respect_ass_styling() {
                    rows.extend([PLY_SUB_COLOR, PLY_SUB_BACKGROUND]);
                }
            }
            rows.push(PLY_CACHE_SECS);
            rows.push(PLY_CACHE_MAX_MB);
            rows.push(PLY_INTRO_MODE);
            if g.get_settings_skip_intro_mode().as_str() == "ask-timed" {
                rows.push(PLY_INTRO_SECS);
            }
            rows.push(PLY_RECAP_MODE);
            if g.get_settings_skip_recap_mode().as_str() == "ask-timed" {
                rows.push(PLY_RECAP_SECS);
            }
            rows.push(PLY_PREVIEW_MODE);
            if g.get_settings_skip_preview_mode().as_str() == "ask-timed" {
                rows.push(PLY_PREVIEW_SECS);
            }
            rows.push(PLY_COMMERCIAL_MODE);
            if g.get_settings_skip_commercial_mode().as_str() == "ask-timed" {
                rows.push(PLY_COMMERCIAL_SECS);
            }
            rows.push(PLY_CREDITS_MODE);
            if g.get_settings_skip_credits_mode().as_str() == "ask" {
                rows.push(PLY_CREDITS_SECS);
            }
            rows.push(PLY_SEEK_STEP);
            rows.push(PLY_SEEK_STEP_LONG);
            rows.push(PLY_SKIP_FADE_MS);
            rows
        }
        SECTION_UI => vec![
            UI_SCROLL_SPEED,
            UI_ANIMATION_SPEED,
            UI_FONT_FAMILY,
            UI_ONSCREEN_KEYBOARD,
        ],
        SECTION_INTEGRATIONS => {
            let mut rows = vec![INT_SEERR_ENABLED];
            if g.get_settings_seerr_enabled() {
                rows.push(INT_SEERR_CONNECT);
                if g.get_seerr_connected() {
                    rows.extend([
                        INT_STREAMING_REGION,
                        INT_TRAILER_QUALITY,
                        INT_DISPLAY_LANGUAGE,
                        INT_DISCOVER_LANGUAGE,
                        INT_DISCOVER_REGION,
                    ]);
                    if g.get_seerr_can_manage_blocklist() {
                        rows.push(INT_MANAGE_BLOCKLIST);
                    }
                }
            }
            rows
        }
        _ => vec![],
    }
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
    // Code review, 2026-08-08: a dropdown popup opened via keyboard Confirm
    // has no backdrop and doesn't cover the whole right pane, so a row
    // behind it stayed clickable while the popup was open — clicking one
    // re-pointed settings-focused without closing the popup, so the next
    // Confirm applied the NEWLY-clicked row's value using the OLD popup's
    // stale cursor position. Closing it here means a row click always means
    // "focus this row", never "also silently keep an unrelated popup open".
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
    // Disconnect Seerr confirmation (2026-08-22, see show-seerr-disconnect-
    // confirm's own doc comment in app_state.slint) — checked first, same
    // shape as the dropdown-open gate right below: intercepts all Settings
    // input while open, regardless of section/row focus. Settings-only
    // (single trigger context, the Integrations row), unlike Sign Out /
    // Cancel Request, so this doesn't need a global main.slint-level
    // dialog or a pre-active_mode() keys.rs tier.
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
        let model_len = dropdown_model(sf.as_str())
            .map(|m| m.len() as i32)
            .unwrap_or_else(|| g.get_settings_dropdown_model().row_count() as i32);
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
                // Code review, 2026-08-08: unlike Up/Down (which already
                // look up `idx` and self-heal on a miss), Confirm/Right used
                // to act on `sf` unconditionally — if a MOUSE interaction
                // elsewhere hid the row `sf` still pointed at (e.g. toggling
                // a setting that hides other rows, without going through
                // settings-row-focused), Enter/Right would silently open a
                // dropdown for, or mutate, a row the user can no longer see.
                // Self-heal the same way Up/Down already do instead of
                // acting on a key that's no longer actually on screen.
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
                set_section(g, SECTION_GENERAL);
                Some(true)
            }
            _ => None,
        }
    }
}

// ── Dropdown helpers ──────────────────────────────────────────────────────────

const LANG_MODEL: &[&str] = &[
    "",
    "English",
    "German",
    "French",
    "Japanese",
    "Spanish",
    "Italian",
    "Portuguese",
    "Russian",
    "Korean",
    "Chinese",
    "Dutch",
    "Swedish",
    "Polish",
    "Czech",
    "Arabic",
    "Turkish",
    "Finnish",
    "Danish",
    "Norwegian",
];

const AUDIO_CHANNELS_MODEL: &[&str] = &[
    "auto-safe",
    "auto",
    "stereo",
    "5.1",
    "7.1",
    "7.1,5.1,stereo",
];

const HWDEC_MODEL: &[&str] = &[
    "auto",
    "vulkan",
    "vulkan-copy",
    "nvdec",
    "nvdec-copy",
    "vaapi",
    "vaapi-copy",
    "vdpau",
    "vdpau-copy",
    "none",
];
const VF_MODEL: &[&str] = &[
    "auto: nv12/p010",
    "auto: yuv420p/yuv420p10le",
    "format=yuv420p",
    "format=yuv420p10le",
    "format=nv12",
    "format=p010",
];
const DEINTERLACE_MODEL: &[&str] = &["no", "auto", "yes"];
const VIDEO_SYNC_MODEL: &[&str] = &[
    "audio",
    "display-resample",
    "display-vdrop",
    "display-adrop",
    "desync",
];
const TSCALE_MODEL: &[&str] = &[
    "oversample",
    "catmull_rom",
    "mitchell",
    "gaussian",
    "bicubic",
];
const TONE_MAPPING_MODEL: &[&str] = &[
    "auto", "hable", "bt.2390", "reinhard", "mobius", "clip", "gamma", "linear",
];
// display_sync (2026-09-18) — Default resolution/Default refresh rate
// started as a small, pragmatic static list here (hand-picked common
// values, since display_sync::get_supported_modes was private to
// display_sync.rs at the time) but a real dev-machine report ("not many
// choices... none of them necessarily even valid for my display") showed
// that was the wrong trade-off — both are now genuinely dynamic dropdowns
// (VID_DISPLAY_SYNC_DEFAULT_RESOLUTION/_HZ in is_dynamic_dropdown below),
// sourced from display_sync::supported_resolutions_and_hz for whichever
// screen is actually selected, the same shape VID_DISPLAY_SYNC_SCREEN
// already used. Scale still has no per-output "supported scales" concept
// to query, so it stays a plain static list.
const DISPLAY_SYNC_SCALE_MODEL: &[&str] = &["1.0", "1.25", "1.5", "1.75", "2.0"];
const DISPLAY_SYNC_4K_ODD_FPS_MODEL: &[&str] = &["fallback", "stay_4k"];
const DISPLAY_SYNC_HDR_MODE_MODEL: &[&str] = &["yes", "no", "always"];
const DISPLAY_SYNC_WCG_MODE_MODEL: &[&str] = &["auto", "yes", "no"];
const SUB_TYPE_MODEL: &[&str] = &["Any", "Normal", "Forced", "Hearing Impaired"];
// "0" = mpv's own huge default (effectively unlimited, capped by
// CACHE_MAX_MB_MODEL below) — displayed as "Unlimited" via display_val.
const CACHE_SECS_MODEL: &[&str] = &["0", "10", "30", "60", "120", "300"];
const CACHE_SECS_VALUES: &[i32] = &[0, 10, 30, 60, 120, 300];
// "0" here means a genuinely raised byte ceiling (mpv.rs sets
// demuxer-max-bytes to a large fixed value, not mpv's own 150 MiB stock
// default — see its own doc comment), so Cache Duration alone governs —
// also displayed as "Unlimited" via display_val.
const CACHE_MAX_MB_MODEL: &[&str] = &["0", "150", "300", "500", "1000", "2000"];
const CACHE_MAX_MB_VALUES: &[i32] = &[0, 150, 300, 500, 1000, 2000];
const SKIP_MODE_4_MODEL: &[&str] = &["always-skip", "ask", "ask-timed", "never-skip"];
const SKIP_MODE_3_MODEL: &[&str] = &["always-skip", "ask", "never-skip"];
const SKIP_SECS_MODEL: &[&str] = &["3", "5", "8", "10", "15", "20", "30"];
const CREDITS_SECS_MODEL: &[&str] = &["10", "15", "20", "30", "45", "60"];
const LOG_LEVEL_MODEL: &[&str] = &["error", "warn", "info", "debug"];
const LAUNCH_POLICY_MODEL: &[&str] = &["always_ask", "remember_last", "default"];
const SUB_SCALE_MODEL: &[&str] = &["50", "75", "100", "125", "150", "175", "200"];
// mpv's real supported range is 0-150 (verified via `man mpv` 0.41.0) — 100 is
// mpv's own "default bottom" position, not the screen edge; values above 100
// push subtitles further down still. Text/ASS subs can get clipped above 100
// (a libass restriction, per the same manual page), which is why the row's
// subtitle string calls this out rather than silently allowing it.
const SUB_POS_MODEL: &[&str] = &[
    "50", "60", "70", "80", "90", "95", "100", "110", "120", "130", "140", "150",
];
// Display names stored directly in Config.sub_color (like LANG_MODEL stores
// display language names) — translated to an actual mpv hex color at point
// of use, not here.
const SUB_COLOR_MODEL: &[&str] = &["", "White", "Yellow", "Cyan", "Green"];
const SEEK_STEP_MODEL: &[&str] = &["5", "10", "15", "20", "30"];
const SEEK_STEP_LONG_MODEL: &[&str] = &["15", "30", "45", "60", "120"];
// "0" = instant (today's pre-feature hard cut, display_val below reads it
// as "Off"); 200 is the shipped default. Both halves of the fade (out and
// in) use this same duration — see wire_mpv_timer's own doc comment.
const SKIP_FADE_MS_MODEL: &[&str] = &["0", "100", "150", "200", "300", "400", "500", "750", "1000"];
// Percentage is a DURATION multiplier, not a rate — bigger % means the
// transition takes longer, i.e. slower, which is the opposite of what
// "speed" suggests at a glance (see the row subtitle text in settings.slint).
const SPEED_PCT_MODEL: &[&str] = &[
    "0", "25", "50", "75", "100", "150", "200", "300", "400", "500",
];
// Display-ready values stored directly in Config.trailer_quality, same
// idiom as SUB_COLOR_MODEL above — translated to an mpv ytdl-format string
// only at point of use (main.rs::trailer_ytdl_format), not here.
const TRAILER_QUALITY_MODEL: &[&str] = &["Best", "1080p", "720p", "480p"];

fn display_val<'a>(val: &'a str, key: &str) -> &'a str {
    if val.is_empty() {
        return match key {
            // "Default" (2026-08-08, user feedback): an empty language
            // preference doesn't mean "no subtitle/audio track selected at
            // all" — playback.rs's wire_mpv_timer tries sub_lang/sub_lang2
            // by lang.starts_with(code) only when actually set, and "if no
            // match, mpv's default selection is left unchanged" (see
            // CLAUDE.md's Subtitle auto-select section) — so an empty value
            // genuinely means "use whatever the video container's own
            // default track is," which "Default" names directly rather
            // than "Any" (which reads as "no filtering," the correct word
            // for PLY_SUB_TYPE below, a real type filter, but a misleading
            // one for a language preference that actually does fall
            // through to the container's default track).
            AUD_AUDIO_LANG | PLY_SUB_LANG | PLY_SUB_LANG2 => "Default",
            PLY_SUB_TYPE => "Any",
            PLY_SUB_COLOR => "Default",
            _ => "(none)",
        };
    }
    // "0" means "no cache-secs override" (mpv's own default is effectively
    // unlimited, bounded only by the max-size row) — "Unlimited" is honest
    // about that, unlike the raw "0" which reads as "no cache at all."
    if key == PLY_CACHE_SECS && val == "0" {
        return "Unlimited";
    }
    // "0" here means the byte ceiling itself is raised out of the way (see
    // mpv.rs) so Cache Duration alone decides — "Unlimited", not "no cache."
    if key == PLY_CACHE_MAX_MB && val == "0" {
        return "Unlimited";
    }
    // "0" here means no delay at all — today's pre-feature instant hard cut,
    // both for the video fade and the audio ramp/mute alongside it.
    if key == PLY_SKIP_FADE_MS && val == "0" {
        return "Off (instant)";
    }
    if key == PROF_LAUNCH_POLICY {
        return match val {
            "always_ask" => "Always Ask",
            "remember_last" => "Remember Last",
            "default" => "Default Profile",
            _ => val,
        };
    }
    if key == PROF_ACCOUNT_LAUNCH_POLICY {
        return match val {
            "always_ask" => "Always Ask",
            "remember_last" => "Remember Last",
            "default" => "Default Account",
            _ => val,
        };
    }
    // Skip mode display names (only for skip mode rows)
    if matches!(
        key,
        PLY_INTRO_MODE | PLY_RECAP_MODE | PLY_PREVIEW_MODE | PLY_COMMERCIAL_MODE | PLY_CREDITS_MODE
    ) {
        return match val {
            "always-skip" => "Always skip",
            "ask" => "Ask",
            "ask-timed" => "Ask (timed)",
            "never-skip" => "Never skip",
            _ => val,
        };
    }
    if key == VID_DISPLAY_SYNC_4K_ODD_FPS_MODE {
        return match val {
            "fallback" => "Fallback to default resolution",
            "stay_4k" => "Stay at 4K",
            _ => val,
        };
    }
    if key == VID_DISPLAY_SYNC_HDR_MODE {
        return match val {
            "yes" => "Match source",
            "no" => "Never",
            "always" => "Always",
            _ => val,
        };
    }
    if key == VID_DISPLAY_SYNC_WCG_MODE {
        return match val {
            "auto" => "Follow HDR",
            "yes" => "Always",
            "no" => "Never",
            _ => val,
        };
    }
    val
}

// Static, compile-time-fixed option lists. Returns None for toggle/button/
// action rows AND for the 7 dynamic-dropdown rows (see is_dynamic_dropdown).
fn dropdown_model(key: &str) -> Option<&'static [&'static str]> {
    match key {
        GEN_LOG_LEVEL => Some(LOG_LEVEL_MODEL),
        PROF_LAUNCH_POLICY | PROF_ACCOUNT_LAUNCH_POLICY => Some(LAUNCH_POLICY_MODEL),
        VID_HWDEC => Some(HWDEC_MODEL),
        VID_VF => Some(VF_MODEL),
        VID_DEINTERLACE => Some(DEINTERLACE_MODEL),
        VID_VIDEO_SYNC => Some(VIDEO_SYNC_MODEL),
        VID_TSCALE => Some(TSCALE_MODEL),
        VID_TONE_MAPPING => Some(TONE_MAPPING_MODEL),
        AUD_CHANNELS => Some(AUDIO_CHANNELS_MODEL),
        AUD_AUDIO_LANG | PLY_SUB_LANG | PLY_SUB_LANG2 => Some(LANG_MODEL),
        PLY_SUB_TYPE => Some(SUB_TYPE_MODEL),
        PLY_SUB_SCALE => Some(SUB_SCALE_MODEL),
        PLY_SUB_POS => Some(SUB_POS_MODEL),
        PLY_SUB_COLOR => Some(SUB_COLOR_MODEL),
        PLY_CACHE_SECS => Some(CACHE_SECS_MODEL),
        PLY_CACHE_MAX_MB => Some(CACHE_MAX_MB_MODEL),
        PLY_INTRO_MODE | PLY_RECAP_MODE | PLY_PREVIEW_MODE | PLY_COMMERCIAL_MODE => {
            Some(SKIP_MODE_4_MODEL)
        }
        PLY_CREDITS_MODE => Some(SKIP_MODE_3_MODEL),
        PLY_INTRO_SECS | PLY_RECAP_SECS | PLY_PREVIEW_SECS | PLY_COMMERCIAL_SECS => {
            Some(SKIP_SECS_MODEL)
        }
        PLY_CREDITS_SECS => Some(CREDITS_SECS_MODEL),
        PLY_SEEK_STEP => Some(SEEK_STEP_MODEL),
        PLY_SEEK_STEP_LONG => Some(SEEK_STEP_LONG_MODEL),
        PLY_SKIP_FADE_MS => Some(SKIP_FADE_MS_MODEL),
        UI_SCROLL_SPEED | UI_ANIMATION_SPEED => Some(SPEED_PCT_MODEL),
        INT_TRAILER_QUALITY => Some(TRAILER_QUALITY_MODEL),
        VID_DISPLAY_SYNC_SCALE_4K | VID_DISPLAY_SYNC_SCALE_1080P => Some(DISPLAY_SYNC_SCALE_MODEL),
        VID_DISPLAY_SYNC_4K_ODD_FPS_MODE => Some(DISPLAY_SYNC_4K_ODD_FPS_MODEL),
        VID_DISPLAY_SYNC_HDR_MODE => Some(DISPLAY_SYNC_HDR_MODE_MODEL),
        VID_DISPLAY_SYNC_WCG_MODE => Some(DISPLAY_SYNC_WCG_MODE_MODEL),
        _ => None,
    }
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
    is_dynamic_dropdown(key) || dropdown_model(key).is_some()
}

fn current_value_str(key: &str, g: &crate::AppState<'_>) -> String {
    match key {
        GEN_LOG_LEVEL => g.get_settings_log_level().to_string(),
        PROF_LAUNCH_POLICY => g.get_settings_launch_policy().to_string(),
        PROF_ACCOUNT_LAUNCH_POLICY => g.get_settings_account_launch_policy().to_string(),
        VID_HWDEC => g.get_settings_hwdec().to_string(),
        VID_VF => g.get_settings_vf().to_string(),
        VID_DEINTERLACE => g.get_settings_deinterlace().to_string(),
        VID_VIDEO_SYNC => g.get_settings_video_sync().to_string(),
        VID_TSCALE => g.get_settings_tscale().to_string(),
        VID_TONE_MAPPING => g.get_settings_tone_mapping().to_string(),
        AUD_AUDIO_DEVICE => g.get_settings_audio_device_desc().to_string(),
        AUD_CHANNELS => g.get_settings_audio_channels().to_string(),
        AUD_PASSTHROUGH_DEVICE => g.get_settings_passthrough_device_desc().to_string(),
        AUD_AUDIO_LANG => g.get_settings_audio_lang().to_string(),
        PLY_SUB_LANG => g.get_settings_sub_lang().to_string(),
        PLY_SUB_LANG2 => g.get_settings_sub_lang2().to_string(),
        PLY_SUB_TYPE => {
            let v = g.get_settings_sub_type().to_string();
            if v.is_empty() { "Any".to_string() } else { v }
        }
        PLY_SUB_SCALE => g.get_settings_sub_scale_pct().to_string(),
        PLY_SUB_POS => g.get_settings_sub_pos_pct().to_string(),
        PLY_SUB_COLOR => g.get_settings_sub_color().to_string(),
        PLY_CACHE_SECS => g.get_settings_cache_secs().to_string(),
        PLY_CACHE_MAX_MB => g.get_settings_cache_max_mb().to_string(),
        PLY_INTRO_MODE => g.get_settings_skip_intro_mode().to_string(),
        PLY_INTRO_SECS => g.get_settings_skip_intro_secs().to_string(),
        PLY_RECAP_MODE => g.get_settings_skip_recap_mode().to_string(),
        PLY_RECAP_SECS => g.get_settings_skip_recap_secs().to_string(),
        PLY_PREVIEW_MODE => g.get_settings_skip_preview_mode().to_string(),
        PLY_PREVIEW_SECS => g.get_settings_skip_preview_secs().to_string(),
        PLY_COMMERCIAL_MODE => g.get_settings_skip_commercial_mode().to_string(),
        PLY_COMMERCIAL_SECS => g.get_settings_skip_commercial_secs().to_string(),
        PLY_CREDITS_MODE => g.get_settings_skip_credits_mode().to_string(),
        PLY_CREDITS_SECS => g.get_settings_skip_credits_secs().to_string(),
        PLY_SEEK_STEP => g.get_settings_seek_step_secs().to_string(),
        PLY_SEEK_STEP_LONG => g.get_settings_seek_step_long_secs().to_string(),
        PLY_SKIP_FADE_MS => g.get_settings_skip_fade_ms().to_string(),
        UI_SCROLL_SPEED => g.get_settings_scroll_speed_pct().to_string(),
        UI_ANIMATION_SPEED => g.get_settings_animation_speed_pct().to_string(),
        UI_FONT_FAMILY => g.get_settings_font_family_desc().to_string(),
        INT_STREAMING_REGION => g.get_settings_streaming_region_desc().to_string(),
        INT_TRAILER_QUALITY => g.get_settings_trailer_quality().to_string(),
        INT_DISCOVER_REGION => g.get_settings_discover_region_desc().to_string(),
        VID_DISPLAY_SYNC_SCALE_4K => g.get_settings_display_sync_scale_4k().to_string(),
        VID_DISPLAY_SYNC_SCALE_1080P => g.get_settings_display_sync_scale_1080p().to_string(),
        VID_DISPLAY_SYNC_4K_ODD_FPS_MODE => {
            g.get_settings_display_sync_4k_odd_fps_mode().to_string()
        }
        VID_DISPLAY_SYNC_HDR_MODE => g.get_settings_display_sync_hdr_mode().to_string(),
        VID_DISPLAY_SYNC_WCG_MODE => g.get_settings_display_sync_wcg_mode().to_string(),
        _ => String::new(),
    }
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
        // Output's own desc is the annotated display label ("HDMI-A-2
        // (Primary)"), not the raw connector name — a real name<->desc
        // lookup exists for this one (FjordState.display_sync_outputs,
        // resolved in main.rs's own on_display_sync_screen_selected), same
        // shape as audio-device/font-family. Resolution/Hz's own desc IS
        // still the value directly (a plain resolution/Hz string, nothing
        // to annotate) — see display-sync-hz/resolution-selected's own doc
        // comments in app_state.slint.
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
    let Some(model) = dropdown_model(key) else {
        debug!(
            "settings: open_dropdown_popup({key}) — no model, not a dropdown row (bug if this fires)"
        );
        return;
    };
    let current = current_value_str(key, g);
    let cursor = model
        .iter()
        .position(|&v| v == current.as_str())
        .unwrap_or(0) as i32;
    let display_items: Vec<SharedString> =
        model.iter().map(|&v| display_val(v, key).into()).collect();
    let current_display: SharedString = display_val(current.as_str(), key).into();
    debug!(
        "settings: open static dropdown {key} ({} options, cursor={cursor}, current={current})",
        model.len()
    );
    g.set_settings_dropdown_model(ModelRc::new(VecModel::from(display_items)));
    g.set_settings_dropdown_display(current_display);
    g.set_settings_dropdown_cursor(cursor);
    g.set_settings_dropdown_open(true);
}

pub(crate) fn apply_dropdown_selection(key: &str, cursor: i32, g: &crate::AppState<'_>) {
    debug!("settings: apply_dropdown_selection {key} cursor={cursor}");
    match key {
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
    let Some(model) = dropdown_model(key) else {
        return;
    };
    let Some(&val) = model.get(cursor as usize) else {
        return;
    };
    match key {
        GEN_LOG_LEVEL => g.set_settings_log_level(val.into()),
        PROF_LAUNCH_POLICY => g.set_settings_launch_policy(val.into()),
        PROF_ACCOUNT_LAUNCH_POLICY => g.set_settings_account_launch_policy(val.into()),
        VID_HWDEC => g.set_settings_hwdec(val.into()),
        VID_VF => g.set_settings_vf(val.into()),
        VID_DEINTERLACE => g.set_settings_deinterlace(val.into()),
        VID_VIDEO_SYNC => g.set_settings_video_sync(val.into()),
        VID_TSCALE => g.set_settings_tscale(val.into()),
        VID_TONE_MAPPING => g.set_settings_tone_mapping(val.into()),
        AUD_CHANNELS => g.set_settings_audio_channels(val.into()),
        AUD_AUDIO_LANG => g.set_settings_audio_lang(val.into()),
        PLY_SUB_LANG => g.set_settings_sub_lang(val.into()),
        PLY_SUB_LANG2 => g.set_settings_sub_lang2(val.into()),
        PLY_SUB_TYPE => g.set_settings_sub_type(if val == "Any" { "".into() } else { val.into() }),
        PLY_SUB_SCALE => g.set_settings_sub_scale_pct(val.parse().unwrap_or(100)),
        PLY_SUB_POS => g.set_settings_sub_pos_pct(val.parse().unwrap_or(100)),
        PLY_SUB_COLOR => g.set_settings_sub_color(val.into()),
        PLY_CACHE_SECS => g.set_settings_cache_secs(val.parse().unwrap_or(60)),
        PLY_CACHE_MAX_MB => g.set_settings_cache_max_mb(val.parse().unwrap_or(500)),
        PLY_INTRO_MODE => g.set_settings_skip_intro_mode(val.into()),
        PLY_INTRO_SECS => g.set_settings_skip_intro_secs(val.parse().unwrap_or(8)),
        PLY_RECAP_MODE => g.set_settings_skip_recap_mode(val.into()),
        PLY_RECAP_SECS => g.set_settings_skip_recap_secs(val.parse().unwrap_or(8)),
        PLY_PREVIEW_MODE => g.set_settings_skip_preview_mode(val.into()),
        PLY_PREVIEW_SECS => g.set_settings_skip_preview_secs(val.parse().unwrap_or(8)),
        PLY_COMMERCIAL_MODE => g.set_settings_skip_commercial_mode(val.into()),
        PLY_COMMERCIAL_SECS => g.set_settings_skip_commercial_secs(val.parse().unwrap_or(8)),
        PLY_CREDITS_MODE => g.set_settings_skip_credits_mode(val.into()),
        PLY_CREDITS_SECS => g.set_settings_skip_credits_secs(val.parse().unwrap_or(30)),
        PLY_SEEK_STEP => g.set_settings_seek_step_secs(val.parse().unwrap_or(10)),
        PLY_SEEK_STEP_LONG => g.set_settings_seek_step_long_secs(val.parse().unwrap_or(30)),
        UI_SCROLL_SPEED => {
            let pct: i32 = val.parse().unwrap_or(100);
            g.set_settings_scroll_speed_pct(pct);
            g.set_settings_scroll_speed(pct as f32 / 100.0);
        }
        UI_ANIMATION_SPEED => {
            let pct: i32 = val.parse().unwrap_or(100);
            g.set_settings_animation_speed_pct(pct);
            g.set_settings_animation_speed(pct as f32 / 100.0);
        }
        INT_TRAILER_QUALITY => g.set_settings_trailer_quality(val.into()),
        _ => return,
    }
    g.invoke_settings_changed();
}

// ── Per-row action (Confirm on a non-dropdown row, or Right on any row) ───────

fn settings_row_action(key: &str, g: &crate::AppState<'_>) {
    debug!("settings: row_action {key}");
    fn cycle<'a>(current: &str, vals: &[&'a str]) -> &'a str {
        let idx = vals.iter().position(|v| *v == current).unwrap_or(0);
        vals[(idx + 1) % vals.len()]
    }
    fn cycle_i32(current: i32, vals: &[i32]) -> i32 {
        let idx = vals.iter().position(|v| *v == current).unwrap_or(0);
        vals[(idx + 1) % vals.len()]
    }
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

    match key {
        GEN_LAUNCH_FULLSCREEN => {
            g.set_settings_launch_fullscreen(!g.get_settings_launch_fullscreen());
            g.invoke_settings_changed();
        }
        GEN_VIDEO_BEHIND => {
            g.set_settings_video_behind(!g.get_settings_video_behind());
            g.invoke_settings_changed();
        }
        GEN_LOG_LEVEL => {
            let v = cycle(g.get_settings_log_level().as_str(), LOG_LEVEL_MODEL);
            g.set_settings_log_level(v.into());
            g.invoke_settings_changed();
        }
        GEN_PREWARM_METADATA => g.invoke_prewarm_metadata(),
        GEN_PREWARM_IMAGES => g.invoke_prewarm_images(),

        PROF_LAUNCH_POLICY => {
            let v = cycle(g.get_settings_launch_policy().as_str(), LAUNCH_POLICY_MODEL);
            g.set_settings_launch_policy(v.into());
            g.invoke_settings_changed();
        }
        PROF_DEFAULT_PROFILE => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_default_profile_display(),
                g.get_settings_default_profile_desc().as_str(),
            ) {
                g.invoke_default_profile_selected(desc);
            }
        }
        PROF_MANAGE_PROFILES => g.invoke_open_manage_profiles(),
        PROF_ACCOUNT_LAUNCH_POLICY => {
            let v = cycle(
                g.get_settings_account_launch_policy().as_str(),
                LAUNCH_POLICY_MODEL,
            );
            g.set_settings_account_launch_policy(v.into());
            g.invoke_settings_changed();
        }
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
            // Confirmation dialog, 2026-08-22 — see show-sign-out-confirm's
            // own doc comment in app_state.slint. This row's own Confirm/
            // Enter no longer signs out directly; it opens the (global,
            // main.slint-level) dialog instead — the same one the sidebar
            // quick-menu's "Sign Out" row and OfflineScreen's "Change
            // Server" button now also open.
            g.set_sign_out_confirm_focused(0);
            g.set_show_sign_out_confirm(true);
        }
        PROF_BONFIRE_GROUP => g.invoke_open_bonfire_group(),
        PROF_BONFIRE_ADMIN => g.invoke_open_bonfire_admin(),

        VID_HWDEC => {
            let v = cycle(g.get_settings_hwdec().as_str(), HWDEC_MODEL);
            g.set_settings_hwdec(v.into());
            g.invoke_settings_changed();
        }
        VID_VF => {
            let v = cycle(g.get_settings_vf().as_str(), VF_MODEL);
            g.set_settings_vf(v.into());
            g.invoke_settings_changed();
        }
        VID_DEINTERLACE => {
            let v = cycle(g.get_settings_deinterlace().as_str(), DEINTERLACE_MODEL);
            g.set_settings_deinterlace(v.into());
            g.invoke_settings_changed();
        }
        VID_VIDEO_SYNC => {
            let v = cycle(g.get_settings_video_sync().as_str(), VIDEO_SYNC_MODEL);
            g.set_settings_video_sync(v.into());
            g.invoke_settings_changed();
        }
        VID_INTERPOLATION => {
            g.set_settings_interpolation(!g.get_settings_interpolation());
            g.invoke_settings_changed();
        }
        VID_TSCALE => {
            let v = cycle(g.get_settings_tscale().as_str(), TSCALE_MODEL);
            g.set_settings_tscale(v.into());
            g.invoke_settings_changed();
        }
        VID_TONE_MAPPING => {
            let v = cycle(g.get_settings_tone_mapping().as_str(), TONE_MAPPING_MODEL);
            g.set_settings_tone_mapping(v.into());
            g.invoke_settings_changed();
        }
        VID_TARGET_COLORSPACE => {
            g.set_settings_target_colorspace_hint(!g.get_settings_target_colorspace_hint());
            g.invoke_settings_changed();
        }
        VID_SEPARATE_VIDEO_SURFACE => {
            g.set_settings_separate_video_surface(!g.get_settings_separate_video_surface());
            g.invoke_settings_changed();
        }
        VID_OWN_BUFFERS => {
            g.set_settings_video_own_buffers(!g.get_settings_video_own_buffers());
            g.invoke_settings_changed();
        }
        VID_DITHER_OFF => {
            g.set_settings_video_dither_off(!g.get_settings_video_dither_off());
            g.invoke_settings_changed();
        }
        VID_OPENGL_EARLY_FLUSH => {
            g.set_settings_opengl_early_flush(!g.get_settings_opengl_early_flush());
            g.invoke_settings_changed();
        }
        VID_VIDEO_LATENCY_HACKS if g.get_settings_video_sync().as_str() == "display-resample" => {
            g.set_settings_video_latency_hacks(!g.get_settings_video_latency_hacks());
            g.invoke_settings_changed();
        }
        VID_DISPLAY_SYNC_ENABLED => {
            g.set_settings_display_sync_enabled(!g.get_settings_display_sync_enabled());
            g.invoke_settings_changed();
        }
        VID_DISPLAY_SYNC_SCREEN => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_display_sync_screen_options(),
                g.get_settings_display_sync_screen_desc().as_str(),
            ) {
                g.invoke_display_sync_screen_selected(desc);
            }
        }
        VID_DISPLAY_SYNC_SYNC_RESOLUTION => {
            g.set_settings_display_sync_sync_resolution(
                !g.get_settings_display_sync_sync_resolution(),
            );
            g.invoke_settings_changed();
        }
        VID_DISPLAY_SYNC_TRAILERS => {
            g.set_settings_display_sync_trailers(!g.get_settings_display_sync_trailers());
            g.invoke_settings_changed();
        }
        VID_DISPLAY_SYNC_SYNC_REFRESH_RATE => {
            g.set_settings_display_sync_sync_refresh_rate(
                !g.get_settings_display_sync_sync_refresh_rate(),
            );
            g.invoke_settings_changed();
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
        VID_DISPLAY_SYNC_SCALE_4K => {
            let v = cycle(
                g.get_settings_display_sync_scale_4k().as_str(),
                DISPLAY_SYNC_SCALE_MODEL,
            );
            g.set_settings_display_sync_scale_4k(v.into());
            g.invoke_settings_changed();
        }
        VID_DISPLAY_SYNC_SCALE_1080P => {
            let v = cycle(
                g.get_settings_display_sync_scale_1080p().as_str(),
                DISPLAY_SYNC_SCALE_MODEL,
            );
            g.set_settings_display_sync_scale_1080p(v.into());
            g.invoke_settings_changed();
        }
        VID_DISPLAY_SYNC_4K_ODD_FPS_MODE => {
            let v = cycle(
                g.get_settings_display_sync_4k_odd_fps_mode().as_str(),
                DISPLAY_SYNC_4K_ODD_FPS_MODEL,
            );
            g.set_settings_display_sync_4k_odd_fps_mode(v.into());
            g.invoke_settings_changed();
        }
        VID_DISPLAY_SYNC_HDR_MODE => {
            let v = cycle(
                g.get_settings_display_sync_hdr_mode().as_str(),
                DISPLAY_SYNC_HDR_MODE_MODEL,
            );
            g.set_settings_display_sync_hdr_mode(v.into());
            g.invoke_settings_changed();
        }
        VID_DISPLAY_SYNC_WCG_MODE => {
            let v = cycle(
                g.get_settings_display_sync_wcg_mode().as_str(),
                DISPLAY_SYNC_WCG_MODE_MODEL,
            );
            g.set_settings_display_sync_wcg_mode(v.into());
            g.invoke_settings_changed();
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
        AUD_SPDIF => {
            g.set_settings_audio_spdif(!g.get_settings_audio_spdif());
            g.invoke_settings_changed();
        }
        AUD_SPDIF_AC3 => {
            g.set_settings_spdif_ac3(!g.get_settings_spdif_ac3());
            g.invoke_settings_changed();
        }
        AUD_SPDIF_EAC3 => {
            g.set_settings_spdif_eac3(!g.get_settings_spdif_eac3());
            g.invoke_settings_changed();
        }
        AUD_SPDIF_DTS => {
            g.set_settings_spdif_dts(!g.get_settings_spdif_dts());
            g.invoke_settings_changed();
        }
        AUD_SPDIF_DTS_HD => {
            g.set_settings_spdif_dts_hd(!g.get_settings_spdif_dts_hd());
            g.invoke_settings_changed();
        }
        AUD_SPDIF_TRUEHD => {
            g.set_settings_spdif_truehd(!g.get_settings_spdif_truehd());
            g.invoke_settings_changed();
        }
        AUD_ALSA_IRQ => {
            g.set_settings_alsa_irq_scheduling(!g.get_settings_alsa_irq_scheduling());
            g.invoke_settings_changed();
        }
        AUD_SKIP_FADE_MUTE => {
            g.set_settings_skip_fade_mute_passthrough(!g.get_settings_skip_fade_mute_passthrough());
            g.invoke_settings_changed();
        }
        AUD_CHANNELS => {
            let v = cycle(
                g.get_settings_audio_channels().as_str(),
                AUDIO_CHANNELS_MODEL,
            );
            g.set_settings_audio_channels(v.into());
            g.invoke_settings_changed();
        }
        AUD_AUDIO_LANG => {
            let v = cycle(g.get_settings_audio_lang().as_str(), LANG_MODEL);
            g.set_settings_audio_lang(v.into());
            g.invoke_settings_changed();
        }
        AUD_GAPLESS => {
            g.set_settings_gapless_audio(!g.get_settings_gapless_audio());
            g.invoke_settings_changed();
        }
        AUD_NOW_PLAYING_AUTO_OPEN => {
            g.set_settings_now_playing_auto_open(!g.get_settings_now_playing_auto_open());
            g.invoke_settings_changed();
        }
        UI_ONSCREEN_KEYBOARD => {
            g.set_settings_onscreen_keyboard_enabled(!g.get_settings_onscreen_keyboard_enabled());
            g.invoke_settings_changed();
        }

        PLY_SUB_ENABLED => {
            g.set_settings_sub_enabled(!g.get_settings_sub_enabled());
            g.invoke_settings_changed();
        }
        PLY_SUB_LANG => {
            let v = cycle(g.get_settings_sub_lang().as_str(), LANG_MODEL);
            g.set_settings_sub_lang(v.into());
            g.invoke_settings_changed();
        }
        PLY_SUB_LANG2 => {
            let v = cycle(g.get_settings_sub_lang2().as_str(), LANG_MODEL);
            g.set_settings_sub_lang2(v.into());
            g.invoke_settings_changed();
        }
        PLY_SUB_TYPE => {
            let current = g.get_settings_sub_type().to_string();
            let current = if current.is_empty() { "Any" } else { &current };
            let v = cycle(current, SUB_TYPE_MODEL);
            g.set_settings_sub_type(if v == "Any" { "".into() } else { v.into() });
            g.invoke_settings_changed();
        }
        PLY_SUB_SCALE => {
            let v = cycle(
                g.get_settings_sub_scale_pct().to_string().as_str(),
                SUB_SCALE_MODEL,
            );
            g.set_settings_sub_scale_pct(v.parse().unwrap_or(100));
            g.invoke_settings_changed();
        }
        PLY_SUB_POS => {
            let v = cycle(
                g.get_settings_sub_pos_pct().to_string().as_str(),
                SUB_POS_MODEL,
            );
            g.set_settings_sub_pos_pct(v.parse().unwrap_or(100));
            g.invoke_settings_changed();
        }
        PLY_SUB_RESPECT_ASS => {
            g.set_settings_sub_respect_ass_styling(!g.get_settings_sub_respect_ass_styling());
            g.invoke_settings_changed();
        }
        PLY_SUB_COLOR => {
            let current = g.get_settings_sub_color().to_string();
            let v = cycle(current.as_str(), SUB_COLOR_MODEL);
            g.set_settings_sub_color(v.into());
            g.invoke_settings_changed();
        }
        PLY_SUB_BACKGROUND => {
            g.set_settings_sub_background(!g.get_settings_sub_background());
            g.invoke_settings_changed();
        }
        PLY_CACHE_SECS => {
            let next = cycle_i32(g.get_settings_cache_secs(), CACHE_SECS_VALUES);
            g.set_settings_cache_secs(next);
            g.invoke_settings_changed();
        }
        PLY_CACHE_MAX_MB => {
            let next = cycle_i32(g.get_settings_cache_max_mb(), CACHE_MAX_MB_VALUES);
            g.set_settings_cache_max_mb(next);
            g.invoke_settings_changed();
        }
        PLY_INTRO_MODE => {
            let v = cycle(g.get_settings_skip_intro_mode().as_str(), SKIP_MODE_4_MODEL);
            g.set_settings_skip_intro_mode(v.into());
            g.invoke_settings_changed();
        }
        PLY_INTRO_SECS => {
            let v = cycle(
                g.get_settings_skip_intro_secs().to_string().as_str(),
                SKIP_SECS_MODEL,
            );
            g.set_settings_skip_intro_secs(v.parse().unwrap_or(8));
            g.invoke_settings_changed();
        }
        PLY_RECAP_MODE => {
            let v = cycle(g.get_settings_skip_recap_mode().as_str(), SKIP_MODE_4_MODEL);
            g.set_settings_skip_recap_mode(v.into());
            g.invoke_settings_changed();
        }
        PLY_RECAP_SECS => {
            let v = cycle(
                g.get_settings_skip_recap_secs().to_string().as_str(),
                SKIP_SECS_MODEL,
            );
            g.set_settings_skip_recap_secs(v.parse().unwrap_or(8));
            g.invoke_settings_changed();
        }
        PLY_PREVIEW_MODE => {
            let v = cycle(
                g.get_settings_skip_preview_mode().as_str(),
                SKIP_MODE_4_MODEL,
            );
            g.set_settings_skip_preview_mode(v.into());
            g.invoke_settings_changed();
        }
        PLY_PREVIEW_SECS => {
            let v = cycle(
                g.get_settings_skip_preview_secs().to_string().as_str(),
                SKIP_SECS_MODEL,
            );
            g.set_settings_skip_preview_secs(v.parse().unwrap_or(8));
            g.invoke_settings_changed();
        }
        PLY_COMMERCIAL_MODE => {
            let v = cycle(
                g.get_settings_skip_commercial_mode().as_str(),
                SKIP_MODE_4_MODEL,
            );
            g.set_settings_skip_commercial_mode(v.into());
            g.invoke_settings_changed();
        }
        PLY_COMMERCIAL_SECS => {
            let v = cycle(
                g.get_settings_skip_commercial_secs().to_string().as_str(),
                SKIP_SECS_MODEL,
            );
            g.set_settings_skip_commercial_secs(v.parse().unwrap_or(8));
            g.invoke_settings_changed();
        }
        PLY_CREDITS_MODE => {
            let v = cycle(
                g.get_settings_skip_credits_mode().as_str(),
                SKIP_MODE_3_MODEL,
            );
            g.set_settings_skip_credits_mode(v.into());
            g.invoke_settings_changed();
        }
        PLY_CREDITS_SECS => {
            let v = cycle(
                g.get_settings_skip_credits_secs().to_string().as_str(),
                CREDITS_SECS_MODEL,
            );
            g.set_settings_skip_credits_secs(v.parse().unwrap_or(30));
            g.invoke_settings_changed();
        }
        PLY_SEEK_STEP => {
            let v = cycle(
                g.get_settings_seek_step_secs().to_string().as_str(),
                SEEK_STEP_MODEL,
            );
            g.set_settings_seek_step_secs(v.parse().unwrap_or(10));
            g.invoke_settings_changed();
        }
        PLY_SEEK_STEP_LONG => {
            let v = cycle(
                g.get_settings_seek_step_long_secs().to_string().as_str(),
                SEEK_STEP_LONG_MODEL,
            );
            g.set_settings_seek_step_long_secs(v.parse().unwrap_or(30));
            g.invoke_settings_changed();
        }
        PLY_SKIP_FADE_MS => {
            let v = cycle(
                g.get_settings_skip_fade_ms().to_string().as_str(),
                SKIP_FADE_MS_MODEL,
            );
            g.set_settings_skip_fade_ms(v.parse().unwrap_or(200));
            g.invoke_settings_changed();
        }

        UI_SCROLL_SPEED => {
            let v = cycle(
                g.get_settings_scroll_speed_pct().to_string().as_str(),
                SPEED_PCT_MODEL,
            );
            let pct: i32 = v.parse().unwrap_or(100);
            g.set_settings_scroll_speed_pct(pct);
            g.set_settings_scroll_speed(pct as f32 / 100.0);
            g.invoke_settings_changed();
        }
        UI_ANIMATION_SPEED => {
            let v = cycle(
                g.get_settings_animation_speed_pct().to_string().as_str(),
                SPEED_PCT_MODEL,
            );
            let pct: i32 = v.parse().unwrap_or(100);
            g.set_settings_animation_speed_pct(pct);
            g.set_settings_animation_speed(pct as f32 / 100.0);
            g.invoke_settings_changed();
        }
        UI_FONT_FAMILY => {
            if let Some(desc) = cycle_dynamic(
                g.get_settings_font_family_display(),
                g.get_settings_font_family_desc().as_str(),
            ) {
                g.invoke_font_family_selected(desc);
            }
        }

        INT_SEERR_ENABLED => {
            g.set_settings_seerr_enabled(!g.get_settings_seerr_enabled());
            g.invoke_settings_changed();
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
        INT_TRAILER_QUALITY => {
            let v = cycle(
                g.get_settings_trailer_quality().to_string().as_str(),
                TRAILER_QUALITY_MODEL,
            );
            g.set_settings_trailer_quality(v.into());
            g.invoke_settings_changed();
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
/// Wires audio/passthrough device and font lists (fetched once) + their dropdowns: audio_device_selected, passthrough_device_selected, font_family_selected.
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
        let cfg_font = state.lock().unwrap().config.device.ui_font_family.clone();
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
    // ── default profile selected callback (Bonfire Phase 1, step 7) ──────────
    // 100% local, no network round trip — same shape as font-family above.
    // Resolves the selected display label back to a user_id (duplicate
    // labels resolve to whichever profile matches first, the same known
    // limitation refresh_profile_settings_dropdown's own doc comment
    // already states) and lets the generic on_settings_changed handler
    // below persist it via read_settings_from_window + save_config.
    {
        let state_dp = Arc::clone(&state);
        let ww_dp = window.as_weak();
        AppState::get(&window).on_default_profile_selected(move |desc| {
            let Some(w) = ww_dp.upgrade() else { return };
            let g = AppState::get(&w);
            let user_id = {
                let s = state_dp.lock().unwrap();
                // Scoped to the current Default Account (2026-08-17, same
                // fix as refresh_profile_settings_dropdown's own doc
                // comment) — the dropdown's own option list is already
                // scoped this way, so this just avoids the pre-existing,
                // documented "duplicate display label" edge case picking a
                // same-named profile under a DIFFERENT account by mistake.
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

    // ── default account selected callback (2026-08-14) ───────────────────────
    // Account-tier mirror of default-profile-selected just above — same
    // 100%-local shape, resolves the display label back to an account's own
    // root_id via group_into_accounts.
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
            // Re-scope Default Profile's own option list to the just-picked
            // account immediately (2026-08-17) — without this, the profile
            // dropdown kept showing whatever account's profiles it happened
            // to load with until Settings was reopened, which could still
            // let a stale cross-account combination through the UI in the
            // gap between the two picks even though refresh_profile_settings_dropdown
            // is now correctly scoped. cfg is cloned+patched locally rather
            // than persisted here — the real Config write still happens
            // below via invoke_settings_changed, this is purely a same-tick
            // display refresh.
            {
                let mut cfg = state_da.lock().unwrap().config.clone();
                cfg.device.default_account_id = root_id;
                profile::refresh_profile_settings_dropdown(&g, &cfg);
            }
            g.invoke_settings_changed();
        });
    }
}

// ── wire_regions (moved from main(), 0.5.0 step 3) ───────────────────────
/// Wires streaming/discover region + display/discover language dropdowns: streaming_region_selected, discover_region_selected, display_language_selected, discover_language_selected.
pub(crate) fn wire_regions(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── streaming region selected callback ────────────────────────────────────
    // Unlike font-family (100% local, no network write), this needs an
    // actual round trip to Seerr — GET the connected user's current general
    // settings first (see UserGeneralSettings' own doc comment for why a
    // bare {"streamingRegion": ...} body would blank out their username),
    // mutate just streamingRegion, POST the whole thing back. Updates
    // AppState + the FjordState cache resolve_streaming_region reads from
    // only on success, so a failed write leaves the picker showing the
    // still-actually-current value rather than a value that didn't take.
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
                    Err(e) => show_toast(ww2, format!("Couldn't update streaming region: {e:#}")),
                }
            });
        });
    }

    // ── discover region selected callback (2026-07-18, Watchlist + Release ──
    // Calendar) — same GET-mutate-POST shape as streaming region above, just
    // mutating discoverRegion instead. Reuses the same seerr_regions list
    // (region codes are shared between the two settings) for the code lookup.
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
                    Err(e) => show_toast(ww2, format!("Couldn't update discover region: {e:#}")),
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
                    Err(e) => show_toast(ww2, format!("Couldn't update display language: {e:#}")),
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
                    Err(e) => show_toast(ww2, format!("Couldn't update discover language: {e:#}")),
                }
            });
        });
    }
}

// ── wire_settings_changed (moved from main(), 0.5.0 step 3) ──────────────
/// Wires settings-changed, dropdown mouse pick, settings row focus: settings_changed, dropdown_pick, profile_edit_dropdown_pick, settings_row_focused.
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
            // Live-reflect the seerr-enabled toggle: rebuild seerr_client
            // (build_seerr_client already returns None when seerr_enabled
            // is false, so this both tears it down on disable and rebuilds
            // it from the still-saved credentials on re-enable — no forced
            // reconnect either way) and push seerr-connected/-label so
            // every row/block gated on `seerr-connected` (Streaming Region,
            // Display/Discover Language, Trailer Quality, the Settings
            // sidebar SEERR info block) hides/shows immediately rather than
            // only after the next app restart. Real bug, user-reported
            // 2026-07-17 ("for me it shuld turn off seerr") — previously
            // only the Discover sidebar tab responded to this toggle live.
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
            let sub_background = s.config.active().sub_background;
            let cfg = s.config.clone();
            drop(s);
            save_config(&cfg);
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                p.set_sub_style(
                    sub_scale,
                    sub_pos,
                    sub_respect_ass,
                    &sub_color,
                    sub_background,
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

pub(crate) fn apply_settings_to_window(w: &MainWindow, s: &FjordState) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    let g = AppState::get(w);
    let c = &s.config.device;
    let cp = s.config.active();
    g.set_settings_audio_device(ss(&c.audio_device));
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
    g.set_settings_passthrough_device(ss(&c.audio_device_passthrough));
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
    g.set_settings_gapless_audio(c.gapless_audio);
    g.set_settings_now_playing_auto_open(cp.now_playing_auto_open);
    g.set_settings_audio_spdif(c.audio_spdif);
    g.set_settings_spdif_ac3(c.spdif_ac3);
    g.set_settings_spdif_eac3(c.spdif_eac3);
    g.set_settings_spdif_dts(c.spdif_dts);
    g.set_settings_spdif_dts_hd(c.spdif_dts_hd);
    g.set_settings_spdif_truehd(c.spdif_truehd);
    g.set_settings_hwdec(ss(&c.hwdec));
    g.set_settings_vf(ss(&c.vf));
    g.set_settings_video_sync(ss(&c.video_sync));
    g.set_settings_opengl_early_flush(c.opengl_early_flush);
    g.set_settings_video_latency_hacks(c.video_latency_hacks);
    g.set_settings_interpolation(c.interpolation);
    g.set_settings_tscale(ss(&c.tscale));
    g.set_settings_tone_mapping(ss(&c.tone_mapping));
    g.set_settings_target_colorspace_hint(c.target_colorspace_hint);
    g.set_settings_separate_video_surface(c.separate_video_surface);
    g.set_settings_video_own_buffers(c.video_own_buffers);
    g.set_settings_video_dither_off(c.video_dither_off);
    g.set_settings_deinterlace(ss(&c.deinterlace));
    g.set_settings_cache_secs(c.cache_secs as i32);
    g.set_settings_cache_max_mb(c.cache_max_mb as i32);
    g.set_settings_video_behind(c.video_behind);
    g.set_settings_launch_fullscreen(c.launch_fullscreen);
    g.set_settings_log_level(ss(&c.log_level));
    g.set_settings_display_sync_enabled(c.display_sync_enabled);
    g.set_settings_display_sync_trailers(c.display_sync_trailers);
    g.set_settings_display_sync_screen_name(ss(&c.display_sync_screen_name));
    g.set_settings_display_sync_default_resolution(ss(&c.display_sync_default_resolution));
    g.set_settings_display_sync_default_hz(ss(&c.display_sync_default_hz));
    g.set_settings_display_sync_scale_4k(ss(&c.display_sync_scale_4k));
    g.set_settings_display_sync_scale_1080p(ss(&c.display_sync_scale_1080p));
    g.set_settings_display_sync_sync_resolution(c.display_sync_sync_resolution);
    g.set_settings_display_sync_sync_refresh_rate(c.display_sync_sync_refresh_rate);
    g.set_settings_display_sync_4k_odd_fps_mode(ss(&c.display_sync_4k_odd_fps_mode));
    g.set_settings_display_sync_hdr_mode(ss(&c.display_sync_hdr_mode));
    g.set_settings_display_sync_wcg_mode(ss(&c.display_sync_wcg_mode));
    g.set_settings_sub_enabled(cp.sub_enabled);
    g.set_settings_sub_lang(ss(&cp.sub_lang));
    g.set_settings_sub_lang2(ss(&cp.sub_lang2));
    g.set_settings_sub_type(ss(&cp.sub_type));
    g.set_settings_sub_scale_pct(cp.sub_scale_pct as i32);
    g.set_settings_sub_pos_pct(cp.sub_pos_pct as i32);
    g.set_settings_sub_respect_ass_styling(cp.sub_respect_ass_styling);
    g.set_settings_sub_color(ss(&cp.sub_color));
    g.set_settings_sub_background(cp.sub_background);
    g.set_settings_audio_lang(ss(&cp.audio_lang));
    g.set_settings_alsa_irq_scheduling(c.alsa_irq_scheduling);
    g.set_settings_skip_fade_mute_passthrough(c.skip_fade_mute_passthrough);
    g.set_settings_skip_intro_mode(ss(&cp.skip_intro_mode));
    g.set_settings_skip_intro_secs(cp.skip_intro_secs as i32);
    g.set_settings_skip_recap_mode(ss(&cp.skip_recap_mode));
    g.set_settings_skip_recap_secs(cp.skip_recap_secs as i32);
    g.set_settings_skip_preview_mode(ss(&cp.skip_preview_mode));
    g.set_settings_skip_preview_secs(cp.skip_preview_secs as i32);
    g.set_settings_skip_commercial_mode(ss(&cp.skip_commercial_mode));
    g.set_settings_skip_commercial_secs(cp.skip_commercial_secs as i32);
    g.set_settings_skip_credits_mode(ss(&cp.skip_credits_mode));
    g.set_settings_skip_credits_secs(cp.skip_credits_secs as i32);
    g.set_settings_seek_step_secs(c.seek_step_secs as i32);
    g.set_settings_seek_step_long_secs(c.seek_step_long_secs as i32);
    g.set_settings_skip_fade_ms(c.skip_fade_ms as i32);
    g.set_settings_scroll_speed_pct(c.scroll_speed_pct as i32);
    g.set_settings_scroll_speed(c.scroll_speed_pct as f32 / 100.0);
    g.set_settings_animation_speed_pct(c.animation_speed_pct as i32);
    g.set_settings_animation_speed(c.animation_speed_pct as f32 / 100.0);
    // Set synchronously from Config so MainWindow.font-family (bound to this)
    // renders correctly from the very first frame — system_fonts (used only
    // for the human-readable desc) is fetched asynchronously and may still be
    // empty here; fall back to a sensible label rather than waiting on it.
    g.set_settings_font_family(ss(&c.ui_font_family));
    let font_desc = s
        .system_fonts
        .iter()
        .find(|(v, _)| v == &c.ui_font_family)
        .map(|(_, d)| d.as_str())
        .unwrap_or(if c.ui_font_family == "Inter" {
            "Inter (Fjord default)"
        } else if c.ui_font_family.is_empty() {
            "System default"
        } else {
            c.ui_font_family.as_str()
        })
        .to_string();
    g.set_settings_font_family_desc(ss(&font_desc));
    g.set_settings_onscreen_keyboard_enabled(c.onscreen_keyboard_enabled);
    g.set_settings_launch_policy(ss(&c.launch_policy));
    g.set_settings_default_profile_id(ss(&c.default_profile_id));
    g.set_settings_account_launch_policy(ss(&c.account_launch_policy));
    g.set_settings_default_account_id(ss(&c.default_account_id));
    profile::refresh_profile_settings_dropdown(&g, &s.config);
    profile::refresh_account_settings_dropdown(&g, &s.config);
    // Bonfire Phase 5: `is_true_master`, not bare `!is_bonfire` — a session
    // actively impersonating a foreign group account (`is_group_account`)
    // also has `is_bonfire == true` on its own local entry, but it's "a
    // fully privileged session for that account" per Bonfire's own docs,
    // and should still see Manage Profiles / Bonfire Group in Settings.
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
    g.set_settings_trailer_quality(ss(&cp.trailer_quality));
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
    c.audio_spdif = g.get_settings_audio_spdif();
    c.spdif_ac3 = g.get_settings_spdif_ac3();
    c.spdif_eac3 = g.get_settings_spdif_eac3();
    c.spdif_dts = g.get_settings_spdif_dts();
    c.spdif_dts_hd = g.get_settings_spdif_dts_hd();
    c.spdif_truehd = g.get_settings_spdif_truehd();
    c.hwdec = g.get_settings_hwdec().to_string();
    c.vf = g.get_settings_vf().to_string();
    c.video_sync = g.get_settings_video_sync().to_string();
    c.opengl_early_flush = g.get_settings_opengl_early_flush();
    c.video_latency_hacks = g.get_settings_video_latency_hacks();
    c.interpolation = g.get_settings_interpolation();
    c.tscale = g.get_settings_tscale().to_string();
    c.tone_mapping = g.get_settings_tone_mapping().to_string();
    c.target_colorspace_hint = g.get_settings_target_colorspace_hint();
    c.separate_video_surface = g.get_settings_separate_video_surface();
    c.video_own_buffers = g.get_settings_video_own_buffers();
    c.video_dither_off = g.get_settings_video_dither_off();
    c.deinterlace = g.get_settings_deinterlace().to_string();
    c.cache_secs = g.get_settings_cache_secs().max(0) as u32;
    c.cache_max_mb = g.get_settings_cache_max_mb().max(0) as u32;
    c.video_behind = g.get_settings_video_behind();
    c.launch_fullscreen = g.get_settings_launch_fullscreen();
    c.log_level = g.get_settings_log_level().to_string();
    c.audio_device = g.get_settings_audio_device().to_string();
    c.audio_device_passthrough = g.get_settings_passthrough_device().to_string();
    c.audio_channels = g.get_settings_audio_channels().to_string();
    c.gapless_audio = g.get_settings_gapless_audio();
    c.alsa_irq_scheduling = g.get_settings_alsa_irq_scheduling();
    c.skip_fade_mute_passthrough = g.get_settings_skip_fade_mute_passthrough();
    c.seek_step_secs = g.get_settings_seek_step_secs().max(0) as u32;
    c.seek_step_long_secs = g.get_settings_seek_step_long_secs().max(0) as u32;
    c.skip_fade_ms = g.get_settings_skip_fade_ms().max(0) as u32;
    c.scroll_speed_pct = g.get_settings_scroll_speed_pct().max(0) as u32;
    c.animation_speed_pct = g.get_settings_animation_speed_pct().max(0) as u32;
    c.ui_font_family = g.get_settings_font_family().to_string();
    c.onscreen_keyboard_enabled = g.get_settings_onscreen_keyboard_enabled();
    c.launch_policy = g.get_settings_launch_policy().to_string();
    c.default_profile_id = g.get_settings_default_profile_id().to_string();
    c.account_launch_policy = g.get_settings_account_launch_policy().to_string();
    c.default_account_id = g.get_settings_default_account_id().to_string();
    c.display_sync_enabled = g.get_settings_display_sync_enabled();
    c.display_sync_trailers = g.get_settings_display_sync_trailers();
    c.display_sync_screen_name = g.get_settings_display_sync_screen_name().to_string();
    c.display_sync_default_resolution =
        g.get_settings_display_sync_default_resolution().to_string();
    c.display_sync_default_hz = g.get_settings_display_sync_default_hz().to_string();
    c.display_sync_scale_4k = g.get_settings_display_sync_scale_4k().to_string();
    c.display_sync_scale_1080p = g.get_settings_display_sync_scale_1080p().to_string();
    c.display_sync_sync_resolution = g.get_settings_display_sync_sync_resolution();
    c.display_sync_sync_refresh_rate = g.get_settings_display_sync_sync_refresh_rate();
    c.display_sync_4k_odd_fps_mode = g.get_settings_display_sync_4k_odd_fps_mode().to_string();
    c.display_sync_hdr_mode = g.get_settings_display_sync_hdr_mode().to_string();
    c.display_sync_wcg_mode = g.get_settings_display_sync_wcg_mode().to_string();

    let cp = s.config.active_mut();
    cp.sub_enabled = g.get_settings_sub_enabled();
    cp.sub_lang = g.get_settings_sub_lang().to_string();
    cp.sub_lang2 = g.get_settings_sub_lang2().to_string();
    cp.sub_type = g.get_settings_sub_type().to_string();
    cp.sub_scale_pct = g.get_settings_sub_scale_pct().max(0) as u32;
    cp.sub_pos_pct = g.get_settings_sub_pos_pct().max(0) as u32;
    cp.sub_respect_ass_styling = g.get_settings_sub_respect_ass_styling();
    cp.sub_color = g.get_settings_sub_color().to_string();
    cp.sub_background = g.get_settings_sub_background();
    cp.audio_lang = g.get_settings_audio_lang().to_string();
    cp.now_playing_auto_open = g.get_settings_now_playing_auto_open();
    cp.skip_intro_mode = g.get_settings_skip_intro_mode().to_string();
    cp.skip_intro_secs = g.get_settings_skip_intro_secs().max(0) as u32;
    cp.skip_recap_mode = g.get_settings_skip_recap_mode().to_string();
    cp.skip_recap_secs = g.get_settings_skip_recap_secs().max(0) as u32;
    cp.skip_preview_mode = g.get_settings_skip_preview_mode().to_string();
    cp.skip_preview_secs = g.get_settings_skip_preview_secs().max(0) as u32;
    cp.skip_commercial_mode = g.get_settings_skip_commercial_mode().to_string();
    cp.skip_commercial_secs = g.get_settings_skip_commercial_secs().max(0) as u32;
    cp.skip_credits_mode = g.get_settings_skip_credits_mode().to_string();
    cp.skip_credits_secs = g.get_settings_skip_credits_secs().max(0) as u32;
    cp.seerr_enabled = g.get_settings_seerr_enabled();
    cp.trailer_quality = g.get_settings_trailer_quality().to_string();
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
    // Real devices can be exposed under more than one backend with an
    // identical parenthetical description — confirmed live 2026-08-07: a USB
    // interface listed once as `pipewire/alsa_output...` and once as
    // `pulse/alsa_output...`, both described "UAC-2 Digital Stereo (IEC958)".
    // Selection in Settings round-trips purely through this description
    // string (the dropdown widget only knows strings, not indices), so two
    // entries sharing one desc made the second unselectable — picking it
    // always resolved back to the first matching desc instead. Suffix every
    // duplicate with its backend (the part of `name` before the first '/')
    // so every entry's desc is unique; `Config.audio_device`/
    // `audio_device_passthrough` store the device NAME, not desc, so this is
    // purely a display-string fix with nothing to migrate on disk.
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
    // Code review, 2026-08-08: the backend suffix above only disambiguates
    // ACROSS backends — two devices under the SAME backend with the same
    // description (a real, confirmed case: two USB devices both enumerating
    // as "HD-Audio Generic/USB Stream Output" under `alsa`) still collide
    // after suffixing, reproducing the exact unselectable-second-entry bug
    // this whole fix was for, just narrower. Re-check after the backend
    // suffix and fall back to the raw device name (mpv's own identifier,
    // guaranteed unique) for anything still colliding.
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
