// ── fjord-app · stats.rs ─────────────────────────────────────────────────────
//   update_stats_window  StatsData → AppState stat-* props
//                        VSYNC: mpv's video-sync mode (+ vsync-ratio while mpv's display sync
//                        is on — it needs timer.rs's display_rate under the render API); INTERP:
//                        interpolation on (+ tscale) / waiting for display sync / off; SCALER
//                        (scaler_line, unit-tested): the upscaler (FSR 1 / mpv's scale) and
//                        whether it scales up — the source's width against the drawn width
//                        (StatsData.drawn_w, osd-dimensions); OUT: the drawn size + pixel formats;
//                        SPEED: audio/video speed corrections; DROP: VO drops ·
//                        decoder drops · mistimed frames; CLR IN/OUT via color_line (unit-tested:
//                        "BT.709 colors · BT.1886 gamma (SDR)"; IN = the
//                        source's video-params; OUT = video-target-params, never available under
//                        vo=libmpv → "n/a"); HDR row from hdr::status_text
// ─────────────────────────────────────────────────────────────────────────────
use slint::{Global, SharedString};

use crate::AppState;
use crate::MainWindow;

/// SCALER: the upscaler, and whether it scales up at all — the source's width against the width
/// the video is drawn at (with display sync matching the TV to the video, nothing is; the TV
/// upscales). Unit-tested.
fn scaler_line(scaler: String, source_w: i64, drawn_w: i64) -> String {
    if source_w > 0 && drawn_w > source_w {
        format!("{scaler}  ·  {source_w}→{drawn_w} px wide")
    } else if source_w > 0 && drawn_w > 0 {
        format!("{scaler}  ·  not scaling up (1:1 or smaller)")
    } else {
        scaler
    }
}

/// CLR IN/OUT in words: "BT.709 colors · BT.1886 gamma (SDR)", "BT.2020 colors · PQ (HDR10, peak
/// 1000 nits)" — mpv's bare "bt.709 · bt.1886" read like a conversion from one to the other
/// (2026-11-13). Primaries are the colors, gamma the brightness curve (BT.1886 is the display
/// curve for BT.709 video). Unknown names pass through. Unit-tested.
fn color_line(primaries: &str, gamma: &str, sig_peak: f64) -> String {
    if primaries.is_empty() && gamma.is_empty() {
        return "—".into();
    }
    let colors = match primaries {
        "" => "unknown colors".to_string(),
        "bt.601-525" => "BT.601 (NTSC) colors".into(),
        "bt.601-625" => "BT.601 (PAL) colors".into(),
        "dci-p3" => "DCI-P3 colors".into(),
        "display-p3" => "Display P3 colors".into(),
        p => format!("{} colors", p.replace("bt.", "BT.")),
    };
    let curve = match gamma {
        "pq" if sig_peak > 0.0 => format!("PQ (HDR10, peak {:.0} nits)", sig_peak * 100.0),
        "pq" => "PQ (HDR10)".into(),
        "hlg" => "HLG (HDR)".into(),
        "" => "unknown gamma".into(),
        "bt.1886" => "BT.1886 gamma (SDR)".into(),
        "srgb" => "sRGB gamma (SDR)".into(),
        g => match g.strip_prefix("gamma") {
            Some(n) => format!("gamma {n} (SDR)"),
            None => format!("{g} gamma"),
        },
    };
    format!("{colors}  ·  {curve}")
}

fn ss(s: &str) -> SharedString {
    SharedString::from(s)
}

pub(crate) fn update_stats_window(w: &MainWindow, s: &fjord_player::StatsData) {
    // VID IN: codec  ·  WxH  ·  fps (no pixel format — avoids elide on long pix_fmt strings)
    let vid_in = if s.width > 0 {
        let codec = if s.video_codec.is_empty() {
            "?"
        } else {
            &s.video_codec
        };
        format!(
            "{}  ·  {}×{}  ·  {:.2} fps",
            codec, s.width, s.height, s.fps
        )
    } else {
        "Buffering…".into()
    };

    // VID OUT: the size the video is drawn at (after the --vf chain while mpv hasn't drawn yet)
    //  ·  in_pix  →  out_pix
    let vid_out = if s.video_out_w > 0 {
        let scale = if s.drawn_w > 0 {
            format!("{}×{} on screen", s.drawn_w, s.drawn_h)
        } else {
            format!("{}×{}", s.video_out_w, s.video_out_h)
        };
        let in_fmt = if s.video_pix_fmt.is_empty() {
            String::new()
        } else {
            format!("  ·  {}", s.video_pix_fmt)
        };
        let out_fmt = if s.video_out_pix_fmt.is_empty() {
            String::new()
        } else {
            format!("  →  {}", s.video_out_pix_fmt)
        };
        format!("{}{}{}", scale, in_fmt, out_fmt)
    } else {
        "—".into()
    };

    // color_line words primaries/gamma/peak for both rows. CLR IN = video-params (the decoded
    // source — Dolby Vision/HDR stays "BT.2020 colors · PQ" even when tone-mapped); CLR OUT =
    // video-target-params (video-out-params never reflected tone-mapping; see StatsData in
    // fjord-player).
    let color_in = color_line(&s.video_primaries, &s.video_gamma, s.video_sig_peak);
    // CLR OUT: video-target-params is always unavailable under vo=libmpv — it describes a VO's
    // own swapchain, and under the render API mpv has none. Shown as an explicit "n/a" (CLR IN's
    // "—" just means not yet known). What actually reaches the screen is Fjord's FBO (8-bit,
    // or 10-bit RGB10_A2 with HDR passthrough — create_fbo) and the surface tagging (hdr.rs).
    let color_out = if s.video_out_primaries.is_empty() && s.video_out_gamma.is_empty() {
        "n/a — not reported under this render path".into()
    } else {
        color_line(
            &s.video_out_primaries,
            &s.video_out_gamma,
            s.video_out_sig_peak,
        )
    };

    let hwdec = match s.hwdec_current.as_str() {
        "" | "no" => "CPU (software)".into(),
        v => v.to_string(),
    };

    let aud_in = {
        let name = if !s.audio_codec_name.is_empty() {
            &s.audio_codec_name
        } else {
            &s.audio_codec
        };
        if name.is_empty() {
            "—".into()
        } else {
            let ch = if s.audio_channels.is_empty() {
                String::new()
            } else {
                format!("  ·  {}", s.audio_channels)
            };
            let sr = if s.audio_samplerate == 0 {
                String::new()
            } else {
                format!("  ·  {} Hz", s.audio_samplerate)
            };
            format!("{}{}{}", name, ch, sr)
        }
    };

    let aud_out = if s.current_ao.is_empty() {
        "—".into()
    } else {
        let passthrough = s.audio_out_format.starts_with("iec61937");
        if passthrough {
            format!("{}  ·  passthrough  ({})", s.current_ao, s.audio_out_format)
        } else {
            let fmt = if s.audio_out_format.is_empty() {
                String::new()
            } else {
                format!("  ·  {}", s.audio_out_format)
            };
            let ch = if s.audio_out_channels.is_empty() {
                String::new()
            } else {
                format!("  ·  {}", s.audio_out_channels)
            };
            let sr = if s.audio_out_samplerate == 0 {
                String::new()
            } else {
                format!("  ·  {} Hz", s.audio_out_samplerate)
            };
            format!("{}{}{}{}", s.current_ao, fmt, sr, ch)
        }
    };

    let display = if s.display_fps > 0.0 {
        format!("{:.3} Hz", s.display_fps)
    } else {
        "—".into()
    };

    // video-sync is read back from mpv; vsync-ratio is only non-zero in display-sync modes.
    let vsync = match s.video_sync_mode.as_str() {
        "" | "audio" => "audio  ·  N/A".into(),
        mode => {
            if s.vsync_ratio > 0.0 {
                format!("{}  ·  ratio {:.4}", mode, s.vsync_ratio)
            } else {
                mode.to_string()
            }
        }
    };
    // Interpolation: mpv only gets it with a display video sync (Player::new); it blends frames
    // once display sync runs (a ratio on the VSYNC line).
    let interp = match (s.interpolation, s.vsync_ratio > 0.0) {
        (true, true) => format!(
            "on  ·  {}",
            if s.tscale.is_empty() {
                "oversample"
            } else {
                &s.tscale
            }
        ),
        (true, false) => "on  ·  waiting for display sync".into(),
        (false, _) => "off".into(),
    };
    let scaler = if s.glsl_shaders.to_ascii_lowercase().contains("fsr") {
        "FSR 1".to_string()
    } else if s.scale.is_empty() {
        "lanczos".to_string()
    } else {
        s.scale.clone()
    };
    let scaler = scaler_line(scaler, s.width, s.drawn_w);
    let avsync = format!("{:+.3}s", s.avsync);

    // audio-speed-correction ≈ 0 when passthrough is active (can't resample);
    // large drift here with passthrough means the AO clock is unstable → dropout.
    let speed = format!(
        "A: {:+.6}  V: {:+.6}",
        s.audio_speed_correction, s.video_speed_correction
    );

    let drop_ = format!(
        "{} VO  ·  {} decoder  ·  {} mistimed",
        s.dropped_frames, s.decoder_dropped, s.mistimed_frames
    );

    let bitrate = format!(
        "V: {:.1} Mbps  A: {:.0} kbps",
        s.video_bitrate / 1_000_000.0,
        s.audio_bitrate / 1_000.0
    );
    // cache_state (cache-buffering-state) is "% until playback unpauses" (cache-pause-wait,
    // 1 s), not how full the buffer is — ~100% almost at once; cache_duration_secs (shown only
    // when > 0) is the seconds actually held.
    let cache = if s.cache_duration_secs > 0.0 {
        format!(
            "{}%  ·  {:.1}s buffered",
            s.cache_state, s.cache_duration_secs
        )
    } else {
        format!("{}%", s.cache_state)
    };

    let passthrough_active = s.audio_out_format.starts_with("iec61937");
    let g = AppState::get(w);
    g.set_audio_passthrough_active(passthrough_active);
    g.set_stat_vid_in(ss(&vid_in));
    g.set_stat_vid_out(ss(&vid_out));
    g.set_stat_color_in(ss(&color_in));
    g.set_stat_color_out(ss(&color_out));
    // hdr branch, Stage 3 — real Wayland color-management negotiation status
    // (a Fjord-internal thing, not an mpv property at all — see hdr.rs's own
    // HdrStatus doc comment). Refreshed on the same cadence as everything
    // else in this function.
    g.set_stat_hdr(ss(crate::hdr::status_text()));
    g.set_stat_hwdec(ss(&hwdec));
    g.set_stat_aud_in(ss(&aud_in));
    g.set_stat_aud_out(ss(&aud_out));
    g.set_stat_display(ss(&display));
    g.set_stat_vsync(ss(&vsync));
    g.set_stat_interp(ss(&interp));
    g.set_stat_scaler(ss(&scaler));
    g.set_stat_avsync(ss(&avsync));
    g.set_stat_speed(ss(&speed));
    g.set_stat_drop(ss(&drop_));
    g.set_stat_bitrate(ss(&bitrate));
    g.set_stat_cache(ss(&cache));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scaler_line_compares_the_source_with_the_drawn_width() {
        // A 1080p film on a 4K mode (HD video in 4K): drawn 3840 wide.
        assert_eq!(
            scaler_line("FSR 1".into(), 1920, 3840),
            "FSR 1  ·  1920→3840 px wide"
        );
        assert_eq!(
            scaler_line("lanczos".into(), 1920, 1920),
            "lanczos  ·  not scaling up (1:1 or smaller)"
        );
        // Nothing drawn yet: just the scaler.
        assert_eq!(scaler_line("FSR 1".into(), 1920, 0), "FSR 1");
    }

    #[test]
    fn the_color_line_says_what_each_part_is() {
        assert_eq!(
            color_line("bt.709", "bt.1886", 1.0),
            "BT.709 colors  ·  BT.1886 gamma (SDR)"
        );
        assert_eq!(
            color_line("bt.2020", "pq", 10.0),
            "BT.2020 colors  ·  PQ (HDR10, peak 1000 nits)"
        );
        assert_eq!(
            color_line("bt.2020", "hlg", 0.0),
            "BT.2020 colors  ·  HLG (HDR)"
        );
        assert_eq!(
            color_line("bt.601-625", "gamma2.2", 1.0),
            "BT.601 (PAL) colors  ·  gamma 2.2 (SDR)"
        );
        assert_eq!(color_line("", "", 0.0), "—");
    }
}
