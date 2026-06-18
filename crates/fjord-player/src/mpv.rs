// ── fjord-player · mpv.rs ────────────────────────────────────────────────────
//   PlayerConfig    hwdec, gpu-api, sync, tscale and all other mpv options
//   PollResult      enum returned by Player::poll_events
//   StatsData       snapshot of mpv property values for the stats overlay
//                   includes video_sync_mode (reads "video-sync" property back from mpv)
//   Player          libmpv2 wrapper: init, property set/get, seek, volume, tracks
//                   get_buffering: paused-for-cache + cache-buffering-state 0-100
//                   get_buffer_end_fraction: (time-pos + demuxer-cache-duration) / duration as f32
//                   is_paused: reads mpv "pause" property directly (used by pause_play_toggle to stay in sync)
//                   log_decoder_info: also logs effective video-sync after playback starts
//   TrackInfo       audio / video / subtitle track descriptor; external_filename for external subs
//   MpvRenderCtx    OpenGL render context + FBO management; drop before Player
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::{ensure, Result};
use libmpv2::{events::Event, FileState, Format, Mpv};
use std::ffi::{c_void, CStr};
use tracing::{debug, info, warn};

use libmpv2_sys as sys;

// ── PlayerConfig ──────────────────────────────────────────────────────────────

/// All user-configurable mpv settings.  `vo` is always forced to "libmpv"
/// internally; the render context takes care of GPU output.
#[derive(Clone, Debug)]
pub struct PlayerConfig {
    pub gpu_api:                String,
    pub video_sync:             String,
    pub opengl_early_flush:     bool,
    pub video_latency_hacks:    bool,
    pub interpolation:          bool,
    pub tscale:                 String,
    pub tone_mapping:           String,
    pub target_colorspace_hint: bool,
    pub hwdec:                  String,
    pub vf:                     String,
    pub deinterlace:            bool,
    pub audio_spdif:            bool,
    pub cache_size_mb:          u32,
    pub start_position_secs:    Option<f64>,
}

impl Default for PlayerConfig {
    fn default() -> Self {
        Self {
            gpu_api:                "auto".into(),
            video_sync:             "audio".into(),
            opengl_early_flush:     false,
            video_latency_hacks:    false,
            interpolation:          false,
            tscale:                 "oversample".into(),
            tone_mapping:           "auto".into(),
            target_colorspace_hint: false,
            hwdec:                  "auto".into(),
            vf:                     "".into(),
            deinterlace:            false,
            audio_spdif:            false,
            cache_size_mb:          0,
            start_position_secs:    None,
        }
    }
}

// ── PollResult ────────────────────────────────────────────────────────────────

#[derive(Debug, PartialEq, Eq)]
pub enum PollResult {
    Running,
    Finished,
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
            if config.gpu_api != "auto" && !config.gpu_api.is_empty() {
                init.set_option("gpu-api", config.gpu_api.as_str())?;
            }
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
            if config.deinterlace { init.set_option("deinterlace", "yes")?; }
            if config.audio_spdif { init.set_option("audio-spdif", "ac3,eac3,dts,dts-hd,truehd")?; }
            if config.cache_size_mb > 0 {
                let secs = ((config.cache_size_mb as f64) * 0.8).max(10.0);
                init.set_option("cache-secs", format!("{:.0}", secs).as_str())?;
            }
            if let Some(pos) = config.start_position_secs {
                if pos > 0.0 {
                    init.set_option("start", format!("{:.3}", pos).as_str())?;
                }
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
            "mpv player started: {} [hwdec={}, vf={:?}, gpu-api={}, video-sync={}, opengl-early-flush={}, video-latency-hacks={}]",
            url,
            config.hwdec,
            config.vf,
            config.gpu_api,
            config.video_sync,
            config.opengl_early_flush,
            config.video_latency_hacks,
        );
        Ok(Player { mpv, vf_auto: config.vf == "auto" })
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
                Some(Ok(Event::EndFile(reason))) => { info!("mpv: end-of-file ({:?})", reason); return PollResult::Finished; }
                Some(Ok(ev))                     => { debug!("mpv event: {:?}", ev); }
                Some(Err(e))                     => { warn!("mpv error event: {:?}", e);        return PollResult::Finished; }
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

    /// Returns all tracks from mpv's track-list property.
    pub fn get_tracks(&self) -> Vec<TrackInfo> {
        let count = self.mpv.get_property::<i64>("track-list/count").unwrap_or(0);
        (0..count as usize).filter_map(|i| {
            let g  = |k: &str| self.mpv.get_property::<String>(&format!("track-list/{}/{}", i, k)).unwrap_or_default();
            let gi = |k: &str| self.mpv.get_property::<i64>(&format!("track-list/{}/{}", i, k)).unwrap_or(0);
            Some(TrackInfo {
                id:                gi("id"),
                track_type:        g("type"),
                title:             g("title"),
                lang:              g("lang"),
                selected:          gi("selected") != 0,
                codec:             g("codec"),
                external_filename: g("external-filename"),
            })
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

        // Drop existing callback first
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
            sys::mpv_render_context_set_update_callback(self.ctx, None, std::ptr::null_mut());
            sys::mpv_render_context_free(self.ctx);
            // cb_data is now safe to free.
            if !self.cb_data.is_null() {
                drop(Box::from_raw(self.cb_data));
            }
        }
    }
}
