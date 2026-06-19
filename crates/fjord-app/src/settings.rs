// ── fjord-app · settings.rs ───────────────────────────────────────────────────
//   Section constants     SECTION_GENERAL, SECTION_VIDEO, SECTION_AUDIO,
//                         SECTION_PLAYER_CFG, SECTION_KEYBINDINGS
//   General row consts    GEN_LAUNCH_FULLSCREEN, GEN_VIDEO_BEHIND, GEN_SIGN_OUT
//   Video row consts      VID_HWDEC … VID_VIDEO_LATENCY_HACKS (VID_TSCALE virtual)
//   Audio row consts      AUD_SPDIF, AUD_SUB_ENABLED, AUD_SUB_LANG, AUD_SUB_LANG2
//   Player row consts     PLY_CACHE_MB
//   dispatch_settings     keyboard nav for the settings screen (three-state:
//                           sidebar → left pane → right pane / keybindings)
//   settings_row_action   per-row action handler
// ─────────────────────────────────────────────────────────────────────────────

use crate::keys::Action;

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
                    g.set_settings_focused(prev);
                }
                Some(true)
            }
            Action::Back | Action::Left => {
                g.set_settings_focused(-1);
                Some(true)
            }
            Action::Confirm | Action::Right => {
                let forward = !matches!(action, Action::Left);
                settings_row_action(sf, forward, ss, g);
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

// ── Per-row action ────────────────────────────────────────────────────────────

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
