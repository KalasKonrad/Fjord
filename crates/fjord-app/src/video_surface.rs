// ── fjord-app · video_surface.rs ──────────────────────────────────────────
//   HDR Stage 5 (2026-10-04, branch hdr-subsurface) — the "video backplane":
//   one child wl_surface + wl_subsurface placed BELOW Fjord's own window
//   surface (sync mode, full window size, empty input region, opaque) with
//   its own EGL context, so video can be presented — and colour-tagged —
//   independently of Slint's sRGB UI surface. Main/GL thread only: lives in a
//   thread_local and is driven from playback.rs's BeforeRendering, where
//   Slint's own EGL context is current.
//   Created lazily when the first video starts with Settings → "Separate video
//   surface" on. Video players render through it — mpv's render context lives
//   on OUR context, each frame fills the plane with the window background and
//   draws the video into the spot Slint reports (fullscreen player / video-
//   behind-menus layer / mini-player thumbnail), while Slint's window goes
//   transparent on top (AppState.video-surface-active).
//   Two ways of presenting (Present, chosen once at setup, 2026-10-08):
//     EglWindow  an EGL window surface on the child; eglSwapBuffers commits.
//                Used where EGL offers a 10-bit window config (AMD), and as
//                the 8-bit fallback.
//     Dmabuf     Fjord's own 10-bit buffers (dmabuf_plane::Swapchain) where
//                EGL has no 10-bit window config (NVIDIA), or always with
//                Settings → "Use Fjord's own 10-bit buffers". Our context is
//                surfaceless (or on a 1×1 pbuffer); mpv renders with
//                flip_y = false (a wl_buffer's row 0 is the top), spots are
//                blitted at top-origin rects, glFinish before KWin gets the
//                buffer, then attach + damage + commit. No free buffer →
//                that frame is skipped.
//
//   set_wayland_handles  activity.rs hands over the real wl_display/wl_surface
//                        (same capture that starts hdr.rs's worker)
//   is_wayland           true once those handles are known (Settings row gate)
//   ensure_ready         lazy one-time setup + size sync; false = unavailable
//                        this session (not Wayland / setup failed / broken)
//   present_summary      how the plane presents + mpv's target depth (log line)
//   create_render_ctx    mpv render context for a player, on our context
//   Target / FrameOutcome  what render_frame's mpv call gets / how a frame went
//   render_frame         one frame: fill + video in the spot, then commit
//   free_render_ctx      frees a player's render context with our context
//                        current (mpv requires it), logs the player's frame
//                        stats, then drops a broken plane
//   mark_broken          stop using it after an error mid-item (kept alive
//                        until its render context is freed)
//   idle_fill            repaint the plain background once after a stop
//   Backplane            child surface + our EGL context + Present. create()
//                        picks the mode and logs every setup step; resize()/
//                        sync_size() follow window size/scale (Dmabuf:
//                        reallocates); drop_present()/destroy() tear down
//   OurGl                our display/context/surface: with_current() runs GL on
//                        our context and always restores Slint's
//   SavedCurrent         Slint's current EGL display/surfaces/context
//   load_egl             libEGL.so.1 via libloading, the way glutin loads it
//   rank_config          pure: preference order of EGL configs (unit-tested)
//   logical_size         pure: physical → logical size like winit (unit-tested)
//   Spot / pick_spot     pure: which video spot is showing (unit-tested)
//   to_buffer_rect       pure: logical spot → physical top-origin rect; flip_rect_y
//                        turns it into GL's bottom-left origin (unit-tested)
// ───────────────────────────────────────────────────────────────────────────

use std::cell::RefCell;
use std::ffi::{c_char, c_void, CStr, CString};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

use anyhow::{anyhow, bail, Context as _, Result};
use fjord_player::{MpvRenderCtx, Player};
use glutin_egl_sys::egl;
use glutin_egl_sys::egl::types::{EGLConfig, EGLContext, EGLDisplay, EGLSurface, EGLenum, EGLint};
use tracing::{debug, error, info, warn};
use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_region::WlRegion;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_subcompositor::WlSubcompositor;
use wayland_client::protocol::wl_subsurface::WlSubsurface;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1};
use wayland_protocols::wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::{self, ZwpLinuxDmabufV1};
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_sys::egl::{wayland_egl_option, wl_egl_window, WaylandEgl};

use crate::dmabuf_plane::{self, Swapchain};

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
/// `force_own_buffers` (Settings → "Use Fjord's own 10-bit buffers") only
/// matters on the first call — the presentation mode is fixed per run.
/// Main/GL thread only, from BeforeRendering (Slint's EGL context current).
pub(crate) fn ensure_ready(phys: (u32, u32), scale: f32, force_own_buffers: bool) -> bool {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if matches!(*slot, Slot::Untried) {
            *slot = if HANDLES.get().is_none() {
                debug!("video backplane: not running under Wayland — in-window video path");
                Slot::Failed
            } else {
                match Backplane::create(phys, scale, force_own_buffers) {
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

/// How the plane presents and the depth mpv is told, for playback.rs's
/// per-player path line (empty when it isn't set up).
pub(crate) fn present_summary() -> String {
    BACKPLANE.with(|slot| match &*slot.borrow() {
        Slot::Ready(bp) => bp.present.summary(),
        _ => String::new(),
    })
}

/// mpv's render context for `player`, created on OUR context (mpv resolves
/// its GL functions with eglGetProcAddress). It must also be freed on it —
/// free_render_ctx.
pub(crate) fn create_render_ctx(player: &Player) -> Result<MpvRenderCtx> {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Slot::Ready(bp) = &mut *slot else { bail!("video backplane isn't ready") };
        let gl = bp.gl;
        let handle = player.raw_handle_ptr();
        bp.stats = FrameStats::default();
        let get_proc = |name: &CStr| -> *const c_void {
            // Safety: plain symbol lookup.
            unsafe { gl.egl.GetProcAddress(name.as_ptr()) as *const c_void }
        };
        // Safety: our context is current inside with_current; `handle` is the
        // live player's, and the caller keeps the player alive longer.
        gl.with_current(|| unsafe { MpvRenderCtx::new(handle, &get_proc) })?
    })
}

/// Where render_frame's `render` call is to draw: mpv's FBO, size and
/// internal format, whether to flip (only for an EGL window's own
/// framebuffer and what's blitted into it), and the depth to dither to
/// (0 = leave it out — mpv assumes 8).
pub(crate) struct Target {
    pub fbo:    i32,
    pub w:      i32,
    pub h:      i32,
    pub format: i32,
    pub flip_y: bool,
    pub depth:  i32,
}

/// How one render_frame call went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameOutcome {
    /// A video frame was drawn and committed.
    Drawn,
    /// mpv's render call failed (frame committed without video).
    MpvFailed,
    /// Own buffers: none free (KWin holds all three) — nothing drawn, mpv not
    /// called; the next redraw tries again.
    Skipped,
    /// The plane isn't usable (never set up, broken) — nothing drawn.
    Unusable,
}

/// One video frame: resize to the window if needed, then — on our context —
/// let mpv render straight into the plane's buffer when `rect` covers it, or
/// into an offscreen buffer that's blitted into `rect` over a `fill`-coloured
/// background; then commit (in sync mode it lands with Slint's next commit).
/// `rect` = [x, y, w, h] in physical px, top-left origin (to_buffer_rect).
pub(crate) fn render_frame(
    phys: (u32, u32),
    scale: f32,
    rect: [i32; 4],
    fill: [f32; 3],
    render: impl FnOnce(Target) -> bool,
) -> Result<FrameOutcome> {
    BACKPLANE.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Slot::Ready(bp) = &mut *slot else { return Ok(FrameOutcome::Unusable) };
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
                if let Err(e) = bp.gl.with_current(|| drop(ctx.take())) {
                    warn!("video backplane: freeing mpv's render context without its GL context: {e:#}");
                    bp.gl.with_no_context(|| drop(ctx.take()));
                }
                let st = bp.stats;
                let own = if matches!(bp.present, Present::Dmabuf(_)) {
                    format!(
                        "; GPU wait before handing a buffer to KWin over 4 ms on {} (slowest {:.1} ms), \
                         {} frame(s) skipped (no free buffer)",
                        st.slow_gpu_wait, st.max_gpu_wait_ms, st.skipped,
                    )
                } else {
                    String::new()
                };
                info!(
                    "video backplane: this player drew {} frame(s); our overhead over 4 ms on {} (slowest {:.1} ms), \
                     slowest mpv render call {:.1} ms{own}; render context freed",
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
        match bp.fill_frame(fill) {
            Ok(true) => bp.dirty = false,
            Ok(false) => {} // no free buffer yet — the next redraw tries again
            Err(e) => {
                bp.dirty = false;
                debug!("video backplane: idle fill failed: {e:#}");
            }
        }
    })
}

// ── Backplane ─────────────────────────────────────────────────────────────
/// How the plane's pictures reach KWin (see the file header).
enum Present {
    /// Between setups — only inside create().
    Unset,
    EglWindow { window: *mut wl_egl_window, bits: [EGLint; 4] },
    Dmabuf(Swapchain),
}

impl Present {
    /// Bits per component of the plane — what mpv should dither to (0 = 8,
    /// mpv's default; not passed).
    fn depth(&self) -> i32 {
        match self {
            Present::EglWindow { bits, .. } if bits[0] == 10 => 10,
            Present::Dmabuf(_) => 10,
            _ => 0,
        }
    }

    fn summary(&self) -> String {
        match self {
            Present::Unset => "not set up".into(),
            Present::EglWindow { bits: [r, g, b, a], .. } => {
                format!("EGL window R{r}G{g}B{b}A{a}, mpv depth {}", if *r == 10 { 10 } else { 8 })
            }
            Present::Dmabuf(sc) => format!("own dmabuf buffers {}, mpv depth 10", sc.format_name),
        }
    }
}

struct Backplane {
    gl:         OurGl,
    wl_egl:     &'static WaylandEgl,
    present:    Present,
    conn:       Connection,
    queue:      EventQueue<BpState>,
    state:      BpState,
    child:      WlSurface,
    /// Kept for the process lifetime once published (see destroy()).
    subsurface: WlSubsurface,
    viewport:   WpViewport,
    /// linux-dmabuf v3, if KWin offers it (own buffers only).
    dmabuf:     Option<ZwpLinuxDmabufV1>,
    phys:       (u32, u32),
    logical:    (i32, i32),
    /// Offscreen target for a video spot smaller than the plane:
    /// (fbo, texture, w, h), RGB10_A2, on our context.
    rect_fbo:   Option<(u32, u32, i32, i32)>,
    /// Holds a video frame (set by render, cleared by idle_fill).
    dirty:      bool,
    /// Per-player frame timing, logged when the player's render context is
    /// freed: frames, and for OUR overhead (context switches + blit + swap/
    /// GPU wait, i.e. everything but mpv's own render call — which by default
    /// blocks until the frame's display time, as on the in-window path) the
    /// number over 4 ms and the slowest; mpv's slowest render call; with own
    /// buffers also the glFinish wait and skipped frames.
    stats:      FrameStats,
}

/// The backplane's event-queue state: linux-dmabuf's format list, the answer
/// to a buffer creation, and buffers KWin has released. wl_surface
/// enter/leave are ignored.
#[derive(Default)]
struct BpState {
    /// (fourcc, modifier) pairs KWin advertised (linux-dmabuf v3 events;
    /// None = a bare `format` event).
    formats:  Vec<(u32, Option<u64>)>,
    /// Answer to the last zwp_linux_buffer_params_v1.create: the new buffer,
    /// or None if KWin refused it.
    created:  Option<Option<WlBuffer>>,
    /// wl_buffer.release events not yet handed to the swapchain.
    released: Vec<ObjectId>,
}

/// Our EGL display/context/surface (window surface, pbuffer, or none when
/// surfaceless). Copy, so GL work can run while other fields are borrowed.
#[derive(Clone, Copy)]
struct OurGl {
    egl:  &'static egl::Egl,
    dpy:  EGLDisplay,
    ctx:  EGLContext,
    surf: EGLSurface,
}

impl OurGl {
    /// Runs `f` with our context current, then makes Slint's current again —
    /// always, whatever `f` did. (If that restore ever failed, Slint's own
    /// `ensure_current` re-makes its context current before its next frame.)
    fn with_current<R>(&self, f: impl FnOnce() -> R) -> Result<R> {
        // Safety: plain EGL calls on the main/GL thread; our surface/context
        // are live until destroyed, Slint's are whatever was current on entry.
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

    /// eglSwapBuffers on our window surface (EglWindow mode; our context current).
    fn swap(&self) -> Result<()> {
        // Safety: our surface is live.
        if unsafe { self.egl.SwapBuffers(self.dpy, self.surf) } == egl::TRUE {
            Ok(())
        } else {
            Err(anyhow!("eglSwapBuffers(backplane) failed: 0x{:x}", unsafe { self.egl.GetError() }))
        }
    }
}

impl Backplane {
    fn create(phys: (u32, u32), scale: f32, force_own_buffers: bool) -> Result<Self> {
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
        let (globals, queue) = registry_queue_init::<BpState>(&conn).context("registry_queue_init")?;
        let qh = queue.handle();
        let compositor: WlCompositor = globals.bind(&qh, 1..=4, ()).context("binding wl_compositor")?;
        let subcompositor: WlSubcompositor = globals.bind(&qh, 1..=1, ()).context("binding wl_subcompositor")?;
        let viewporter: WpViewporter = globals.bind(&qh, 1..=1, ()).context("binding wp_viewporter")?;
        // v3: the format/modifier list arrives as events right after binding.
        let dmabuf: Option<ZwpLinuxDmabufV1> = globals.bind(&qh, 3..=3, ()).ok();
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

        let mut bp = Backplane {
            gl: OurGl { egl, dpy, ctx: egl::NO_CONTEXT, surf: egl::NO_SURFACE },
            wl_egl, present: Present::Unset, conn, queue, state: BpState::default(), child, subsurface,
            viewport, dmabuf, phys, logical, rect_fbo: None, dirty: false, stats: FrameStats::default(),
        };

        // ── Presentation: own 10-bit buffers where EGL can't do 10-bit ────
        let renderable = if api == egl::OPENGL_ES_API { egl::OPENGL_ES3_BIT } else { egl::OPENGL_BIT };
        log_window_configs(egl, dpy, renderable);
        let window_configs = ranked_window_configs(egl, dpy, renderable);
        let ten_bit_window = window_configs.as_ref().is_ok_and(|c| c.iter().any(|(rank, ..)| *rank <= 1));
        let own_note = if ten_bit_window && !force_own_buffers {
            "own buffers not needed: EGL has a 10-bit window config".to_string()
        } else {
            match bp.start_dmabuf(renderable) {
                Ok(()) => String::new(),
                Err(e) => {
                    bp.drop_present();
                    format!("own buffers unavailable: {e:#}")
                }
            }
        };
        if matches!(bp.present, Present::Unset) {
            let started = window_configs.and_then(|configs| bp.start_egl_window(&configs));
            if let Err(e) = started {
                bp.discard();
                return Err(e.context(own_note));
            }
        }
        if let Err(e) = bp.queue.dispatch_pending(&mut bp.state) {
            debug!("video backplane: dispatch_pending after setup: {e}");
        }

        // Our context's first use (EGL window: no vsync wait in our swap —
        // Slint's own swap paces frames, and in sync mode our commit waits for
        // its commit), then one black frame so the child has a buffer of the
        // right size.
        let gl = bp.gl;
        let first = gl.with_current(|| unsafe {
            if gl.surf != egl::NO_SURFACE && matches!(bp.present, Present::EglWindow { .. }) {
                egl.SwapInterval(dpy, 0);
            }
            gl_string(gl::VERSION)
        });
        let first = first.and_then(|our_version| {
            bp.fill_frame([0.0, 0.0, 0.0])?;
            Ok(our_version)
        });
        match first {
            Ok(our_version) => {
                CHILD_ADDR.store(bp.child.id().as_ptr() as usize, Ordering::Relaxed);
                let how = match &bp.present {
                    Present::Dmabuf(sc) => format!(
                        "own dmabuf buffers {} (modifier {}, {} buffers{})",
                        sc.format_name,
                        dmabuf_plane::modifier_text(sc.modifier()),
                        dmabuf_plane::BUFFER_COUNT,
                        if gl.surf == egl::NO_SURFACE { ", surfaceless context" } else { ", pbuffer context" },
                    ),
                    Present::EglWindow { bits: [r, g, b, a], .. } => format!("EGL window R{r}G{g}B{b}A{a} ({own_note})"),
                    Present::Unset => unreachable!("set up above"),
                };
                info!(
                    "video backplane ready: {}x{} px ({}x{} logical), presenting through {how}, our GL_VERSION \"{our_version}\"",
                    phys.0, phys.1, logical.0, logical.1,
                );
                Ok(bp)
            }
            Err(e) => {
                bp.discard();
                Err(e)
            }
        }
    }

    /// EglWindow mode: the first of `configs` (best first) that gives a 3.0
    /// context of Slint's client API (glBlitFramebuffer) and a window surface
    /// on the child.
    fn start_egl_window(&mut self, configs: &[(u8, EGLConfig, [EGLint; 4])]) -> Result<()> {
        let OurGl { egl, dpy, .. } = self.gl;
        let surf_attribs = [egl::NONE as EGLint];
        for &(_, cfg, bits) in configs {
            let Some(ctx) = create_context(egl, dpy, cfg) else {
                debug!("video backplane: no 3.0 context for config {bits:?}: 0x{:x}", unsafe { egl.GetError() });
                continue;
            };
            // Safety: standard EGL/wayland-egl object creation on Slint's
            // (initialised) display and our live child surface.
            unsafe {
                let window = (self.wl_egl.wl_egl_window_create)(self.child.id().as_ptr(), self.phys.0 as i32, self.phys.1 as i32);
                if window.is_null() {
                    egl.DestroyContext(dpy, ctx);
                    bail!("wl_egl_window_create failed");
                }
                let surf = egl.CreateWindowSurface(dpy, cfg, window as *const c_void, surf_attribs.as_ptr());
                if surf == egl::NO_SURFACE {
                    debug!("video backplane: no window surface for config {bits:?}: 0x{:x}", egl.GetError());
                    (self.wl_egl.wl_egl_window_destroy)(window);
                    egl.DestroyContext(dpy, ctx);
                    continue;
                }
                self.gl.ctx = ctx;
                self.gl.surf = surf;
                self.present = Present::EglWindow { window, bits };
                return Ok(());
            }
        }
        bail!("no EGL config gave both a 3.0 context and a window surface")
    }

    /// Dmabuf mode: KWin's formats, a context without a window surface
    /// (surfaceless, else a 1×1 pbuffer), the swapchain and its first buffers.
    /// On error the caller runs drop_present().
    fn start_dmabuf(&mut self, renderable: EGLenum) -> Result<()> {
        if self.dmabuf.is_none() {
            bail!("the compositor has no linux-dmabuf v3");
        }
        self.queue.roundtrip(&mut self.state).context("linux-dmabuf roundtrip")?;
        let OurGl { egl, dpy, .. } = self.gl;
        let surfaceless = egl_string(egl, dpy, egl::EXTENSIONS)
            .split_whitespace()
            .any(|e| e == "EGL_KHR_surfaceless_context");
        let attribs = [
            // 0 = any surface type (surfaceless); else it must do pbuffers.
            egl::SURFACE_TYPE as EGLint, if surfaceless { 0 } else { egl::PBUFFER_BIT as EGLint },
            egl::RENDERABLE_TYPE as EGLint, renderable as EGLint,
            egl::COLOR_BUFFER_TYPE as EGLint, egl::RGB_BUFFER as EGLint,
            egl::RED_SIZE as EGLint, 8,
            egl::GREEN_SIZE as EGLint, 8,
            egl::BLUE_SIZE as EGLint, 8,
            egl::NONE as EGLint,
        ];
        let configs = choose_configs(egl, dpy, &attribs)?;
        let pbuffer_attribs = [egl::WIDTH as EGLint, 1, egl::HEIGHT as EGLint, 1, egl::NONE as EGLint];
        for cfg in configs {
            let Some(ctx) = create_context(egl, dpy, cfg) else { continue };
            let surf = if surfaceless {
                egl::NO_SURFACE
            } else {
                // Safety: a pbuffer-capable config on Slint's display.
                let s = unsafe { egl.CreatePbufferSurface(dpy, cfg, pbuffer_attribs.as_ptr()) };
                if s == egl::NO_SURFACE {
                    unsafe { egl.DestroyContext(dpy, ctx) };
                    continue;
                }
                s
            };
            self.gl.ctx = ctx;
            self.gl.surf = surf;
            break;
        }
        if self.gl.ctx == egl::NO_CONTEXT {
            bail!(
                "no 3.0 context {}",
                if surfaceless { "for a surfaceless context" } else { "with a pbuffer (no EGL_KHR_surfaceless_context)" }
            );
        }
        self.present = Present::Dmabuf(Swapchain::new(egl, dpy, &self.state.formats)?);
        self.allocate_buffers()
    }

    /// Dmabuf mode: a fresh set of buffers at the current size (the old set
    /// is freed once KWin has released it). Waits for KWin's answer to each
    /// buffer (a roundtrip — only at setup and on resize).
    fn allocate_buffers(&mut self) -> Result<()> {
        let Present::Dmabuf(sc) = &mut self.present else { return Ok(()) };
        let Some(dm) = self.dmabuf.as_ref() else { bail!("no linux-dmabuf") };
        let qh = self.queue.handle();
        let (queue, state) = (&mut self.queue, &mut self.state);
        let size = (self.phys.0.max(1), self.phys.1.max(1));
        let fourcc = sc.fourcc;
        let gl = self.gl;
        gl.with_current(|| {
            sc.allocate(size, |fd, stride, modifier| {
                let params = dm.create_params(&qh, ());
                params.add(fd, 0, 0, stride, (modifier >> 32) as u32, modifier as u32);
                state.created = None;
                // Asynchronous create: a refusal is an event, never a protocol
                // error on the display winit shares (create_immed's would be).
                params.create(size.0 as i32, size.1 as i32, fourcc, zwp_linux_buffer_params_v1::Flags::empty());
                for _ in 0..5 {
                    if state.created.is_some() { break; }
                    queue.roundtrip(state).context("linux-dmabuf roundtrip")?;
                }
                params.destroy();
                match state.created.take() {
                    Some(Some(buffer)) => Ok(buffer),
                    Some(None) => bail!("KWin refused the {}x{} buffer", size.0, size.1),
                    None => bail!("KWin didn't answer the buffer creation"),
                }
            })
        })??;
        debug!("video backplane: {} own buffers allocated at {}x{}", dmabuf_plane::BUFFER_COUNT, size.0, size.1);
        Ok(())
    }

    /// Dispatches our queue's events and hands buffer releases to the swapchain.
    fn dispatch(&mut self) -> Result<()> {
        if let Err(e) = self.queue.dispatch_pending(&mut self.state) {
            bail!("Wayland dispatch failed: {e}");
        }
        if !self.state.released.is_empty() {
            let ids = std::mem::take(&mut self.state.released);
            if let Present::Dmabuf(sc) = &mut self.present {
                for id in &ids {
                    sc.released(id);
                }
            }
        }
        Ok(())
    }

    /// Follows the window's physical size and scale; redraws the (black)
    /// fill whenever it changes so the child's buffer always matches.
    fn sync_size(&mut self, phys: (u32, u32), scale: f32) -> Result<()> {
        if self.resize(phys, scale)? {
            self.fill_frame([0.0, 0.0, 0.0])?;
        }
        Ok(())
    }

    /// Follows the window's physical size and scale (EGL window: takes effect
    /// with the next swap; own buffers: a new set). True if it changed.
    fn resize(&mut self, phys: (u32, u32), scale: f32) -> Result<bool> {
        self.dispatch()?;
        let logical = logical_size(phys, scale);
        if phys == self.phys && logical == self.logical {
            return Ok(false);
        }
        self.phys = phys;
        self.logical = logical;
        match &self.present {
            // Safety: the window is live until drop_present().
            Present::EglWindow { window, .. } => unsafe {
                (self.wl_egl.wl_egl_window_resize)(*window, phys.0 as i32, phys.1 as i32, 0, 0)
            },
            Present::Dmabuf(_) => self.allocate_buffers()?,
            Present::Unset => {}
        }
        self.viewport.set_destination(logical.0, logical.1);
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
        render: impl FnOnce(Target) -> bool,
    ) -> Result<FrameOutcome> {
        self.resize(phys, scale)?;
        let (w, h) = (self.phys.0 as i32, self.phys.1 as i32);
        let [rx, ry, rw, rh] = rect;
        let full = rect == [0, 0, w, h];
        let depth = self.present.depth();
        let gl = self.gl;
        let rect_fbo = &mut self.rect_fbo;
        let started = Instant::now();
        let mut mpv_ms = 0.0;
        let mut gpu_ms = None;
        let timed = |t: Target| {
            let s = Instant::now();
            let ok = render(t);
            mpv_ms = s.elapsed().as_secs_f64() * 1000.0;
            ok
        };
        let rendered = match &mut self.present {
            Present::Unset => return Ok(FrameOutcome::Unusable),
            Present::EglWindow { bits, .. } => {
                let window_format = if bits[0] == 10 { gl::RGB10_A2 as i32 } else { 0 };
                gl.with_current(|| -> Result<bool> {
                    // Safety: our context is current; every GL object used
                    // here was created on it.
                    unsafe {
                        if full {
                            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
                            let ok = timed(Target { fbo: 0, w, h, format: window_format, flip_y: true, depth });
                            gl.swap()?;
                            return Ok(ok);
                        }
                        let src = ensure_rect_fbo(rect_fbo, rw, rh)?;
                        let ok = timed(Target { fbo: src as i32, w: rw, h: rh, format: gl::RGB10_A2 as i32, flip_y: true, depth });
                        blit_into(0, src, (w, h), flip_rect_y(rect, h), fill);
                        gl.swap()?;
                        Ok(ok)
                    }
                })??
            }
            Present::Dmabuf(sc) => {
                let Some(i) = sc.acquire() else {
                    self.stats.skipped += 1;
                    debug!("video backplane: frame skipped (no free buffer)");
                    return Ok(FrameOutcome::Skipped);
                };
                let dst = sc.buffer(i).fbo;
                let ok = gl.with_current(|| -> Result<bool> {
                    sc.purge_retired();
                    // Safety: as above.
                    unsafe {
                        let ok = if full {
                            gl::BindFramebuffer(gl::FRAMEBUFFER, dst);
                            timed(Target { fbo: dst as i32, w, h, format: gl::RGB10_A2 as i32, flip_y: false, depth })
                        } else {
                            let src = ensure_rect_fbo(rect_fbo, rw, rh)?;
                            let ok = timed(Target { fbo: src as i32, w: rw, h: rh, format: gl::RGB10_A2 as i32, flip_y: false, depth });
                            // Top-origin rect: the buffer's row 0 is the top.
                            blit_into(dst, src, (w, h), [rx, ry, rw, rh], fill);
                            ok
                        };
                        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
                        // KWin must only read a finished frame.
                        let t = Instant::now();
                        gl::Finish();
                        gpu_ms = Some(t.elapsed().as_secs_f64() * 1000.0);
                        Ok(ok)
                    }
                })??;
                present_buffer(&self.child, sc, i, (w, h));
                ok
            }
        };
        let _ = self.conn.flush();
        self.dirty = true;
        let overhead = started.elapsed().as_secs_f64() * 1000.0 - mpv_ms;
        let st = &mut self.stats;
        st.frames += 1;
        if overhead > 4.0 { st.slow_overhead += 1; }
        st.max_overhead_ms = st.max_overhead_ms.max(overhead);
        st.max_mpv_ms = st.max_mpv_ms.max(mpv_ms);
        if let Some(g) = gpu_ms {
            if g > 4.0 { st.slow_gpu_wait += 1; }
            st.max_gpu_wait_ms = st.max_gpu_wait_ms.max(g);
        }
        Ok(if rendered { FrameOutcome::Drawn } else { FrameOutcome::MpvFailed })
    }

    /// Clears the whole plane to `rgb` and commits it. Ok(false) = own
    /// buffers, none free (nothing done).
    fn fill_frame(&mut self, rgb: [f32; 3]) -> Result<bool> {
        let gl = self.gl;
        let (w, h) = (self.phys.0 as i32, self.phys.1 as i32);
        // Safety (both arms): our context is current inside with_current.
        let clear = |fbo: u32| unsafe {
            gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
            gl::Disable(gl::SCISSOR_TEST);
            gl::Viewport(0, 0, w, h);
            gl::ClearColor(rgb[0], rgb[1], rgb[2], 1.0);
            gl::Clear(gl::COLOR_BUFFER_BIT);
        };
        match &mut self.present {
            Present::Unset => bail!("video backplane isn't set up"),
            Present::EglWindow { .. } => {
                gl.with_current(|| {
                    clear(0);
                    gl.swap()
                })??;
            }
            Present::Dmabuf(sc) => {
                let Some(i) = sc.acquire() else { return Ok(false) };
                let dst = sc.buffer(i).fbo;
                gl.with_current(|| {
                    clear(dst);
                    // Safety: as above.
                    unsafe {
                        gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
                        gl::Finish();
                    }
                })?;
                present_buffer(&self.child, sc, i, (w, h));
            }
        }
        let _ = self.conn.flush();
        Ok(true)
    }

    /// Frees the presentation — own buffers, window surface or pbuffer,
    /// context — leaving the Wayland objects. Our context isn't current here
    /// (with_current always switches back), so GL objects go with the context.
    fn drop_present(&mut self) {
        let gl = self.gl;
        let mut window: *mut wl_egl_window = std::ptr::null_mut();
        match std::mem::replace(&mut self.present, Present::Unset) {
            Present::Dmabuf(sc) => {
                let mut sc = Some(sc);
                let current = gl.ctx != egl::NO_CONTEXT
                    && gl.with_current(|| if let Some(sc) = sc.take() { sc.destroy() }).is_ok();
                if !current {
                    if let Some(sc) = sc.take() {
                        gl.with_no_context(|| sc.destroy());
                    }
                }
            }
            Present::EglWindow { window: w, .. } => window = w,
            Present::Unset => {}
        }
        // Safety: created by start_*; each destroyed once (fields reset below).
        unsafe {
            if gl.surf != egl::NO_SURFACE { gl.egl.DestroySurface(gl.dpy, gl.surf); }
            if gl.ctx != egl::NO_CONTEXT { gl.egl.DestroyContext(gl.dpy, gl.ctx); }
            if !window.is_null() { (self.wl_egl.wl_egl_window_destroy)(window); }
        }
        self.gl.ctx = egl::NO_CONTEXT;
        self.gl.surf = egl::NO_SURFACE;
        self.rect_fbo = None;
    }

    /// Releases everything once published (CHILD_ADDR set).
    fn destroy(mut self) {
        // Unmapped, not destroyed: hdr.rs's worker may hold a proxy of the
        // child surface (CHILD_ADDR), and using a destroyed object would be a
        // protocol error on the display winit shares. No buffer = invisible.
        self.child.attach(None, 0, 0);
        self.child.commit();
        self.drop_present();
        let _ = self.conn.flush();
        debug!("video backplane: destroyed (subsurface unmapped)");
    }

    /// A setup failure inside create(): nothing is published yet, so the
    /// Wayland objects can go too.
    fn discard(mut self) {
        self.drop_present();
        if let Some(dm) = self.dmabuf.take() { dm.destroy(); }
        self.viewport.destroy();
        self.subsurface.destroy();
        self.child.destroy();
        let _ = self.conn.flush();
    }
}

/// Own buffers: hands buffer `i` to KWin (attach + damage + commit; in sync
/// mode it lands with Slint's next commit) and marks it busy until released.
fn present_buffer(child: &WlSurface, sc: &mut Swapchain, i: usize, (w, h): (i32, i32)) {
    child.attach(Some(&sc.buffer(i).wl), 0, 0);
    if child.version() >= 4 {
        child.damage_buffer(0, 0, w, h);
    } else {
        child.damage(0, 0, i32::MAX, i32::MAX);
    }
    child.commit();
    sc.mark_busy(i);
}

/// The offscreen buffer for a spot of `rw`×`rh` (recreated when the size
/// changes). Our context must be current.
unsafe fn ensure_rect_fbo(rect_fbo: &mut Option<(u32, u32, i32, i32)>, rw: i32, rh: i32) -> Result<u32> {
    if rect_fbo.map(|f| (f.2, f.3)) != Some((rw, rh)) {
        if let Some((fbo, tex, _, _)) = rect_fbo.take() {
            crate::playback::delete_fbo(fbo, tex);
        }
        *rect_fbo = crate::playback::create_fbo(rw.max(1) as u32, rh.max(1) as u32, true)
            .map(|(fbo, tex)| (fbo, tex, rw, rh));
    }
    match rect_fbo {
        Some((fbo, ..)) => Ok(*fbo),
        None => bail!("couldn't create a {rw}x{rh} video buffer"),
    }
}

/// Clears framebuffer `dst` (`w`×`h`) to `fill` and copies `src` into `rect`
/// (in `dst`'s own coordinates). Our context must be current.
unsafe fn blit_into(dst: u32, src: u32, (w, h): (i32, i32), [x, y, rw, rh]: [i32; 4], fill: [f32; 3]) {
    gl::BindFramebuffer(gl::FRAMEBUFFER, dst);
    gl::Disable(gl::SCISSOR_TEST);
    gl::Viewport(0, 0, w, h);
    gl::ClearColor(fill[0], fill[1], fill[2], 1.0);
    gl::Clear(gl::COLOR_BUFFER_BIT);
    gl::BindFramebuffer(gl::READ_FRAMEBUFFER, src);
    gl::BlitFramebuffer(0, 0, rw, rh, x, y, x + rw, y + rh, gl::COLOR_BUFFER_BIT, gl::NEAREST);
    gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
}

/// Window configs for our client API, best first (`rank_config`), with their
/// [R, G, B, A] bits. Empty = none usable.
fn ranked_window_configs(egl: &egl::Egl, dpy: EGLDisplay, renderable: EGLenum) -> Result<Vec<(u8, EGLConfig, [EGLint; 4])>> {
    let attribs = [
        egl::SURFACE_TYPE as EGLint, egl::WINDOW_BIT as EGLint,
        egl::RENDERABLE_TYPE as EGLint, renderable as EGLint,
        egl::COLOR_BUFFER_TYPE as EGLint, egl::RGB_BUFFER as EGLint,
        egl::RED_SIZE as EGLint, 8,
        egl::GREEN_SIZE as EGLint, 8,
        egl::BLUE_SIZE as EGLint, 8,
        egl::NONE as EGLint,
    ];
    let mut ranked: Vec<(u8, EGLConfig, [EGLint; 4])> = choose_configs(egl, dpy, &attribs)?
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
    Ok(ranked)
}

fn choose_configs(egl: &egl::Egl, dpy: EGLDisplay, attribs: &[EGLint]) -> Result<Vec<EGLConfig>> {
    let mut configs: Vec<EGLConfig> = vec![std::ptr::null(); 128];
    let mut n: EGLint = 0;
    // Safety: buffers sized as declared; attribs is NONE-terminated.
    let ok = unsafe { egl.ChooseConfig(dpy, attribs.as_ptr(), configs.as_mut_ptr(), configs.len() as EGLint, &mut n) };
    if ok != egl::TRUE {
        bail!("eglChooseConfig failed: 0x{:x}", unsafe { egl.GetError() });
    }
    configs.truncate(n.max(0) as usize);
    Ok(configs)
}

/// A context of the current client API at version 3.0+ (glBlitFramebuffer).
fn create_context(egl: &egl::Egl, dpy: EGLDisplay, cfg: EGLConfig) -> Option<EGLContext> {
    let attribs = [
        egl::CONTEXT_MAJOR_VERSION as EGLint, 3,
        egl::CONTEXT_MINOR_VERSION as EGLint, 0,
        egl::NONE as EGLint,
    ];
    // Safety: standard EGL context creation on Slint's (initialised) display.
    let ctx = unsafe { egl.CreateContext(dpy, cfg, egl::NO_CONTEXT, attribs.as_ptr()) };
    (ctx != egl::NO_CONTEXT).then_some(ctx)
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
    /// Own buffers: glFinish waits over 4 ms, the slowest, frames skipped.
    slow_gpu_wait:   u64,
    max_gpu_wait_ms: f64,
    skipped:         u64,
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
/// origin) → physical px, still top-left origin (a wl_buffer's own
/// coordinates), clipped to the window. None when nothing of it is on screen.
pub(crate) fn to_buffer_rect(logical: (f32, f32, f32, f32), scale: f32, win: (i32, i32)) -> Option<[i32; 4]> {
    let s = if scale.is_finite() && scale > 0.0 { scale } else { 1.0 };
    let (x, y, w, h) = logical;
    let x0 = ((x * s).round() as i32).clamp(0, win.0);
    let y0 = ((y * s).round() as i32).clamp(0, win.1);
    let x1 = (((x + w) * s).round() as i32).clamp(0, win.0);
    let y1 = (((y + h) * s).round() as i32).clamp(0, win.1);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some([x0, y0, x1 - x0, y1 - y0])
}

/// A top-origin rect → GL window coordinates (bottom-left origin) in a
/// framebuffer `win_h` high.
fn flip_rect_y([x, y, w, h]: [i32; 4], win_h: i32) -> [i32; 4] {
    [x, win_h - y - h, w, h]
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

impl Dispatch<ZwpLinuxDmabufV1, ()> for BpState {
    fn event(state: &mut Self, _: &ZwpLinuxDmabufV1, event: zwp_linux_dmabuf_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwp_linux_dmabuf_v1::Event::Format { format } => state.formats.push((format, None)),
            zwp_linux_dmabuf_v1::Event::Modifier { format, modifier_hi, modifier_lo } => {
                state.formats.push((format, Some(((modifier_hi as u64) << 32) | modifier_lo as u64)));
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwpLinuxBufferParamsV1, ()> for BpState {
    fn event(state: &mut Self, _: &ZwpLinuxBufferParamsV1, event: zwp_linux_buffer_params_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            zwp_linux_buffer_params_v1::Event::Created { buffer } => state.created = Some(Some(buffer)),
            zwp_linux_buffer_params_v1::Event::Failed => state.created = Some(None),
            _ => {}
        }
    }

    wayland_client::event_created_child!(BpState, ZwpLinuxBufferParamsV1, [
        zwp_linux_buffer_params_v1::EVT_CREATED_OPCODE => (WlBuffer, ()),
    ]);
}

impl Dispatch<WlBuffer, ()> for BpState {
    fn event(state: &mut Self, buffer: &WlBuffer, event: wl_buffer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            state.released.push(buffer.id());
        }
    }
}

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
    fn spot_rects_scale_clip_and_flip() {
        let gl = |l, s, win: (i32, i32)| to_buffer_rect(l, s, win).map(|r| flip_rect_y(r, win.1));
        // Fullscreen at scale 2.
        assert_eq!(to_buffer_rect((0.0, 0.0, 1920.0, 1080.0), 2.0, (3840, 2160)), Some([0, 0, 3840, 2160]));
        assert_eq!(gl((0.0, 0.0, 1920.0, 1080.0), 2.0, (3840, 2160)), Some([0, 0, 3840, 2160]));
        // Mini-player thumbnail at the bottom-left of a 1920x1012 window:
        // own buffers keep Slint's top-origin y, GL counts from the bottom.
        assert_eq!(to_buffer_rect((0.0, 904.0, 192.0, 108.0), 1.0, (1920, 1012)), Some([0, 904, 192, 108]));
        assert_eq!(gl((0.0, 904.0, 192.0, 108.0), 1.0, (1920, 1012)), Some([0, 0, 192, 108]));
        // Content area above a 108px bar.
        assert_eq!(to_buffer_rect((0.0, 0.0, 1920.0, 904.0), 1.0, (1920, 1012)), Some([0, 0, 1920, 904]));
        assert_eq!(gl((0.0, 0.0, 1920.0, 904.0), 1.0, (1920, 1012)), Some([0, 108, 1920, 904]));
        // Fractional scale rounds; partly off-screen is clipped; off-screen is None.
        assert_eq!(to_buffer_rect((10.0, 10.0, 100.0, 50.0), 1.25, (1600, 900)), Some([13, 13, 125, 62]));
        assert_eq!(gl((10.0, 10.0, 100.0, 50.0), 1.25, (1600, 900)), Some([13, 825, 125, 62]));
        assert_eq!(to_buffer_rect((-10.0, 90.0, 50.0, 50.0), 1.0, (100, 100)), Some([0, 90, 40, 10]));
        assert_eq!(to_buffer_rect((0.0, 2000.0, 10.0, 10.0), 1.0, (100, 100)), None);
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
