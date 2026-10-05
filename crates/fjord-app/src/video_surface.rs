// ── fjord-app · video_surface.rs ──────────────────────────────────────────
//   HDR Stage 5 (2026-10-04, branch hdr-subsurface) — the "video backplane":
//   one child wl_surface + wl_subsurface placed BELOW Fjord's own window
//   surface (sync mode, full window size, empty input region, opaque) with
//   its own EGL context/surface, so video can be presented — and later
//   colour-tagged — independently of Slint's sRGB UI surface. Main/GL thread
//   only: lives in a thread_local and is driven from playback.rs's
//   BeforeRendering, where Slint's own EGL context is current.
//   Created lazily when the first video starts with Settings → "Separate video
//   surface" on (step 1); step 2: video players render through it — mpv's
//   render context lives on OUR context, each frame fills the window with the
//   window background and draws the video into the spot Slint reports
//   (fullscreen player / video-behind-menus layer / mini-player thumbnail),
//   while Slint's window goes transparent on top (AppState.video-surface-active).
//
//   set_wayland_handles  activity.rs hands over the real wl_display/wl_surface
//                        (same capture that starts hdr.rs's worker)
//   is_wayland           true once those handles are known (Settings row gate)
//   ensure_ready         lazy one-time setup + size sync; false = unavailable
//                        this session (not Wayland / setup failed / broken)
//   create_render_ctx    mpv render context for a player, on our context
//   render_frame         one frame: fill + video in the spot, then commit
//   free_render_ctx      frees a player's render context with our context
//                        current (mpv requires it), then drops a broken plane
//   mark_broken          stop using it after an error mid-item (kept alive
//                        until its render context is freed)
//   idle_fill            repaint the plain background once after a stop
//   Backplane            child surface + EGL objects. create() logs every
//                        setup step; resize()/sync_size() follow window size/
//                        scale; with_current() runs GL on our context and
//                        always restores Slint's; destroy() on failure
//   SavedCurrent         Slint's current EGL display/surfaces/context
//   load_egl             libEGL.so.1 via libloading, the way glutin loads it
//   rank_config          pure: preference order of EGL configs (unit-tested)
//   logical_size         pure: physical → logical size like winit (unit-tested)
//   Spot / pick_spot     pure: which video spot is showing (unit-tested)
//   to_gl_rect           pure: logical spot → physical GL rect (unit-tested)
// ───────────────────────────────────────────────────────────────────────────

use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;

use anyhow::{anyhow, bail, Context as _, Result};
use fjord_player::{MpvRenderCtx, Player};
use glutin_egl_sys::egl;
use glutin_egl_sys::egl::types::{EGLConfig, EGLContext, EGLDisplay, EGLSurface, EGLenum, EGLint};
use tracing::{debug, error, info, warn};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_sys::egl::{wayland_egl_option, wl_egl_window, WaylandEgl};

/// Raw `wl_display`/`wl_surface` addresses of Fjord's one window, set once by
/// activity.rs. Plain addresses because raw pointers aren't `Sync`; both stay
/// valid for the life of the process (the window never closes before it).
static HANDLES: OnceLock<(usize, usize)> = OnceLock::new();

/// The video subsurface's wl_surface address once it exists, for hdr.rs's
/// worker to tag (0 = none). Never cleared: the surface is never destroyed
/// once published (Backplane::destroy only unmaps it), so the worker's proxy
/// of it can't dangle.
static CHILD_ADDR: AtomicUsize = AtomicUsize::new(0);

pub(crate) fn child_surface_addr() -> Option<usize> {
    match CHILD_ADDR.load(Ordering::Relaxed) {
        0 => None,
        a => Some(a),
    }
}

pub(crate) fn set_wayland_handles(display: NonNull<c_void>, surface: NonNull<c_void>) {
    let _ = HANDLES.set((display.as_ptr() as usize, surface.as_ptr() as usize));
    debug!("video backplane: Wayland handles captured");
}

enum Slot {
    Untried,
    Ready(Box<Backplane>),
    /// Failed mid-item: no longer drawn to, but alive until the player's
    /// render context (created on our context) has been freed.
    Broken(Box<Backplane>),
    Failed,
}

pub(crate) fn is_wayland() -> bool {
    HANDLES.get().is_some()
}

thread_local! {
    static BACKPLANE: RefCell<Slot> = const { RefCell::new(Slot::Untried) };
}

/// Sets the backplane up on first call and keeps its size in step with the
/// window. Returns false when it's unavailable for the rest of the session.
/// Main/GL thread only, from BeforeRendering (Slint's EGL context current).
pub(crate) fn ensure_ready(phys: (u32, u32), scale: f32) -> bool {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if matches!(*slot, Slot::Untried) {
            *slot = if HANDLES.get().is_none() {
                debug!("video backplane: not running under Wayland — in-window video path");
                Slot::Failed
            } else {
                match Backplane::create(phys, scale) {
                    Ok(bp) => Slot::Ready(Box::new(bp)),
                    Err(e) => {
                        warn!("video backplane unavailable — in-window video path for this session: {e:#}");
                        Slot::Failed
                    }
                }
            };
        }
        let Slot::Ready(bp) = &mut *slot else { return false };
        if let Err(e) = bp.sync_size(phys, scale) {
            warn!("video backplane failed — in-window video path for the rest of this session: {e:#}");
            if let Slot::Ready(bp) = std::mem::replace(&mut *slot, Slot::Failed) {
                bp.destroy();
            }
            return false;
        }
        true
    })
}

/// mpv's render context for `player`, created on OUR context (mpv resolves
/// its GL functions with eglGetProcAddress). It must also be freed on it —
/// free_render_ctx.
pub(crate) fn create_render_ctx(player: &Player) -> Result<MpvRenderCtx> {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Slot::Ready(bp) = &mut *slot else { bail!("video backplane isn't ready") };
        let egl = bp.egl;
        let handle = player.raw_handle_ptr();
        bp.stats = FrameStats::default();
        let get_proc = |name: &CStr| -> *const c_void {
            // Safety: plain symbol lookup.
            unsafe { egl.GetProcAddress(name.as_ptr()) as *const c_void }
        };
        // Safety: our context is current inside with_current; `handle` is the
        // live player's, and the caller keeps the player alive longer.
        bp.with_current(|| unsafe { MpvRenderCtx::new(handle, &get_proc) })?
    })
}

/// One video frame: resize to the window if needed, then — on our context —
/// let mpv render straight into the window buffer when `rect` covers it, or
/// into an offscreen buffer that's blitted into `rect` over a `fill`-coloured
/// background; then commit (eglSwapBuffers; in sync mode it lands with
/// Slint's next commit). `render(fbo, w, h, internal_format)` is mpv's render
/// call. `rect` = [x, y, w, h] in physical px, GL coordinates (bottom-left).
/// Ok(false) when the plane isn't usable (never set up, broken) — nothing
/// drawn, no error to report again.
pub(crate) fn render_frame(
    phys: (u32, u32),
    scale: f32,
    rect: [i32; 4],
    fill: [f32; 3],
    render: impl FnOnce(i32, i32, i32, i32) -> bool,
) -> Result<bool> {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Slot::Ready(bp) = &mut *slot else { return Ok(false) };
        bp.render(phys, scale, rect, fill, render)
    })
}

/// Frees a render context made by create_render_ctx, with our context
/// current (mpv requires its own GL context for that). If that context can't
/// be made current, it's freed with NO context current instead — mpv's GL
/// calls then do nothing (leaking a few GPU objects) rather than deleting
/// objects in Slint's context. A broken plane is destroyed afterwards.
pub(crate) fn free_render_ctx(ctx: MpvRenderCtx) {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        match &*slot {
            Slot::Ready(bp) | Slot::Broken(bp) => {
                let mut ctx = Some(ctx);
                if let Err(e) = bp.with_current(|| drop(ctx.take())) {
                    warn!("video backplane: freeing mpv's render context without its GL context: {e:#}");
                    bp.with_no_context(|| drop(ctx.take()));
                }
                let st = bp.stats;
                info!(
                    "video backplane: this player drew {} frame(s); our overhead over 4 ms on {} (slowest {:.1} ms), \
                     slowest mpv render call {:.1} ms; render context freed",
                    st.frames, st.slow_overhead, st.max_overhead_ms, st.max_mpv_ms,
                );
            }
            _ => {
                error!("video backplane: render context outlived its backplane — freeing without a GL context");
                drop(ctx);
            }
        }
        if matches!(*slot, Slot::Broken(_)) {
            if let Slot::Broken(bp) = std::mem::replace(&mut *slot, Slot::Failed) {
                bp.destroy();
            }
        }
    })
}

/// Stop drawing to the plane after an error mid-item; the in-window path is
/// used from the next player on. Logged once.
pub(crate) fn mark_broken(why: &str) {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if let Slot::Ready(_) = &*slot {
            warn!("video backplane failed mid-item — in-window video path from the next video on: {why}");
            if let Slot::Ready(bp) = std::mem::replace(&mut *slot, Slot::Failed) {
                *slot = Slot::Broken(bp);
            }
        }
    })
}

/// After a stop: repaint the plain background once, so the next video can
/// never briefly show the previous one's last frame.
pub(crate) fn idle_fill(fill: [f32; 3]) {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Slot::Ready(bp) = &mut *slot else { return };
        if !bp.dirty { return; }
        bp.dirty = false;
        if let Err(e) = bp.fill_frame(fill) {
            debug!("video backplane: idle fill failed: {e:#}");
        }
    })
}

// ── Backplane ─────────────────────────────────────────────────────────────
struct Backplane {
    egl:        &'static egl::Egl,
    wl_egl:     &'static WaylandEgl,
    dpy:        EGLDisplay,
    ctx:        EGLContext,
    surf:       EGLSurface,
    egl_window: *mut wl_egl_window,
    conn:       Connection,
    queue:      EventQueue<BpState>,
    child:      WlSurface,
    /// Kept for the process lifetime, never destroyed (see destroy()).
    _subsurface: WlSubsurface,
    viewport:   WpViewport,
    phys:       (u32, u32),
    logical:    (i32, i32),
    /// The window buffer is 10-bit (tells mpv, so it dithers to 10 bits).
    ten_bit:    bool,
    /// Offscreen target for a video spot smaller than the window:
    /// (fbo, texture, w, h), RGB10_A2, on our context.
    rect_fbo:   Option<(u32, u32, i32, i32)>,
    /// Holds a video frame (set by render, cleared by idle_fill).
    dirty:      bool,
    /// Per-player frame timing, logged when the player's render context is
    /// freed: frames, and for OUR overhead (context switches + blit + swap,
    /// i.e. everything but mpv's own render call — which by default blocks
    /// until the frame's display time, as on the in-window path) the number
    /// over 4 ms and the slowest; plus mpv's slowest render call.
    stats:      FrameStats,
}

/// The backplane's event-queue state. None of its objects send events Fjord
/// cares about; wl_surface enter/leave are ignored.
struct BpState;

impl Backplane {
    fn create(phys: (u32, u32), scale: f32) -> Result<Self> {
        let &(display_addr, surface_addr) =
            HANDLES.get().ok_or_else(|| anyhow!("not running under Wayland"))?;
        let wl_egl = wayland_egl_option().ok_or_else(|| anyhow!("libwayland-egl.so.1 not found"))?;
        let egl = load_egl()?;

        // ── Slint's context: we must use the same display and client API ──
        // Safety: plain EGL queries on the current thread's state.
        let saved = unsafe { SavedCurrent::capture(egl) };
        if saved.ctx == egl::NO_CONTEXT || saved.dpy == egl::NO_DISPLAY {
            bail!("no EGL context is current — Slint isn't rendering through EGL");
        }
        let dpy = saved.dpy;
        // glutin re-binds its API on every make-current, so this is Slint's.
        let api = unsafe { egl.QueryAPI() };
        let api_name = match api {
            egl::OPENGL_ES_API => "OpenGL ES",
            egl::OPENGL_API => "OpenGL",
            other => bail!("Slint's EGL client API 0x{other:x} is neither OpenGL ES nor OpenGL"),
        };
        let slint_version = query_context(egl, dpy, saved.ctx, egl::CONTEXT_CLIENT_VERSION);
        let slint_alpha = query_context(egl, dpy, saved.ctx, egl::CONFIG_ID)
            .and_then(|id| config_by_id(egl, dpy, id))
            .and_then(|cfg| config_attr(egl, dpy, cfg, egl::ALPHA_SIZE));
        let egl_version = egl_string(egl, dpy, egl::VERSION);
        info!(
            "video backplane: Slint renders with {api_name} (EGL client version {slint_version:?}, \
             window alpha bits {slint_alpha:?}) — GL_VERSION \"{}\", GL_RENDERER \"{}\"; EGL {egl_version} by {}",
            gl_string(gl::VERSION),
            gl_string(gl::RENDERER),
            egl_string(egl, dpy, egl::VENDOR),
        );
        if !slint_alpha.is_some_and(|a| a > 0) {
            bail!("Slint's window has no alpha channel — it can't be made transparent over the video");
        }
        // Fjord's `gl::` pointers were loaded once from Slint's context; they
        // are valid on ours only if eglGetProcAddress results are context-
        // independent (EGL 1.5, or the get_all_proc_addresses extensions).
        let egl_15 = egl_version
            .split_whitespace()
            .next()
            .and_then(|v| v.split_once('.'))
            .and_then(|(maj, min)| Some((maj.parse::<u32>().ok()?, min.parse::<u32>().ok()?)))
            .is_some_and(|v| v >= (1, 5));
        let all_procs = egl_15
            || egl_string(egl, egl::NO_DISPLAY, egl::EXTENSIONS).contains("EGL_KHR_client_get_all_proc_addresses")
            || egl_string(egl, dpy, egl::EXTENSIONS).contains("EGL_KHR_get_all_proc_addresses");
        if !all_procs {
            bail!("EGL {egl_version} doesn't guarantee context-independent GL function pointers");
        }

        // ── Wayland: child surface below Fjord's own ──────────────────────
        // Safety: see HANDLES — a live wl_display for the process lifetime.
        let backend = unsafe { Backend::from_foreign_display(display_addr as *mut _) };
        let conn = Connection::from_backend(backend);
        let (globals, mut queue) = registry_queue_init::<BpState>(&conn).context("registry_queue_init")?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).context("binding wl_compositor")?;
        let subcompositor: WlSubcompositor = globals.bind(&qh, 1..=1, ()).context("binding wl_subcompositor")?;
        let viewporter: WpViewporter = globals.bind(&qh, 1..=1, ()).context("binding wp_viewporter")?;
        // Safety: see HANDLES. `from_ptr` checks the proxy's real interface.
        let parent_id = unsafe { ObjectId::from_ptr(WlSurface::interface(), surface_addr as *mut _) }
            .context("wrapping Fjord's wl_surface")?;
        let parent: WlSurface = Proxy::from_id(&conn, parent_id).context("wrapping Fjord's wl_surface")?;

        let child = compositor.create_surface(&qh, ());
        let subsurface = subcompositor.get_subsurface(&child, &parent, &qh, ());
        // Sync mode: the child's commits are applied together with Slint's
        // next parent commit, so video and UI always change in the same frame.
        subsurface.set_sync();
        subsurface.place_below(&parent);
        subsurface.set_position(0, 0);
        let input = compositor.create_region(&qh, ());
        child.set_input_region(Some(&input)); // empty: input goes to Slint's surface
        input.destroy();
        let opaque = compositor.create_region(&qh, ());
        opaque.add(0, 0, i32::MAX, i32::MAX);
        child.set_opaque_region(Some(&opaque));
        opaque.destroy();
        let viewport = viewporter.get_viewport(&child, &qh, ());
        let logical = logical_size(phys, scale);
        viewport.set_destination(logical.0, logical.1);

        // ── EGL: our own context + window surface on Slint's display ──────
        let egl_parts = create_egl(egl, wl_egl, dpy, api, &child, phys);
        let (ctx, surf, egl_window, bits) = match egl_parts {
            Ok(parts) => parts,
            Err(e) => {
                viewport.destroy();
                subsurface.destroy();
                child.destroy();
                let _ = conn.flush();
                return Err(e);
            }
        };
        if let Err(e) = queue.dispatch_pending(&mut BpState) {
            debug!("video backplane: dispatch_pending after setup: {e}");
        }

        let bp = Backplane {
            egl, wl_egl, dpy, ctx, surf, egl_window, conn, queue, child, _subsurface: subsurface, viewport, phys, logical,
            ten_bit: bits[0] == 10, rect_fbo: None, dirty: false, stats: FrameStats::default(),
        };
        // Our context's first use: no vsync wait in our swap (Slint's own swap
        // paces frames, and in sync mode our commit waits for its commit),
        // then one black frame so the child has a buffer of the right size.
        let first = bp.with_current(|| unsafe {
            egl.SwapInterval(dpy, 0);
            gl_string(gl::VERSION)
        });
        let first = first.and_then(|our_version| {
            bp.fill_frame([0.0, 0.0, 0.0])?;
            Ok(our_version)
        });
        match first {
            Ok(our_version) => {
                CHILD_ADDR.store(bp.child.id().as_ptr() as usize, Ordering::Relaxed);
                info!(
                    "video backplane ready: {}x{} px ({}x{} logical), config R{}G{}B{}A{}, our GL_VERSION \"{our_version}\"",
                    phys.0, phys.1, logical.0, logical.1, bits[0], bits[1], bits[2], bits[3],
                );
                Ok(bp)
            }
            Err(e) => {
                bp.destroy();
                Err(e)
            }
        }
    }

    /// Follows the window's physical size and scale; redraws the (black)
    /// fill whenever it changes so the child's buffer always matches.
    fn sync_size(&mut self, phys: (u32, u32), scale: f32) -> Result<()> {
        if self.resize(phys, scale)? {
            self.fill_frame([0.0, 0.0, 0.0])?;
        }
        Ok(())
    }

    /// Follows the window's physical size and scale (takes effect with the
    /// next swap). True if it changed.
    fn resize(&mut self, phys: (u32, u32), scale: f32) -> Result<bool> {
        if let Err(e) = self.queue.dispatch_pending(&mut BpState) {
            bail!("Wayland dispatch failed: {e}");
        }
        let logical = logical_size(phys, scale);
        if phys == self.phys && logical == self.logical {
            return Ok(false);
        }
        // Safety: egl_window is live until destroy().
        unsafe { (self.wl_egl.wl_egl_window_resize)(self.egl_window, phys.0 as i32, phys.1 as i32, 0, 0) };
        self.viewport.set_destination(logical.0, logical.1);
        self.phys = phys;
        self.logical = logical;
        debug!(
            "video backplane: resized to {}x{} px ({}x{} logical)",
            phys.0, phys.1, logical.0, logical.1
        );
        Ok(true)
    }

    fn render(
        &mut self,
        phys: (u32, u32),
        scale: f32,
        rect: [i32; 4],
        fill: [f32; 3],
        render: impl FnOnce(i32, i32, i32, i32) -> bool,
    ) -> Result<bool> {
        self.resize(phys, scale)?;
        let (w, h) = (self.phys.0 as i32, self.phys.1 as i32);
        let window_format = if self.ten_bit { gl::RGB10_A2 as i32 } else { 0 };
        let mut rect_fbo = self.rect_fbo;
        let [rx, ry, rw, rh] = rect;
        let full = rect == [0, 0, w, h];
        let started = std::time::Instant::now();
        let mut mpv_ms = 0.0;
        let render = |fbo: i32, rw: i32, rh: i32, fmt: i32| {
            let t = std::time::Instant::now();
            let ok = render(fbo, rw, rh, fmt);
            mpv_ms = t.elapsed().as_secs_f64() * 1000.0;
            ok
        };
        let out = self.with_current(|| {
            // Safety: our context is current; every GL object used here was
            // created on it.
            unsafe {
                let swap = || -> Result<()> {
                    if self.egl.SwapBuffers(self.dpy, self.surf) == egl::TRUE {
                        Ok(())
                    } else {
                        Err(anyhow!("eglSwapBuffers(backplane) failed: 0x{:x}", self.egl.GetError()))
                    }
                };
                if full {
                    gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
                    let ok = render(0, w, h, window_format);
                    swap()?;
                    return Ok(ok);
                }
                if rect_fbo.map(|f| (f.2, f.3)) != Some((rw, rh)) {
                    if let Some((fbo, tex, _, _)) = rect_fbo.take() {
                        crate::playback::delete_fbo(fbo, tex);
                    }
                    rect_fbo = crate::playback::create_fbo(rw.max(1) as u32, rh.max(1) as u32, true)
                        .map(|(fbo, tex)| (fbo, tex, rw, rh));
                }
                let Some((fbo, _, _, _)) = rect_fbo else {
                    bail!("couldn't create a {rw}x{rh} video buffer");
                };
                let ok = render(fbo as i32, rw, rh, gl::RGB10_A2 as i32);
                gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
                gl::Disable(gl::SCISSOR_TEST);
                gl::Viewport(0, 0, w, h);
                gl::ClearColor(fill[0], fill[1], fill[2], 1.0);
                gl::Clear(gl::COLOR_BUFFER_BIT);
                gl::BindFramebuffer(gl::READ_FRAMEBUFFER, fbo);
                gl::BlitFramebuffer(0, 0, rw, rh, rx, ry, rx + rw, ry + rh, gl::COLOR_BUFFER_BIT, gl::NEAREST);
                gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
                swap()?;
                Ok(ok)
            }
        });
        self.rect_fbo = rect_fbo;
        let rendered = out??;
        let _ = self.conn.flush();
        self.dirty = true;
        let overhead = started.elapsed().as_secs_f64() * 1000.0 - mpv_ms;
        let st = &mut self.stats;
        st.frames += 1;
        if overhead > 4.0 { st.slow_overhead += 1; }
        st.max_overhead_ms = st.max_overhead_ms.max(overhead);
        st.max_mpv_ms = st.max_mpv_ms.max(mpv_ms);
        Ok(rendered)
    }

    /// Clears the whole backplane to `rgb` and commits it (eglSwapBuffers).
    fn fill_frame(&self, rgb: [f32; 3]) -> Result<()> {
        let swapped = self.with_current(|| unsafe {
            gl::Viewport(0, 0, self.phys.0 as i32, self.phys.1 as i32);
            gl::ClearColor(rgb[0], rgb[1], rgb[2], 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT);
            if self.egl.SwapBuffers(self.dpy, self.surf) == egl::TRUE {
                Ok(())
            } else {
                Err(self.egl.GetError())
            }
        })?;
        swapped.map_err(|code| anyhow!("eglSwapBuffers(backplane) failed: 0x{code:x}"))?;
        let _ = self.conn.flush();
        Ok(())
    }

    /// Runs `f` with our context current, then makes Slint's current again —
    /// always, whatever `f` did. (If that restore ever failed, Slint's own
    /// `ensure_current` re-makes its context current before its next frame.)
    fn with_current<R>(&self, f: impl FnOnce() -> R) -> Result<R> {
        // Safety: plain EGL calls on the main/GL thread; our surface/context
        // are live until destroy(), Slint's are whatever was current on entry.
        unsafe {
            let saved = SavedCurrent::capture(self.egl);
            if self.egl.MakeCurrent(self.dpy, self.surf, self.surf, self.ctx) != egl::TRUE {
                bail!("eglMakeCurrent(backplane) failed: 0x{:x}", self.egl.GetError());
            }
            let out = f();
            if !saved.restore(self.egl, self.dpy) {
                error!(
                    "video backplane: couldn't make Slint's EGL context current again: 0x{:x}",
                    self.egl.GetError()
                );
                bail!("couldn't restore Slint's EGL context");
            }
            Ok(out)
        }
    }

    /// Runs `f` with NO context current, then makes Slint's current again.
    fn with_no_context(&self, f: impl FnOnce()) {
        // Safety: plain EGL calls on the main/GL thread.
        unsafe {
            let saved = SavedCurrent::capture(self.egl);
            self.egl.MakeCurrent(self.dpy, egl::NO_SURFACE, egl::NO_SURFACE, egl::NO_CONTEXT);
            f();
            if !saved.restore(self.egl, self.dpy) {
                error!("video backplane: couldn't make Slint's EGL context current again: 0x{:x}", self.egl.GetError());
            }
        }
    }

    /// Releases everything. Only called while our context is not current
    /// (with_current always switches back before returning).
    fn destroy(self) {
        // Safety: the objects were created by create() and are destroyed once.
        unsafe {
            self.egl.DestroySurface(self.dpy, self.surf);
            self.egl.DestroyContext(self.dpy, self.ctx);
            (self.wl_egl.wl_egl_window_destroy)(self.egl_window);
        }
        // Unmapped, not destroyed: hdr.rs's worker may hold a proxy of the
        // child surface (CHILD_ADDR), and using a destroyed object would be a
        // protocol error on the display winit shares. No buffer = invisible.
        self.child.attach(None, 0, 0);
        self.child.commit();
        let _ = self.conn.flush();
        debug!("video backplane: destroyed (subsurface unmapped)");
    }
}

/// Picks our EGL config (see `rank_config`) and creates a context of Slint's
/// client API at version 3.0+ (glBlitFramebuffer) plus a window surface on
/// `child`. Tries configs in preference order until one works.
fn create_egl(
    egl: &egl::Egl,
    wl_egl: &WaylandEgl,
    dpy: EGLDisplay,
    api: EGLenum,
    child: &WlSurface,
    phys: (u32, u32),
) -> Result<(EGLContext, EGLSurface, *mut wl_egl_window, [EGLint; 4])> {
    let renderable = if api == egl::OPENGL_ES_API { egl::OPENGL_ES3_BIT } else { egl::OPENGL_BIT };
    log_window_configs(egl, dpy, renderable);
    let attribs = [
        egl::SURFACE_TYPE as EGLint, egl::WINDOW_BIT as EGLint,
        egl::RENDERABLE_TYPE as EGLint, renderable as EGLint,
        egl::COLOR_BUFFER_TYPE as EGLint, egl::RGB_BUFFER as EGLint,
        egl::RED_SIZE as EGLint, 8,
        egl::GREEN_SIZE as EGLint, 8,
        egl::BLUE_SIZE as EGLint, 8,
        egl::NONE as EGLint,
    ];
    let mut configs: Vec<EGLConfig> = vec![std::ptr::null(); 128];
    let mut n: EGLint = 0;
    // Safety: buffers sized as declared.
    let ok = unsafe { egl.ChooseConfig(dpy, attribs.as_ptr(), configs.as_mut_ptr(), configs.len() as EGLint, &mut n) };
    if ok != egl::TRUE {
        bail!("eglChooseConfig failed: 0x{:x}", unsafe { egl.GetError() });
    }
    configs.truncate(n.max(0) as usize);
    let mut ranked: Vec<(u8, EGLConfig, [EGLint; 4])> = configs
        .into_iter()
        .filter_map(|cfg| {
            let bits = [egl::RED_SIZE, egl::GREEN_SIZE, egl::BLUE_SIZE, egl::ALPHA_SIZE]
                .map(|a| config_attr(egl, dpy, cfg, a).unwrap_or(-1));
            rank_config(bits).map(|rank| (rank, cfg, bits))
        })
        .collect();
    ranked.sort_by_key(|(rank, ..)| *rank); // stable: keeps EGL's own order within a rank
    if ranked.is_empty() {
        bail!("no usable EGL window config (want 10- or 8-bit RGB, renderable type 0x{renderable:x})");
    }

    let ctx_attribs = [
        egl::CONTEXT_MAJOR_VERSION as EGLint, 3,
        egl::CONTEXT_MINOR_VERSION as EGLint, 0,
        egl::NONE as EGLint,
    ];
    let surf_attribs = [egl::NONE as EGLint];
    for (_, cfg, bits) in ranked {
        // Safety: standard EGL object creation on Slint's (initialised) display.
        unsafe {
            let ctx = egl.CreateContext(dpy, cfg, egl::NO_CONTEXT, ctx_attribs.as_ptr());
            if ctx == egl::NO_CONTEXT {
                debug!("video backplane: no 3.0 context for config {bits:?}: 0x{:x}", egl.GetError());
                continue;
            }
            let window = (wl_egl.wl_egl_window_create)(child.id().as_ptr(), phys.0 as i32, phys.1 as i32);
            if window.is_null() {
                egl.DestroyContext(dpy, ctx);
                bail!("wl_egl_window_create failed");
            }
            let surf = egl.CreateWindowSurface(dpy, cfg, window as *const c_void, surf_attribs.as_ptr());
            if surf == egl::NO_SURFACE {
                debug!("video backplane: no window surface for config {bits:?}: 0x{:x}", egl.GetError());
                (wl_egl.wl_egl_window_destroy)(window);
                egl.DestroyContext(dpy, ctx);
                continue;
            }
            return Ok((ctx, surf, window, bits));
        }
    }
    bail!("no EGL config gave both a 3.0 context and a window surface")
}

// ── EGL helpers ───────────────────────────────────────────────────────────
/// One debug line listing every distinct colour format the driver offers for
/// a window with our client API — including float ones, which eglChooseConfig
/// hides by default — so a log answers "could this be 10-bit?" (the NVIDIA
/// HTPC only got R8G8B8A0, 2026-10-04).
fn log_window_configs(egl: &egl::Egl, dpy: EGLDisplay, renderable: EGLenum) {
    let mut n: EGLint = 0;
    // Safety: count query, then a buffer of exactly that size.
    if unsafe { egl.GetConfigs(dpy, std::ptr::null_mut(), 0, &mut n) } != egl::TRUE || n <= 0 {
        return;
    }
    let mut all: Vec<EGLConfig> = vec![std::ptr::null(); n as usize];
    if unsafe { egl.GetConfigs(dpy, all.as_mut_ptr(), n, &mut n) } != egl::TRUE {
        return;
    }
    all.truncate(n.max(0) as usize);
    let mut formats: Vec<String> = Vec::new();
    for cfg in all {
        let attr = |a: EGLenum| config_attr(egl, dpy, cfg, a).unwrap_or(0);
        if attr(egl::SURFACE_TYPE) & egl::WINDOW_BIT as EGLint == 0 { continue; }
        if attr(egl::RENDERABLE_TYPE) & renderable as EGLint == 0 { continue; }
        let float = attr(egl::COLOR_COMPONENT_TYPE_EXT) == egl::COLOR_COMPONENT_TYPE_FLOAT_EXT as EGLint;
        let f = format!(
            "R{}G{}B{}A{}{}",
            attr(egl::RED_SIZE), attr(egl::GREEN_SIZE), attr(egl::BLUE_SIZE), attr(egl::ALPHA_SIZE),
            if float { " float" } else { "" },
        );
        if !formats.contains(&f) { formats.push(f); }
    }
    debug!("video backplane: window colour formats offered: {}", formats.join(", "));
}

struct SavedCurrent {
    dpy:  EGLDisplay,
    draw: EGLSurface,
    read: EGLSurface,
    ctx:  EGLContext,
}

impl SavedCurrent {
    unsafe fn capture(egl: &egl::Egl) -> Self {
        Self {
            dpy:  egl.GetCurrentDisplay(),
            draw: egl.GetCurrentSurface(egl::DRAW as EGLint),
            read: egl.GetCurrentSurface(egl::READ as EGLint),
            ctx:  egl.GetCurrentContext(),
        }
    }

    /// Makes the saved context current again — or, when nothing was current
    /// (e.g. at quit, after Slint's context is gone), releases ours on `dpy`.
    unsafe fn restore(&self, egl: &egl::Egl, dpy: EGLDisplay) -> bool {
        if self.ctx == egl::NO_CONTEXT {
            return egl.MakeCurrent(dpy, egl::NO_SURFACE, egl::NO_SURFACE, egl::NO_CONTEXT) == egl::TRUE;
        }
        egl.MakeCurrent(self.dpy, self.draw, self.read, self.ctx) == egl::TRUE
    }
}

/// libEGL.so.1 — the same library glutin (Slint's GL context) already loaded,
/// so dlopen just returns the existing handle. Resolved like glutin does it:
/// exported symbol first, eglGetProcAddress for the rest. Leaked on purpose:
/// it's needed for the life of the process. Only ever called once (setup).
fn load_egl() -> Result<&'static egl::Egl> {
    type GetProcAddress = unsafe extern "C" fn(*const c_char) -> *const c_void;
    // Safety: loading the system EGL library; no init routines with side effects.
    let lib = unsafe { libloading::Library::new("libEGL.so.1") }.context("loading libEGL.so.1")?;
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));
    // Safety: eglGetProcAddress has exactly this signature.
    let get_proc: Option<GetProcAddress> =
        unsafe { lib.get::<GetProcAddress>(b"eglGetProcAddress\0") }.ok().map(|s| *s);
    let egl = egl::Egl::load_with(|name| {
        let Ok(cname) = CString::new(name) else { return std::ptr::null() };
        // Safety: symbol lookups only; the address is what the loader wants.
        if let Ok(sym) = unsafe { lib.get::<*const c_void>(cname.as_bytes_with_nul()) } {
            return *sym;
        }
        get_proc.map_or(std::ptr::null(), |f| unsafe { f(cname.as_ptr()) })
    });
    Ok(Box::leak(Box::new(egl)))
}

fn query_context(egl: &egl::Egl, dpy: EGLDisplay, ctx: EGLContext, attr: EGLenum) -> Option<EGLint> {
    let mut v: EGLint = 0;
    (unsafe { egl.QueryContext(dpy, ctx, attr as EGLint, &mut v) } == egl::TRUE).then_some(v)
}

fn config_attr(egl: &egl::Egl, dpy: EGLDisplay, cfg: EGLConfig, attr: EGLenum) -> Option<EGLint> {
    let mut v: EGLint = 0;
    (unsafe { egl.GetConfigAttrib(dpy, cfg, attr as EGLint, &mut v) } == egl::TRUE).then_some(v)
}

fn config_by_id(egl: &egl::Egl, dpy: EGLDisplay, id: EGLint) -> Option<EGLConfig> {
    let attribs = [egl::CONFIG_ID as EGLint, id, egl::NONE as EGLint];
    let mut cfg: EGLConfig = std::ptr::null();
    let mut n: EGLint = 0;
    let ok = unsafe { egl.ChooseConfig(dpy, attribs.as_ptr(), &mut cfg, 1, &mut n) };
    (ok == egl::TRUE && n == 1).then_some(cfg)
}

fn egl_string(egl: &egl::Egl, dpy: EGLDisplay, name: EGLenum) -> String {
    let p = unsafe { egl.QueryString(dpy, name as EGLint) };
    if p.is_null() {
        return String::new();
    }
    // Safety: EGL returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

/// GL string of whichever context is current.
fn gl_string(name: gl::types::GLenum) -> String {
    let p = unsafe { gl::GetString(name) };
    if p.is_null() {
        return String::new();
    }
    // Safety: GL returns a static NUL-terminated string.
    unsafe { CStr::from_ptr(p.cast()) }.to_string_lossy().into_owned()
}

// ── Pure helpers ──────────────────────────────────────────────────────────

/// Preference order for the backplane's EGL config, by [R, G, B, A] bits:
/// 10-bit without alpha (XRGB2101010) first — HDR10 precision, and the child
/// is opaque anyway — then 10-bit with 2-bit alpha, then 8-bit without/with
/// alpha. None = not usable.
fn rank_config(bits: [EGLint; 4]) -> Option<u8> {
    match bits {
        [10, 10, 10, 0] => Some(0),
        [10, 10, 10, 2] => Some(1),
        [8, 8, 8, 0] => Some(2),
        [8, 8, 8, 8] => Some(3),
        _ => None,
    }
}

/// Logical (surface-local) size for a physical buffer at `scale`. winit gets
/// physical = round(logical × scale), and for scale ≥ 1 rounding back
/// recovers the same logical size, so the child matches Slint's surface.
fn logical_size(phys: (u32, u32), scale: f32) -> (i32, i32) {
    let s = if scale.is_finite() && scale > 0.0 { scale as f64 } else { 1.0 };
    let l = |p: u32| ((p as f64) / s).round().max(1.0) as i32;
    (l(phys.0), l(phys.1))
}

#[derive(Debug, Clone, Copy, Default)]
struct FrameStats {
    frames:          u64,
    slow_overhead:   u64,
    max_overhead_ms: f64,
    max_mpv_ms:      f64,
}

/// The window background as the subsurface must fill it while it's tagged
/// PQ/BT.2020 (HDR active): sRGB → linear → BT.2020 primaries → nits with
/// SDR white at 203 (ITU-R BT.2408's reference white) → PQ. KWin maps SDR
/// windows to its own SDR brightness, so the match is close, not exact —
/// fine for Fjord's near-black background.
pub(crate) fn pq_fill_from_srgb(rgb: [f32; 3]) -> [f32; 3] {
    let lin = rgb.map(|c| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) });
    const M: [[f32; 3]; 3] = [
        [0.6274, 0.3293, 0.0433],
        [0.0691, 0.9195, 0.0114],
        [0.0164, 0.0880, 0.8956],
    ];
    let to2020 = |row: [f32; 3]| row[0] * lin[0] + row[1] * lin[1] + row[2] * lin[2];
    let pq = |y: f32| {
        let (m1, m2) = (0.159_301_76, 78.843_75);
        let (c1, c2, c3) = (0.835_937_5, 18.851_563, 18.687_5);
        let ym = (y.max(0.0) * 203.0 / 10_000.0).powf(m1);
        ((c1 + c2 * ym) / (1.0 + c3 * ym)).powf(m2)
    };
    [pq(to2020(M[0])), pq(to2020(M[1])), pq(to2020(M[2]))]
}

/// Which of Slint's video spots is showing, from the same flags main.slint
/// uses to mount them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Spot {
    /// Fullscreen PlayerScreen.
    Player,
    /// "Video behind menus" layer (content area above the bars).
    Background,
    /// The mini-player bar's 192×108 thumbnail.
    Thumb,
}

pub(crate) fn pick_spot(is_playing: bool, video_behind_ui: bool, has_background_player: bool) -> Option<Spot> {
    if is_playing {
        Some(Spot::Player)
    } else if has_background_player && video_behind_ui {
        Some(Spot::Background)
    } else if has_background_player {
        Some(Spot::Thumb)
    } else {
        None
    }
}

/// A spot's logical rect (x, y, w, h — Slint's absolute position, top-left
/// origin) → physical px in GL coordinates (bottom-left origin), clipped to
/// the window. None when nothing of it is on screen.
pub(crate) fn to_gl_rect(logical: (f32, f32, f32, f32), scale: f32, win: (i32, i32)) -> Option<[i32; 4]> {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (x, y, w, h) = logical;
    let x0 = ((x * s).round() as i32).clamp(0, win.0);
    let y0 = ((y * s).round() as i32).clamp(0, win.1);
    let x1 = (((x + w) * s).round() as i32).clamp(0, win.0);
    let y1 = (((y + h) * s).round() as i32).clamp(0, win.1);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some([x0, win.1 - y1, x1 - x0, y1 - y0])
}

// ── Dispatch ──────────────────────────────────────────────────────────────
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for BpState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(BpState: WlCompositor);
delegate_noop!(BpState: WlSubcompositor);
delegate_noop!(BpState: WlSubsurface);
delegate_noop!(BpState: WlRegion);
delegate_noop!(BpState: WpViewporter);
delegate_noop!(BpState: WpViewport);
delegate_noop!(BpState: ignore WlSurface);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_preference_order() {
        assert_eq!(rank_config([10, 10, 10, 0]), Some(0));
        assert_eq!(rank_config([10, 10, 10, 2]), Some(1));
        assert_eq!(rank_config([8, 8, 8, 0]), Some(2));
        assert_eq!(rank_config([8, 8, 8, 8]), Some(3));
        assert_eq!(rank_config([5, 6, 5, 0]), None);
        assert_eq!(rank_config([16, 16, 16, 16]), None);
    }

    #[test]
    fn pq_fill_matches_reference_values() {
        // SDR white (203 nits) is PQ ≈ 0.5807 (BT.2408); black is ~0.
        let w = pq_fill_from_srgb([1.0, 1.0, 1.0]);
        for c in w { assert!((c - 0.5807).abs() < 0.002, "{c}"); }
        let b = pq_fill_from_srgb([0.0, 0.0, 0.0]);
        for c in b { assert!(c < 0.001, "{c}"); }
        // Fjord's #0d0d0d background stays dark grey, not black or bright.
        let bg = pq_fill_from_srgb([13.0 / 255.0; 3]);
        assert!(bg[0] > 0.05 && bg[0] < 0.2, "{}", bg[0]);
    }

    #[test]
    fn spot_follows_the_mount_flags() {
        assert_eq!(pick_spot(true, true, true), Some(Spot::Player));
        assert_eq!(pick_spot(false, true, true), Some(Spot::Background));
        assert_eq!(pick_spot(false, false, true), Some(Spot::Thumb));
        assert_eq!(pick_spot(false, false, false), None);
    }

    #[test]
    fn gl_rect_flips_y_and_scales() {
        // Fullscreen at scale 2.
        assert_eq!(to_gl_rect((0.0, 0.0, 1920.0, 1080.0), 2.0, (3840, 2160)), Some([0, 0, 3840, 2160]));
        // Mini-player thumbnail at the bottom-left of a 1920x1012 window.
        assert_eq!(to_gl_rect((0.0, 904.0, 192.0, 108.0), 1.0, (1920, 1012)), Some([0, 0, 192, 108]));
        // Content area above a 108px bar: GL y starts above the bar.
        assert_eq!(to_gl_rect((0.0, 0.0, 1920.0, 904.0), 1.0, (1920, 1012)), Some([0, 108, 1920, 904]));
        // Fractional scale rounds; off-screen is None.
        assert_eq!(to_gl_rect((10.0, 10.0, 100.0, 50.0), 1.25, (1600, 900)), Some([13, 825, 125, 62]));
        assert_eq!(to_gl_rect((0.0, 2000.0, 10.0, 10.0), 1.0, (100, 100)), None);
    }

    #[test]
    fn logical_size_round_trips_winit() {
        assert_eq!(logical_size((3840, 2160), 2.0), (1920, 1080));
        assert_eq!(logical_size((1600, 900), 1.25), (1280, 720));
        // logical 1281 × 1.25 = 1601.25 → winit rounds to 1601 physical.
        assert_eq!(logical_size((1601, 901), 1.25), (1281, 721));
        assert_eq!(logical_size((1920, 1080), 1.0), (1920, 1080));
        assert_eq!(logical_size((1920, 1080), 0.0), (1920, 1080));
        assert_eq!(logical_size((1920, 1080), f32::NAN), (1920, 1080));
    }
}
