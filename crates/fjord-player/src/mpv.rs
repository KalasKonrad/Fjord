// ── fjord-player · mpv.rs ────────────────────────────────────────────────────
//   PlayerConfig    hwdec, sync, tscale, audio device, cache, HDR passthrough, subtitle appearance
//                   (sub_scale/sub_pos always applied; ass override/colour/background only when
//                   non-default), ytdl_format (trailers only), dither_off (test aid), …
//   PollResult      Running | Finished | TrackChanged (gapless, same instance) | Failed(code)
//   redact_api_key  api_key= value → REDACTED, for every logged URL and forwarded mpv log line
//   StatsData       mpv values for the stats overlay; video_* = the decoded source (video-params),
//                   video_out_primaries/gamma/sig_peak = what goes to the display
//                   (video-target-params — after tone-mapping), video_out_pix_fmt/w/h = after
//                   the --vf chain (video-out-params)
//   SourceHdrMetadata  one-shot per-file HDR10 metadata for hdr.rs (Option<f64> luma/CLL/FALL —
//                   None = unavailable, not 0)
//   Player          libmpv2 wrapper:
//                   new(config) builds the core only; load(url) issues the loadfile once a render
//                     context is attached (vo=libmpv's init without one fails for good)
//                   playback state: get_buffering, get_buffer_end_fraction, is_paused,
//                     poll_passthrough, get_drop_counts, startup_snapshot, file_loaded_at,
//                     has_seen_video_reconfig (diagnostic), log_decoder_info
//                   query_source_hdr_metadata (after VideoReconfig), query_video_dimensions
//                     (width/height/fps for display sync; fps 0.0 = unknown), apply_hdr_output
//                   tracks: set_track_preferences (before load), get_tracks, set_sub_track /
//                     set_audio_track / set_video_track, apply_auto_vf,
//                     adjust_sub_delay / adjust_audio_delay, set_sub_style (live, same
//                     conditional rules as PlayerConfig)
//                   chapters: get_chapter_count, get_chapters, chapter_step
//                   transport: toggle_pause / set_paused, seek_forward / seek_backward / seek_to, stop,
//                     get_position / get_duration, poll_stats, raw_handle_ptr
//                   volume: adjust_volume, get_volume / set_volume (absolute — the skip-fade ramp),
//                     toggle_mute / set_mute
//                     (set_mute is the passthrough fallback; volume changes skip passthrough audio)
//                   append_gapless / cancel_pending (skips an entry mpv already made active)
//                   poll: EndFile → TrackChanged only for EOF; an abnormal end drops a pending
//                     append; END_FILE error codes (-13…-20) → Failed(code); Event::LogMessage
//                     (mpv's own log at warn+) → tracing
//   TrackInfo       audio / video / subtitle track; external_filename for external subs
//   MpvRenderCtx    OpenGL render context — drop before Player; render(fbo, w, h, flip,
//                   internal_format, depth) (depth = MPV_RENDER_PARAM_DEPTH, 0 = 8), report_swap,
//                   set_update_callback
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::{Result, ensure};
use libmpv2::{FileState, Format, Mpv, events::Event, mpv_end_file_reason};
use std::ffi::{CStr, c_void};
use tracing::{debug, error, info, warn};

use libmpv2_sys as sys;

// ── PlayerConfig ──────────────────────────────────────────────────────────────

/// All user-configurable mpv settings.  `vo` is always forced to "libmpv"
/// internally; the render context takes care of GPU output.
#[derive(Clone, Debug)]
pub struct PlayerConfig {
    pub video_sync: String,
    pub opengl_early_flush: bool,
    pub video_latency_hacks: bool,
    /// `dither-depth=no` (2026-10-08, a test aid): mpv's default dithering
    /// (`auto`, "fruit") hides 8-bit steps, so 8- vs 10-bit output can only
    /// be compared with it off.
    pub dither_off: bool,
    pub interpolation: bool,
    pub tscale: String,
    pub tone_mapping: String,
    pub target_colorspace_hint: bool,
    pub hwdec: String,
    pub vf: String,
    pub deinterlace: String,
    pub audio_spdif_formats: String,
    pub audio_device: String,
    // Passthrough-only device ("" = use audio_device). Resolved by the caller
    // (start_playback) into audio_device before Player::new — never read here.
    pub audio_device_passthrough: String,
    // mpv --audio-channels ("auto-safe" = mpv default, not set explicitly).
    pub audio_channels: String,
    // Network cache — see DeviceConfig's own doc comment in fjord-app for the
    // full story on why these are two separate mpv options, not one. 0 means
    // "don't set the option, use mpv's own default" for either field.
    pub cache_secs: u32,
    pub cache_max_mb: u32,
    pub start_position_secs: Option<f64>,
    // ── Subtitle appearance ──────────────────────────────────────────────────
    // sub-scale/sub-pos apply to ASS-styled subtitles too under mpv's own
    // default sub-ass-override (="scale"), so these are always applied —
    // 1.0/100 are mpv's own defaults, so that's a genuine no-op, not a
    // behavior change for anyone who hasn't touched these settings.
    pub sub_scale: f64,
    pub sub_pos: i64,
    // false forces sub-ass-override=force so sub_color/sub_background below
    // also apply to ASS-styled subtitles (mpv's own default leaves ASS
    // styling alone for those two). true (default) never touches the option.
    pub sub_respect_ass_styling: bool,
    // Raw mpv color string (e.g. "#FFFF00"), already resolved from a display
    // name by the caller — empty means "don't touch sub-color at all".
    pub sub_color: String,
    pub sub_background: bool,
    // mpv `ytdl-format` — a yt-dlp format-selector string, only meaningful
    // when the loaded URL isn't directly playable media (e.g. a YouTube
    // watch-page URL, resolved via mpv's bundled ytdl_hook). `None` = don't
    // set the property at all, leaving yt-dlp's own default selection —
    // every non-trailer call site leaves this `None`, a genuine no-op.
    pub ytdl_format: Option<String>,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            video_sync: "audio".into(),
            opengl_early_flush: false,
            video_latency_hacks: false,
            dither_off: false,
            interpolation: false,
            tscale: "oversample".into(),
            tone_mapping: "auto".into(),
            target_colorspace_hint: false,
            hwdec: "auto".into(),
            vf: "".into(),
            deinterlace: "no".into(),
            audio_spdif_formats: String::new(),
            audio_device: String::new(),
            audio_device_passthrough: String::new(),
            audio_channels: String::new(),
            cache_secs: 0,
            cache_max_mb: 0,
            start_position_secs: None,
            sub_scale: 1.0,
            sub_pos: 100,
            sub_respect_ass_styling: true,
            sub_color: String::new(),
            sub_background: false,
            ytdl_format: None,
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
    /// The file failed to open or play (mpv's END_FILE with an error code,
    /// e.g. -17 unknown format, -16 nothing to play, -13 loading failed).
    /// Unlike `Finished` it never counts as a natural end — the caller
    /// retries or closes the player with a message.
    Failed(i32),
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
    pub video_codec: String,
    pub width: i64,
    pub height: i64,
    pub fps: f64,
    pub video_pix_fmt: String,   // video-params/pixelformat
    pub video_primaries: String, // video-params/primaries  (bt.709, bt.2020, …)
    pub video_gamma: String,     // video-params/gamma      (srgb, bt.1886, pq, hlg, …)
    pub video_sig_peak: f64,     // video-params/sig-peak   (1.0 = SDR, 10 = 1000 nit HDR)
    // Video output after the --vf chain (confirms e.g. the NVIDIA stride-fix
    // vf=format=... took effect); video_primaries/video_gamma above are the decoded
    // source's video-params.
    pub video_out_pix_fmt: String, // video-out-params/pixelformat
    pub video_out_w: i64,
    pub video_out_h: i64,
    // Read from video-target-params — "the target properties that VO outputs to" (mpv
    // manual), i.e. what's actually sent to the display. video-out-params is only after
    // the --vf chain and never reflects the VO's tone-mapping/HDR passthrough. (So any
    // CLR IN/OUT conclusion drawn before this switch was from data that couldn't show a
    // difference.)
    pub video_out_primaries: String, // video-target-params/primaries
    pub video_out_gamma: String,     // video-target-params/gamma
    pub video_out_sig_peak: f64,     // video-target-params/sig-peak
    // hardware decode
    pub hwdec_current: String,
    // audio input
    pub audio_codec: String,
    pub audio_codec_name: String, // audio-codec-name (short: "truehd", "eac3", …)
    pub audio_channels: String,   // audio-params/channels  ("stereo", "5.1", "7.1", …)
    pub audio_samplerate: i64,    // audio-params/samplerate
    // audio output
    pub current_ao: String,         // current-ao  ("pipewire", "alsa", …)
    pub audio_out_format: String,   // audio-out-params/format ("f32", "iec61937-…" for passthrough)
    pub audio_out_channels: String, // audio-out-params/channels
    pub audio_out_samplerate: i64,  // audio-out-params/samplerate
    // display
    pub display_fps: f64, // display-fps
    // display sync
    pub video_sync_mode: String, // "video-sync" property (audio / display-resample / …)
    // timing / performance
    pub vsync_ratio: f64,
    pub avsync: f64,
    pub audio_speed_correction: f64, // audio-speed-correction  (~0 with passthrough; drift = sync stress)
    pub video_speed_correction: f64, // video-speed-correction  (vsync=audio compensation)
    pub dropped_frames: i64,         // frame-drop-count         (VO-level drops)
    pub decoder_dropped: i64,        // decoder-frame-drop-count (pipeline/decoder drops)
    pub mistimed_frames: i64,        // mistimed-frame-count     (wrong display timing)
    pub video_bitrate: f64,
    pub audio_bitrate: f64,
    pub cache_state: i64,
    // demuxer-cache-duration: seconds of video currently held in the
    // forward demuxer cache — mpv's own manual warns this guess "is very
    // unreliable, and often the property will not be available at all,
    // even if data is buffered" (reads 0.0 via the fallback in that case);
    // shown alongside cache_state since cache-buffering-state (the % most
    // players show) is specifically "% until the player will unpause" —
    // governed by cache-pause-wait (1s by default), not how full the real
    // configured buffer (cache-secs/demuxer-max-bytes) actually is, so it
    // reads ~100% almost immediately during normal healthy playback.
    pub cache_duration_secs: f64,
}

// ── SourceHdrMetadata ───────────────────────────────────────────────────────
// One-shot input for the HDR negotiation worker (fjord-app's hdr.rs), read once after
// VideoReconfig — separate from the overlay's StatsData. Option<f64>, so "not
// reported" stays distinguishable from a real 0 luma/CLL value.

#[derive(Clone, Debug, Default)]
pub struct SourceHdrMetadata {
    pub gamma: String, // video-params/gamma      ("pq", "bt.1886", "hlg", "srgb", …)
    pub primaries: String, // video-params/primaries  ("bt.2020", "bt.709", …)
    pub min_luma: Option<f64>, // video-params/min-luma (cd/m²) — real per-file HDR10 SEI value
    pub max_luma: Option<f64>, // video-params/max-luma (cd/m²)
    pub max_cll: Option<f64>, // video-params/max-cll  (cd/m²)
    pub max_fall: Option<f64>, // video-params/max-fall (cd/m²)
}

// ── Player ────────────────────────────────────────────────────────────────────

pub struct Player {
    // Number of gapless-appended playlist entries not yet consumed by EndFile.
    pending_appends: u32,
    mpv: Mpv,
    vf_auto: bool,
    // Set true the first time this instance's mpv core fires VideoReconfig.
    // Diagnostic for the audio-only-forever bug below, and now also the
    // signal has_seen_video_reconfig() exposes to wire_mpv_timer's own
    // (separate, still-present) 5s no-VideoReconfig warning.
    saw_video_reconfig: bool,
    // When this instance's mpv core fired its first FileLoaded — i.e. the file
    // is actually open and time-pos/duration/chapters/tracks are real. Before
    // that, time-pos reads 0 (get_position's fallback), which the app must not
    // treat as a real position.
    file_loaded_at: Option<std::time::Instant>,
    // Pre-formatted "[hwdec=..., vf=..., ...]" + resume position, captured at
    // construction so `load()` can log its "mpv player started" line without keeping the
    // whole PlayerConfig.
    startup_log_suffix: String,
    resume_secs: Option<f64>,
}

// "Unlimited" for PlayerConfig.cache_max_mb (Settings' 0 sentinel) — a fixed,
// large-enough-to-never-realistically-bind ceiling so cache_secs is the only
// real constraint, rather than mpv's own much smaller 150 MiB stock default.
const UNLIMITED_CACHE_MB: u32 = 65536;

impl Player {
    /// Initialise mpv with `vo=libmpv` (render-API mode). Deliberately does
    /// NOT load anything yet — call `load(url)` once a render context has
    /// been attached (see `load()`'s own doc comment for the full reasoning).
    pub fn new(config: &PlayerConfig) -> Result<Self> {
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
            if config.opengl_early_flush {
                init.set_option("opengl-early-flush", "yes")?;
            }
            if config.video_latency_hacks {
                init.set_option("video-latency-hacks", "yes")?;
            }
            if config.dither_off {
                init.set_option("dither-depth", "no")?;
            }
            if config.tone_mapping != "auto" && !config.tone_mapping.is_empty() {
                init.set_option("tone-mapping", config.tone_mapping.as_str())?;
            }
            // Always explicit "yes"/"no", never mpv's own default "auto": with "auto" mpv may
            // still attempt passthrough on its own, so the OFF toggle (and the chosen
            // tone-mapping curve) wouldn't apply.
            init.set_option(
                "target-colorspace-hint",
                if config.target_colorspace_hint {
                    "yes"
                } else {
                    "no"
                },
            )?;
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
            // cache-secs is a CEILING: mpv's default is ~3.6M s, so demuxer-max-bytes below is
            // what binds in practice (mpv manual: readahead "will usually be limited by
            // --demuxer-max-bytes"). The two are separate Settings rows.
            if config.cache_secs > 0 {
                init.set_option("cache-secs", format!("{}", config.cache_secs).as_str())?;
            }
            // demuxer-max-bytes: the real readahead byte ceiling (mpv default 150 MiB) — what
            // decides how long an outage is absorbed at a given bitrate. 0 = the row's
            // "Unlimited": a fixed, never-hit ceiling (not mpv's 150 MiB, the smallest value on
            // the row), so cache_secs alone governs.
            let cache_max_mb = if config.cache_max_mb > 0 {
                config.cache_max_mb
            } else {
                UNLIMITED_CACHE_MB
            };
            init.set_option("demuxer-max-bytes", format!("{}MiB", cache_max_mb).as_str())?;
            // Explicit ffmpeg HTTP reconnect options (stream-lavf-o) instead of
            // mpv/ffmpeg's undocumented defaults, which stopped reconnecting after a 503 + failed
            // seek although the server was back ~17 s later. Fjord only plays http(s) URLs, so
            // they always apply.
            init.set_option(
                "stream-lavf-o",
                "reconnect=1,reconnect_streamed=1,reconnect_delay_max=30",
            )?;
            if let Some(pos) = config.start_position_secs
                && pos > 0.0
            {
                init.set_option("start", format!("{:.3}", pos).as_str())?;
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
            if let Some(fmt) = &config.ytdl_format {
                init.set_option("ytdl-format", fmt.as_str())?;
            }
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("mpv init failed: {}", e))?;

        mpv.event_context_mut()
            .observe_property("vsync-ratio", Format::Double, 1)
            .map_err(|e| anyhow::anyhow!("observe vsync-ratio: {}", e))?;

        // Forward mpv's OWN internal log at warn and above (hwdec/vo/decoder failures that
        // never become an Event) — the "why" behind e.g. a video that never initialized.
        // mpv filters below warn itself, whatever Fjord's log level. See poll()'s
        // Event::LogMessage arm.
        let min_level = std::ffi::CString::new("warn").unwrap();
        let rc = unsafe { sys::mpv_request_log_messages(mpv.ctx.as_ptr(), min_level.as_ptr()) };
        if rc < 0 {
            warn!("mpv_request_log_messages failed: {}", rc);
        }

        // Deliberately does NOT call playlist_load_files here — see load()'s
        // own doc comment for why the actual loadfile is deferred to a
        // separate call the caller makes only once Fjord's own render
        // context has been created and attached to this mpv core.
        let startup_log_suffix = format!(
            "[hwdec={}, vf={:?}, video-sync={}, opengl-early-flush={}, video-latency-hacks={}, dither-depth={}, audio-device={:?}, audio-channels={}, ytdl-format={:?}]",
            config.hwdec,
            config.vf,
            config.video_sync,
            config.opengl_early_flush,
            config.video_latency_hacks,
            if config.dither_off { "no" } else { "auto" },
            config.audio_device,
            config.audio_channels,
            config.ytdl_format,
        );
        Ok(Player {
            pending_appends: 0,
            mpv,
            vf_auto: config.vf == "auto",
            saw_video_reconfig: false,
            file_loaded_at: None,
            startup_log_suffix,
            resume_secs: config.start_position_secs,
        })
    }

    /// Issues the actual `loadfile` for `url`. Called by the caller, once Fjord's
    /// `mpv_render_context` exists and is attached — from fjord-app's
    /// `wire_rendering_notifier` (`BeforeRendering`), the only call site.
    ///
    /// Why: `vo=libmpv` initializes video output as soon as the first frame needs it,
    /// which can be before Slint's GL thread has run `BeforeRendering` and created the
    /// context. Without a context that VO init fails ("No render context set.") and is
    /// never retried — audio plays, the screen stays black for that mpv instance. Loading
    /// only after `MpvRenderCtx::new()` succeeded, on that same GL thread, makes the order
    /// a guarantee instead of a race.
    pub fn load(&self, url: &str) -> Result<()> {
        self.mpv
            .playlist_load_files(&[(url, FileState::Replace, None)])
            .map_err(|e| anyhow::anyhow!("loadfile failed: {}", e))?;

        if let Some(pos) = self.resume_secs {
            info!(
                "resuming from {:.0}s ({:.0}m {:.0}s)",
                pos,
                pos / 60.0,
                pos % 60.0
            );
        }
        info!(
            "mpv player started: {} {}",
            redact_api_key(url),
            self.startup_log_suffix
        );
        Ok(())
    }

    /// Queue the next file into mpv's internal playlist. mpv's gapless-audio
    /// (default `weak`) then transitions seamlessly when the current file ends;
    /// poll() reports `TrackChanged` instead of `Finished`.
    pub fn append_gapless(&mut self, url: &str) -> anyhow::Result<()> {
        self.mpv
            .command("loadfile", &[url, "append"])
            .map_err(|e| anyhow::anyhow!("loadfile append failed: {}", e))?;
        self.pending_appends += 1;
        Ok(())
    }

    /// Drop the queued gapless entry (the upcoming order changed — shuffle,
    /// repeat, queue edits). Entry 0 is the playing file; pending ones follow.
    pub fn cancel_pending(&mut self) {
        if self.pending_appends > 0 {
            // mpv's demux/decode thread may already have advanced into the appended entry
            // (shuffle/repeat toggled as a track ends): if playlist-pos isn't 0, entry 1 is
            // already playing — removing it would cut off live audio. Nothing to cancel then; the
            // next poll() sees a normal EndFile/TrackChanged.
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
                Some(Ok(Event::Shutdown)) => {
                    info!("mpv: shutdown");
                    return PollResult::Finished;
                }
                Some(Ok(Event::EndFile(reason))) => {
                    if self.pending_appends > 0 {
                        self.pending_appends -= 1;
                        if reason == mpv_end_file_reason::Eof {
                            // Current file ended cleanly and a gapless-appended
                            // entry follows — mpv keeps playing without a gap.
                            info!("mpv: end-of-file ({:?}) — gapless transition", reason);
                            return PollResult::TrackChanged;
                        }
                        // The file ended abnormally with an append still queued: mpv may never have started
                        // it, so don't report a transition (a phantom "now playing" with no audio) — drop the
                        // queued entry.
                        warn!(
                            "mpv: end-of-file ({:?}) with a pending gapless append — discarding it",
                            reason
                        );
                        let _ = self.mpv.command("playlist-remove", &["1"]);
                    }
                    info!("mpv: end-of-file ({:?})", reason);
                    return PollResult::Finished;
                }
                Some(Ok(Event::VideoReconfig)) => {
                    self.saw_video_reconfig = true;
                    debug!("mpv event: VideoReconfig");
                }
                Some(Ok(Event::FileLoaded)) => {
                    if self.file_loaded_at.is_none() {
                        self.file_loaded_at = Some(std::time::Instant::now());
                    }
                    debug!("mpv event: FileLoaded");
                }
                // mpv's own internal log (requested at "warn" in Player::new) — hwdec init failures,
                // vo errors.
                Some(Ok(Event::LogMessage {
                    prefix,
                    level,
                    text,
                    ..
                })) => {
                    // mpv can quote the stream URL (api_key=…) in its messages.
                    let msg =
                        redact_api_key(&format!("mpv[{}] {}: {}", prefix, level, text.trim_end()));
                    match level {
                        "fatal" | "error" => error!("{}", msg),
                        // ffmpeg repeats this for every frame of some Dolby Vision
                        // files (836 lines in one HTPC session) — harmless, and it
                        // buried every other warning in fjord.log.
                        _ if text.contains("Multiple Dolby Vision RPUs") => debug!("{}", msg),
                        _ => warn!("{}", msg),
                    }
                }
                Some(Ok(ev)) => {
                    debug!("mpv event: {:?}", ev);
                }
                // A file that fails to open/play ends with an END_FILE carrying an mpv error code,
                // which libmpv2 returns as Err(Raw(code)), not Ok(EndFile) (libmpv2 events.rs).
                // Ignoring it left a black player forever. Only mpv's END_FILE codes (-13
                // LOADING_FAILED … -20 GENERIC) count; other errors stay transient.
                Some(Err(libmpv2::Error::Raw(code))) if (-20..=-13).contains(&code) => {
                    if self.pending_appends > 0 {
                        // Same as an abnormal EndFile above (CR11-11): don't
                        // let a queued gapless entry surface as a phantom track.
                        self.pending_appends -= 1;
                        let _ = self.mpv.command("playlist-remove", &["1"]);
                    }
                    warn!("mpv: file failed to open/play (mpv error {code})");
                    return PollResult::Failed(code);
                }
                // Transient error events (e.g. property errors) must not tear down
                // playback — only Shutdown/EndFile end it (CR10-15).
                Some(Err(e)) => {
                    warn!("mpv error event (ignored): {:?}", e);
                }
                None => return PollResult::Running,
            }
        }
    }

    pub fn poll_stats(&self) -> StatsData {
        let g_s = |k: &str| self.mpv.get_property::<String>(k).unwrap_or_default();
        let g_i = |k: &str| self.mpv.get_property::<i64>(k).unwrap_or(0);
        let g_f = |k: &str| self.mpv.get_property::<f64>(k).unwrap_or(0.0);
        StatsData {
            video_codec: g_s("video-codec"),
            width: g_i("width"),
            height: g_i("height"),
            fps: g_f("estimated-vf-fps"),
            video_pix_fmt: g_s("video-params/pixelformat"),
            video_primaries: g_s("video-params/primaries"),
            video_gamma: g_s("video-params/gamma"),
            video_sig_peak: g_f("video-params/sig-peak"),
            video_out_pix_fmt: g_s("video-out-params/pixelformat"),
            video_out_w: g_i("video-out-params/w"),
            video_out_h: g_i("video-out-params/h"),
            video_out_primaries: g_s("video-target-params/primaries"),
            video_out_gamma: g_s("video-target-params/gamma"),
            video_out_sig_peak: g_f("video-target-params/sig-peak"),
            hwdec_current: g_s("hwdec-current"),
            audio_codec: g_s("audio-codec"),
            audio_codec_name: g_s("audio-codec-name"),
            audio_channels: g_s("audio-params/channels"),
            audio_samplerate: g_i("audio-params/samplerate"),
            current_ao: g_s("current-ao"),
            audio_out_format: g_s("audio-out-params/format"),
            audio_out_channels: g_s("audio-out-params/channels"),
            audio_out_samplerate: g_i("audio-out-params/samplerate"),
            display_fps: {
                let d = g_f("display-fps");
                if d > 0.0 {
                    d
                } else {
                    g_f("estimated-display-fps")
                }
            },
            video_sync_mode: g_s("video-sync"),
            vsync_ratio: g_f("vsync-ratio"),
            avsync: g_f("avsync"),
            audio_speed_correction: g_f("audio-speed-correction"),
            video_speed_correction: g_f("video-speed-correction"),
            dropped_frames: g_i("frame-drop-count"),
            decoder_dropped: g_i("decoder-frame-drop-count"),
            mistimed_frames: g_i("mistimed-frame-count"),
            video_bitrate: g_f("video-bitrate"),
            audio_bitrate: g_f("audio-bitrate"),
            cache_state: g_i("cache-buffering-state"),
            cache_duration_secs: g_f("demuxer-cache-duration"),
        }
    }

    /// Returns true when audio is bitstream passthrough (iec61937). Single IPC read —
    /// used by the 16 ms timer when the stats overlay is hidden to keep the passthrough
    /// flag current without running the full 31-read poll_stats.
    pub fn poll_passthrough(&self) -> bool {
        self.mpv
            .get_property::<String>("audio-out-params/format")
            .unwrap_or_default()
            .starts_with("iec61937")
    }

    /// Returns (frame-drop-count, decoder-frame-drop-count). Two IPC reads.
    /// Used for stop-time logging and periodic in-session log lines.
    pub fn get_drop_counts(&self) -> (i64, i64) {
        let dropped = self
            .mpv
            .get_property::<i64>("frame-drop-count")
            .unwrap_or(0);
        let decoder_dropped = self
            .mpv
            .get_property::<i64>("decoder-frame-drop-count")
            .unwrap_or(0);
        (dropped, decoder_dropped)
    }

    /// True once this instance's mpv core has fired at least one VideoReconfig
    /// event — used to detect a video item whose video track is selected but
    /// never actually initializes (see `saw_video_reconfig`'s doc comment).
    pub fn has_seen_video_reconfig(&self) -> bool {
        self.saw_video_reconfig
    }

    /// When mpv finished opening the first file for this instance (its first
    /// FileLoaded event), or `None` while it's still opening. Position,
    /// duration, chapters and tracks are only meaningful after this.
    pub fn file_loaded_at(&self) -> Option<std::time::Instant> {
        self.file_loaded_at
    }

    /// The file's source colourspace/HDR10 metadata for the Wayland colour-management
    /// worker. Call only after VideoReconfig (`has_seen_video_reconfig()`) — before that
    /// the properties are unset/stale. The luma/CLL/FALL fields use `.ok()` so
    /// "unavailable" differs from a real 0 (see SourceHdrMetadata).
    pub fn query_source_hdr_metadata(&self) -> SourceHdrMetadata {
        let g_s = |k: &str| self.mpv.get_property::<String>(k).unwrap_or_default();
        let g_f = |k: &str| self.mpv.get_property::<f64>(k).ok();
        SourceHdrMetadata {
            gamma: g_s("video-params/gamma"),
            primaries: g_s("video-params/primaries"),
            min_luma: g_f("video-params/min-luma"),
            max_luma: g_f("video-params/max-luma"),
            max_cll: g_f("video-params/max-cll"),
            max_fall: g_f("video-params/max-fall"),
        }
    }

    /// Width, height and frame rate of the decoded video, for display sync's mode choice
    /// (same properties log_decoder_info / poll_stats use). The rate is mpv's estimate
    /// (estimated-vf-fps) once frames flow — not reliable at the instant VideoReconfig
    /// fires — else the container's declared rate; 0.0 = not known yet, and callers must
    /// not act on it (an "unusual" 0 once switched a 4K film to 1080p59.94).
    pub fn query_video_dimensions(&self) -> (i64, i64, f64) {
        let w = self.mpv.get_property::<i64>("width").unwrap_or(0);
        let h = self.mpv.get_property::<i64>("height").unwrap_or(0);
        let est = self
            .mpv
            .get_property::<f64>("estimated-vf-fps")
            .unwrap_or(0.0);
        let fps = if est > 0.0 {
            est
        } else {
            self.mpv.get_property::<f64>("container-fps").unwrap_or(0.0)
        };
        (w, h, fps)
    }

    /// Track preferences for the NEXT file, set before it loads: mpv then enables the
    /// right subtitle/audio tracks from the first byte. Switching a track after reading
    /// starts makes mpv drop and re-read its read-ahead (a ~1 s stop mid-film, slower
    /// start-ups on high-bitrate 4K). `slang`/`alang`: language codes in priority order
    /// (2- and 3-letter codes match each other); `subs` false = no subtitles.
    pub fn set_track_preferences(&self, slang: &[String], alang: &[String], subs: bool) {
        for (prop, value) in [("slang", slang.join(",")), ("alang", alang.join(","))] {
            if let Err(e) = self.mpv.set_property(prop, value.as_str()) {
                warn!("set_track_preferences: {prop}={value:?} failed: {e}");
            }
        }
        if let Err(e) = self
            .mpv
            .set_property("sid", if subs { "auto" } else { "no" })
        {
            warn!("set_track_preferences: sid failed: {e}");
        }
        debug!("track preferences: slang={slang:?} alang={alang:?} subs={subs}");
    }

    pub fn log_decoder_info(&self) {
        let hwdec = self
            .mpv
            .get_property::<String>("hwdec-current")
            .unwrap_or_default();
        let codec = self
            .mpv
            .get_property::<String>("video-codec")
            .unwrap_or_default();
        let w: i64 = self.mpv.get_property("width").unwrap_or(0);
        let h: i64 = self.mpv.get_property("height").unwrap_or(0);
        let fps = self
            .mpv
            .get_property::<f64>("estimated-vf-fps")
            .unwrap_or(0.0);
        let video_sync = self
            .mpv
            .get_property::<String>("video-sync")
            .unwrap_or_default();
        info!(
            "active decoder: hwdec-current={:?}, codec={}, {}x{} {:.2}fps, video-sync={}",
            hwdec, codec, w, h, fps, video_sync,
        );
    }

    /// If vf=auto was requested, detect the active decoder + input pixel format
    /// and apply the appropriate tight-packed format filter at runtime.
    /// Called ~2 s after playback starts once the decoder is confirmed active.
    pub fn apply_auto_vf(&self) {
        if !self.vf_auto {
            return;
        }

        let hwdec = self
            .mpv
            .get_property::<String>("hwdec-current")
            .unwrap_or_default();
        let pix_fmt = self
            .mpv
            .get_property::<String>("video-params/pixelformat")
            .unwrap_or_default();

        if !hwdec.contains("nvdec") {
            info!("auto vf: no filter needed (hwdec={})", hwdec);
            return;
        }

        // Apply the real stride fix (yuv420p/yuv420p10le, by bit depth) for any nvdec mode:
        // nv12/p010 would be NVDEC's own output format again, i.e. no fix at all.
        let is_high_bit = pix_fmt.contains("p010")
            || pix_fmt.contains("10le")
            || pix_fmt.contains("10be")
            || pix_fmt.contains("16");

        let fmt = if is_high_bit {
            "format=yuv420p10le"
        } else {
            "format=yuv420p"
        };

        match self.mpv.command("vf", &["set", fmt]) {
            Ok(_) => info!(
                "auto vf: applied {} (hwdec={}, input={})",
                fmt, hwdec, pix_fmt
            ),
            Err(e) => warn!("auto vf: failed to apply {}: {:#}", fmt, e),
        }
    }

    pub fn toggle_pause(&self) {
        let paused: bool = self.mpv.get_property("pause").unwrap_or(false);
        if paused {
            self.mpv.unpause().ok();
        } else {
            self.mpv.pause().ok();
        }
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
    pub fn seek_forward(&self, secs: f64) {
        self.mpv.seek_forward(secs).ok();
    }
    pub fn seek_backward(&self, secs: f64) {
        self.mpv.seek_backward(secs).ok();
    }
    pub fn stop(&self) {
        self.mpv.command("quit", &[]).ok();
    }

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

    /// Read the current volume level (0–130) — a snapshot point for a caller
    /// that needs to ramp away from and back to it (the skip-fade audio
    /// effect's own `set_volume` calls), not for display (that path already
    /// goes through `adjust_volume`'s own return value).
    pub fn get_volume(&self) -> f64 {
        self.mpv.get_property::<f64>("volume").unwrap_or(100.0)
    }

    /// Set volume to an absolute level (0–130) — distinct from
    /// `adjust_volume`'s relative `add`, since a per-tick ramp needs to set
    /// an exact interpolated value each tick rather than nudge by a delta.
    /// Has no audible effect on SPDIF passthrough audio, for the same
    /// reason `adjust_volume` is already skipped there elsewhere in this
    /// app: touching sample values on a raw compressed bitstream would
    /// corrupt the encoded frames, so mpv's own volume filter doesn't apply.
    pub fn set_volume(&self, vol: f64) {
        if let Err(e) = self.mpv.set_property("volume", vol) {
            warn!("set_volume {} failed: {}", vol, e);
        }
    }

    /// Set mute to an absolute state — distinct from `toggle_mute` (flips
    /// based on current state), since a caller driving a deterministic
    /// on/off window (the skip-fade audio effect's passthrough fallback,
    /// where volume can't be ramped at all — see `set_volume`'s own doc
    /// comment) needs the exact state it asks for regardless of whatever
    /// the user last set manually.
    pub fn set_mute(&self, muted: bool) {
        if let Err(e) = self.mpv.set_property("mute", muted) {
            warn!("set_mute {} failed: {}", muted, e);
        }
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
    /// One-line snapshot of mpv's playback state for diagnosing start-up hiccups:
    /// position, core-idle (not actually playing), seeking, paused-for-cache, A/V sync,
    /// seconds demuxed ahead, dropped frames so far.
    pub fn startup_snapshot(&self) -> String {
        let f = |p: &str| {
            self.mpv
                .get_property::<f64>(p)
                .map(|v| format!("{v:.3}"))
                .unwrap_or_else(|_| "-".into())
        };
        let b = |p: &str| {
            self.mpv
                .get_property::<bool>(p)
                .map(|v| if v { "yes" } else { "no" })
                .unwrap_or("-")
        };
        let i = |p: &str| {
            self.mpv
                .get_property::<i64>(p)
                .map(|v| v.to_string())
                .unwrap_or_else(|_| "-".into())
        };
        format!(
            "pos={} core-idle={} seeking={} paused-for-cache={} avsync={} cache-ahead={}s drops={}/{}",
            f("time-pos"),
            b("core-idle"),
            b("seeking"),
            b("paused-for-cache"),
            f("avsync"),
            f("demuxer-cache-duration"),
            i("frame-drop-count"),
            i("decoder-frame-drop-count"),
        )
    }

    pub fn get_buffering(&self) -> (bool, i32) {
        let stalled = self
            .mpv
            .get_property::<bool>("paused-for-cache")
            .unwrap_or(false);
        let pct = self
            .mpv
            .get_property::<i64>("cache-buffering-state")
            .unwrap_or(0);
        (stalled, pct as i32)
    }
    pub fn get_buffer_end_fraction(&self) -> f32 {
        let dur = self.get_duration();
        if dur <= 0.0 {
            return 0.0;
        }
        let pos = self.mpv.get_property::<f64>("time-pos").unwrap_or(0.0);
        let buf = self
            .mpv
            .get_property::<f64>("demuxer-cache-duration")
            .unwrap_or(0.0);
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
    pub fn set_sub_style(
        &self,
        scale: f64,
        pos: i64,
        respect_ass_styling: bool,
        color: &str,
        background: bool,
    ) {
        if let Err(e) = self.mpv.set_property("sub-scale", scale) {
            warn!("set_sub_style: sub-scale failed: {}", e);
        }
        if let Err(e) = self.mpv.set_property("sub-pos", pos) {
            warn!("set_sub_style: sub-pos failed: {}", e);
        }
        if !respect_ass_styling && let Err(e) = self.mpv.set_property("sub-ass-override", "force") {
            warn!("set_sub_style: sub-ass-override failed: {}", e);
        }
        if !color.is_empty()
            && let Err(e) = self.mpv.set_property("sub-color", color)
        {
            warn!("set_sub_style: sub-color failed: {}", e);
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

    /// HDR Stage 4: make mpv emit real PQ/BT.2020 values instead of its
    /// `--target-trc=auto`/`--target-prim=auto` defaults, which tone-map HDR/wide-gamut
    /// sources to gamma-2.2/BT.709. Called once per item, only after Stage 3's Wayland
    /// negotiation for THIS item is confirmed `Active` (wire_mpv_timer) — PQ values on a
    /// surface the compositor still treats as sRGB would look badly wrong.
    ///
    /// `target-peak` stays `auto`: through the render API mpv can't learn the display's
    /// peak; the compositor knows it (Stage 3 tells it the file's mastering luminance /
    /// CLL / FALL), so mpv encodes the source faithfully and the compositor adapts.
    ///
    /// Live-settable: target-trc/-prim are gl_video_conf options re-read at the start of
    /// every gl_video_render_frame() (mpv video/out/gpu/video.c), so it applies from the
    /// next frame. No revert needed — every item gets a fresh mpv core (`Player::new`).
    pub fn apply_hdr_output(&self) {
        if let Err(e) = self.mpv.set_property("target-trc", "pq") {
            warn!("apply_hdr_output: target-trc failed: {}", e);
        }
        if let Err(e) = self.mpv.set_property("target-prim", "bt.2020") {
            warn!("apply_hdr_output: target-prim failed: {}", e);
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
        self.mpv
            .get_property::<i64>("chapter-list/count")
            .unwrap_or(0)
    }

    /// Return all chapters as (start_secs, title) pairs.
    pub fn get_chapters(&self) -> Vec<(f64, String)> {
        let count = self.get_chapter_count();
        (0..count as usize)
            .map(|i| {
                let time = self
                    .mpv
                    .get_property::<f64>(&format!("chapter-list/{}/time", i))
                    .unwrap_or(0.0);
                let title = self
                    .mpv
                    .get_property::<String>(&format!("chapter-list/{}/title", i))
                    .unwrap_or_default();
                (time, title)
            })
            .collect()
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
        let count = self
            .mpv
            .get_property::<i64>("track-list/count")
            .unwrap_or(0);
        (0..count as usize)
            .map(|i| {
                let g = |k: &str| {
                    self.mpv
                        .get_property::<String>(&format!("track-list/{}/{}", i, k))
                        .unwrap_or_default()
                };
                let gi = |k: &str| {
                    self.mpv
                        .get_property::<i64>(&format!("track-list/{}/{}", i, k))
                        .unwrap_or(0)
                };
                // selected/forced/hearing-impaired are mpv FLAG properties — read as i64 they
                // fail and come back 0 (every track read as unselected, forced/HI never matched).
                let gb = |k: &str| {
                    self.mpv
                        .get_property::<bool>(&format!("track-list/{}/{}", i, k))
                        .unwrap_or(false)
                };
                TrackInfo {
                    id: gi("id"),
                    track_type: g("type"),
                    title: g("title"),
                    lang: g("lang"),
                    selected: gb("selected"),
                    codec: g("codec"),
                    external_filename: g("external-filename"),
                    forced: gb("forced"),
                    hearing_impaired: gb("hearing-impaired"),
                }
            })
            .collect()
    }
}

// ── TrackInfo ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct TrackInfo {
    pub id: i64,
    pub track_type: String,
    pub title: String,
    pub lang: String,
    pub selected: bool,
    pub codec: String,
    pub external_filename: String,
    pub forced: bool,
    pub hearing_impaired: bool,
}

// ── MpvRenderCtx ─────────────────────────────────────────────────────────────

/// Wraps `mpv_render_context` for OpenGL rendering via the mpv render API.
///
/// Drop ordering: always drop `MpvRenderCtx` **before** dropping `Player`.
/// mpv docs: `mpv_render_context_free` must be called before `mpv_terminate_destroy`.
pub struct MpvRenderCtx {
    ctx: *mut sys::mpv_render_context,
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
        handle: *mut sys::mpv_handle,
        get_proc: &dyn Fn(&CStr) -> *const c_void,
    ) -> Result<Self> {
        // C trampoline: mpv calls this to resolve OpenGL function pointers.
        // `ctx` points to the `get_proc` reference on the stack — safe because
        // `mpv_render_context_create` is synchronous (all lookups happen before
        // it returns).
        unsafe extern "C" fn gpa(
            ctx: *mut c_void,
            name: *const std::os::raw::c_char,
        ) -> *mut c_void {
            // SAFETY: `ctx` is the `get_proc` reference passed below; `name` is a C string from mpv.
            unsafe {
                let f = &*(ctx as *const &dyn Fn(&CStr) -> *const c_void);
                f(CStr::from_ptr(name)) as *mut c_void
            }
        }

        let mut init_params = sys::mpv_opengl_init_params {
            get_proc_address: Some(gpa),
            get_proc_address_ctx: &get_proc as *const _ as *mut c_void,
        };

        let api_type = b"opengl\0";
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_API_TYPE,
                data: api_type.as_ptr() as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_INIT_PARAMS,
                data: &mut init_params as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: 0,
                data: std::ptr::null_mut(),
            },
        ];

        let mut ctx: *mut sys::mpv_render_context = std::ptr::null_mut();
        // SAFETY: GL context current and `handle` valid (this fn's contract);
        // `params` and `get_proc` outlive the synchronous call.
        let rc = unsafe { sys::mpv_render_context_create(&mut ctx, handle, params.as_mut_ptr()) };
        ensure!(rc == 0, "mpv_render_context_create failed (code {})", rc);
        ensure!(!ctx.is_null(), "mpv_render_context_create returned null");

        Ok(Self {
            ctx,
            cb_data: std::ptr::null_mut(),
        })
    }

    /// Render the current video frame into the given OpenGL FBO.
    /// `flip`: `true` for OpenGL's bottom-left origin.
    /// `internal_format`: the GL internal format the FBO's texture was allocated with
    /// (e.g. `gl::RGB10_A2 as i32`), or 0 if unknown — a hint mpv uses, so pass the real
    /// one (fjord-app's create_fbo can widen the FBO).
    /// `depth`: bits per component of what the frame finally lands in
    /// (MPV_RENDER_PARAM_DEPTH — mpv dithers to it); 0 leaves it out, which mpv takes as 8.
    pub fn render(
        &self,
        fbo: i32,
        w: i32,
        h: i32,
        flip: bool,
        internal_format: i32,
        depth: i32,
    ) -> Result<()> {
        let flip_i: i32 = flip as i32;
        let mut fbo_params = sys::mpv_opengl_fbo {
            fbo,
            w,
            h,
            internal_format,
        };
        let end = sys::mpv_render_param {
            type_: 0,
            data: std::ptr::null_mut(),
        };
        let mut params = [
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_OPENGL_FBO,
                data: &mut fbo_params as *mut _ as *mut c_void,
            },
            sys::mpv_render_param {
                type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_FLIP_Y,
                data: &flip_i as *const _ as *mut c_void,
            },
            if depth > 0 {
                sys::mpv_render_param {
                    type_: sys::mpv_render_param_type_MPV_RENDER_PARAM_DEPTH,
                    data: &depth as *const _ as *mut c_void,
                }
            } else {
                end
            },
            end,
        ];
        let rc = unsafe { sys::mpv_render_context_render(self.ctx, params.as_mut_ptr()) };
        ensure!(rc == 0, "mpv_render_context_render failed (code {})", rc);
        Ok(())
    }

    /// Inform mpv that the frame has been presented (vsync feedback).
    pub fn report_swap(&self) {
        unsafe {
            sys::mpv_render_context_report_swap(self.ctx);
        }
    }

    /// Set a callback invoked by mpv (from its internal thread) when a new
    /// video frame is ready to be rendered.  The callback must not call any
    /// mpv API — use `slint::invoke_from_event_loop` to queue work.
    pub fn set_update_callback<F: Fn() + Send + 'static>(&mut self, cb: F) {
        unsafe extern "C" fn trampoline(ctx: *mut c_void) {
            // SAFETY: `ctx` is the boxed callback set below, alive until it is replaced or dropped.
            unsafe {
                if ctx.is_null() {
                    return;
                }
                let f = &*(ctx as *const Box<dyn Fn() + Send + 'static>);
                f();
            }
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

#[cfg(test)]
mod tests {
    use super::redact_api_key;

    #[test]
    fn redacts_the_token_wherever_it_sits() {
        // WebSocket URL: key in the middle, more query after it.
        assert_eq!(
            redact_api_key("ws://host/socket?api_key=abc123&deviceId=dev"),
            "ws://host/socket?api_key=REDACTED&deviceId=dev"
        );
        // Stream URL: key at the end; an error message quoting a URL.
        assert_eq!(
            redact_api_key(
                "Unable to connect to http://h/Videos/1/stream?static=true&api_key=abc123"
            ),
            "Unable to connect to http://h/Videos/1/stream?static=true&api_key=REDACTED"
        );
        assert_eq!(redact_api_key("no secrets here"), "no secrets here");
    }
}
