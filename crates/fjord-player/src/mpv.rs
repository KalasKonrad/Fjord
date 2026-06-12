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
    pub deinterlace:            bool,
    pub audio_spdif:            bool,
    pub cache_size_mb:          u32,
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
            deinterlace:            false,
            audio_spdif:            false,
            cache_size_mb:          0,
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
    pub video_codec:    String,
    pub audio_codec:    String,
    pub width:          i64,
    pub height:         i64,
    pub fps:            f64,
    pub hwdec_current:  String,
    pub vsync_ratio:    f64,
    pub avsync:         f64,
    pub dropped_frames: i64,
    pub video_bitrate:  f64,
    pub audio_bitrate:  f64,
    pub cache_state:    i64,
}

// ── Player ────────────────────────────────────────────────────────────────────

pub struct Player {
    mpv: Mpv,
}

impl Player {
    /// Initialise mpv with `vo=libmpv` (render-API mode) and start loading `url`.
    pub fn new(url: &str, config: &PlayerConfig) -> Result<Self> {
        let mut mpv = Mpv::with_initializer(|init| {
            // vo=libmpv: mpv never creates its own window; all rendering goes
            // through mpv_render_context_render() called by the host.
            init.set_option("vo", "libmpv")?;
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
            if config.deinterlace { init.set_option("deinterlace", "yes")?; }
            if config.audio_spdif { init.set_option("audio-spdif", "ac3,dts,truehd")?; }
            if config.cache_size_mb > 0 {
                let secs = ((config.cache_size_mb as f64) * 0.8).max(10.0);
                init.set_option("cache-secs", format!("{:.0}", secs).as_str())?;
            }
            Ok(())
        })
        .map_err(|e| anyhow::anyhow!("mpv init failed: {}", e))?;

        mpv.event_context_mut()
            .observe_property("vsync-ratio", Format::Double, 1)
            .map_err(|e| anyhow::anyhow!("observe vsync-ratio: {}", e))?;

        mpv.playlist_load_files(&[(url, FileState::Replace, None)])
            .map_err(|e| anyhow::anyhow!("loadfile failed: {}", e))?;

        info!("mpv player started: {}", url);
        Ok(Player { mpv })
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
        StatsData {
            video_codec:    self.mpv.get_property::<String>("video-codec").unwrap_or_default(),
            audio_codec:    self.mpv.get_property::<String>("audio-codec").unwrap_or_default(),
            width:          self.mpv.get_property::<i64>("width").unwrap_or(0),
            height:         self.mpv.get_property::<i64>("height").unwrap_or(0),
            fps:            self.mpv.get_property::<f64>("estimated-vf-fps").unwrap_or(0.0),
            hwdec_current:  self.mpv.get_property::<String>("hwdec-current").unwrap_or_default(),
            vsync_ratio:    self.mpv.get_property::<f64>("vsync-ratio").unwrap_or(0.0),
            avsync:         self.mpv.get_property::<f64>("avsync").unwrap_or(0.0),
            dropped_frames: self.mpv.get_property::<i64>("frame-drop-count").unwrap_or(0),
            video_bitrate:  self.mpv.get_property::<f64>("video-bitrate").unwrap_or(0.0),
            audio_bitrate:  self.mpv.get_property::<f64>("audio-bitrate").unwrap_or(0.0),
            cache_state:    self.mpv.get_property::<i64>("cache-buffering-state").unwrap_or(0),
        }
    }

    pub fn toggle_pause(&self) {
        let paused: bool = self.mpv.get_property("pause").unwrap_or(false);
        if paused { self.mpv.unpause().ok(); } else { self.mpv.pause().ok(); }
    }
    pub fn seek_forward(&self, secs: f64)  { self.mpv.seek_forward(secs).ok(); }
    pub fn seek_backward(&self, secs: f64) { self.mpv.seek_backward(secs).ok(); }
    pub fn stop(&self)                     { self.mpv.command("quit", &[]).ok(); }
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
