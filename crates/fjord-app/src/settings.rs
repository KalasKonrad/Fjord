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
//                toggle/button/action rows and for the 7 dynamic-dropdown
//                rows, whose list comes from an AppState property instead).
//   is_dynamic_dropdown(key)    the 9 rows whose list/current-desc are
//                fetched at runtime (Default Profile/Default Account — built
//                from Config.profiles itself, not a network fetch — audio
//                device ×2, font family, Seerr streaming region/display
//                language/discover language/discover region) rather than a
//                fixed compile-time list.
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
// ─────────────────────────────────────────────────────────────────────────────

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
const GEN_VIDEO_BEHIND:      &str = "general.video_behind";
const GEN_LOG_LEVEL:         &str = "general.log_level";
const GEN_PREWARM_METADATA:  &str = "general.prewarm_metadata";
const GEN_PREWARM_IMAGES:    &str = "general.prewarm_images";

// ── Profiles section rows (Phase 0 — shell; Phase 1 step 7 adds the
// launch-policy rows; Phase 2 adds Manage Profiles) ─────────────────────────
const PROF_LAUNCH_POLICY:    &str = "profiles.launch_policy";
const PROF_DEFAULT_PROFILE:  &str = "profiles.default_profile"; // virtual — only when launch_policy == "default"
// virtual — only when the active profile is itself a master account; a
// Bonfire sub-profile can't manage siblings (bonfire_list_profiles' own
// "all profiles under THIS master account" semantics — see profile_edit.rs).
const PROF_MANAGE_PROFILES:  &str = "profiles.manage_profiles";
// 2026-08-14, the 2-tier account/profile redesign — the identical
// launch-policy shape one tier up, appended after the existing
// profile-level rows (not inserted before them) per this codebase's own
// "append, don't insert" convention for exactly this reason.
const PROF_ACCOUNT_LAUNCH_POLICY: &str = "profiles.account_launch_policy";
const PROF_DEFAULT_ACCOUNT:       &str = "profiles.default_account"; // virtual — only when account_launch_policy == "default"
// Always visible, regardless of how many accounts already exist — the
// picker's own "+ Add Account" tile only shows once there's a 2nd one to
// switch between; this is the actual way to go from 1 to 2 in the first
// place.
const PROF_ADD_ACCOUNT:      &str = "profiles.add_account";
// Live-questioned 2026-08-17 ("no why to change this on the accaunt
// without sinign out and in again") — see app_state.slint's own doc
// comment on settings-remember-login for the full design. A toggle, not a
// dropdown: OFF is immediate, ON opens the confirm-password modal instead
// of flipping directly (handled entirely in profile::on_remember_login_toggle,
// not the generic toggle-row shape most other bool rows use).
const PROF_REMEMBER_LOGIN:   &str = "profiles.remember_login";
const PROF_SIGN_OUT:         &str = "profiles.sign_out";

// ── Video section rows ────────────────────────────────────────────────────────
const VID_HWDEC:               &str = "video.hwdec";
const VID_VF:                  &str = "video.vf";
const VID_DEINTERLACE:         &str = "video.deinterlace";
const VID_VIDEO_SYNC:          &str = "video.video_sync";
const VID_INTERPOLATION:       &str = "video.interpolation";
const VID_TSCALE:              &str = "video.tscale";              // virtual — only when interpolation is on
const VID_TARGET_COLORSPACE:   &str = "video.target_colorspace";
const VID_TONE_MAPPING:        &str = "video.tone_mapping";        // always visible (2026-08-15 — was
                                                                     // virtual, hidden while HDR passthrough
                                                                     // was on; see section_row_keys's own
                                                                     // comment for why that was wrong)
const VID_OPENGL_EARLY_FLUSH:  &str = "video.opengl_early_flush";
const VID_VIDEO_LATENCY_HACKS: &str = "video.video_latency_hacks"; // virtual — only when video-sync == display-resample

// ── Audio section rows ────────────────────────────────────────────────────────
const AUD_AUDIO_DEVICE:  &str = "audio.device";
const AUD_CHANNELS:      &str = "audio.channels";
const AUD_SPDIF:         &str = "audio.spdif";
const AUD_SPDIF_AC3:     &str = "audio.spdif_ac3";
const AUD_SPDIF_EAC3:    &str = "audio.spdif_eac3";
const AUD_SPDIF_DTS:     &str = "audio.spdif_dts";
const AUD_SPDIF_DTS_HD:  &str = "audio.spdif_dts_hd";
const AUD_SPDIF_TRUEHD:  &str = "audio.spdif_truehd";
const AUD_PASSTHROUGH_DEVICE: &str = "audio.passthrough_device"; // hidden when SPDIF off
const AUD_ALSA_IRQ:      &str = "audio.alsa_irq"; // virtual — hidden when SPDIF off or non-PipeWire device
const AUD_SKIP_FADE_MUTE: &str = "audio.skip_fade_mute"; // virtual — hidden when SPDIF off
const AUD_AUDIO_LANG:    &str = "audio.lang";
const AUD_GAPLESS:       &str = "audio.gapless";
const AUD_NOW_PLAYING_AUTO_OPEN: &str = "audio.now_playing_auto_open";

// ── Player (config) section rows ──────────────────────────────────────────────
const PLY_SUB_ENABLED:     &str = "player.sub_enabled";
const PLY_SUB_LANG:        &str = "player.sub_lang";
const PLY_SUB_LANG2:       &str = "player.sub_lang2";
const PLY_SUB_TYPE:        &str = "player.sub_type";
const PLY_SUB_SCALE:       &str = "player.sub_scale";
const PLY_SUB_POS:         &str = "player.sub_pos";
const PLY_SUB_RESPECT_ASS: &str = "player.sub_respect_ass";
const PLY_SUB_COLOR:       &str = "player.sub_color";      // virtual — only when !sub_respect_ass_styling
const PLY_SUB_BACKGROUND:  &str = "player.sub_background"; // virtual — only when !sub_respect_ass_styling
const PLY_CACHE_SECS:      &str = "player.cache_secs";
const PLY_CACHE_MAX_MB:    &str = "player.cache_max_mb";
const PLY_INTRO_MODE:      &str = "player.intro_mode";
const PLY_INTRO_SECS:      &str = "player.intro_secs";      // virtual — only when intro_mode == "ask-timed"
const PLY_RECAP_MODE:      &str = "player.recap_mode";
const PLY_RECAP_SECS:      &str = "player.recap_secs";      // virtual
const PLY_PREVIEW_MODE:    &str = "player.preview_mode";
const PLY_PREVIEW_SECS:    &str = "player.preview_secs";    // virtual
const PLY_COMMERCIAL_MODE: &str = "player.commercial_mode";
const PLY_COMMERCIAL_SECS: &str = "player.commercial_secs"; // virtual
const PLY_CREDITS_MODE:    &str = "player.credits_mode";
const PLY_CREDITS_SECS:    &str = "player.credits_secs";    // virtual — only when credits_mode == "ask"
const PLY_SEEK_STEP:       &str = "player.seek_step";
const PLY_SEEK_STEP_LONG:  &str = "player.seek_step_long";
const PLY_SKIP_FADE_MS:    &str = "player.skip_fade_ms";

// ── UI section rows ───────────────────────────────────────────────────────────
const UI_SCROLL_SPEED:    &str = "ui.scroll_speed";
const UI_ANIMATION_SPEED: &str = "ui.animation_speed";
const UI_FONT_FAMILY:     &str = "ui.font_family";

// ── Integrations section rows ─────────────────────────────────────────────────
const INT_SEERR_ENABLED:     &str = "integrations.seerr_enabled";
const INT_SEERR_CONNECT:     &str = "integrations.seerr_connect"; // "Connect Seerr" / "Disconnect"
const INT_STREAMING_REGION:  &str = "integrations.streaming_region";
const INT_TRAILER_QUALITY:   &str = "integrations.trailer_quality";
const INT_DISPLAY_LANGUAGE:  &str = "integrations.display_language";
const INT_DISCOVER_LANGUAGE: &str = "integrations.discover_language";
const INT_DISCOVER_REGION:   &str = "integrations.discover_region";
const INT_MANAGE_BLOCKLIST:  &str = "integrations.manage_blocklist";

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
            rows
        }
        SECTION_AUDIO => {
            let mut rows = vec![AUD_AUDIO_DEVICE, AUD_CHANNELS, AUD_SPDIF];
            if g.get_settings_audio_spdif() {
                rows.extend([
                    AUD_SPDIF_AC3, AUD_SPDIF_EAC3, AUD_SPDIF_DTS, AUD_SPDIF_DTS_HD,
                    AUD_SPDIF_TRUEHD, AUD_PASSTHROUGH_DEVICE,
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
                    PLY_SUB_LANG, PLY_SUB_LANG2, PLY_SUB_TYPE, PLY_SUB_SCALE,
                    PLY_SUB_POS, PLY_SUB_RESPECT_ASS,
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
        SECTION_UI => vec![UI_SCROLL_SPEED, UI_ANIMATION_SPEED, UI_FONT_FAMILY],
        SECTION_INTEGRATIONS => {
            let mut rows = vec![INT_SEERR_ENABLED];
            if g.get_settings_seerr_enabled() {
                rows.push(INT_SEERR_CONNECT);
                if g.get_seerr_connected() {
                    rows.extend([
                        INT_STREAMING_REGION, INT_TRAILER_QUALITY, INT_DISPLAY_LANGUAGE,
                        INT_DISCOVER_LANGUAGE, INT_DISCOVER_REGION,
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
    debug!("settings: mouse click on {key} (resolved index={idx} of {} visible rows)", rows.len());
    set_focused(g, key, idx);
}

pub(crate) fn dispatch_settings(action: &Action, g: &crate::AppState<'_>) -> Option<bool> {
    let sf = g.get_settings_focused();
    let ss = g.get_settings_section();
    debug!("settings: dispatch action={action:?} section={:?} focused={:?}", ss.as_str(), sf.as_str());

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
            debug!("settings: {ss} has {} visible row(s), current idx={idx:?}, rows={rows:?}", rows.len());
        }
        match action {
            Action::Down => {
                match idx {
                    Some(i) if i + 1 < rows.len() => set_focused(g, rows[i + 1], (i + 1) as i32),
                    None => if let Some(&first) = rows.first() { set_focused(g, first, 0); },
                    _ => {}
                }
                Some(true)
            }
            Action::Up => {
                match idx {
                    Some(0) => g.set_settings_focused("".into()),
                    Some(i) => set_focused(g, rows[i - 1], (i - 1) as i32),
                    None => if let Some(&first) = rows.first() { set_focused(g, first, 0); },
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
                    if let Some(&first) = rows.first() { set_focused(g, first, 0); }
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
                    if let Some(&first) = rows.first() { set_focused(g, first, 0); }
                    return Some(true);
                };
                settings_row_action(sf.as_str(), g);
                Some(true)
            }
            _ => None,
        }
    } else if !ss.as_str().is_empty() {
        // ── Left pane: section list navigation ───────────────────────────
        let idx = ALL_SECTIONS.iter().position(|&s| s == ss.as_str()).unwrap_or(0);
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
    "", "English", "German", "French", "Japanese", "Spanish", "Italian",
    "Portuguese", "Russian", "Korean", "Chinese", "Dutch", "Swedish",
    "Polish", "Czech", "Arabic", "Turkish", "Finnish", "Danish", "Norwegian",
];

const AUDIO_CHANNELS_MODEL: &[&str] = &[
    "auto-safe", "auto", "stereo", "5.1", "7.1", "7.1,5.1,stereo",
];

const HWDEC_MODEL: &[&str] = &[
    "auto","vulkan","vulkan-copy","nvdec","nvdec-copy",
    "vaapi","vaapi-copy","vdpau","vdpau-copy","none",
];
const VF_MODEL: &[&str] = &[
    "auto: nv12/p010","auto: yuv420p/yuv420p10le",
    "format=yuv420p","format=yuv420p10le","format=nv12","format=p010",
];
const DEINTERLACE_MODEL: &[&str] = &["no","auto","yes"];
const VIDEO_SYNC_MODEL: &[&str] = &[
    "audio","display-resample","display-vdrop","display-adrop","desync",
];
const TSCALE_MODEL: &[&str] = &[
    "oversample","catmull_rom","mitchell","gaussian","bicubic",
];
const TONE_MAPPING_MODEL: &[&str] = &[
    "auto","hable","bt.2390","reinhard","mobius","clip","gamma","linear",
];
const SUB_TYPE_MODEL:   &[&str] = &["Any","Normal","Forced","Hearing Impaired"];
// "0" = mpv's own huge default (effectively unlimited, capped by
// CACHE_MAX_MB_MODEL below) — displayed as "Unlimited" via display_val.
const CACHE_SECS_MODEL:    &[&str] = &["0","10","30","60","120","300"];
const CACHE_SECS_VALUES:   &[i32]  = &[0, 10, 30, 60, 120, 300];
// "0" here means a genuinely raised byte ceiling (mpv.rs sets
// demuxer-max-bytes to a large fixed value, not mpv's own 150 MiB stock
// default — see its own doc comment), so Cache Duration alone governs —
// also displayed as "Unlimited" via display_val.
const CACHE_MAX_MB_MODEL:  &[&str] = &["0","150","300","500","1000","2000"];
const CACHE_MAX_MB_VALUES: &[i32]  = &[0, 150, 300, 500, 1000, 2000];
const SKIP_MODE_4_MODEL: &[&str] = &["always-skip","ask","ask-timed","never-skip"];
const SKIP_MODE_3_MODEL: &[&str] = &["always-skip","ask","never-skip"];
const SKIP_SECS_MODEL:   &[&str] = &["3","5","8","10","15","20","30"];
const CREDITS_SECS_MODEL: &[&str] = &["10","15","20","30","45","60"];
const LOG_LEVEL_MODEL: &[&str] = &["error","warn","info","debug"];
const LAUNCH_POLICY_MODEL: &[&str] = &["always_ask","remember_last","default"];
const SUB_SCALE_MODEL: &[&str] = &["50","75","100","125","150","175","200"];
// mpv's real supported range is 0-150 (verified via `man mpv` 0.41.0) — 100 is
// mpv's own "default bottom" position, not the screen edge; values above 100
// push subtitles further down still. Text/ASS subs can get clipped above 100
// (a libass restriction, per the same manual page), which is why the row's
// subtitle string calls this out rather than silently allowing it.
const SUB_POS_MODEL:   &[&str] = &["50","60","70","80","90","95","100","110","120","130","140","150"];
// Display names stored directly in Config.sub_color (like LANG_MODEL stores
// display language names) — translated to an actual mpv hex color at point
// of use, not here.
const SUB_COLOR_MODEL: &[&str] = &["", "White", "Yellow", "Cyan", "Green"];
const SEEK_STEP_MODEL:      &[&str] = &["5","10","15","20","30"];
const SEEK_STEP_LONG_MODEL: &[&str] = &["15","30","45","60","120"];
// "0" = instant (today's pre-feature hard cut, display_val below reads it
// as "Off"); 200 is the shipped default. Both halves of the fade (out and
// in) use this same duration — see wire_mpv_timer's own doc comment.
const SKIP_FADE_MS_MODEL: &[&str] = &["0","100","150","200","300","400","500","750","1000"];
// Percentage is a DURATION multiplier, not a rate — bigger % means the
// transition takes longer, i.e. slower, which is the opposite of what
// "speed" suggests at a glance (see the row subtitle text in settings.slint).
const SPEED_PCT_MODEL: &[&str] = &["0","25","50","75","100","150","200","300","400","500"];
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
            "always_ask"    => "Always Ask",
            "remember_last" => "Remember Last",
            "default"       => "Default Profile",
            _               => val,
        };
    }
    if key == PROF_ACCOUNT_LAUNCH_POLICY {
        return match val {
            "always_ask"    => "Always Ask",
            "remember_last" => "Remember Last",
            "default"       => "Default Account",
            _               => val,
        };
    }
    // Skip mode display names (only for skip mode rows)
    if matches!(key, PLY_INTRO_MODE | PLY_RECAP_MODE | PLY_PREVIEW_MODE | PLY_COMMERCIAL_MODE | PLY_CREDITS_MODE) {
        return match val {
            "always-skip" => "Always skip",
            "ask"         => "Ask",
            "ask-timed"   => "Ask (timed)",
            "never-skip"  => "Never skip",
            _             => val,
        };
    }
    val
}

// Static, compile-time-fixed option lists. Returns None for toggle/button/
// action rows AND for the 7 dynamic-dropdown rows (see is_dynamic_dropdown).
fn dropdown_model(key: &str) -> Option<&'static [&'static str]> {
    match key {
        GEN_LOG_LEVEL      => Some(LOG_LEVEL_MODEL),
        PROF_LAUNCH_POLICY | PROF_ACCOUNT_LAUNCH_POLICY => Some(LAUNCH_POLICY_MODEL),
        VID_HWDEC          => Some(HWDEC_MODEL),
        VID_VF             => Some(VF_MODEL),
        VID_DEINTERLACE    => Some(DEINTERLACE_MODEL),
        VID_VIDEO_SYNC     => Some(VIDEO_SYNC_MODEL),
        VID_TSCALE         => Some(TSCALE_MODEL),
        VID_TONE_MAPPING   => Some(TONE_MAPPING_MODEL),
        AUD_CHANNELS       => Some(AUDIO_CHANNELS_MODEL),
        AUD_AUDIO_LANG | PLY_SUB_LANG | PLY_SUB_LANG2 => Some(LANG_MODEL),
        PLY_SUB_TYPE       => Some(SUB_TYPE_MODEL),
        PLY_SUB_SCALE      => Some(SUB_SCALE_MODEL),
        PLY_SUB_POS        => Some(SUB_POS_MODEL),
        PLY_SUB_COLOR      => Some(SUB_COLOR_MODEL),
        PLY_CACHE_SECS     => Some(CACHE_SECS_MODEL),
        PLY_CACHE_MAX_MB   => Some(CACHE_MAX_MB_MODEL),
        PLY_INTRO_MODE | PLY_RECAP_MODE | PLY_PREVIEW_MODE | PLY_COMMERCIAL_MODE => Some(SKIP_MODE_4_MODEL),
        PLY_CREDITS_MODE   => Some(SKIP_MODE_3_MODEL),
        PLY_INTRO_SECS | PLY_RECAP_SECS | PLY_PREVIEW_SECS | PLY_COMMERCIAL_SECS => Some(SKIP_SECS_MODEL),
        PLY_CREDITS_SECS   => Some(CREDITS_SECS_MODEL),
        PLY_SEEK_STEP      => Some(SEEK_STEP_MODEL),
        PLY_SEEK_STEP_LONG => Some(SEEK_STEP_LONG_MODEL),
        PLY_SKIP_FADE_MS   => Some(SKIP_FADE_MS_MODEL),
        UI_SCROLL_SPEED | UI_ANIMATION_SPEED => Some(SPEED_PCT_MODEL),
        INT_TRAILER_QUALITY => Some(TRAILER_QUALITY_MODEL),
        _ => None,
    }
}

// Rows whose option list + current display value come from an AppState
// property populated by an async fetch (mpv --audio-device=help, fc-list,
// or Seerr's own settings/regions/languages endpoints) rather than a fixed
// compile-time list.
fn is_dynamic_dropdown(key: &str) -> bool {
    matches!(key,
        PROF_DEFAULT_PROFILE | PROF_DEFAULT_ACCOUNT
        | AUD_AUDIO_DEVICE | AUD_PASSTHROUGH_DEVICE | UI_FONT_FAMILY
        | INT_STREAMING_REGION | INT_DISPLAY_LANGUAGE | INT_DISCOVER_LANGUAGE | INT_DISCOVER_REGION
    )
}

fn is_dropdown_row(key: &str) -> bool {
    is_dynamic_dropdown(key) || dropdown_model(key).is_some()
}

fn current_value_str(key: &str, g: &crate::AppState<'_>) -> String {
    match key {
        GEN_LOG_LEVEL      => g.get_settings_log_level().to_string(),
        PROF_LAUNCH_POLICY => g.get_settings_launch_policy().to_string(),
        PROF_ACCOUNT_LAUNCH_POLICY => g.get_settings_account_launch_policy().to_string(),
        VID_HWDEC          => g.get_settings_hwdec().to_string(),
        VID_VF             => g.get_settings_vf().to_string(),
        VID_DEINTERLACE    => g.get_settings_deinterlace().to_string(),
        VID_VIDEO_SYNC     => g.get_settings_video_sync().to_string(),
        VID_TSCALE         => g.get_settings_tscale().to_string(),
        VID_TONE_MAPPING   => g.get_settings_tone_mapping().to_string(),
        AUD_AUDIO_DEVICE       => g.get_settings_audio_device_desc().to_string(),
        AUD_CHANNELS           => g.get_settings_audio_channels().to_string(),
        AUD_PASSTHROUGH_DEVICE => g.get_settings_passthrough_device_desc().to_string(),
        AUD_AUDIO_LANG     => g.get_settings_audio_lang().to_string(),
        PLY_SUB_LANG       => g.get_settings_sub_lang().to_string(),
        PLY_SUB_LANG2      => g.get_settings_sub_lang2().to_string(),
        PLY_SUB_TYPE       => {
            let v = g.get_settings_sub_type().to_string();
            if v.is_empty() { "Any".to_string() } else { v }
        }
        PLY_SUB_SCALE      => g.get_settings_sub_scale_pct().to_string(),
        PLY_SUB_POS        => g.get_settings_sub_pos_pct().to_string(),
        PLY_SUB_COLOR      => g.get_settings_sub_color().to_string(),
        PLY_CACHE_SECS     => g.get_settings_cache_secs().to_string(),
        PLY_CACHE_MAX_MB   => g.get_settings_cache_max_mb().to_string(),
        PLY_INTRO_MODE      => g.get_settings_skip_intro_mode().to_string(),
        PLY_INTRO_SECS      => g.get_settings_skip_intro_secs().to_string(),
        PLY_RECAP_MODE      => g.get_settings_skip_recap_mode().to_string(),
        PLY_RECAP_SECS      => g.get_settings_skip_recap_secs().to_string(),
        PLY_PREVIEW_MODE    => g.get_settings_skip_preview_mode().to_string(),
        PLY_PREVIEW_SECS    => g.get_settings_skip_preview_secs().to_string(),
        PLY_COMMERCIAL_MODE => g.get_settings_skip_commercial_mode().to_string(),
        PLY_COMMERCIAL_SECS => g.get_settings_skip_commercial_secs().to_string(),
        PLY_CREDITS_MODE    => g.get_settings_skip_credits_mode().to_string(),
        PLY_CREDITS_SECS    => g.get_settings_skip_credits_secs().to_string(),
        PLY_SEEK_STEP       => g.get_settings_seek_step_secs().to_string(),
        PLY_SEEK_STEP_LONG  => g.get_settings_seek_step_long_secs().to_string(),
        PLY_SKIP_FADE_MS    => g.get_settings_skip_fade_ms().to_string(),
        UI_SCROLL_SPEED     => g.get_settings_scroll_speed_pct().to_string(),
        UI_ANIMATION_SPEED  => g.get_settings_animation_speed_pct().to_string(),
        UI_FONT_FAMILY      => g.get_settings_font_family_desc().to_string(),
        INT_STREAMING_REGION => g.get_settings_streaming_region_desc().to_string(),
        INT_TRAILER_QUALITY  => g.get_settings_trailer_quality().to_string(),
        INT_DISCOVER_REGION  => g.get_settings_discover_region_desc().to_string(),
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
        PROF_DEFAULT_PROFILE   => Some((g.get_settings_default_profile_display(), g.get_settings_default_profile_desc())),
        PROF_DEFAULT_ACCOUNT   => Some((g.get_settings_default_account_display(), g.get_settings_default_account_desc())),
        AUD_AUDIO_DEVICE       => Some((g.get_settings_audio_device_display(), g.get_settings_audio_device_desc())),
        AUD_PASSTHROUGH_DEVICE => Some((g.get_settings_audio_device_display(), g.get_settings_passthrough_device_desc())),
        UI_FONT_FAMILY         => Some((g.get_settings_font_family_display(), g.get_settings_font_family_desc())),
        INT_STREAMING_REGION   => Some((g.get_settings_streaming_region_display(), g.get_settings_streaming_region_desc())),
        INT_DISPLAY_LANGUAGE   => Some((g.get_settings_display_language_display(), g.get_settings_display_language_desc())),
        INT_DISCOVER_LANGUAGE  => Some((g.get_settings_discover_language_display(), g.get_settings_discover_language_desc())),
        // Discover Region reuses Streaming Region's own already-fetched
        // list — the two settings share one region catalog, just a
        // different desc value for which one's currently set.
        INT_DISCOVER_REGION    => Some((g.get_settings_streaming_region_display(), g.get_settings_discover_region_desc())),
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
        debug!("settings: open dynamic dropdown {key} ({n} options, cursor={cursor}, current={current_display})");
        g.set_settings_dropdown_model(ModelRc::new(VecModel::from(items)));
        g.set_settings_dropdown_display(current_display);
        g.set_settings_dropdown_cursor(cursor);
        g.set_settings_dropdown_open(true);
        return;
    }
    let Some(model) = dropdown_model(key) else {
        debug!("settings: open_dropdown_popup({key}) — no model, not a dropdown row (bug if this fires)");
        return;
    };
    let current = current_value_str(key, g);
    let cursor = model.iter().position(|&v| v == current.as_str()).unwrap_or(0) as i32;
    let display_items: Vec<SharedString> = model.iter().map(|&v| display_val(v, key).into()).collect();
    let current_display: SharedString = display_val(current.as_str(), key).into();
    debug!("settings: open static dropdown {key} ({} options, cursor={cursor}, current={current})", model.len());
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
            if let Some(desc) = display.row_data(cursor as usize) { g.invoke_default_profile_selected(desc); }
            return;
        }
        PROF_DEFAULT_ACCOUNT => {
            let display = g.get_settings_default_account_display();
            if let Some(desc) = display.row_data(cursor as usize) { g.invoke_default_account_selected(desc); }
            return;
        }
        AUD_AUDIO_DEVICE | AUD_PASSTHROUGH_DEVICE => {
            let display = g.get_settings_audio_device_display();
            if let Some(desc) = display.row_data(cursor as usize) {
                if key == AUD_AUDIO_DEVICE { g.invoke_audio_device_selected(desc); }
                else { g.invoke_passthrough_device_selected(desc); }
            }
            return;
        }
        UI_FONT_FAMILY => {
            let display = g.get_settings_font_family_display();
            if let Some(desc) = display.row_data(cursor as usize) { g.invoke_font_family_selected(desc); }
            return;
        }
        INT_STREAMING_REGION => {
            let display = g.get_settings_streaming_region_display();
            if let Some(desc) = display.row_data(cursor as usize) { g.invoke_streaming_region_selected(desc); }
            return;
        }
        INT_DISPLAY_LANGUAGE => {
            let display = g.get_settings_display_language_display();
            if let Some(desc) = display.row_data(cursor as usize) { g.invoke_display_language_selected(desc); }
            return;
        }
        INT_DISCOVER_LANGUAGE => {
            let display = g.get_settings_discover_language_display();
            if let Some(desc) = display.row_data(cursor as usize) { g.invoke_discover_language_selected(desc); }
            return;
        }
        INT_DISCOVER_REGION => {
            let display = g.get_settings_streaming_region_display();
            if let Some(desc) = display.row_data(cursor as usize) { g.invoke_discover_region_selected(desc); }
            return;
        }
        _ => {}
    }
    let Some(model) = dropdown_model(key) else { return };
    let Some(&val) = model.get(cursor as usize) else { return };
    match key {
        GEN_LOG_LEVEL      => g.set_settings_log_level(val.into()),
        PROF_LAUNCH_POLICY => g.set_settings_launch_policy(val.into()),
        PROF_ACCOUNT_LAUNCH_POLICY => g.set_settings_account_launch_policy(val.into()),
        VID_HWDEC          => g.set_settings_hwdec(val.into()),
        VID_VF             => g.set_settings_vf(val.into()),
        VID_DEINTERLACE    => g.set_settings_deinterlace(val.into()),
        VID_VIDEO_SYNC     => g.set_settings_video_sync(val.into()),
        VID_TSCALE         => g.set_settings_tscale(val.into()),
        VID_TONE_MAPPING   => g.set_settings_tone_mapping(val.into()),
        AUD_CHANNELS       => g.set_settings_audio_channels(val.into()),
        AUD_AUDIO_LANG     => g.set_settings_audio_lang(val.into()),
        PLY_SUB_LANG       => g.set_settings_sub_lang(val.into()),
        PLY_SUB_LANG2      => g.set_settings_sub_lang2(val.into()),
        PLY_SUB_TYPE       => g.set_settings_sub_type(if val == "Any" { "".into() } else { val.into() }),
        PLY_SUB_SCALE      => g.set_settings_sub_scale_pct(val.parse().unwrap_or(100)),
        PLY_SUB_POS        => g.set_settings_sub_pos_pct(val.parse().unwrap_or(100)),
        PLY_SUB_COLOR      => g.set_settings_sub_color(val.into()),
        PLY_CACHE_SECS     => g.set_settings_cache_secs(val.parse().unwrap_or(60)),
        PLY_CACHE_MAX_MB   => g.set_settings_cache_max_mb(val.parse().unwrap_or(500)),
        PLY_INTRO_MODE      => g.set_settings_skip_intro_mode(val.into()),
        PLY_INTRO_SECS      => g.set_settings_skip_intro_secs(val.parse().unwrap_or(8)),
        PLY_RECAP_MODE      => g.set_settings_skip_recap_mode(val.into()),
        PLY_RECAP_SECS      => g.set_settings_skip_recap_secs(val.parse().unwrap_or(8)),
        PLY_PREVIEW_MODE    => g.set_settings_skip_preview_mode(val.into()),
        PLY_PREVIEW_SECS    => g.set_settings_skip_preview_secs(val.parse().unwrap_or(8)),
        PLY_COMMERCIAL_MODE => g.set_settings_skip_commercial_mode(val.into()),
        PLY_COMMERCIAL_SECS => g.set_settings_skip_commercial_secs(val.parse().unwrap_or(8)),
        PLY_CREDITS_MODE    => g.set_settings_skip_credits_mode(val.into()),
        PLY_CREDITS_SECS    => g.set_settings_skip_credits_secs(val.parse().unwrap_or(30)),
        PLY_SEEK_STEP       => g.set_settings_seek_step_secs(val.parse().unwrap_or(10)),
        PLY_SEEK_STEP_LONG  => g.set_settings_seek_step_long_secs(val.parse().unwrap_or(30)),
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
        if n == 0 { return None; }
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
            g.set_settings_log_level(v.into()); g.invoke_settings_changed();
        }
        GEN_PREWARM_METADATA => g.invoke_prewarm_metadata(),
        GEN_PREWARM_IMAGES   => g.invoke_prewarm_images(),

        PROF_LAUNCH_POLICY => {
            let v = cycle(g.get_settings_launch_policy().as_str(), LAUNCH_POLICY_MODEL);
            g.set_settings_launch_policy(v.into()); g.invoke_settings_changed();
        }
        PROF_DEFAULT_PROFILE => {
            if let Some(desc) = cycle_dynamic(g.get_settings_default_profile_display(), g.get_settings_default_profile_desc().as_str()) {
                g.invoke_default_profile_selected(desc);
            }
        }
        PROF_MANAGE_PROFILES => g.invoke_open_manage_profiles(),
        PROF_ACCOUNT_LAUNCH_POLICY => {
            let v = cycle(g.get_settings_account_launch_policy().as_str(), LAUNCH_POLICY_MODEL);
            g.set_settings_account_launch_policy(v.into()); g.invoke_settings_changed();
        }
        PROF_DEFAULT_ACCOUNT => {
            if let Some(desc) = cycle_dynamic(g.get_settings_default_account_display(), g.get_settings_default_account_desc().as_str()) {
                g.invoke_default_account_selected(desc);
            }
        }
        PROF_ADD_ACCOUNT => g.invoke_settings_add_account(),
        PROF_REMEMBER_LOGIN => g.invoke_settings_remember_login_toggle(),
        PROF_SIGN_OUT => g.invoke_sign_out(),

        VID_HWDEC => {
            let v = cycle(g.get_settings_hwdec().as_str(), HWDEC_MODEL);
            g.set_settings_hwdec(v.into()); g.invoke_settings_changed();
        }
        VID_VF => {
            let v = cycle(g.get_settings_vf().as_str(), VF_MODEL);
            g.set_settings_vf(v.into()); g.invoke_settings_changed();
        }
        VID_DEINTERLACE => {
            let v = cycle(g.get_settings_deinterlace().as_str(), DEINTERLACE_MODEL);
            g.set_settings_deinterlace(v.into()); g.invoke_settings_changed();
        }
        VID_VIDEO_SYNC => {
            let v = cycle(g.get_settings_video_sync().as_str(), VIDEO_SYNC_MODEL);
            g.set_settings_video_sync(v.into()); g.invoke_settings_changed();
        }
        VID_INTERPOLATION => {
            g.set_settings_interpolation(!g.get_settings_interpolation());
            g.invoke_settings_changed();
        }
        VID_TSCALE => {
            let v = cycle(g.get_settings_tscale().as_str(), TSCALE_MODEL);
            g.set_settings_tscale(v.into()); g.invoke_settings_changed();
        }
        VID_TONE_MAPPING => {
            let v = cycle(g.get_settings_tone_mapping().as_str(), TONE_MAPPING_MODEL);
            g.set_settings_tone_mapping(v.into()); g.invoke_settings_changed();
        }
        VID_TARGET_COLORSPACE => {
            g.set_settings_target_colorspace_hint(!g.get_settings_target_colorspace_hint());
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

        AUD_AUDIO_DEVICE => {
            if let Some(desc) = cycle_dynamic(g.get_settings_audio_device_display(), g.get_settings_audio_device_desc().as_str()) {
                g.invoke_audio_device_selected(desc);
            }
        }
        AUD_PASSTHROUGH_DEVICE => {
            if let Some(desc) = cycle_dynamic(g.get_settings_audio_device_display(), g.get_settings_passthrough_device_desc().as_str()) {
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
            let v = cycle(g.get_settings_audio_channels().as_str(), AUDIO_CHANNELS_MODEL);
            g.set_settings_audio_channels(v.into()); g.invoke_settings_changed();
        }
        AUD_AUDIO_LANG => {
            let v = cycle(g.get_settings_audio_lang().as_str(), LANG_MODEL);
            g.set_settings_audio_lang(v.into()); g.invoke_settings_changed();
        }
        AUD_GAPLESS => {
            g.set_settings_gapless_audio(!g.get_settings_gapless_audio());
            g.invoke_settings_changed();
        }
        AUD_NOW_PLAYING_AUTO_OPEN => {
            g.set_settings_now_playing_auto_open(!g.get_settings_now_playing_auto_open());
            g.invoke_settings_changed();
        }

        PLY_SUB_ENABLED => {
            g.set_settings_sub_enabled(!g.get_settings_sub_enabled());
            g.invoke_settings_changed();
        }
        PLY_SUB_LANG => {
            let v = cycle(g.get_settings_sub_lang().as_str(), LANG_MODEL);
            g.set_settings_sub_lang(v.into()); g.invoke_settings_changed();
        }
        PLY_SUB_LANG2 => {
            let v = cycle(g.get_settings_sub_lang2().as_str(), LANG_MODEL);
            g.set_settings_sub_lang2(v.into()); g.invoke_settings_changed();
        }
        PLY_SUB_TYPE => {
            let current = g.get_settings_sub_type().to_string();
            let current = if current.is_empty() { "Any" } else { &current };
            let v = cycle(current, SUB_TYPE_MODEL);
            g.set_settings_sub_type(if v == "Any" { "".into() } else { v.into() });
            g.invoke_settings_changed();
        }
        PLY_SUB_SCALE => {
            let v = cycle(g.get_settings_sub_scale_pct().to_string().as_str(), SUB_SCALE_MODEL);
            g.set_settings_sub_scale_pct(v.parse().unwrap_or(100)); g.invoke_settings_changed();
        }
        PLY_SUB_POS => {
            let v = cycle(g.get_settings_sub_pos_pct().to_string().as_str(), SUB_POS_MODEL);
            g.set_settings_sub_pos_pct(v.parse().unwrap_or(100)); g.invoke_settings_changed();
        }
        PLY_SUB_RESPECT_ASS => {
            g.set_settings_sub_respect_ass_styling(!g.get_settings_sub_respect_ass_styling());
            g.invoke_settings_changed();
        }
        PLY_SUB_COLOR => {
            let current = g.get_settings_sub_color().to_string();
            let v = cycle(current.as_str(), SUB_COLOR_MODEL);
            g.set_settings_sub_color(v.into()); g.invoke_settings_changed();
        }
        PLY_SUB_BACKGROUND => {
            g.set_settings_sub_background(!g.get_settings_sub_background());
            g.invoke_settings_changed();
        }
        PLY_CACHE_SECS => {
            let next = cycle_i32(g.get_settings_cache_secs(), CACHE_SECS_VALUES);
            g.set_settings_cache_secs(next); g.invoke_settings_changed();
        }
        PLY_CACHE_MAX_MB => {
            let next = cycle_i32(g.get_settings_cache_max_mb(), CACHE_MAX_MB_VALUES);
            g.set_settings_cache_max_mb(next); g.invoke_settings_changed();
        }
        PLY_INTRO_MODE => {
            let v = cycle(g.get_settings_skip_intro_mode().as_str(), SKIP_MODE_4_MODEL);
            g.set_settings_skip_intro_mode(v.into()); g.invoke_settings_changed();
        }
        PLY_INTRO_SECS => {
            let v = cycle(g.get_settings_skip_intro_secs().to_string().as_str(), SKIP_SECS_MODEL);
            g.set_settings_skip_intro_secs(v.parse().unwrap_or(8)); g.invoke_settings_changed();
        }
        PLY_RECAP_MODE => {
            let v = cycle(g.get_settings_skip_recap_mode().as_str(), SKIP_MODE_4_MODEL);
            g.set_settings_skip_recap_mode(v.into()); g.invoke_settings_changed();
        }
        PLY_RECAP_SECS => {
            let v = cycle(g.get_settings_skip_recap_secs().to_string().as_str(), SKIP_SECS_MODEL);
            g.set_settings_skip_recap_secs(v.parse().unwrap_or(8)); g.invoke_settings_changed();
        }
        PLY_PREVIEW_MODE => {
            let v = cycle(g.get_settings_skip_preview_mode().as_str(), SKIP_MODE_4_MODEL);
            g.set_settings_skip_preview_mode(v.into()); g.invoke_settings_changed();
        }
        PLY_PREVIEW_SECS => {
            let v = cycle(g.get_settings_skip_preview_secs().to_string().as_str(), SKIP_SECS_MODEL);
            g.set_settings_skip_preview_secs(v.parse().unwrap_or(8)); g.invoke_settings_changed();
        }
        PLY_COMMERCIAL_MODE => {
            let v = cycle(g.get_settings_skip_commercial_mode().as_str(), SKIP_MODE_4_MODEL);
            g.set_settings_skip_commercial_mode(v.into()); g.invoke_settings_changed();
        }
        PLY_COMMERCIAL_SECS => {
            let v = cycle(g.get_settings_skip_commercial_secs().to_string().as_str(), SKIP_SECS_MODEL);
            g.set_settings_skip_commercial_secs(v.parse().unwrap_or(8)); g.invoke_settings_changed();
        }
        PLY_CREDITS_MODE => {
            let v = cycle(g.get_settings_skip_credits_mode().as_str(), SKIP_MODE_3_MODEL);
            g.set_settings_skip_credits_mode(v.into()); g.invoke_settings_changed();
        }
        PLY_CREDITS_SECS => {
            let v = cycle(g.get_settings_skip_credits_secs().to_string().as_str(), CREDITS_SECS_MODEL);
            g.set_settings_skip_credits_secs(v.parse().unwrap_or(30)); g.invoke_settings_changed();
        }
        PLY_SEEK_STEP => {
            let v = cycle(g.get_settings_seek_step_secs().to_string().as_str(), SEEK_STEP_MODEL);
            g.set_settings_seek_step_secs(v.parse().unwrap_or(10)); g.invoke_settings_changed();
        }
        PLY_SEEK_STEP_LONG => {
            let v = cycle(g.get_settings_seek_step_long_secs().to_string().as_str(), SEEK_STEP_LONG_MODEL);
            g.set_settings_seek_step_long_secs(v.parse().unwrap_or(30)); g.invoke_settings_changed();
        }
        PLY_SKIP_FADE_MS => {
            let v = cycle(g.get_settings_skip_fade_ms().to_string().as_str(), SKIP_FADE_MS_MODEL);
            g.set_settings_skip_fade_ms(v.parse().unwrap_or(200)); g.invoke_settings_changed();
        }

        UI_SCROLL_SPEED => {
            let v = cycle(g.get_settings_scroll_speed_pct().to_string().as_str(), SPEED_PCT_MODEL);
            let pct: i32 = v.parse().unwrap_or(100);
            g.set_settings_scroll_speed_pct(pct);
            g.set_settings_scroll_speed(pct as f32 / 100.0);
            g.invoke_settings_changed();
        }
        UI_ANIMATION_SPEED => {
            let v = cycle(g.get_settings_animation_speed_pct().to_string().as_str(), SPEED_PCT_MODEL);
            let pct: i32 = v.parse().unwrap_or(100);
            g.set_settings_animation_speed_pct(pct);
            g.set_settings_animation_speed(pct as f32 / 100.0);
            g.invoke_settings_changed();
        }
        UI_FONT_FAMILY => {
            if let Some(desc) = cycle_dynamic(g.get_settings_font_family_display(), g.get_settings_font_family_desc().as_str()) {
                g.invoke_font_family_selected(desc);
            }
        }

        INT_SEERR_ENABLED => {
            g.set_settings_seerr_enabled(!g.get_settings_seerr_enabled());
            g.invoke_settings_changed();
        }
        INT_SEERR_CONNECT => {
            if g.get_seerr_connected() { g.invoke_seerr_disconnect(); } else { g.invoke_open_connect_seerr(); }
        }
        INT_STREAMING_REGION => {
            if let Some(desc) = cycle_dynamic(g.get_settings_streaming_region_display(), g.get_settings_streaming_region_desc().as_str()) {
                g.invoke_streaming_region_selected(desc);
            }
        }
        INT_TRAILER_QUALITY => {
            let v = cycle(g.get_settings_trailer_quality().to_string().as_str(), TRAILER_QUALITY_MODEL);
            g.set_settings_trailer_quality(v.into()); g.invoke_settings_changed();
        }
        INT_DISPLAY_LANGUAGE => {
            if let Some(desc) = cycle_dynamic(g.get_settings_display_language_display(), g.get_settings_display_language_desc().as_str()) {
                g.invoke_display_language_selected(desc);
            }
        }
        INT_DISCOVER_LANGUAGE => {
            if let Some(desc) = cycle_dynamic(g.get_settings_discover_language_display(), g.get_settings_discover_language_desc().as_str()) {
                g.invoke_discover_language_selected(desc);
            }
        }
        INT_DISCOVER_REGION => {
            if let Some(desc) = cycle_dynamic(g.get_settings_streaming_region_display(), g.get_settings_discover_region_desc().as_str()) {
                g.invoke_discover_region_selected(desc);
            }
        }
        // Plain button row (2026-08-06, Seerr Blocklist support) — same
        // shape as INT_SEERR_CONNECT above, no dropdown to cycle.
        INT_MANAGE_BLOCKLIST => g.invoke_open_blocklist(),

        _ => {}
    }
}
