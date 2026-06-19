// ── fjord-app · settings.rs ───────────────────────────────────────────────────
//   Section constants     SECTION_GENERAL, SECTION_VIDEO, SECTION_AUDIO,
//                         SECTION_PLAYER_CFG, SECTION_KEYBINDINGS
//   General row consts    GEN_LAUNCH_FULLSCREEN, GEN_VIDEO_BEHIND, GEN_SIGN_OUT
//   Video row consts      VID_HWDEC … VID_VIDEO_LATENCY_HACKS (VID_TSCALE virtual)
//   Audio row consts      AUD_SPDIF, AUD_AUDIO_LANG
//   Player row consts     PLY_SUB_ENABLED, PLY_SUB_LANG, PLY_SUB_LANG2, PLY_CACHE_MB
//   dispatch_settings     keyboard nav for the settings screen (three-state:
//                           sidebar → left pane → right pane / keybindings;
//                           Enter opens dropdown popup; Up/Down/Enter/Esc navigate popup)
//   dropdown_model        model strings for each dropdown row (None for toggle rows)
//   current_value_str     current AppState value as string for a given (section, row)
//   apply_dropdown_selection  apply model[cursor] to AppState for a given (section, row)
//   settings_row_action   per-row Left/Right cycle action handler
// ─────────────────────────────────────────────────────────────────────────────

use crate::keys::Action;
use slint::{ModelRc, SharedString, VecModel};

// ── Section indices ───────────────────────────────────────────────────────────
pub(crate) const SECTION_GENERAL:     i32 = 0;
pub(crate) const SECTION_VIDEO:       i32 = 1;
pub(crate) const SECTION_AUDIO:       i32 = 2;
pub(crate) const SECTION_PLAYER_CFG:  i32 = 3;
pub(crate) const SECTION_KEYBINDINGS: i32 = 4;
const SECTION_MAX: i32 = SECTION_KEYBINDINGS;

// ── General section rows ──────────────────────────────────────────────────────
const GEN_LAUNCH_FULLSCREEN: i32 = 0;
const GEN_VIDEO_BEHIND:      i32 = 1;
const GEN_SIGN_OUT:          i32 = 2;

// ── Video section rows ────────────────────────────────────────────────────────
const VID_HWDEC:               i32 = 0;
const VID_VF:                  i32 = 1;
const VID_DEINTERLACE:         i32 = 2;
const VID_VIDEO_SYNC:          i32 = 3;
const VID_INTERPOLATION:       i32 = 4;
const VID_TSCALE:              i32 = 5;  // virtual — only shown when interpolation is on
const VID_TONE_MAPPING:        i32 = 6;
const VID_TARGET_COLORSPACE:   i32 = 7;
const VID_OPENGL_EARLY_FLUSH:  i32 = 8;
const VID_VIDEO_LATENCY_HACKS: i32 = 9;

// ── Audio section rows ────────────────────────────────────────────────────────
const AUD_SPDIF:      i32 = 0;
const AUD_AUDIO_LANG: i32 = 1;

// ── Player (config) section rows ──────────────────────────────────────────────
const PLY_SUB_ENABLED: i32 = 0;
const PLY_SUB_LANG:    i32 = 1;
const PLY_SUB_LANG2:   i32 = 2;
const PLY_CACHE_MB:    i32 = 3;

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
        let model_len = dropdown_model(ss, sf).map(|m| m.len() as i32).unwrap_or(0);
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
            SECTION_GENERAL    => GEN_SIGN_OUT,
            SECTION_VIDEO      => VID_VIDEO_LATENCY_HACKS,
            SECTION_AUDIO      => AUD_AUDIO_LANG,
            SECTION_PLAYER_CFG => PLY_CACHE_MB,
            _                  => 0,
        };
        match action {
            Action::Down => {
                if sf < max_row {
                    let mut next = sf + 1;
                    if ss == SECTION_VIDEO && next == VID_TSCALE
                       && !g.get_settings_interpolation()
                    {
                        next = VID_TONE_MAPPING;
                    }
                    if ss == SECTION_PLAYER_CFG && !g.get_settings_sub_enabled()
                       && (next == PLY_SUB_LANG || next == PLY_SUB_LANG2)
                    {
                        next = PLY_CACHE_MB;
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
                    if ss == SECTION_PLAYER_CFG && !g.get_settings_sub_enabled()
                       && (prev == PLY_SUB_LANG || prev == PLY_SUB_LANG2)
                    {
                        prev = PLY_SUB_ENABLED;
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
                if let Some(model) = dropdown_model(ss, sf) {
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

fn display_val(val: &str, section: i32, row: i32) -> &str {
    if !val.is_empty() { return val; }
    match (section, row) {
        (SECTION_AUDIO, AUD_AUDIO_LANG)
        | (SECTION_PLAYER_CFG, PLY_SUB_LANG)
        | (SECTION_PLAYER_CFG, PLY_SUB_LANG2) => "Off",
        _ => "(none)",
    }
}

fn dropdown_model(section: i32, row: i32) -> Option<&'static [&'static str]> {
    match (section, row) {
        (SECTION_VIDEO, VID_HWDEC) => Some(&[
            "auto","vulkan","vulkan-copy","nvdec","nvdec-copy",
            "vaapi","vaapi-copy","vdpau","vdpau-copy","none",
        ]),
        (SECTION_VIDEO, VID_VF) => Some(&[
            "","auto","format=yuv420p","format=yuv420p10le","format=nv12","format=p010",
        ]),
        (SECTION_VIDEO, VID_VIDEO_SYNC) => Some(&[
            "audio","display-resample","display-vdrop","display-adrop","desync",
        ]),
        (SECTION_VIDEO, VID_TSCALE) => Some(&[
            "oversample","catmull_rom","mitchell","gaussian","bicubic",
        ]),
        (SECTION_VIDEO, VID_TONE_MAPPING) => Some(&[
            "auto","hable","bt.2390","reinhard","mobius","clip","gamma","linear",
        ]),
        (SECTION_AUDIO, AUD_AUDIO_LANG)
        | (SECTION_PLAYER_CFG, PLY_SUB_LANG)
        | (SECTION_PLAYER_CFG, PLY_SUB_LANG2) => Some(LANG_MODEL),
        (SECTION_PLAYER_CFG, PLY_CACHE_MB) => Some(&["0","50","150","300","500","1000"]),
        _ => None,
    }
}

fn current_value_str(section: i32, row: i32, g: &crate::AppState<'_>) -> String {
    match (section, row) {
        (SECTION_VIDEO, VID_HWDEC)          => g.get_settings_hwdec().to_string(),
        (SECTION_VIDEO, VID_VF)             => g.get_settings_vf().to_string(),
        (SECTION_VIDEO, VID_VIDEO_SYNC)     => g.get_settings_video_sync().to_string(),
        (SECTION_VIDEO, VID_TSCALE)         => g.get_settings_tscale().to_string(),
        (SECTION_VIDEO, VID_TONE_MAPPING)   => g.get_settings_tone_mapping().to_string(),
        (SECTION_AUDIO, AUD_AUDIO_LANG)     => g.get_settings_audio_lang().to_string(),
        (SECTION_PLAYER_CFG, PLY_SUB_LANG)  => g.get_settings_sub_lang().to_string(),
        (SECTION_PLAYER_CFG, PLY_SUB_LANG2) => g.get_settings_sub_lang2().to_string(),
        (SECTION_PLAYER_CFG, PLY_CACHE_MB)  => g.get_settings_cache_mb().to_string(),
        _ => String::new(),
    }
}

pub(crate) fn apply_dropdown_selection(section: i32, row: i32, cursor: i32, g: &crate::AppState<'_>) {
    let Some(model) = dropdown_model(section, row) else { return };
    let Some(&val) = model.get(cursor as usize) else { return };
    match (section, row) {
        (SECTION_VIDEO, VID_HWDEC)          => g.set_settings_hwdec(val.into()),
        (SECTION_VIDEO, VID_VF)             => g.set_settings_vf(val.into()),
        (SECTION_VIDEO, VID_VIDEO_SYNC)     => g.set_settings_video_sync(val.into()),
        (SECTION_VIDEO, VID_TSCALE)         => g.set_settings_tscale(val.into()),
        (SECTION_VIDEO, VID_TONE_MAPPING)   => g.set_settings_tone_mapping(val.into()),
        (SECTION_AUDIO, AUD_AUDIO_LANG)     => g.set_settings_audio_lang(val.into()),
        (SECTION_PLAYER_CFG, PLY_SUB_LANG)  => g.set_settings_sub_lang(val.into()),
        (SECTION_PLAYER_CFG, PLY_SUB_LANG2) => g.set_settings_sub_lang2(val.into()),
        (SECTION_PLAYER_CFG, PLY_CACHE_MB)  => {
            g.set_settings_cache_mb(val.parse().unwrap_or(0));
        }
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
            GEN_SIGN_OUT => { g.invoke_sign_out(); }
            _ => {}
        },

        SECTION_VIDEO => match sf {
            VID_HWDEC => {
                let v = cycles(g.get_settings_hwdec().as_str(),
                    &["auto","vulkan","vulkan-copy","nvdec","nvdec-copy","vaapi","vaapi-copy",
                      "vdpau","vdpau-copy","none"], forward);
                g.set_settings_hwdec(v.into()); g.invoke_settings_changed();
            }
            VID_VF => {
                let v = cycles(g.get_settings_vf().as_str(),
                    &["","auto","format=yuv420p","format=yuv420p10le","format=nv12","format=p010"],
                    forward);
                g.set_settings_vf(v.into()); g.invoke_settings_changed();
            }
            VID_DEINTERLACE => {
                g.set_settings_deinterlace(!g.get_settings_deinterlace());
                g.invoke_settings_changed();
            }
            VID_VIDEO_SYNC => {
                let v = cycles(g.get_settings_video_sync().as_str(),
                    &["audio","display-resample","display-vdrop","display-adrop","desync"], forward);
                g.set_settings_video_sync(v.into()); g.invoke_settings_changed();
            }
            VID_INTERPOLATION => {
                g.set_settings_interpolation(!g.get_settings_interpolation());
                g.invoke_settings_changed();
            }
            VID_TSCALE => {
                let v = cycles(g.get_settings_tscale().as_str(),
                    &["oversample","catmull_rom","mitchell","gaussian","bicubic"], forward);
                g.set_settings_tscale(v.into()); g.invoke_settings_changed();
            }
            VID_TONE_MAPPING => {
                let v = cycles(g.get_settings_tone_mapping().as_str(),
                    &["auto","hable","bt.2390","reinhard","mobius","clip","gamma","linear"],
                    forward);
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
            VID_VIDEO_LATENCY_HACKS => {
                g.set_settings_video_latency_hacks(!g.get_settings_video_latency_hacks());
                g.invoke_settings_changed();
            }
            _ => {}
        },

        SECTION_AUDIO => match sf {
            AUD_SPDIF => {
                g.set_settings_audio_spdif(!g.get_settings_audio_spdif());
                g.invoke_settings_changed();
            }
            AUD_AUDIO_LANG => {
                let v = cycles(g.get_settings_audio_lang().as_str(),
                    &["","English","German","French","Japanese","Spanish","Italian",
                      "Portuguese","Russian","Korean","Chinese","Dutch","Swedish",
                      "Polish","Czech","Arabic","Turkish","Finnish","Danish","Norwegian"],
                    forward);
                g.set_settings_audio_lang(v.into()); g.invoke_settings_changed();
            }
            _ => {}
        },

        SECTION_PLAYER_CFG => match sf {
            PLY_SUB_ENABLED => {
                g.set_settings_sub_enabled(!g.get_settings_sub_enabled());
                g.invoke_settings_changed();
            }
            PLY_SUB_LANG => {
                let v = cycles(g.get_settings_sub_lang().as_str(),
                    &["","English","German","French","Japanese","Spanish","Italian",
                      "Portuguese","Russian","Korean","Chinese","Dutch","Swedish",
                      "Polish","Czech","Arabic","Turkish","Finnish","Danish","Norwegian"],
                    forward);
                g.set_settings_sub_lang(v.into()); g.invoke_settings_changed();
            }
            PLY_SUB_LANG2 => {
                let v = cycles(g.get_settings_sub_lang2().as_str(),
                    &["","English","German","French","Japanese","Spanish","Italian",
                      "Portuguese","Russian","Korean","Chinese","Dutch","Swedish",
                      "Polish","Czech","Arabic","Turkish","Finnish","Danish","Norwegian"],
                    forward);
                g.set_settings_sub_lang2(v.into()); g.invoke_settings_changed();
            }
            PLY_CACHE_MB => {
                let next = cycle_i32(g.get_settings_cache_mb(),
                    &[0, 50, 150, 300, 500, 1000], forward);
                g.set_settings_cache_mb(next); g.invoke_settings_changed();
            }
            _ => {}
        },

        _ => {}
    }
}
