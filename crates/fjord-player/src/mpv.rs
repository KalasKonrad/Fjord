// ── fjord-player · mpv.rs ────────────────────────────────────────────────────
//   PlayerConfig    hwdec, sync, tscale, audio_device, subtitle appearance
//                   (sub_scale/sub_pos always applied; sub_respect_ass_styling/
//                   sub_color/sub_background only applied when non-default —
//                   see the doc comment above those fields) and all other mpv options
//   PollResult      Running | Finished | TrackChanged (gapless transition, same instance)
//   redact_api_key  replace api_key= query value with REDACTED for token-safe URL logging
//   StatsData       snapshot of mpv property values for the stats overlay
//                   includes video_sync_mode (reads "video-sync" property back from mpv)
//   Player          libmpv2 wrapper: init, property set/get, seek, volume, tracks
//                   get_buffering: paused-for-cache + cache-buffering-state 0-100
//                   get_buffer_end_fraction: (time-pos + demuxer-cache-duration) / duration as f32
//                   is_paused: reads mpv "pause" property directly (used by pause_play_toggle to stay in sync)
//                   poll_passthrough: 1 IPC read — true when audio-out-params/format is iec61937 (passthrough)
//                   get_drop_counts: 2 IPC reads — (frame-drop-count, decoder-frame-drop-count)
//                   log_decoder_info: also logs effective video-sync after playback starts
//                   get_chapter_count: chapter-list/count (cheap — used for polling)
//                   get_chapters: Vec<(start_secs, title)> for all chapters
//                   chapter_step: add chapter ±1 (next/prev chapter navigation)
//                   adjust_sub_delay: add delta_ms to sub-delay; returns new value in seconds
//                   adjust_audio_delay: add delta_ms to audio-delay; returns new value in seconds
//                   set_sub_style: live-update sub-scale/sub-pos/sub-ass-override/sub-color/
//                     sub-back-color+sub-border-style on a running instance — same
//                     conditional-apply rules as PlayerConfig's construction-time fields
//                   append_gapless/cancel_pending: queue/drop a gapless-appended playlist entry
//                   poll: EndFile only reports TrackChanged when reason is Eof — an abnormal end
//                     (error/stop/quit) with a pending append discards it instead of claiming a
//                     transition that may never have started (CR11-11); cancel_pending checks
//                     playlist-pos first so it doesn't remove an entry mpv already made active (CR11-13)
//   TrackInfo       audio / video / subtitle track descriptor; external_filename for external subs
//   MpvRenderCtx    OpenGL render context + FBO management; drop before Player
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::{ensure, Result};
use libmpv2::{events::Event, mpv_end_file_reason, FileState, Format, Mpv};
use std::ffi::{c_void, CStr};
use tracing::{debug, info, warn};

use libmpv2_sys as sys;

// ── PlayerConfig ──────────────────────────────────────────────────────────────

/// All user-configurable mpv settings.  `vo` is always forced to "libmpv"
/// internally; the render context takes care of GPU output.
#[derive(Clone, Debug)]
pub struct PlayerConfig {
    pub video_sync:             String,
    pub opengl_early_flush:     bool,
    pub video_latency_hacks:    bool,
    pub interpolation:          bool,
    pub tscale:                 String,
    pub tone_mapping:           String,
    pub target_colorspace_hint: bool,
    pub hwdec:                  String,
    pub vf:                     String,
    pub deinterlace:            String,
    pub audio_spdif_formats:    String,
    pub audio_device:           String,
    // Passthrough-only device ("" = use audio_device). Resolved by the caller
    // (start_playback) into audio_device before Player::new — never read here.
    pub audio_device_passthrough: String,
    // mpv --audio-channels ("auto-safe" = mpv default, not set explicitly).
    pub audio_channels:         String,
    pub cache_size_mb:          u32,
    pub start_position_secs:    Option<f64>,
    // ── Subtitle appearance ──────────────────────────────────────────────────
    // sub-scale/sub-pos apply to ASS-styled subtitles too under mpv's own
    // default sub-ass-override (="scale"), so these are always applied —
    // 1.0/100 are mpv's own defaults, so that's a genuine no-op, not a
    // behavior change for anyone who hasn't touched these settings.
    pub sub_scale:              f64,
    pub sub_pos:                i64,
    // false forces sub-ass-override=force so sub_color/sub_background below
    // also apply to ASS-styled subtitles (mpv's own default leaves ASS
    // styling alone for those two). true (default) never touches the option.
    pub sub_respect_ass_styling: bool,
    // Raw mpv color string (e.g. "#FFFF00"), already resolved from a display
    // name by the caller — empty means "don't touch sub-color at all".
    pub sub_color:              String,
    pub sub_background:         bool,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            video_sync:             "audio".into(),
            opengl_early_flush:     false,
            video_latency_hacks:    false,
            interpolation:          false,
            tscale:                 "oversample".into(),
            tone_mapping:           "auto".into(),
            target_colorspace_hint: false,
            hwdec:                  "auto".into(),
            vf:                     "".into(),
            deinterlace:            "no".into(),
            audio_spdif_formats:    String::new(),
            audio_device:           String::new(),
            audio_device_passthrough: String::new(),
            audio_channels:         String::new(),
            cache_size_mb:          0,
            start_position_secs:    None,
            sub_scale:              1.0,
            sub_pos:                100,
            sub_respect_ass_styling: true,
            sub_color:              String::new(),
            sub_background:         false,
        }
    }
}

// ── PollResult ────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum PollResult {
    Running,
    Finished,
    /// The current file ended but a gapless-appended entry took over —
    /// playback continues in the SAME mpv instance (no teardown).
    TrackChanged,
}

/// Replace the `api_key=` query value with `REDACTED` so stream URLs can be
/// logged without writing the Jellyfin token to the log file.
pub fn redact_api_key(url: &str) -> String {
    match url.find("api_key=") {
        Some(start) => {
            let val_start = start + "api_key=".len();
            let val_end = url[val_start..]
                .find('&')
                .map(|i| val_start + i)
                .unwrap_or(url.len());
            format!("{}REDACTED{}", &url[..val_start], &url[val_end..])
        }
        None => url.to_string(),
    }
}

// ── StatsData ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct StatsData {
    // video input (decoder output)
    pub video_codec:      String,
    pub width:            i64,
    pub height:           i64,
    pub fps:              f64,
    pub video_pix_fmt:    String, // video-params/pixelformat
    pub video_primaries:  String, // video-params/primaries  (bt.709, bt.2020, …)
    pub video_gamma:      String, // video-params/gamma      (srgb, bt.1886, pq, hlg, …)
    pub video_sig_peak:   f64,    // video-params/sig-peak   (1.0 = SDR, 10 = 1000 nit HDR)
    // video output (after filters / scaling)
    pub video_out_pix_fmt: String, // video-out-params/pixelformat
    pub video_out_w:       i64,
    pub video_out_h:       i64,
    // hardware decode
    pub hwdec_current:    String,
    // audio input
    pub audio_codec:      String,
    pub audio_codec_name: String, // audio-codec-name (short: "truehd", "eac3", …)
    pub audio_channels:   String, // audio-params/channels  ("stereo", "5.1", "7.1", …)
    pub audio_samplerate: i64,    // audio-params/samplerate
    // audio output
    pub current_ao:           String, // current-ao  ("pipewire", "alsa", …)
    pub audio_out_format:     String, // audio-out-params/format ("f32", "iec61937-…" for passthrough)
    pub audio_out_channels:   String, // audio-out-params/channels
    pub audio_out_samplerate: i64,    // audio-out-params/samplerate
    // display
    pub display_fps:      f64,    // display-fps
    // display sync
    pub video_sync_mode:  String, // "video-sync" property (audio / display-resample / …)
    // timing / performance
    pub vsync_ratio:             f64,
    pub avsync:                  f64,
    pub audio_speed_correction:  f64,   // audio-speed-correction  (~0 with passthrough; drift = sync stress)
    pub video_speed_correction:  f64,   // video-speed-correction  (vsync=audio compensation)
    pub dropped_frames:          i64,   // frame-drop-count         (VO-level drops)
    pub decoder_dropped:         i64,   // decoder-frame-drop-count (pipeline/decoder drops)
    pub mistimed_frames:         i64,   // mistimed-frame-count     (wrong display timing)
    pub video_bitrate:           f64,
    pub audio_bitrate:           f64,
    pub cache_state:             i64,
}

// ── Player ────────────────────────────────────────────────────────────────────

pub struct Player {
    // Number of gapless-appended playlist entries not yet consumed by EndFile.
    pending_appends: u32,
    mpv:      Mpv,
    vf_auto:  bool,
}

impl Player {
    /// Initialise mpv with `vo=libmpv` (render-API mode) and start loading `url`.
    pub fn new(url: &str, config: &PlayerConfig) -> Result<Self> {
        let mut mpv = Mpv::with_initializer(|init| {
            // vo=libmpv: mpv never creates its own window; all rendering goes
            // through mpv_render_context_render() called by the host.
            init.set_option("vo", "libmpv")?;
            // Suppress mpv's own OSD — we render controls and seek position in Slint.
            init.set_option("osd-level", "0")?;
            if config.video_sync != "audio" && !config.video_sync.is_empty() {
                init.set_option("video-sync", config.video_sync.as_str())?;
            }
            if config.interpolation {
                init.set_option("interpolation", "yes")?;
                if !config.tscale.is_empty() {
                    init.set_option("tscale", config.tscale.as_str())?;
                }
            }
            if config.opengl_early_flush   { init.set_option("opengl-early-flush",   "yes")?; }
            if config.video_latency_hacks  { init.set_option("video-latency-hacks",  "yes")?; }
            if config.tone_mapping != "auto" && !config.tone_mapping.is_empty() {
                init.set_option("tone-mapping", config.tone_mapping.as_str())?;
            }
            if config.target_colorspace_hint { init.set_option("target-colorspace-hint", "yes")?; }
            init.set_option("hwdec", config.hwdec.as_str())?;
            if !config.vf.is_empty() && config.vf != "auto" {
                init.set_option("vf", config.vf.as_str())?;
            }
            if config.deinterlace != "no" && !config.deinterlace.is_empty() {
                init.set_option("deinterlace", config.deinterlace.as_str())?;
            }
            if !config.audio_spdif_formats.is_empty() {
                init.set_option("audio-spdif", config.audio_spdif_formats.as_str())?;
            }
            if !config.audio_device.is_empty() {
                init.set_option("audio-device", config.audio_device.as_str())?;
            }
            if !config.audio_channels.is_empty() && config.audio_channels != "auto-safe" {
                init.set_option("audio-channels", config.audio_channels.as_str())?;
            }
            if config.cache_size_mb > 0 {
                let secs = ((config.cache_size_mb as f64) * 0.8).max(10.0);
                init.set_option("cache-secs", format!("{:.0}", secs).as_str())?;
            }
            if let Some(pos) = config.start_position_secs {
                if pos > 0.0 {
                    init.set_option("start", format!("{:.3}", pos).as_str())?;
                }
            }
            // Subtitle appearance — scale/pos are safe to always set (1.0/100
            // are mpv's own defaults). ass-override/color/background are only
            // set when non-default, since even a "looks like default" value
            // engages mpv's override machinery for ASS-styled subtitles.
            init.set_option("sub-scale", format!("{:.2}", config.sub_scale).as_str())?;
            init.set_option("sub-pos", format!("{}", config.sub_pos).as_str())?;
            if !config.sub_respect_ass_styling {
                init.set_option("sub-ass-override", "force")?;
            }
            if !config.sub_color.is_empty() {
                init.set_option("sub-color", config.sub_color.as_str())?;
            }
            if config.sub_background {
                init.set_option("sub-back-color", "#C0000000")?;
                init.set_option("sub-border-style", "background-box")?;
            }
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("mpv init failed: {}", e))?;

        mpv.event_context_mut()
            .observe_property("vsync-ratio", Format::Double, 1)
            .map_err(|e| anyhow::anyhow!("observe vsync-ratio: {}", e))?;

        mpv.playlist_load_files(&[(url, FileState::Replace, None)])
            .map_err(|e| anyhow::anyhow!("loadfile failed: {}", e))?;

        if let Some(pos) = config.start_position_secs {
            info!("resuming from {:.0}s ({:.0}m {:.0}s)", pos, pos / 60.0, pos % 60.0);
        }
        info!(
            "mpv player started: {} [hwdec={}, vf={:?}, video-sync={}, opengl-early-flush={}, video-latency-hacks={}, audio-device={:?}, audio-channels={}]",
            redact_api_key(url),
            config.hwdec,
            config.vf,
            config.video_sync,
            config.opengl_early_flush,
            config.video_latency_hacks,
            config.audio_device,
            config.audio_channels,
        );
        Ok(Player { pending_appends: 0, mpv, vf_auto: config.vf == "auto" })
    }

    /// Queue the next file into mpv's internal playlist. mpv's gapless-audio
    /// (default `weak`) then transitions seamlessly when the current file ends;
    /// poll() reports `TrackChanged` instead of `Finished`.
    pub fn append_gapless(&mut self, url: &str) -> anyhow::Result<()> {
        self.mpv.command("loadfile", &[url, "append"])
            .map_err(|e| anyhow::anyhow!("loadfile append failed: {}", e))?;
        self.pending_appends += 1;
        Ok(())
    }

    /// Drop the queued gapless entry (the upcoming order changed — shuffle,
    /// repeat, queue edits). Entry 0 is the playing file; pending ones follow.
    pub fn cancel_pending(&mut self) {
        if self.pending_appends > 0 {
            // CR11-13: mpv's own demux/decode thread can advance into the
            // appended entry between our last poll() and this call (a user
            // toggling shuffle/repeat right as a track ends). If playlist-pos
            // is no longer 0, mpv already made entry 1 the active one — remove
            // it now and we'd cut off audio mpv is already playing. In that
            // case there's nothing left to cancel; the next poll() will see it
            // as a normal EndFile/TrackChanged instead.
            let already_active = self.mpv.get_property::<i64>("playlist-pos").unwrap_or(0) != 0;
            if !already_active {
                let _ = self.mpv.command("playlist-remove", &["1"]);
            }
            self.pending_appends -= 1;
        }
    }

    /// Raw mpv handle for `MpvRenderCtx::new`.  Valid for the lifetime of this
    /// `Player` — do not store it beyond that.
    pub fn raw_handle_ptr(&self) -> *mut sys::mpv_handle {
        self.mpv.ctx.as_ptr()
    }

    /// Drain all pending mpv events without blocking.  Call every frame from a
    /// Slint timer.  Returns `Finished` when the file ends or mpv shuts down.
    pub fn poll(&mut self) -> PollResult {
        loop {
            match self.mpv.event_context_mut().wait_event(0.0) {
                Some(Ok(Event::Shutdown))        => { info!("mpv: shutdown");                   return PollResult::Finished; }
                Some(Ok(Event::EndFile(reason))) => {
                    if self.pending_appends > 0 {
                        self.pending_appends -= 1;
                        if reason == mpv_end_file_reason::Eof {
                            // Current file ended cleanly and a gapless-appended
                            // entry follows — mpv keeps playing without a gap.
                            info!("mpv: end-of-file ({:?}) — gapless transition", reason);
                            return PollResult::TrackChanged;
                        }
                        // CR11-11: the file ended abnormally (error/stop/quit) with
                        // an append still queued. mpv may never have actually
                        // started the appended entry, so don't claim the
                        // transition happened — that showed a track as "now
                        // playing" (with a start/stop report pair sent) that
                        // never produced audio. Drop the still-queued entry so it
                        // can't surface later as a phantom track.
                        warn!("mpv: end-of-file ({:?}) with a pending gapless append — discarding it", reason);
                        let _ = self.mpv.command("playlist-remove", &["1"]);
                    }
                    info!("mpv: end-of-file ({:?})", reason);
                    return PollResult::Finished;
                }
                Some(Ok(ev))                     => { debug!("mpv event: {:?}", ev); }
                // Transient error events (e.g. property errors) must not tear down
                // playback — only Shutdown/EndFile end it (CR10-15).
                Some(Err(e))                     => { warn!("mpv error event (ignored): {:?}", e); }
                None                             => return PollResult::Running,
            }
        }
    }

    pub fn poll_stats(&self) -> StatsData {
        let g_s  = |k: &str| self.mpv.get_property::<String>(k).unwrap_or_default();
        let g_i  = |k: &str| self.mpv.get_property::<i64>(k).unwrap_or(0);
        let g_f  = |k: &str| self.mpv.get_property::<f64>(k).unwrap_or(0.0);
        StatsData {
            video_codec:          g_s("video-codec"),
            width:                g_i("width"),
            height:               g_i("height"),
            fps:                  g_f("estimated-vf-fps"),
            video_pix_fmt:        g_s("video-params/pixelformat"),
            video_primaries:      g_s("video-params/primaries"),
            video_gamma:          g_s("video-params/gamma"),
            video_sig_peak:       g_f("video-params/sig-peak"),
            video_out_pix_fmt:    g_s("video-out-params/pixelformat"),
            video_out_w:          g_i("video-out-params/w"),
            video_out_h:          g_i("video-out-params/h"),
            hwdec_current:        g_s("hwdec-current"),
            audio_codec:          g_s("audio-codec"),
            audio_codec_name:     g_s("audio-codec-name"),
            audio_channels:       g_s("audio-params/channels"),
            audio_samplerate:     g_i("audio-params/samplerate"),
            current_ao:           g_s("current-ao"),
            audio_out_format:     g_s("audio-out-params/format"),
            audio_out_channels:   g_s("audio-out-params/channels"),
            audio_out_samplerate: g_i("audio-out-params/samplerate"),
            display_fps:          { let d = g_f("display-fps"); if d > 0.0 { d } else { g_f("estimated-display-fps") } },
            video_sync_mode:      g_s("video-sync"),
            vsync_ratio:             g_f("vsync-ratio"),
            avsync:                  g_f("avsync"),
            audio_speed_correction:  g_f("audio-speed-correction"),
            video_speed_correction:  g_f("video-speed-correction"),
            dropped_frames:          g_i("frame-drop-count"),
            decoder_dropped:         g_i("decoder-frame-drop-count"),
            mistimed_frames:         g_i("mistimed-frame-count"),
            video_bitrate:           g_f("video-bitrate"),
            audio_bitrate:           g_f("audio-bitrate"),
            cache_state:             g_i("cache-buffering-state"),
        }
    }

    /// Returns true when audio is bitstream passthrough (iec61937). Single IPC read —
    /// used by the 16 ms timer when the stats overlay is hidden to keep the passthrough
    /// flag current without running the full 31-read poll_stats.
    pub fn poll_passthrough(&self) -> bool {
        self.mpv.get_property::<String>("audio-out-params/format")
            .unwrap_or_default()
            .starts_with("iec61937")
    }

    /// Returns (frame-drop-count, decoder-frame-drop-count). Two IPC reads.
    /// Used for stop-time logging and periodic in-session log lines.
    pub fn get_drop_counts(&self) -> (i64, i64) {
        let dropped         = self.mpv.get_property::<i64>("frame-drop-count").unwrap_or(0);
        let decoder_dropped = self.mpv.get_property::<i64>("decoder-frame-drop-count").unwrap_or(0);
        (dropped, decoder_dropped)
    }

    pub fn log_decoder_info(&self) {
        let hwdec      = self.mpv.get_property::<String>("hwdec-current").unwrap_or_default();
        let codec      = self.mpv.get_property::<String>("video-codec").unwrap_or_default();
        let w: i64     = self.mpv.get_property("width").unwrap_or(0);
        let h: i64     = self.mpv.get_property("height").unwrap_or(0);
        let fps        = self.mpv.get_property::<f64>("estimated-vf-fps").unwrap_or(0.0);
        let video_sync = self.mpv.get_property::<String>("video-sync").unwrap_or_default();
        info!(
            "active decoder: hwdec-current={:?}, codec={}, {}x{} {:.2}fps, video-sync={}",
            hwdec, codec, w, h, fps, video_sync,
        );
    }

    /// If vf=auto was requested, detect the active decoder + input pixel format
    /// and apply the appropriate tight-packed format filter at runtime.
    /// Called ~2 s after playback starts once the decoder is confirmed active.
    pub fn apply_auto_vf(&self) {
        if !self.vf_auto { return; }

        let hwdec   = self.mpv.get_property::<String>("hwdec-current").unwrap_or_default();
        let pix_fmt = self.mpv.get_property::<String>("video-params/pixelformat").unwrap_or_default();

        if !hwdec.contains("nvdec") {
            info!("auto vf: no filter needed (hwdec={})", hwdec);
            return;
        }

        let is_copy    = hwdec.ends_with("-copy");
        let is_high_bit = pix_fmt.contains("p010") || pix_fmt.contains("10le")
                       || pix_fmt.contains("10be") || pix_fmt.contains("16");

        let fmt = match (is_copy, is_high_bit) {
            (true,  true)  => "format=yuv420p10le",
            (true,  false) => "format=yuv420p",
            (false, true)  => "format=p010",
            (false, false) => "format=nv12",
        };

        match self.mpv.command("vf", &["set", fmt]) {
            Ok(_)  => info!("auto vf: applied {} (hwdec={}, input={})", fmt, hwdec, pix_fmt),
            Err(e) => warn!("auto vf: failed to apply {}: {:#}", fmt, e),
        }
    }

    pub fn toggle_pause(&self) {
        let paused: bool = self.mpv.get_property("pause").unwrap_or(false);
        if paused { self.mpv.unpause().ok(); } else { self.mpv.pause().ok(); }
    }
    /// Set the pause state unconditionally (no read-then-write race).
    pub fn set_paused(&self, paused: bool) {
        if let Err(e) = self.mpv.set_property("pause", paused) {
            warn!("set_paused({}) failed: {}", paused, e);
        }
    }
    pub fn is_paused(&self) -> bool {
        self.mpv.get_property("pause").unwrap_or(false)
    }
    pub fn seek_forward(&self, secs: f64)  { self.mpv.seek_forward(secs).ok(); }
    pub fn seek_backward(&self, secs: f64) { self.mpv.seek_backward(secs).ok(); }
    pub fn stop(&self)                     { self.mpv.command("quit", &[]).ok(); }

    /// Adjust volume by `delta` and return the resulting level (0–130).
    pub fn adjust_volume(&self, delta: f64) -> f64 {
        let s = format!("{}", delta);
        if let Err(e) = self.mpv.command("add", &["volume", &s]) {
            warn!("adjust_volume {} failed: {}", delta, e);
        }
        let vol = self.mpv.get_property::<f64>("volume").unwrap_or(100.0);
        debug!("volume adjusted by {} → {:.0}", delta, vol);
        vol
    }

    pub fn set_video_track(&self, id: i64) {
        if let Err(e) = self.mpv.set_property("vid", id) {
            warn!("set_video_track {} failed: {}", id, e);
        }
    }

    pub fn toggle_mute(&self) {
        let muted = self.mpv.get_property::<bool>("mute").unwrap_or(false);
        if let Err(e) = self.mpv.set_property("mute", !muted) {
            warn!("toggle_mute failed: {}", e);
        } else {
            debug!("mute → {}", !muted);
        }
    }

    pub fn get_position(&self) -> f64 {
        self.mpv.get_property::<f64>("time-pos").unwrap_or(0.0)
    }
    pub fn get_duration(&self) -> f64 {
        self.mpv.get_property::<f64>("duration").unwrap_or(0.0)
    }
    pub fn get_buffering(&self) -> (bool, i32) {
        let stalled = self.mpv.get_property::<bool>("paused-for-cache").unwrap_or(false);
        let pct     = self.mpv.get_property::<i64>("cache-buffering-state").unwrap_or(0);
        (stalled, pct as i32)
    }
    pub fn get_buffer_end_fraction(&self) -> f32 {
        let dur = self.get_duration();
        if dur <= 0.0 { return 0.0; }
        let pos = self.mpv.get_property::<f64>("time-pos").unwrap_or(0.0);
        let buf = self.mpv.get_property::<f64>("demuxer-cache-duration").unwrap_or(0.0);
        ((pos + buf) / dur).min(1.0) as f32
    }
    pub fn seek_to(&self, secs: f64) {
        if let Err(e) = self.mpv.set_property("time-pos", secs) {
            warn!("seek_to {:.1}s failed: {}", secs, e);
        }
    }

    /// Apply subtitle appearance to the running instance — the live-update
    /// counterpart to PlayerConfig's construction-time application, so a
    /// Settings change takes effect immediately instead of waiting for the
    /// next file. Same conditional-apply rules as `Player::new`'s initializer.
    pub fn set_sub_style(&self, scale: f64, pos: i64, respect_ass_styling: bool, color: &str, background: bool) {
        if let Err(e) = self.mpv.set_property("sub-scale", scale) {
            warn!("set_sub_style: sub-scale failed: {}", e);
        }
        if let Err(e) = self.mpv.set_property("sub-pos", pos) {
            warn!("set_sub_style: sub-pos failed: {}", e);
        }
        if !respect_ass_styling {
            if let Err(e) = self.mpv.set_property("sub-ass-override", "force") {
                warn!("set_sub_style: sub-ass-override failed: {}", e);
            }
        }
        if !color.is_empty() {
            if let Err(e) = self.mpv.set_property("sub-color", color) {
                warn!("set_sub_style: sub-color failed: {}", e);
            }
        }
        if background {
            if let Err(e) = self.mpv.set_property("sub-back-color", "#C0000000") {
                warn!("set_sub_style: sub-back-color failed: {}", e);
            }
            if let Err(e) = self.mpv.set_property("sub-border-style", "background-box") {
                warn!("set_sub_style: sub-border-style failed: {}", e);
            }
        }
    }

    pub fn set_sub_track(&self, id: i64) {
        if let Err(e) = self.mpv.set_property("sid", id) {
            warn!("set_sub_track {} failed: {}", id, e);
        }
    }
    pub fn set_audio_track(&self, id: i64) {
        if let Err(e) = self.mpv.set_property("aid", id) {
            warn!("set_audio_track {} failed: {}", id, e);
        }
    }

    /// Cheap probe: number of chapters (0 if none or not yet loaded).
    pub fn get_chapter_count(&self) -> i64 {
        self.mpv.get_property::<i64>("chapter-list/count").unwrap_or(0)
    }

    /// Return all chapters as (start_secs, title) pairs.
    pub fn get_chapters(&self) -> Vec<(f64, String)> {
        let count = self.get_chapter_count();
        (0..count as usize).map(|i| {
            let time  = self.mpv.get_property::<f64>(&format!("chapter-list/{}/time", i)).unwrap_or(0.0);
            let title = self.mpv.get_property::<String>(&format!("chapter-list/{}/title", i)).unwrap_or_default();
            (time, title)
        }).collect()
    }

    /// Step to the next (delta=1) or previous (delta=-1) chapter.
    pub fn chapter_step(&self, delta: i64) {
        let s = delta.to_string();
        if let Err(e) = self.mpv.command("add", &["chapter", &s]) {
            warn!("chapter_step {} failed: {}", delta, e);
        }
    }

    /// Nudge subtitle delay by `delta_ms` milliseconds and return the new value in seconds.
    pub fn adjust_sub_delay(&self, delta_ms: i64) -> f64 {
        let s = format!("{}", delta_ms as f64 / 1000.0);
        if let Err(e) = self.mpv.command("add", &["sub-delay", &s]) {
            warn!("adjust_sub_delay {} ms failed: {}", delta_ms, e);
        }
        self.mpv.get_property::<f64>("sub-delay").unwrap_or(0.0)
    }

    /// Nudge audio delay by `delta_ms` milliseconds and return the new value in seconds.
    pub fn adjust_audio_delay(&self, delta_ms: i64) -> f64 {
        let s = format!("{}", delta_ms as f64 / 1000.0);
        if let Err(e) = self.mpv.command("add", &["audio-delay", &s]) {
            warn!("adjust_audio_delay {} ms failed: {}", delta_ms, e);
        }
        self.mpv.get_property::<f64>("audio-delay").unwrap_or(0.0)
    }

    /// Returns all tracks from mpv's track-list property.
    pub fn get_tracks(&self) -> Vec<TrackInfo> {
        let count = self.mpv.get_property::<i64>("track-list/count").unwrap_or(0);
        (0..count as usize).map(|i| {
            let g  = |k: &str| self.mpv.get_property::<String>(&format!("track-list/{}/{}", i, k)).unwrap_or_default();
            let gi = |k: &str| self.mpv.get_property::<i64>(&format!("track-list/{}/{}", i, k)).unwrap_or(0);
            TrackInfo {
                id:                gi("id"),
                track_type:        g("type"),
                title:             g("title"),
                lang:              g("lang"),
                selected:          gi("selected") != 0,
                codec:             g("codec"),
                external_filename: g("external-filename"),
                forced:            gi("forced") != 0,
                hearing_impaired:  gi("hearing-impaired") != 0,
            }
        }).collect()
    }
}

// ── TrackInfo ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub id:                i64,
    pub track_type:        String,
    pub title:             String,
    pub lang:              String,
    pub selected:          bool,
    pub codec:             String,
    pub external_filename: String,
    pub forced:            bool,
    pub hearing_impaired:  bool,
}

// ── MpvRenderCtx ─────────────────────────────────────────────────────────────

/// Wraps `mpv_render_context` for OpenGL rendering via the mpv render API.
///
/// Drop ordering: always drop `MpvRenderCtx` **before** dropping `Player`.
/// mpv docs: `mpv_render_context_free` must be called before `mpv_terminate_destroy`.
pub struct MpvRenderCtx {
    ctx:     *mut sys::mpv_render_context,
    // Heap-allocated closure called by mpv when a new frame is ready.
    // Freed in Drop after mpv_render_context_free stops the callbacks.
    cb_data: *mut Box<dyn Fn() + Send + 'static>,
}

// We only ever use MpvRenderCtx on the main thread, but the cb_data pointer
// must be Send because the update callback is called from mpv's thread.
unsafe impl Send for MpvRenderCtx {}

impl MpvRenderCtx {
    /// Create the OpenGL render context.
    ///
    /// # Safety
    /// Must be called with the GL context **current** — i.e. from inside a
    /// Slint `BeforeRendering` notifier callback.  `handle` must be the raw
    /// pointer obtained from `Player::raw_handle_ptr()` and remain valid for
    /// the lifetime of the returned `MpvRenderCtx`.
    pub unsafe fn new(
        handle:   *mut sys::mpv_handle,
        get_proc: &dyn Fn(&CStr) -> *const c_void,
    ) -> Result<Self> {
        // C trampoline: mpv calls this to resolve OpenGL function pointers.
        // `ctx` points to the `get_proc` reference on the stack — safe because
        // `mpv_render_context_create` is synchronous (all lookups happen before
        // it returns).
        unsafe extern "C" fn gpa(
            ctx:  *mut c_void,
            name: *const std::os::raw::c_char,
        ) -> *mut c_void {
            let f = &*(ctx as *const &dyn Fn(&CStr) -> *const c_void);
            f(CStr::from_ptr(name)) as *mut c_void
        }

        let mut init_params = sys::mpv_opengl_init_params {
            get_proc_address:     Some(gpa),
            get_proc_address_ctx: &get_proc as *const _ as *mut c_void,
        };

        let api_type = b"opengl\0";
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data:  api_type.as_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data:  &mut init_params as *mut _ as *mut c_void,
            },
            sys::mpv_render_param { type_: 0, data: std::ptr::null_mut() },
        ];

        let mut ctx: *mut sys::mpv_render_context = std::ptr::null_mut();
        let rc = sys::mpv_render_context_create(&mut ctx, handle, params.as_mut_ptr());
        ensure!(rc == 0, "mpv_render_context_create failed (code {})", rc);
        ensure!(!ctx.is_null(), "mpv_render_context_create returned null");

        Ok(Self { ctx, cb_data: std::ptr::null_mut() })
    }

    /// Render the current video frame into the given OpenGL FBO.
    /// `flip`: pass `true` because OpenGL's origin is bottom-left.
    pub fn render(&self, fbo: i32, w: i32, h: i32, flip: bool) -> Result<()> {
        let flip_i: i32 = flip as i32;
        let mut fbo_params = sys::mpv_opengl_fbo { fbo, w, h, internal_format: 0 };
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                data:  &mut fbo_params as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                data:  &flip_i as *const _ as *mut c_void,
            },
            sys::mpv_render_param { type_: 0, data: std::ptr::null_mut() },
        ];
        let rc = unsafe { sys::mpv_render_context_render(self.ctx, params.as_mut_ptr()) };
        ensure!(rc == 0, "mpv_render_context_render failed (code {})", rc);
        Ok(())
    }

    /// Inform mpv that the frame has been presented (vsync feedback).
    pub fn report_swap(&self) {
        unsafe { sys::mpv_render_context_report_swap(self.ctx); }
    }

    /// Set a callback invoked by mpv (from its internal thread) when a new
    /// video frame is ready to be rendered.  The callback must not call any
    /// mpv API — use `slint::invoke_from_event_loop` to queue work.
    pub fn set_update_callback<F: Fn() + Send + 'static>(&mut self, cb: F) {
        unsafe extern "C" fn trampoline(ctx: *mut c_void) {
            if ctx.is_null() { return; }
            let f = &*(ctx as *const Box<dyn Fn() + Send + 'static>);
            f();
        }

        // Drop existing callback first.
        // Safety (CR10-21): mpv invokes the update callback while holding the
        // render context's update_lock — the same mutex set_update_callback
        // takes (mpv render.c). So once the NULL-callback call returns, no
        // callback is in flight and none can start; freeing cb_data is safe.
        if !self.cb_data.is_null() {
            unsafe {
                sys::mpv_render_context_set_update_callback(self.ctx, None, std::ptr::null_mut());
                drop(Box::from_raw(self.cb_data));
            }
            self.cb_data = std::ptr::null_mut();
        }

        let boxed: Box<Box<dyn Fn() + Send + 'static>> = Box::new(Box::new(cb));
        self.cb_data = Box::into_raw(boxed);
        unsafe {
            sys::mpv_render_context_set_update_callback(
                self.ctx,
                Some(trampoline),
                self.cb_data as *mut c_void,
            );
        }
    }
}

impl Drop for MpvRenderCtx {
    fn drop(&mut self) {
        unsafe {
            // Clear callback so mpv stops touching cb_data, then free ctx.
            // Safety (CR10-21): the update callback runs under the same
            // update_lock this call takes, so after it returns no callback is
            // in flight — cb_data can be freed without a race.
            sys::mpv_render_context_set_update_callback(self.ctx, None, std::ptr::null_mut());
            sys::mpv_render_context_free(self.ctx);
            // cb_data is now safe to free.
            if !self.cb_data.is_null() {
                drop(Box::from_raw(self.cb_data));
            }
        }
    }
}
