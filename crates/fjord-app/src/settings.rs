// ── fjord-app · settings.rs ───────────────────────────────────────────────────
//   ROW_* constants      named indices for settings rows (0–16)
//   dispatch_settings    keyboard nav for the settings screen (called from handle_key)
//   settings_row_action  per-row action handler (called from dispatch_settings)
// ─────────────────────────────────────────────────────────────────────────────

use crate::keys::Action;

// Row indices — must match the named elements in settings.slint and the
// row-y() lookup table there.  Tscale (ROW_TSCALE) has no element; row-y()
// computes its position by interpolating between ROW_INTERPOLATION and
// ROW_TONE_MAPPING.
const ROW_LAUNCH_FULLSCREEN:       i32 = 0;
const ROW_AUDIO_SPDIF:             i32 = 1;
const ROW_GPU_API:                 i32 = 2;
const ROW_HWDEC:                   i32 = 3;
const ROW_HWDEC_IMAGE_FORMAT:      i32 = 4;
const ROW_VF:                      i32 = 5;
const ROW_DEINTERLACE:             i32 = 6;
const ROW_VIDEO_BEHIND:            i32 = 7;
const ROW_VIDEO_SYNC:              i32 = 8;
const ROW_INTERPOLATION:           i32 = 9;
const ROW_TSCALE:                  i32 = 10;
const ROW_TONE_MAPPING:            i32 = 11;
const ROW_TARGET_COLORSPACE:       i32 = 12;
const ROW_OPENGL_EARLY_FLUSH:      i32 = 13;
const ROW_VIDEO_LATENCY_HACKS:     i32 = 14;
const ROW_CACHE_MB:                i32 = 15;
pub(crate) const ROW_SIGN_OUT:     i32 = 16;

pub(crate) fn dispatch_settings(action: &Action, g: &crate::AppState<'_>) -> Option<bool> {
    let sf = g.get_settings_focused();

    if sf >= 0 {
        match action {
            Action::Down => {
                if sf < ROW_SIGN_OUT {
                    let mut next = sf + 1;
                    if next == ROW_TSCALE && !g.get_settings_interpolation() { next = ROW_TONE_MAPPING; }
                    g.set_settings_focused(next);
                } else {
                    // Sign Out row → enter keybinding section
                    g.set_settings_focused(-1);
                    g.set_keybinding_focused(0);
                }
                Some(true)
            }
            Action::Up => {
                if sf == 0 {
                    g.set_settings_focused(-1);
                } else {
                    let mut prev = sf - 1;
                    if prev == ROW_TSCALE && !g.get_settings_interpolation() { prev = ROW_INTERPOLATION; }
                    g.set_settings_focused(prev);
                }
                Some(true)
            }
            Action::Back => { g.set_settings_focused(-1); Some(true) }
            Action::Confirm | Action::Left | Action::Right => {
                let forward = !matches!(action, Action::Left);
                settings_row_action(sf, forward, g);
                Some(true)
            }
            _ => None
        }
    } else {
        match action {
            Action::Confirm | Action::Right => { g.set_settings_focused(0); Some(true) }
            _ => None
        }
    }
}

fn settings_row_action(sf: i32, forward: bool, g: &crate::AppState<'_>) {
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

    match sf {
        ROW_LAUNCH_FULLSCREEN   => { g.set_settings_launch_fullscreen(!g.get_settings_launch_fullscreen()); g.invoke_settings_changed(); }
        ROW_AUDIO_SPDIF         => { g.set_settings_audio_spdif(!g.get_settings_audio_spdif()); g.invoke_settings_changed(); }
        ROW_GPU_API             => {
            let v = cycles(g.get_settings_gpu_api().as_str(), &["auto","opengl","vulkan"], forward);
            g.set_settings_gpu_api(v.into()); g.invoke_settings_changed();
        }
        ROW_HWDEC               => {
            let v = cycles(g.get_settings_hwdec().as_str(),
                &["auto","vulkan-copy","nvdec-copy","vaapi-copy","vdpau-copy","nvdec","vaapi","vdpau","none"],
                forward);
            g.set_settings_hwdec(v.into()); g.invoke_settings_changed();
        }
        ROW_HWDEC_IMAGE_FORMAT  => {
            let v = cycles(g.get_settings_hwdec_image_format().as_str(),
                &["","yuv420p","yuv420p10le","nv12","p010"], forward);
            g.set_settings_hwdec_image_format(v.into()); g.invoke_settings_changed();
        }
        ROW_VF                  => {
            let v = cycles(g.get_settings_vf().as_str(),
                &["","auto","format=yuv420p","format=yuv420p10le","format=nv12","format=p010"], forward);
            g.set_settings_vf(v.into()); g.invoke_settings_changed();
        }
        ROW_DEINTERLACE         => { g.set_settings_deinterlace(!g.get_settings_deinterlace()); g.invoke_settings_changed(); }
        ROW_VIDEO_BEHIND        => { g.set_settings_video_behind(!g.get_settings_video_behind()); g.invoke_settings_changed(); }
        ROW_VIDEO_SYNC          => {
            let v = cycles(g.get_settings_video_sync().as_str(),
                &["audio","display-resample","display-vdrop","display-adrop"], forward);
            g.set_settings_video_sync(v.into()); g.invoke_settings_changed();
        }
        ROW_INTERPOLATION       => { g.set_settings_interpolation(!g.get_settings_interpolation()); g.invoke_settings_changed(); }
        ROW_TSCALE              => {
            let v = cycles(g.get_settings_tscale().as_str(),
                &["oversample","catmull_rom","mitchell","gaussian","bicubic"], forward);
            g.set_settings_tscale(v.into()); g.invoke_settings_changed();
        }
        ROW_TONE_MAPPING        => {
            let v = cycles(g.get_settings_tone_mapping().as_str(),
                &["auto","hable","bt.2390","reinhard","mobius","clip","gamma","linear"], forward);
            g.set_settings_tone_mapping(v.into()); g.invoke_settings_changed();
        }
        ROW_TARGET_COLORSPACE   => { g.set_settings_target_colorspace_hint(!g.get_settings_target_colorspace_hint()); g.invoke_settings_changed(); }
        ROW_OPENGL_EARLY_FLUSH  => { g.set_settings_opengl_early_flush(!g.get_settings_opengl_early_flush()); g.invoke_settings_changed(); }
        ROW_VIDEO_LATENCY_HACKS => { g.set_settings_video_latency_hacks(!g.get_settings_video_latency_hacks()); g.invoke_settings_changed(); }
        ROW_CACHE_MB            => {
            let next = cycle_i32(g.get_settings_cache_mb(), &[0, 50, 150, 300, 500, 1000], forward);
            g.set_settings_cache_mb(next); g.invoke_settings_changed();
        }
        ROW_SIGN_OUT            => { g.invoke_sign_out(); }
        _                       => {}
    }
}
