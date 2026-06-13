use slint::{Global, SharedString};

use crate::AppState;
use crate::MainWindow;

fn ss(s: &str) -> SharedString { SharedString::from(s) }

pub(crate) fn update_stats_window(w: &MainWindow, s: &fjord_player::StatsData) {
    let vid_in = if s.width > 0 {
        let codec = if s.video_codec.is_empty() { "?" } else { &s.video_codec };
        let fmt   = if s.video_pix_fmt.is_empty() { String::new() } else { format!("  ·  {}", s.video_pix_fmt) };
        format!("{}  ·  {}×{}  ·  {:.2} fps{}", codec, s.width, s.height, s.fps, fmt)
    } else {
        "Buffering…".into()
    };

    let vid_out = if s.video_out_w > 0 {
        let scale = if s.video_out_w != s.width || s.video_out_h != s.height {
            format!("{}×{}", s.video_out_w, s.video_out_h)
        } else {
            format!("{}×{}", s.width, s.height)
        };
        let fmt = if s.video_out_pix_fmt.is_empty() { String::new() } else { format!("  ·  {}", s.video_out_pix_fmt) };
        format!("{}{}", scale, fmt)
    } else {
        "—".into()
    };

    let color = {
        let prim  = s.video_primaries.as_str();
        let gamma = s.video_gamma.as_str();
        let hdr   = match gamma {
            "pq"  => format!("  ·  HDR10 (peak {:.0} nits)", s.video_sig_peak * 100.0),
            "hlg" => "  ·  HLG".into(),
            _     => String::new(),
        };
        if prim.is_empty() && gamma.is_empty() { "—".into() }
        else { format!("{}  ·  {}{}", prim, gamma, hdr) }
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

    let vsync = if s.vsync_ratio == 0.0 {
        "N/A  (audio-sync mode)".into()
    } else {
        format!("{:.4}  (ideal 1.0000)", s.vsync_ratio)
    };
    let avsync  = format!("{:+.3}s", s.avsync);
    let drop_   = format!("{} dropped", s.dropped_frames);
    let bitrate = format!("V: {:.1} Mbps  A: {:.0} kbps",
        s.video_bitrate / 1_000_000.0, s.audio_bitrate / 1_000.0);
    let cache   = format!("{}%", s.cache_state);

    let g = AppState::get(w);
    g.set_stat_vid_in(ss(&vid_in));
    g.set_stat_vid_out(ss(&vid_out));
    g.set_stat_color(ss(&color));
    g.set_stat_hwdec(ss(&hwdec));
    g.set_stat_aud_in(ss(&aud_in));
    g.set_stat_aud_out(ss(&aud_out));
    g.set_stat_display(ss(&display));
    g.set_stat_vsync(ss(&vsync));
    g.set_stat_avsync(ss(&avsync));
    g.set_stat_drop(ss(&drop_));
    g.set_stat_bitrate(ss(&bitrate));
    g.set_stat_cache(ss(&cache));
}
