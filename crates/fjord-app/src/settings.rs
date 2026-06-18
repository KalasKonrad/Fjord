// ── fjord-app · settings.rs ───────────────────────────────────────────────────
//   Section constants     SECTION_GENERAL, SECTION_PLAYER, SECTION_KEYBINDINGS
//   General row consts    GEN_LAUNCH_FULLSCREEN, GEN_VIDEO_BEHIND, GEN_SIGN_OUT
//   Player row consts     PLY_AUDIO_SPDIF … PLY_SUB_ENABLED/LANG/LANG2, PLY_CACHE_MB (PLY_TSCALE virtual)
//   dispatch_settings     keyboard nav for the settings screen (three-state:
//                           sidebar → left pane → right pane / keybindings)
//   settings_row_action   per-row action handler
// ─────────────────────────────────────────────────────────────────────────────

use crate::keys::Action;

// ── Section indices ───────────────────────────────────────────────────────────
pub(crate) const SECTION_GENERAL:     i32 = 0;
pub(crate) const SECTION_PLAYER:      i32 = 1;
pub(crate) const SECTION_KEYBINDINGS: i32 = 2;
const SECTION_MAX: i32 = SECTION_KEYBINDINGS;

// ── General section rows ──────────────────────────────────────────────────────
const GEN_LAUNCH_FULLSCREEN: i32 = 0;
const GEN_VIDEO_BEHIND:      i32 = 1;
const GEN_SIGN_OUT:          i32 = 2;

// ── Player section rows ───────────────────────────────────────────────────────
const PLY_AUDIO_SPDIF:         i32 = 0;
const PLY_GPU_API:             i32 = 1;
const PLY_HWDEC:               i32 = 2;
const PLY_VF:                  i32 = 3;
const PLY_DEINTERLACE:         i32 = 4;
const PLY_VIDEO_SYNC:          i32 = 5;
const PLY_INTERPOLATION:       i32 = 6;
const PLY_TSCALE:              i32 = 7;  // virtual — no named element in settings.slint
const PLY_TONE_MAPPING:        i32 = 8;
const PLY_TARGET_COLORSPACE:   i32 = 9;
const PLY_OPENGL_EARLY_FLUSH:  i32 = 10;
const PLY_VIDEO_LATENCY_HACKS: i32 = 11;
const PLY_SUB_ENABLED:         i32 = 12;
const PLY_SUB_LANG:            i32 = 13;
const PLY_SUB_LANG2:           i32 = 14;
const PLY_CACHE_MB:            i32 = 15;

// ── Main dispatch ─────────────────────────────────────────────────────────────

pub(crate) fn dispatch_settings(action: &Action, g: &crate::AppState<'_>) -> Option<bool> {
    let sf = g.get_settings_focused();
    let ss = g.get_settings_section();

    if sf >= 0 {
        // ── Right pane: row navigation ────────────────────────────────────
        let max_row = if ss == SECTION_GENERAL { GEN_SIGN_OUT } else { PLY_CACHE_MB };
        match action {
            Action::Down => {
                if sf < max_row {
                    let mut next = sf + 1;
                    if ss == SECTION_PLAYER && next == PLY_TSCALE
                       && !g.get_settings_interpolation()
                    {
                        next = PLY_TONE_MAPPING;
                    }
                    g.set_settings_focused(next);
                }
                // At last row: stay put
                Some(true)
            }
            Action::Up => {
                if sf == 0 {
                    g.set_settings_focused(-1); // back to left pane
                } else {
                    let mut prev = sf - 1;
                    if ss == SECTION_PLAYER && prev == PLY_TSCALE
                       && !g.get_settings_interpolation()
                    {
                        prev = PLY_INTERPOLATION;
                    }
                    g.set_settings_focused(prev);
                }
                Some(true)
            }
            Action::Back | Action::Left => {
                g.set_settings_focused(-1); // back to left pane
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
                    g.set_keybinding_focused(0); // enter keybindings page
                } else {
                    g.set_settings_focused(0); // enter right pane
                }
                Some(true)
            }
            Action::Back | Action::Left => {
                g.set_settings_section(-1); // back to sidebar
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
            _ => None, // Up/Down fall through to sidebar nav
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

    if ss == SECTION_GENERAL {
        match sf {
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
        }
    } else if ss == SECTION_PLAYER {
        match sf {
            PLY_AUDIO_SPDIF => {
                g.set_settings_audio_spdif(!g.get_settings_audio_spdif());
                g.invoke_settings_changed();
            }
            PLY_GPU_API => {
                let v = cycles(g.get_settings_gpu_api().as_str(),
                    &["auto", "opengl", "vulkan"], forward);
                g.set_settings_gpu_api(v.into()); g.invoke_settings_changed();
            }
            PLY_HWDEC => {
                let v = cycles(g.get_settings_hwdec().as_str(),
                    &["auto","vulkan","vulkan-copy","nvdec-copy","vaapi-copy","vdpau-copy",
                      "nvdec","vaapi","vdpau","none"], forward);
                g.set_settings_hwdec(v.into()); g.invoke_settings_changed();
            }
            PLY_VF => {
                let v = cycles(g.get_settings_vf().as_str(),
                    &["","auto","format=yuv420p","format=yuv420p10le","format=nv12","format=p010"],
                    forward);
                g.set_settings_vf(v.into()); g.invoke_settings_changed();
            }
            PLY_DEINTERLACE => {
                g.set_settings_deinterlace(!g.get_settings_deinterlace());
                g.invoke_settings_changed();
            }
            PLY_VIDEO_SYNC => {
                let v = cycles(g.get_settings_video_sync().as_str(),
                    &["audio","display-resample","display-vdrop","display-adrop","desync"], forward);
                g.set_settings_video_sync(v.into()); g.invoke_settings_changed();
            }
            PLY_INTERPOLATION => {
                g.set_settings_interpolation(!g.get_settings_interpolation());
                g.invoke_settings_changed();
            }
            PLY_TSCALE => {
                let v = cycles(g.get_settings_tscale().as_str(),
                    &["oversample","catmull_rom","mitchell","gaussian","bicubic"], forward);
                g.set_settings_tscale(v.into()); g.invoke_settings_changed();
            }
            PLY_TONE_MAPPING => {
                let v = cycles(g.get_settings_tone_mapping().as_str(),
                    &["auto","hable","bt.2390","reinhard","mobius","clip","gamma","linear"],
                    forward);
                g.set_settings_tone_mapping(v.into()); g.invoke_settings_changed();
            }
            PLY_TARGET_COLORSPACE => {
                g.set_settings_target_colorspace_hint(!g.get_settings_target_colorspace_hint());
                g.invoke_settings_changed();
            }
            PLY_OPENGL_EARLY_FLUSH => {
                g.set_settings_opengl_early_flush(!g.get_settings_opengl_early_flush());
                g.invoke_settings_changed();
            }
            PLY_VIDEO_LATENCY_HACKS => {
                g.set_settings_video_latency_hacks(!g.get_settings_video_latency_hacks());
                g.invoke_settings_changed();
            }
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
        }
    }
}
