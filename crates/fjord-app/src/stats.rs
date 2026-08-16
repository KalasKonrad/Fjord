// ── fjord-app · stats.rs ─────────────────────────────────────────────────────
//   update_stats_window  format StatsData fields and push to AppState stat-* props
//                        VSYNC row: reads video_sync_mode (mpv "video-sync" property) directly;
//                        vsync-ratio unused for mode label (render API never populates it)
//                        SPEED row: audio/video speed corrections (key #39 debug signal)
//                        DROP row:  VO drops  ·  decoder drops  ·  mistimed frames
//                        COLOR IN/OUT rows (2026-08-15) — fmt_color, shared by both, formats
//                        primaries/gamma/sig-peak into one line; IN reads the decoded
//                        SOURCE's own mastering info (video-params). OUT was meant to read
//                        video-target-params (2026-08-17 fix from video-out-params, which
//                        never reflected tone-mapping at all) but that property is itself
//                        confirmed (2026-08-17, live-reported "the clr out is just empty" +
//                        a direct empirical check) to always be unavailable under vo=libmpv
//                        — it reflects a real VO's own swapchain target, which mpv never
//                        manages under the render API. color_out now shows an explicit
//                        "n/a" instead of a bare "—" when both fields are empty, distinct
//                        from CLR IN's transient "still buffering" dash since this one never
//                        resolves under the current architecture. See CLAUDE.md's HDR
//                        section for the fuller story, including create_fbo()'s own
//                        unconditional 8-bit-RGBA FBO format — a second, more fundamental
//                        reason real output-colorspace introspection isn't available yet
// ─────────────────────────────────────────────────────────────────────────────
use slint::{Global, SharedString};

use crate::AppState;
use crate::MainWindow;

fn ss(s: &str) -> SharedString { SharedString::from(s) }

pub(crate) fn update_stats_window(w: &MainWindow, s: &fjord_player::StatsData) {
    // VID IN: codec  ·  WxH  ·  fps (no pixel format — avoids elide on long pix_fmt strings)
    let vid_in = if s.width > 0 {
        let codec = if s.video_codec.is_empty() { "?" } else { &s.video_codec };
        format!("{}  ·  {}×{}  ·  {:.2} fps", codec, s.width, s.height, s.fps)
    } else {
        "Buffering…".into()
    };

    // VID OUT: WxH  ·  in_pix  →  out_pix  (carries pixel format info)
    let vid_out = if s.video_out_w > 0 {
        let scale = format!("{}×{}", s.video_out_w, s.video_out_h);
        let in_fmt  = if s.video_pix_fmt.is_empty()     { String::new() } else { format!("  ·  {}", s.video_pix_fmt)     };
        let out_fmt = if s.video_out_pix_fmt.is_empty() { String::new() } else { format!("  →  {}", s.video_out_pix_fmt) };
        format!("{}{}{}", scale, in_fmt, out_fmt)
    } else {
        "—".into()
    };

    // fmt_color: shared by both rows below (2026-08-15, live HDR/gamut troubleshooting —
    // "shuld i use wide gammut" / "you have that toggle both on kde and the lg oled").
    // color_in reads video-params (the DECODED SOURCE's own mastering info — for Dolby
    // Vision/HDR content this stays "bt.2020 · pq" even once tone-mapped away); color_out
    // reads video-target-params (2026-08-17 fix, was video-out-params — see StatsData's
    // own doc comment in fjord-player for why that never actually reflected tone-mapping)
    // — the only field that answers "what am I actually sending to my TV," since Fjord has
    // no target-prim/gamut setting of its own and tone-mapping's whole job is converting
    // the two apart.
    let fmt_color = |prim: &str, gamma: &str, sig_peak: f64| -> String {
        let hdr = match gamma {
            "pq"  => format!("  ·  HDR10 (peak {:.0} nits)", sig_peak * 100.0),
            "hlg" => "  ·  HLG".into(),
            _     => String::new(),
        };
        if prim.is_empty() && gamma.is_empty() { "—".into() }
        else { format!("{}  ·  {}{}", prim, gamma, hdr) }
    };
    let color_in  = fmt_color(&s.video_primaries, &s.video_gamma, s.video_sig_peak);
    // CLR OUT: video-target-params is confirmed (2026-08-17, live-reported "the clr out is
    // just empty" + a direct empirical check) to always come back unavailable under
    // vo=libmpv — the property reflects a real VO's own swapchain target, and mpv never
    // manages a swapchain at all under the render API (Slint does, entirely outside mpv's
    // knowledge). This isn't a transient "still buffering" gap like CLR IN's own "—"
    // fallback can be — it structurally never resolves under this architecture, so it gets
    // its own explicit message rather than being confused with CLR IN's dash. Separately,
    // and more fundamentally: even if this property somehow populated, `create_fbo()`
    // (playback.rs) hardcodes the FBO mpv renders into as 8-bit RGBA unconditionally, so
    // any HDR precision is already discarded before Slint ever sees the frame — see
    // CLAUDE.md's HDR section / the hdr-branch notes for what real output introspection
    // (and real HDR passthrough at all) actually needs.
    let color_out = if s.video_out_primaries.is_empty() && s.video_out_gamma.is_empty() {
        "n/a — not reported under this render path".into()
    } else {
        fmt_color(&s.video_out_primaries, &s.video_out_gamma, s.video_out_sig_peak)
    };

    let hwdec = match s.hwdec_current.as_str() {
        "" | "no" => "CPU (software)".into(),
        v         => v.to_string(),
    };

    let aud_in = {
        let name = if !s.audio_codec_name.is_empty() { &s.audio_codec_name } else { &s.audio_codec };
        if name.is_empty() {
            "—".into()
        } else {
            let ch  = if s.audio_channels.is_empty()  { String::new() } else { format!("  ·  {}", s.audio_channels) };
            let sr  = if s.audio_samplerate == 0       { String::new() } else { format!("  ·  {} Hz", s.audio_samplerate) };
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
            let fmt = if s.audio_out_format.is_empty()     { String::new() } else { format!("  ·  {}", s.audio_out_format) };
            let ch  = if s.audio_out_channels.is_empty()   { String::new() } else { format!("  ·  {}", s.audio_out_channels) };
            let sr  = if s.audio_out_samplerate == 0       { String::new() } else { format!("  ·  {} Hz", s.audio_out_samplerate) };
            format!("{}{}{}{}", s.current_ao, fmt, sr, ch)
        }
    };

    let display = if s.display_fps > 0.0 { format!("{:.3} Hz", s.display_fps) } else { "—".into() };

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
    let avsync  = format!("{:+.3}s", s.avsync);

    // audio-speed-correction ≈ 0 when passthrough is active (can't resample);
    // large drift here with passthrough means the AO clock is unstable → dropout.
    let speed = format!("A: {:+.6}  V: {:+.6}",
        s.audio_speed_correction, s.video_speed_correction);

    let drop_ = format!("{} VO  ·  {} decoder  ·  {} mistimed",
        s.dropped_frames, s.decoder_dropped, s.mistimed_frames);

    let bitrate = format!("V: {:.1} Mbps  A: {:.0} kbps",
        s.video_bitrate / 1_000_000.0, s.audio_bitrate / 1_000.0);
    // cache_state (cache-buffering-state) is "% until playback unpauses"
    // (cache-pause-wait, 1s by default) — NOT how full the real configured
    // buffer is, so it reads ~100% almost immediately during normal
    // playback regardless of cache size; cache_duration_secs (a separate,
    // sometimes-unavailable mpv guess — shown only when > 0) is the actual
    // seconds currently held.
    let cache = if s.cache_duration_secs > 0.0 {
        format!("{}%  ·  {:.1}s buffered", s.cache_state, s.cache_duration_secs)
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
    g.set_stat_hwdec(ss(&hwdec));
    g.set_stat_aud_in(ss(&aud_in));
    g.set_stat_aud_out(ss(&aud_out));
    g.set_stat_display(ss(&display));
    g.set_stat_vsync(ss(&vsync));
    g.set_stat_avsync(ss(&avsync));
    g.set_stat_speed(ss(&speed));
    g.set_stat_drop(ss(&drop_));
    g.set_stat_bitrate(ss(&bitrate));
    g.set_stat_cache(ss(&cache));
}
