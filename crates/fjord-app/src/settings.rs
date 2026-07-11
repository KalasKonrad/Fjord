// ── fjord-app · settings.rs ───────────────────────────────────────────────────
//   Section constants     SECTION_GENERAL, SECTION_VIDEO, SECTION_AUDIO,
//                         SECTION_PLAYER_CFG, SECTION_KEYBINDINGS
//   General row consts    GEN_LAUNCH_FULLSCREEN, GEN_VIDEO_BEHIND, GEN_LOG_LEVEL,
//                         GEN_PREWARM_METADATA, GEN_PREWARM_IMAGES, GEN_SIGN_OUT
//   Video row consts      VID_HWDEC … VID_VIDEO_LATENCY_HACKS (VID_TSCALE virtual)
//   Audio row consts      AUD_AUDIO_DEVICE, AUD_SPDIF, AUD_SPDIF_AC3, AUD_SPDIF_EAC3,
//                         AUD_CHANNELS, AUD_SPDIF_DTS, AUD_SPDIF_DTS_HD, AUD_SPDIF_TRUEHD,
//                         AUD_PASSTHROUGH_DEVICE, AUD_ALSA_IRQ, AUD_AUDIO_LANG, AUD_GAPLESS,
//                         AUD_NOW_PLAYING_AUTO_OPEN
//   Player row consts     PLY_SUB_ENABLED (0), PLY_SUB_LANG (1), PLY_SUB_LANG2 (2),
//                         PLY_SUB_TYPE (3, hidden when disabled), PLY_CACHE_MB (4),
//                         PLY_INTRO_MODE (5), PLY_INTRO_SECS (6 virtual),
//                         PLY_RECAP_MODE (7),  PLY_RECAP_SECS (8 virtual),
//                         PLY_PREVIEW_MODE (9), PLY_PREVIEW_SECS (10 virtual),
//                         PLY_COMMERCIAL_MODE (11), PLY_COMMERCIAL_SECS (12 virtual),
//                         PLY_CREDITS_MODE (13), PLY_CREDITS_SECS (14 virtual)
//   dispatch_settings     keyboard nav for the settings screen (three-state:
//                           sidebar → left pane → right pane / keybindings;
//                           Enter opens dropdown popup; Up/Down/Enter/Esc navigate popup)
//   dropdown_model        model strings for each dropdown row (None for toggle rows)
//   current_value_str     current AppState value as string for a given (section, row)
//   apply_dropdown_selection  apply model[cursor] to AppState for a given (section, row)
//   settings_row_action   per-row Left/Right cycle action handler
// ─────────────────────────────────────────────────────────────────────────────

use crate::keys::Action;
use slint::{Model, ModelRc, SharedString, VecModel};

// ── Section indices ───────────────────────────────────────────────────────────
pub(crate) const SECTION_GENERAL:     i32 = 0;
pub(crate) const SECTION_VIDEO:       i32 = 1;
pub(crate) const SECTION_AUDIO:       i32 = 2;
pub(crate) const SECTION_PLAYER_CFG:  i32 = 3;
pub(crate) const SECTION_KEYBINDINGS: i32 = 4;
const SECTION_MAX: i32 = SECTION_KEYBINDINGS;

// ── General section rows ──────────────────────────────────────────────────────
const GEN_LAUNCH_FULLSCREEN:   i32 = 0;
const GEN_VIDEO_BEHIND:        i32 = 1;
const GEN_LOG_LEVEL:           i32 = 2;
const GEN_PREWARM_METADATA:    i32 = 3;
const GEN_PREWARM_IMAGES:      i32 = 4;
const GEN_SIGN_OUT:            i32 = 5;

// ── Video section rows ────────────────────────────────────────────────────────
const VID_HWDEC:               i32 = 0;
const VID_VF:                  i32 = 1;
const VID_DEINTERLACE:         i32 = 2;
const VID_VIDEO_SYNC:          i32 = 3;
const VID_INTERPOLATION:       i32 = 4;
const VID_TSCALE:              i32 = 5;  // virtual — only shown when interpolation is on
const VID_TARGET_COLORSPACE:   i32 = 6;
const VID_TONE_MAPPING:        i32 = 7;  // virtual — only shown when HDR passthrough is off
const VID_OPENGL_EARLY_FLUSH:  i32 = 8;
const VID_VIDEO_LATENCY_HACKS: i32 = 9;

// ── Audio section rows ────────────────────────────────────────────────────────
const AUD_AUDIO_DEVICE:  i32 = 0;
const AUD_CHANNELS:      i32 = 1;  // mpv --audio-channels
const AUD_SPDIF:         i32 = 2;
const AUD_SPDIF_AC3:     i32 = 3;
const AUD_SPDIF_EAC3:    i32 = 4;
const AUD_SPDIF_DTS:     i32 = 5;
const AUD_SPDIF_DTS_HD:  i32 = 6;
const AUD_SPDIF_TRUEHD:  i32 = 7;
const AUD_PASSTHROUGH_DEVICE: i32 = 8;  // hidden when SPDIF off; "" = same as audio device
const AUD_ALSA_IRQ:      i32 = 9;  // virtual — hidden when SPDIF off or non-PipeWire device
const AUD_AUDIO_LANG:    i32 = 10;
const AUD_GAPLESS:       i32 = 11;
const AUD_NOW_PLAYING_AUTO_OPEN: i32 = 12;

// ── Player (config) section rows ──────────────────────────────────────────────
const PLY_SUB_ENABLED:     i32 = 0;
const PLY_SUB_LANG:        i32 = 1;
const PLY_SUB_LANG2:       i32 = 2;
const PLY_SUB_TYPE:        i32 = 3;  // hidden + indented when sub_enabled is off
const PLY_CACHE_MB:        i32 = 4;
const PLY_INTRO_MODE:      i32 = 5;
const PLY_INTRO_SECS:      i32 = 6;  // virtual — only when intro_mode == "ask-timed"
const PLY_RECAP_MODE:      i32 = 7;
const PLY_RECAP_SECS:      i32 = 8;  // virtual — only when recap_mode == "ask-timed"
const PLY_PREVIEW_MODE:    i32 = 9;
const PLY_PREVIEW_SECS:    i32 = 10; // virtual — only when preview_mode == "ask-timed"
const PLY_COMMERCIAL_MODE: i32 = 11;
const PLY_COMMERCIAL_SECS: i32 = 12; // virtual — only when commercial_mode == "ask-timed"
const PLY_CREDITS_MODE:    i32 = 13;
const PLY_CREDITS_SECS:    i32 = 14; // virtual — only when credits_mode == "ask"

// ── Main dispatch ─────────────────────────────────────────────────────────────

pub(crate) fn dispatch_settings(action: &Action, g: &crate::AppState<'_>) -> Option<bool> {
    let sf = g.get_settings_focused();
    let ss = g.get_settings_section();

    // ── Dropdown popup open: intercept all input for in-popup navigation ──────
    if g.get_settings_dropdown_open() {
        if ss < 0 || sf < 0 {
            g.set_settings_dropdown_open(false);
            return None;
        }
        let model_len = dropdown_model(ss, sf)
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
                    apply_dropdown_selection(ss, sf, cursor, g);
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

    if sf >= 0 {
        // ── Right pane: row navigation ────────────────────────────────────
        let max_row = match ss {
            SECTION_GENERAL    => GEN_SIGN_OUT,   // 5
            SECTION_VIDEO      => if g.get_settings_video_sync().as_str() == "display-resample" {
                                      VID_VIDEO_LATENCY_HACKS
                                  } else {
                                      VID_OPENGL_EARLY_FLUSH
                                  },
            SECTION_AUDIO      => AUD_NOW_PLAYING_AUTO_OPEN,   // 12
            SECTION_PLAYER_CFG => if g.get_settings_skip_credits_mode().as_str() == "ask" {
                                      PLY_CREDITS_SECS
                                  } else {
                                      PLY_CREDITS_MODE
                                  },
            _                  => 0,
        };
        match action {
            Action::Down => {
                if sf < max_row {
                    let mut next = sf + 1;
                    if ss == SECTION_VIDEO && next == VID_TSCALE
                       && !g.get_settings_interpolation()
                    {
                        next = VID_TARGET_COLORSPACE;
                    }
                    if ss == SECTION_VIDEO && next == VID_TONE_MAPPING
                       && g.get_settings_target_colorspace_hint()
                    {
                        next = VID_OPENGL_EARLY_FLUSH;
                    }
                    if ss == SECTION_AUDIO && !g.get_settings_audio_spdif()
                       && (AUD_SPDIF_AC3..=AUD_ALSA_IRQ).contains(&next)
                    {
                        next = AUD_AUDIO_LANG;  // skip rows 2–8 when SPDIF off
                    }
                    if ss == SECTION_AUDIO && next == AUD_ALSA_IRQ
                       && !g.get_settings_device_is_pipewire()
                    {
                        next = AUD_AUDIO_LANG;  // skip IRQ row when non-PipeWire device selected
                    }
                    if ss == SECTION_PLAYER_CFG && !g.get_settings_sub_enabled()
                       && matches!(next, PLY_SUB_LANG | PLY_SUB_LANG2 | PLY_SUB_TYPE)
                    {
                        next = PLY_CACHE_MB;
                    }
                    // Skip *_SECS rows when the corresponding mode is not "ask-timed"
                    if ss == SECTION_PLAYER_CFG && next == PLY_INTRO_SECS
                       && g.get_settings_skip_intro_mode().as_str() != "ask-timed"
                    {
                        next = PLY_RECAP_MODE;
                    }
                    if ss == SECTION_PLAYER_CFG && next == PLY_RECAP_SECS
                       && g.get_settings_skip_recap_mode().as_str() != "ask-timed"
                    {
                        next = PLY_PREVIEW_MODE;
                    }
                    if ss == SECTION_PLAYER_CFG && next == PLY_PREVIEW_SECS
                       && g.get_settings_skip_preview_mode().as_str() != "ask-timed"
                    {
                        next = PLY_COMMERCIAL_MODE;
                    }
                    if ss == SECTION_PLAYER_CFG && next == PLY_COMMERCIAL_SECS
                       && g.get_settings_skip_commercial_mode().as_str() != "ask-timed"
                    {
                        next = PLY_CREDITS_MODE;
                    }
                    g.set_settings_focused(next);
                }
                Some(true)
            }
            Action::Up => {
                if sf == 0 {
                    g.set_settings_focused(-1);
                } else {
                    let mut prev = sf - 1;
                    if ss == SECTION_VIDEO && prev == VID_TSCALE
                       && !g.get_settings_interpolation()
                    {
                        prev = VID_INTERPOLATION;
                    }
                    if ss == SECTION_VIDEO && prev == VID_TONE_MAPPING
                       && g.get_settings_target_colorspace_hint()
                    {
                        prev = VID_TARGET_COLORSPACE;
                    }
                    if ss == SECTION_AUDIO && !g.get_settings_audio_spdif()
                       && (AUD_SPDIF_AC3..=AUD_ALSA_IRQ).contains(&prev)
                    {
                        prev = AUD_SPDIF;  // skip rows 2–8 when SPDIF off
                    }
                    if ss == SECTION_AUDIO && prev == AUD_ALSA_IRQ
                       && !g.get_settings_device_is_pipewire()
                    {
                        prev = AUD_PASSTHROUGH_DEVICE;  // skip IRQ row when non-PipeWire device selected
                    }
                    if ss == SECTION_PLAYER_CFG && !g.get_settings_sub_enabled()
                       && matches!(prev, PLY_SUB_LANG | PLY_SUB_LANG2 | PLY_SUB_TYPE)
                    {
                        prev = PLY_SUB_ENABLED;
                    }
                    // Skip *_SECS rows when the corresponding mode is not "ask-timed"
                    if ss == SECTION_PLAYER_CFG && prev == PLY_INTRO_SECS
                       && g.get_settings_skip_intro_mode().as_str() != "ask-timed"
                    {
                        prev = PLY_INTRO_MODE;
                    }
                    if ss == SECTION_PLAYER_CFG && prev == PLY_RECAP_SECS
                       && g.get_settings_skip_recap_mode().as_str() != "ask-timed"
                    {
                        prev = PLY_RECAP_MODE;
                    }
                    if ss == SECTION_PLAYER_CFG && prev == PLY_PREVIEW_SECS
                       && g.get_settings_skip_preview_mode().as_str() != "ask-timed"
                    {
                        prev = PLY_PREVIEW_MODE;
                    }
                    if ss == SECTION_PLAYER_CFG && prev == PLY_COMMERCIAL_SECS
                       && g.get_settings_skip_commercial_mode().as_str() != "ask-timed"
                    {
                        prev = PLY_COMMERCIAL_MODE;
                    }
                    if ss == SECTION_PLAYER_CFG && prev == PLY_CREDITS_SECS
                       && g.get_settings_skip_credits_mode().as_str() != "ask"
                    {
                        prev = PLY_CREDITS_MODE;
                    }
                    g.set_settings_focused(prev);
                }
                Some(true)
            }
            Action::Back | Action::Left => {
                g.set_settings_focused(-1);
                Some(true)
            }
            Action::Confirm => {
                // Dropdown rows: show overlay with cursor on current value.
                // Toggle/action rows: activate directly.
                if ss == SECTION_AUDIO && (sf == AUD_AUDIO_DEVICE || sf == AUD_PASSTHROUGH_DEVICE) {
                    let display = g.get_settings_audio_device_display();
                    let n = display.row_count();
                    let current_desc = if sf == AUD_AUDIO_DEVICE {
                        g.get_settings_audio_device_desc().to_string()
                    } else {
                        g.get_settings_passthrough_device_desc().to_string()
                    };
                    let cursor = (0..n)
                        .find(|&i| display.row_data(i).map(|s| s.to_string()) == Some(current_desc.clone()))
                        .unwrap_or(0) as i32;
                    let items: Vec<SharedString> = (0..n).filter_map(|i| display.row_data(i)).collect();
                    let current_display = items.get(cursor as usize).cloned().unwrap_or_default();
                    g.set_settings_dropdown_model(ModelRc::new(VecModel::from(items)));
                    g.set_settings_dropdown_display(current_display);
                    g.set_settings_dropdown_cursor(cursor);
                    g.set_settings_dropdown_open(true);
                } else if let Some(model) = dropdown_model(ss, sf) {
                    let current = current_value_str(ss, sf, g);
                    let cursor = model.iter()
                        .position(|&v| v == current.as_str())
                        .unwrap_or(0) as i32;
                    let display_items: Vec<SharedString> = model.iter()
                        .map(|&v| display_val(v, ss, sf).into())
                        .collect();
                    let current_display: SharedString =
                        display_val(current.as_str(), ss, sf).into();
                    g.set_settings_dropdown_model(ModelRc::new(VecModel::from(display_items)));
                    g.set_settings_dropdown_display(current_display);
                    g.set_settings_dropdown_cursor(cursor);
                    g.set_settings_dropdown_open(true);
                } else {
                    settings_row_action(sf, true, ss, g);
                }
                Some(true)
            }
            Action::Right => {
                settings_row_action(sf, true, ss, g);
                Some(true)
            }
            _ => None,
        }
    } else if ss >= 0 {
        // ── Left pane: section list navigation ───────────────────────────
        match action {
            Action::Down => {
                g.set_settings_section((ss + 1).min(SECTION_MAX));
                Some(true)
            }
            Action::Up => {
                g.set_settings_section((ss - 1).max(SECTION_GENERAL));
                Some(true)
            }
            Action::Right | Action::Confirm => {
                if ss == SECTION_KEYBINDINGS {
                    g.set_keybinding_focused(0);
                } else {
                    g.set_settings_focused(0);
                }
                Some(true)
            }
            Action::Back | Action::Left => {
                g.set_settings_section(-1);
                Some(true)
            }
            _ => None,
        }
    } else {
        // ── Sidebar (ss == -1): Right/Enter enters left pane ─────────────
        match action {
            Action::Right | Action::Confirm => {
                g.set_settings_section(SECTION_GENERAL);
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
    "","auto","format=yuv420p","format=yuv420p10le","format=nv12","format=p010",
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
const CACHE_MB_MODEL:   &[&str] = &["0","50","150","300","500","1000"];
const CACHE_MB_VALUES:  &[i32]  = &[0, 50, 150, 300, 500, 1000];
const SKIP_MODE_4_MODEL: &[&str] = &["always-skip","ask","ask-timed","never-skip"];
const SKIP_MODE_3_MODEL: &[&str] = &["always-skip","ask","never-skip"];
const SKIP_SECS_MODEL:   &[&str] = &["3","5","8","10","15","20","30"];
const CREDITS_SECS_MODEL: &[&str] = &["10","15","20","30","45","60"];
const LOG_LEVEL_MODEL: &[&str] = &["error","warn","info","debug"];

fn display_val(val: &str, section: i32, row: i32) -> &str {
    if val.is_empty() {
        return match (section, row) {
            (SECTION_AUDIO, AUD_AUDIO_LANG)
            | (SECTION_PLAYER_CFG, PLY_SUB_LANG)
            | (SECTION_PLAYER_CFG, PLY_SUB_LANG2)
            | (SECTION_PLAYER_CFG, PLY_SUB_TYPE) => "Any",
            _ => "(none)",
        };
    }
    // Skip mode display names (only for skip mode rows)
    if matches!((section, row),
        (SECTION_PLAYER_CFG, PLY_INTRO_MODE)
        | (SECTION_PLAYER_CFG, PLY_RECAP_MODE)
        | (SECTION_PLAYER_CFG, PLY_PREVIEW_MODE)
        | (SECTION_PLAYER_CFG, PLY_COMMERCIAL_MODE)
        | (SECTION_PLAYER_CFG, PLY_CREDITS_MODE))
    {
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

fn dropdown_model(section: i32, row: i32) -> Option<&'static [&'static str]> {
    match (section, row) {
        (SECTION_GENERAL, GEN_LOG_LEVEL) => Some(LOG_LEVEL_MODEL),
        (SECTION_VIDEO, VID_HWDEC)       => Some(HWDEC_MODEL),
        (SECTION_VIDEO, VID_VF)          => Some(VF_MODEL),
        (SECTION_VIDEO, VID_DEINTERLACE) => Some(DEINTERLACE_MODEL),
        (SECTION_VIDEO, VID_VIDEO_SYNC)  => Some(VIDEO_SYNC_MODEL),
        (SECTION_VIDEO, VID_TSCALE)      => Some(TSCALE_MODEL),
        (SECTION_VIDEO, VID_TONE_MAPPING) => Some(TONE_MAPPING_MODEL),
        (SECTION_AUDIO, AUD_CHANNELS) => Some(AUDIO_CHANNELS_MODEL),
        (SECTION_AUDIO, AUD_AUDIO_LANG)
        | (SECTION_PLAYER_CFG, PLY_SUB_LANG)
        | (SECTION_PLAYER_CFG, PLY_SUB_LANG2) => Some(LANG_MODEL),
        (SECTION_PLAYER_CFG, PLY_SUB_TYPE)        => Some(SUB_TYPE_MODEL),
        (SECTION_PLAYER_CFG, PLY_CACHE_MB)        => Some(CACHE_MB_MODEL),
        (SECTION_PLAYER_CFG, PLY_INTRO_MODE)
        | (SECTION_PLAYER_CFG, PLY_RECAP_MODE)
        | (SECTION_PLAYER_CFG, PLY_PREVIEW_MODE)
        | (SECTION_PLAYER_CFG, PLY_COMMERCIAL_MODE) => Some(SKIP_MODE_4_MODEL),
        (SECTION_PLAYER_CFG, PLY_CREDITS_MODE)    => Some(SKIP_MODE_3_MODEL),
        (SECTION_PLAYER_CFG, PLY_INTRO_SECS)
        | (SECTION_PLAYER_CFG, PLY_RECAP_SECS)
        | (SECTION_PLAYER_CFG, PLY_PREVIEW_SECS)
        | (SECTION_PLAYER_CFG, PLY_COMMERCIAL_SECS) => Some(SKIP_SECS_MODEL),
        (SECTION_PLAYER_CFG, PLY_CREDITS_SECS)    => Some(CREDITS_SECS_MODEL),
        _ => None,
    }
}

fn current_value_str(section: i32, row: i32, g: &crate::AppState<'_>) -> String {
    match (section, row) {
        (SECTION_GENERAL, GEN_LOG_LEVEL)    => g.get_settings_log_level().to_string(),
        (SECTION_VIDEO, VID_HWDEC)          => g.get_settings_hwdec().to_string(),
        (SECTION_VIDEO, VID_VF)             => g.get_settings_vf().to_string(),
        (SECTION_VIDEO, VID_DEINTERLACE)    => g.get_settings_deinterlace().to_string(),
        (SECTION_VIDEO, VID_VIDEO_SYNC)     => g.get_settings_video_sync().to_string(),
        (SECTION_VIDEO, VID_TSCALE)         => g.get_settings_tscale().to_string(),
        (SECTION_VIDEO, VID_TONE_MAPPING)   => g.get_settings_tone_mapping().to_string(),
        (SECTION_AUDIO, AUD_AUDIO_DEVICE)        => g.get_settings_audio_device_desc().to_string(),
        (SECTION_AUDIO, AUD_CHANNELS)            => g.get_settings_audio_channels().to_string(),
        (SECTION_AUDIO, AUD_PASSTHROUGH_DEVICE)  => g.get_settings_passthrough_device_desc().to_string(),
        (SECTION_AUDIO, AUD_AUDIO_LANG)     => g.get_settings_audio_lang().to_string(),
        (SECTION_PLAYER_CFG, PLY_SUB_LANG)  => g.get_settings_sub_lang().to_string(),
        (SECTION_PLAYER_CFG, PLY_SUB_LANG2) => g.get_settings_sub_lang2().to_string(),
        (SECTION_PLAYER_CFG, PLY_SUB_TYPE)  => {
            let v = g.get_settings_sub_type().to_string();
            if v.is_empty() { "Any".to_string() } else { v }
        }
        (SECTION_PLAYER_CFG, PLY_CACHE_MB)        => g.get_settings_cache_mb().to_string(),
        (SECTION_PLAYER_CFG, PLY_INTRO_MODE)      => g.get_settings_skip_intro_mode().to_string(),
        (SECTION_PLAYER_CFG, PLY_INTRO_SECS)      => g.get_settings_skip_intro_secs().to_string(),
        (SECTION_PLAYER_CFG, PLY_RECAP_MODE)      => g.get_settings_skip_recap_mode().to_string(),
        (SECTION_PLAYER_CFG, PLY_RECAP_SECS)      => g.get_settings_skip_recap_secs().to_string(),
        (SECTION_PLAYER_CFG, PLY_PREVIEW_MODE)    => g.get_settings_skip_preview_mode().to_string(),
        (SECTION_PLAYER_CFG, PLY_PREVIEW_SECS)    => g.get_settings_skip_preview_secs().to_string(),
        (SECTION_PLAYER_CFG, PLY_COMMERCIAL_MODE) => g.get_settings_skip_commercial_mode().to_string(),
        (SECTION_PLAYER_CFG, PLY_COMMERCIAL_SECS) => g.get_settings_skip_commercial_secs().to_string(),
        (SECTION_PLAYER_CFG, PLY_CREDITS_MODE)    => g.get_settings_skip_credits_mode().to_string(),
        (SECTION_PLAYER_CFG, PLY_CREDITS_SECS)    => g.get_settings_skip_credits_secs().to_string(),
        _ => String::new(),
    }
}

pub(crate) fn apply_dropdown_selection(section: i32, row: i32, cursor: i32, g: &crate::AppState<'_>) {
    if section == SECTION_AUDIO && (row == AUD_AUDIO_DEVICE || row == AUD_PASSTHROUGH_DEVICE) {
        let display = g.get_settings_audio_device_display();
        if let Some(desc) = display.row_data(cursor as usize) {
            if row == AUD_AUDIO_DEVICE {
                g.invoke_audio_device_selected(desc);
            } else {
                g.invoke_passthrough_device_selected(desc);
            }
        }
        return;
    }
    let Some(model) = dropdown_model(section, row) else { return };
    let Some(&val) = model.get(cursor as usize) else { return };
    match (section, row) {
        (SECTION_GENERAL, GEN_LOG_LEVEL)    => g.set_settings_log_level(val.into()),
        (SECTION_VIDEO, VID_HWDEC)          => g.set_settings_hwdec(val.into()),
        (SECTION_VIDEO, VID_VF)             => g.set_settings_vf(val.into()),
        (SECTION_VIDEO, VID_DEINTERLACE)    => g.set_settings_deinterlace(val.into()),
        (SECTION_VIDEO, VID_VIDEO_SYNC)     => g.set_settings_video_sync(val.into()),
        (SECTION_VIDEO, VID_TSCALE)         => g.set_settings_tscale(val.into()),
        (SECTION_VIDEO, VID_TONE_MAPPING)   => g.set_settings_tone_mapping(val.into()),
        (SECTION_AUDIO, AUD_CHANNELS)       => g.set_settings_audio_channels(val.into()),
        (SECTION_AUDIO, AUD_AUDIO_LANG)     => g.set_settings_audio_lang(val.into()),
        (SECTION_PLAYER_CFG, PLY_SUB_LANG)  => g.set_settings_sub_lang(val.into()),
        (SECTION_PLAYER_CFG, PLY_SUB_LANG2) => g.set_settings_sub_lang2(val.into()),
        (SECTION_PLAYER_CFG, PLY_SUB_TYPE)  => g.set_settings_sub_type(
            if val == "Any" { "".into() } else { val.into() }
        ),
        (SECTION_PLAYER_CFG, PLY_CACHE_MB)        => g.set_settings_cache_mb(val.parse().unwrap_or(0)),
        (SECTION_PLAYER_CFG, PLY_INTRO_MODE)      => g.set_settings_skip_intro_mode(val.into()),
        (SECTION_PLAYER_CFG, PLY_INTRO_SECS)      => g.set_settings_skip_intro_secs(val.parse().unwrap_or(8)),
        (SECTION_PLAYER_CFG, PLY_RECAP_MODE)      => g.set_settings_skip_recap_mode(val.into()),
        (SECTION_PLAYER_CFG, PLY_RECAP_SECS)      => g.set_settings_skip_recap_secs(val.parse().unwrap_or(8)),
        (SECTION_PLAYER_CFG, PLY_PREVIEW_MODE)    => g.set_settings_skip_preview_mode(val.into()),
        (SECTION_PLAYER_CFG, PLY_PREVIEW_SECS)    => g.set_settings_skip_preview_secs(val.parse().unwrap_or(8)),
        (SECTION_PLAYER_CFG, PLY_COMMERCIAL_MODE) => g.set_settings_skip_commercial_mode(val.into()),
        (SECTION_PLAYER_CFG, PLY_COMMERCIAL_SECS) => g.set_settings_skip_commercial_secs(val.parse().unwrap_or(8)),
        (SECTION_PLAYER_CFG, PLY_CREDITS_MODE)    => g.set_settings_skip_credits_mode(val.into()),
        (SECTION_PLAYER_CFG, PLY_CREDITS_SECS)    => g.set_settings_skip_credits_secs(val.parse().unwrap_or(30)),
        _ => return,
    }
    g.invoke_settings_changed();
}

// ── Per-row action (Left/Right cycling) ───────────────────────────────────────

fn settings_row_action(sf: i32, forward: bool, ss: i32, g: &crate::AppState<'_>) {
    fn cycles<'a>(current: &str, vals: &[&'a str], forward: bool) -> &'a str {
        let idx = vals.iter().position(|v| *v == current).unwrap_or(0);
        if forward { vals[(idx + 1) % vals.len()] }
        else       { vals[(idx + vals.len() - 1) % vals.len()] }
    }
    fn cycle_i32(current: i32, vals: &[i32], forward: bool) -> i32 {
        let idx = vals.iter().position(|v| *v == current).unwrap_or(0);
        if forward { vals[(idx + 1) % vals.len()] }
        else       { vals[(idx + vals.len() - 1) % vals.len()] }
    }

    match ss {
        SECTION_GENERAL => match sf {
            GEN_LAUNCH_FULLSCREEN => {
                g.set_settings_launch_fullscreen(!g.get_settings_launch_fullscreen());
                g.invoke_settings_changed();
            }
            GEN_VIDEO_BEHIND => {
                g.set_settings_video_behind(!g.get_settings_video_behind());
                g.invoke_settings_changed();
            }
            GEN_LOG_LEVEL => {
                let v = cycles(g.get_settings_log_level().as_str(), LOG_LEVEL_MODEL, forward);
                g.set_settings_log_level(v.into()); g.invoke_settings_changed();
            }
            GEN_PREWARM_METADATA => { g.invoke_prewarm_metadata(); }
            GEN_PREWARM_IMAGES   => { g.invoke_prewarm_images(); }
            GEN_SIGN_OUT => { g.invoke_sign_out(); }
            _ => {}
        },

        SECTION_VIDEO => match sf {
            VID_HWDEC => {
                let v = cycles(g.get_settings_hwdec().as_str(), HWDEC_MODEL, forward);
                g.set_settings_hwdec(v.into()); g.invoke_settings_changed();
            }
            VID_VF => {
                let v = cycles(g.get_settings_vf().as_str(), VF_MODEL, forward);
                g.set_settings_vf(v.into()); g.invoke_settings_changed();
            }
            VID_DEINTERLACE => {
                let v = cycles(g.get_settings_deinterlace().as_str(), DEINTERLACE_MODEL, forward);
                g.set_settings_deinterlace(v.into()); g.invoke_settings_changed();
            }
            VID_VIDEO_SYNC => {
                let v = cycles(g.get_settings_video_sync().as_str(), VIDEO_SYNC_MODEL, forward);
                g.set_settings_video_sync(v.into()); g.invoke_settings_changed();
            }
            VID_INTERPOLATION => {
                g.set_settings_interpolation(!g.get_settings_interpolation());
                g.invoke_settings_changed();
            }
            VID_TSCALE => {
                let v = cycles(g.get_settings_tscale().as_str(), TSCALE_MODEL, forward);
                g.set_settings_tscale(v.into()); g.invoke_settings_changed();
            }
            VID_TONE_MAPPING => {
                let v = cycles(g.get_settings_tone_mapping().as_str(), TONE_MAPPING_MODEL, forward);
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
            VID_VIDEO_LATENCY_HACKS
                if g.get_settings_video_sync().as_str() == "display-resample" =>
            {
                g.set_settings_video_latency_hacks(!g.get_settings_video_latency_hacks());
                g.invoke_settings_changed();
            }
            _ => {}
        },

        SECTION_AUDIO => match sf {
            AUD_AUDIO_DEVICE => {
                let display = g.get_settings_audio_device_display();
                let n = display.row_count();
                if n == 0 { return; }
                let current_desc = g.get_settings_audio_device_desc().to_string();
                let idx = (0..n)
                    .find(|&i| display.row_data(i).map(|s| s.to_string()) == Some(current_desc.clone()))
                    .unwrap_or(0);
                let next = if forward { (idx + 1) % n } else { (idx + n - 1) % n };
                if let Some(desc) = display.row_data(next) {
                    g.invoke_audio_device_selected(desc);
                }
            }
            AUD_PASSTHROUGH_DEVICE => {
                let display = g.get_settings_audio_device_display();
                let n = display.row_count();
                if n == 0 { return; }
                let current_desc = g.get_settings_passthrough_device_desc().to_string();
                let idx = (0..n)
                    .find(|&i| display.row_data(i).map(|s| s.to_string()) == Some(current_desc.clone()))
                    .unwrap_or(0);
                let next = if forward { (idx + 1) % n } else { (idx + n - 1) % n };
                if let Some(desc) = display.row_data(next) {
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
            AUD_CHANNELS => {
                let v = cycles(g.get_settings_audio_channels().as_str(), AUDIO_CHANNELS_MODEL, forward);
                g.set_settings_audio_channels(v.into()); g.invoke_settings_changed();
            }
            AUD_AUDIO_LANG => {
                let v = cycles(g.get_settings_audio_lang().as_str(), LANG_MODEL, forward);
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
            _ => {}
        },

        SECTION_PLAYER_CFG => match sf {
            PLY_SUB_ENABLED => {
                g.set_settings_sub_enabled(!g.get_settings_sub_enabled());
                g.invoke_settings_changed();
            }
            PLY_SUB_LANG => {
                let v = cycles(g.get_settings_sub_lang().as_str(), LANG_MODEL, forward);
                g.set_settings_sub_lang(v.into()); g.invoke_settings_changed();
            }
            PLY_SUB_LANG2 => {
                let v = cycles(g.get_settings_sub_lang2().as_str(), LANG_MODEL, forward);
                g.set_settings_sub_lang2(v.into()); g.invoke_settings_changed();
            }
            PLY_SUB_TYPE => {
                let current = g.get_settings_sub_type().to_string();
                let current = if current.is_empty() { "Any" } else { &current };
                let v = cycles(current, SUB_TYPE_MODEL, forward);
                g.set_settings_sub_type(if v == "Any" { "".into() } else { v.into() });
                g.invoke_settings_changed();
            }
            PLY_CACHE_MB => {
                let next = cycle_i32(g.get_settings_cache_mb(), CACHE_MB_VALUES, forward);
                g.set_settings_cache_mb(next); g.invoke_settings_changed();
            }
            PLY_INTRO_MODE => {
                let v = cycles(g.get_settings_skip_intro_mode().as_str(), SKIP_MODE_4_MODEL, forward);
                g.set_settings_skip_intro_mode(v.into()); g.invoke_settings_changed();
            }
            PLY_INTRO_SECS => {
                let v = cycles(g.get_settings_skip_intro_secs().to_string().as_str(), SKIP_SECS_MODEL, forward);
                g.set_settings_skip_intro_secs(v.parse().unwrap_or(8)); g.invoke_settings_changed();
            }
            PLY_RECAP_MODE => {
                let v = cycles(g.get_settings_skip_recap_mode().as_str(), SKIP_MODE_4_MODEL, forward);
                g.set_settings_skip_recap_mode(v.into()); g.invoke_settings_changed();
            }
            PLY_RECAP_SECS => {
                let v = cycles(g.get_settings_skip_recap_secs().to_string().as_str(), SKIP_SECS_MODEL, forward);
                g.set_settings_skip_recap_secs(v.parse().unwrap_or(8)); g.invoke_settings_changed();
            }
            PLY_PREVIEW_MODE => {
                let v = cycles(g.get_settings_skip_preview_mode().as_str(), SKIP_MODE_4_MODEL, forward);
                g.set_settings_skip_preview_mode(v.into()); g.invoke_settings_changed();
            }
            PLY_PREVIEW_SECS => {
                let v = cycles(g.get_settings_skip_preview_secs().to_string().as_str(), SKIP_SECS_MODEL, forward);
                g.set_settings_skip_preview_secs(v.parse().unwrap_or(8)); g.invoke_settings_changed();
            }
            PLY_COMMERCIAL_MODE => {
                let v = cycles(g.get_settings_skip_commercial_mode().as_str(), SKIP_MODE_4_MODEL, forward);
                g.set_settings_skip_commercial_mode(v.into()); g.invoke_settings_changed();
            }
            PLY_COMMERCIAL_SECS => {
                let v = cycles(g.get_settings_skip_commercial_secs().to_string().as_str(), SKIP_SECS_MODEL, forward);
                g.set_settings_skip_commercial_secs(v.parse().unwrap_or(8)); g.invoke_settings_changed();
            }
            PLY_CREDITS_MODE => {
                let v = cycles(g.get_settings_skip_credits_mode().as_str(), SKIP_MODE_3_MODEL, forward);
                g.set_settings_skip_credits_mode(v.into()); g.invoke_settings_changed();
            }
            PLY_CREDITS_SECS => {
                let v = cycles(g.get_settings_skip_credits_secs().to_string().as_str(), CREDITS_SECS_MODEL, forward);
                g.set_settings_skip_credits_secs(v.parse().unwrap_or(30)); g.invoke_settings_changed();
            }
            _ => {}
        },

        _ => {}
    }
}
