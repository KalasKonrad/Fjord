// ── fjord-app · dmabuf_plane.rs ───────────────────────────────────────────
//   The 10-bit video plane's own buffers (2026-10-08). Where the GPU driver's
//   EGL offers no 10-bit WINDOW config (NVIDIA 580 on the HTPC: only R8G8B8A8/
//   R8G8B8A0/R5G6B5A0), video_surface.rs presents the video subsurface through
//   buffers Fjord allocates itself: GBM XRGB/XBGR2101010, rendered into via an
//   EGLImage-backed GL renderbuffer, handed to KWin as linux-dmabuf wl_buffers.
//   (The 2026-10-06 probe proved each link on the HTPC: GBM allocates
//   XBGR2101010, the framebuffer is complete, KWin accepts the wl_buffer.)
//   Main/GL thread only — owned by video_surface.rs's Backplane.
//
//   Gbm / load_gbm      libgbm.so.1 via libloading (only the calls used here)
//   render_node         DRM render node behind an EGL display (EGL_EXT_device_query)
//   choose_format       pure: first 10-bit format KWin advertises AND GBM supports,
//                       with KWin's explicit modifiers for it (unit-tested)
//   Swapchain           GBM device + BUFFER_COUNT (3) buffers: allocate (old set
//                       retired until KWin releases it; the wl_buffer comes from
//                       the caller's callback), acquire, buffer, mark_busy,
//                       released, purge_retired, modifier, planes, destroy
//   Buffer              bo + one dmabuf fd per plane + EGLImage + renderbuffer +
//                       FBO + wl_buffer
//   Plane               one memory plane of a buffer (fd, offset, stride) — a
//                       modifier can need several (2026-10-08: AMD's compressed
//                       (DCC) 10-bit layouts have 2; describing only plane 0
//                       made the EGLImage import fail with EGL_BAD_MATCH)
//   import_attribs      pure: the EGLImage attribute list for all planes (unit-tested)
//   modifier_text       a modifier for the log ("implicit" for INVALID)
// ───────────────────────────────────────────────────────────────────────────

use std::ffi::{c_void, CStr};
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, OwnedFd};

use anyhow::{anyhow, bail, Context as _, Result};
use glutin_egl_sys::egl;
use glutin_egl_sys::egl::types::{EGLDisplay, EGLenum, EGLint};
use tracing::debug;
use wayland_backend::client::ObjectId;
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::Proxy;

/// DRM fourcc of the 10-bit formats worth trying, best first, with names for the log.
pub(crate) const TEN_BIT_FORMATS: [(u32, &str); 2] = [
    (u32::from_le_bytes(*b"XR30"), "XRGB2101010"),
    (u32::from_le_bytes(*b"XB30"), "XBGR2101010"),
];
pub(crate) const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;
const GBM_BO_USE_RENDERING: u32 = 1 << 2;
/// Buffers in flight: one on screen, one committed, one being drawn.
pub(crate) const BUFFER_COUNT: usize = 3;
// EGL_EXT_image_dma_buf_import(_modifiers) / EGL_EXT_device_drm(_render_node),
// values from Khronos eglext.h.
const EGL_LINUX_DMA_BUF_EXT: EGLenum = 0x3270;
const EGL_LINUX_DRM_FOURCC_EXT: EGLint = 0x3271;
/// Per plane 0–3: FD, OFFSET, PITCH, MODIFIER_LO, MODIFIER_HI.
const EGL_DMA_BUF_PLANE_ATTRS: [[EGLint; 5]; 4] = [
    [0x3272, 0x3273, 0x3274, 0x3443, 0x3444],
    [0x3275, 0x3276, 0x3277, 0x3445, 0x3446],
    [0x3278, 0x3279, 0x327A, 0x3447, 0x3448],
    [0x3440, 0x3441, 0x3442, 0x3449, 0x344A],
];
const EGL_DRM_RENDER_NODE_FILE_EXT: EGLint = 0x3377;

// ── GBM ───────────────────────────────────────────────────────────────────
pub(crate) struct Gbm {
    _lib:           &'static libloading::Library,
    create_device:  unsafe extern "C" fn(i32) -> *mut c_void,
    device_destroy: unsafe extern "C" fn(*mut c_void),
    is_supported:   unsafe extern "C" fn(*mut c_void, u32, u32) -> i32,
    bo_create:      unsafe extern "C" fn(*mut c_void, u32, u32, u32, u32) -> *mut c_void,
    bo_create_mods: unsafe extern "C" fn(*mut c_void, u32, u32, u32, *const u64, u32) -> *mut c_void,
    bo_plane_count: unsafe extern "C" fn(*mut c_void) -> i32,
    bo_plane_fd:    unsafe extern "C" fn(*mut c_void, i32) -> i32,
    bo_plane_stride: unsafe extern "C" fn(*mut c_void, i32) -> u32,
    bo_plane_offset: unsafe extern "C" fn(*mut c_void, i32) -> u32,
    bo_get_mod:     unsafe extern "C" fn(*mut c_void) -> u64,
    bo_destroy:     unsafe extern "C" fn(*mut c_void),
}

/// Loaded once per process (leaked: needed for its whole life).
pub(crate) fn load_gbm() -> Result<&'static Gbm> {
    // Safety: loading the system GBM library and looking up functions with
    // their gbm.h signatures.
    unsafe {
        let lib = libloading::Library::new("libgbm.so.1").context("loading libgbm.so.1")?;
        let lib: &'static libloading::Library = Box::leak(Box::new(lib));
        macro_rules! sym { ($n:literal) => { *lib.get(concat!($n, "\0").as_bytes()).context($n)? } }
        let gbm = Gbm {
            _lib: lib,
            create_device: sym!("gbm_create_device"),
            device_destroy: sym!("gbm_device_destroy"),
            is_supported: sym!("gbm_device_is_format_supported"),
            bo_create: sym!("gbm_bo_create"),
            bo_create_mods: sym!("gbm_bo_create_with_modifiers"),
            bo_plane_count: sym!("gbm_bo_get_plane_count"),
            bo_plane_fd: sym!("gbm_bo_get_fd_for_plane"),
            bo_plane_stride: sym!("gbm_bo_get_stride_for_plane"),
            bo_plane_offset: sym!("gbm_bo_get_offset"),
            bo_get_mod: sym!("gbm_bo_get_modifier"),
            bo_destroy: sym!("gbm_bo_destroy"),
        };
        Ok(Box::leak(Box::new(gbm)))
    }
}

/// The DRM render node of the GPU behind an EGL display (EGL_EXT_device_query).
pub(crate) fn render_node(egl: &egl::Egl, dpy: EGLDisplay) -> Option<String> {
    if !egl.QueryDisplayAttribEXT.is_loaded() || !egl.QueryDeviceStringEXT.is_loaded() {
        return None;
    }
    let mut dev: egl::types::EGLAttrib = 0;
    // Safety: EGL_EXT_device_query calls on a live display.
    unsafe {
        if egl.QueryDisplayAttribEXT(dpy, egl::DEVICE_EXT as EGLint, &mut dev) != egl::TRUE {
            return None;
        }
        for name in [EGL_DRM_RENDER_NODE_FILE_EXT, egl::DRM_DEVICE_FILE_EXT as EGLint] {
            let p = egl.QueryDeviceStringEXT(dev as egl::types::EGLDeviceEXT, name);
            if !p.is_null() {
                return Some(CStr::from_ptr(p).to_string_lossy().into_owned());
            }
        }
    }
    None
}

/// The first 10-bit format (TEN_BIT_FORMATS order) that KWin advertised and
/// GBM supports, with KWin's explicit modifiers for it (`DRM_FORMAT_MOD_INVALID`
/// left out; an empty list = KWin only takes it with an implicit modifier).
/// `kwin`: (fourcc, modifier) pairs from linux-dmabuf v3 (`None` = a bare
/// `format` event, i.e. implicit).
pub(crate) fn choose_format(
    kwin: &[(u32, Option<u64>)],
    gbm_supports: impl Fn(u32) -> bool,
) -> Option<(u32, &'static str, Vec<u64>)> {
    TEN_BIT_FORMATS.iter().find_map(|&(fourcc, name)| {
        let advertised: Vec<Option<u64>> = kwin.iter().filter(|(f, _)| *f == fourcc).map(|(_, m)| *m).collect();
        if advertised.is_empty() || !gbm_supports(fourcc) {
            return None;
        }
        let mods: Vec<u64> = advertised.iter().flatten().copied().filter(|&m| m != DRM_FORMAT_MOD_INVALID).collect();
        Some((fourcc, name, mods))
    })
}

// ── Swapchain ─────────────────────────────────────────────────────────────
type TargetRbStorage = unsafe extern "system" fn(u32, *const c_void);

/// One memory plane of a buffer, as linux-dmabuf's `add` and EGL's import
/// want it.
pub(crate) struct Plane<'a> {
    pub fd:     BorrowedFd<'a>,
    pub offset: u32,
    pub stride: u32,
}

pub(crate) struct Buffer {
    bo:       *mut c_void,
    /// One per plane (kept open for the buffer's life).
    fds:      Vec<OwnedFd>,
    image:    egl::types::EGLImageKHR,
    rb:       u32,
    pub(crate) fbo: u32,
    pub(crate) wl:  WlBuffer,
    busy:     bool,
    modifier: u64,
}

pub(crate) struct Swapchain {
    egl:        &'static egl::Egl,
    dpy:        EGLDisplay,
    gbm:        &'static Gbm,
    _node:      File,
    dev:        *mut c_void,
    pub(crate) fourcc: u32,
    pub(crate) format_name: &'static str,
    modifiers:  Vec<u64>,
    target_rb:  TargetRbStorage,
    buffers:    Vec<Buffer>,
    /// Buffers of an earlier size, kept until KWin releases them.
    retired:    Vec<Buffer>,
    pub(crate) size: (u32, u32),
}

impl Swapchain {
    /// GBM device on Slint's GPU + the format to use. No buffers yet.
    pub(crate) fn new(egl: &'static egl::Egl, dpy: EGLDisplay, kwin: &[(u32, Option<u64>)]) -> Result<Self> {
        if !egl.CreateImageKHR.is_loaded() || !egl.DestroyImageKHR.is_loaded() {
            bail!("EGL has no EGL_KHR_image_base");
        }
        // Safety: symbol lookup.
        let f = unsafe { egl.GetProcAddress(c"glEGLImageTargetRenderbufferStorageOES".as_ptr()) };
        if f.is_null() {
            bail!("no glEGLImageTargetRenderbufferStorageOES");
        }
        // Safety: GL_OES_EGL_image's signature.
        let target_rb: TargetRbStorage = unsafe { std::mem::transmute(f) };
        let gbm = load_gbm()?;
        let node = render_node(egl, dpy).unwrap_or_else(|| "/dev/dri/renderD128".into());
        let file = std::fs::OpenOptions::new().read(true).write(true).open(&node)
            .with_context(|| format!("opening {node}"))?;
        // Safety: a valid fd that outlives the device (kept in `_node`).
        let dev = unsafe { (gbm.create_device)(file.as_raw_fd()) };
        if dev.is_null() {
            bail!("gbm_create_device({node}) failed");
        }
        // Safety: dev is live.
        let chosen = choose_format(kwin, |f| unsafe { (gbm.is_supported)(dev, f, GBM_BO_USE_RENDERING) } != 0);
        let Some((fourcc, format_name, modifiers)) = chosen else {
            // Safety: created above.
            unsafe { (gbm.device_destroy)(dev) };
            bail!("no 10-bit format both KWin and GBM ({node}) take");
        };
        debug!("dmabuf plane: {format_name} on {node}, {} KWin modifier(s)", modifiers.len());
        Ok(Swapchain {
            egl, dpy, gbm, _node: file, dev, fourcc, format_name, modifiers, target_rb,
            buffers: Vec::new(), retired: Vec::new(), size: (0, 0),
        })
    }

    /// A fresh set of buffers at `size`; the current set is retired until KWin
    /// releases it. `make_wl(planes, modifier)` creates the wl_buffer
    /// (linux-dmabuf, asynchronous create — it lives in video_surface.rs with
    /// the rest of the protocol). Our GL context must be current.
    pub(crate) fn allocate(
        &mut self,
        size: (u32, u32),
        mut make_wl: impl FnMut(&[Plane<'_>], u64) -> Result<WlBuffer>,
    ) -> Result<()> {
        self.retired.append(&mut self.buffers);
        self.purge_retired();
        for _ in 0..BUFFER_COUNT {
            let buf = self.new_buffer(size, &mut make_wl)?;
            self.buffers.push(buf);
        }
        self.size = size;
        Ok(())
    }

    fn new_buffer(
        &self,
        (w, h): (u32, u32),
        make_wl: &mut impl FnMut(&[Plane<'_>], u64) -> Result<WlBuffer>,
    ) -> Result<Buffer> {
        let (w, h) = (w.max(1), h.max(1));
        // Safety: dev is live; the modifier list outlives the call.
        let bo = unsafe {
            if self.modifiers.is_empty() {
                (self.gbm.bo_create)(self.dev, w, h, self.fourcc, GBM_BO_USE_RENDERING)
            } else {
                (self.gbm.bo_create_mods)(self.dev, w, h, self.fourcc, self.modifiers.as_ptr(), self.modifiers.len() as u32)
            }
        };
        if bo.is_null() {
            bail!("GBM couldn't allocate a {w}x{h} {} buffer", self.format_name);
        }
        let cleanup_bo = |e: anyhow::Error| { unsafe { (self.gbm.bo_destroy)(bo) }; e };
        // Safety: bo is live.
        let (plane_count, modifier) = unsafe { ((self.gbm.bo_plane_count)(bo), (self.gbm.bo_get_mod)(bo)) };
        // KWin advertised this format only with an implicit modifier: say so
        // (whatever GBM reports) — only advertised pairs may be sent.
        let modifier = if self.modifiers.is_empty() { DRM_FORMAT_MOD_INVALID } else { modifier };
        if !(1..=4).contains(&plane_count) {
            return Err(cleanup_bo(anyhow!("GBM buffer has {plane_count} planes")));
        }
        let (mut fds, mut offsets, mut strides) = (Vec::new(), Vec::new(), Vec::new());
        for i in 0..plane_count {
            // Safety: bo is live, i < its plane count; get_fd_for_plane
            // returns a new fd that we own.
            let (raw_fd, offset, stride) = unsafe {
                ((self.gbm.bo_plane_fd)(bo, i), (self.gbm.bo_plane_offset)(bo, i), (self.gbm.bo_plane_stride)(bo, i))
            };
            if raw_fd < 0 {
                return Err(cleanup_bo(anyhow!("gbm_bo_get_fd_for_plane({i}) failed")));
            }
            // Safety: ours, from gbm_bo_get_fd_for_plane.
            fds.push(unsafe { OwnedFd::from_raw_fd(raw_fd) });
            offsets.push(offset);
            strides.push(stride);
        }
        let raw: Vec<(i32, u32, u32)> =
            (0..fds.len()).map(|i| (fds[i].as_raw_fd(), offsets[i], strides[i])).collect();
        let attribs = import_attribs((w, h), self.fourcc, modifier, &raw);
        // Safety: EGL_EXT_image_dma_buf_import on a live display; the fds stay open.
        let image = unsafe {
            self.egl.CreateImageKHR(self.dpy, egl::NO_CONTEXT, EGL_LINUX_DMA_BUF_EXT, std::ptr::null(), attribs.as_ptr())
        };
        if image == egl::NO_IMAGE_KHR {
            return Err(cleanup_bo(anyhow!(
                "EGLImage import failed (0x{:x}; modifier {}, {} plane(s))",
                unsafe { self.egl.GetError() }, modifier_text(modifier), fds.len(),
            )));
        }
        // Safety: our context is current (caller); image is live.
        let (rb, fbo, status) = unsafe {
            let (mut rb, mut fbo) = (0u32, 0u32);
            gl::GenRenderbuffers(1, &mut rb);
            gl::BindRenderbuffer(gl::RENDERBUFFER, rb);
            (self.target_rb)(gl::RENDERBUFFER, image);
            gl::GenFramebuffers(1, &mut fbo);
            gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
            gl::FramebufferRenderbuffer(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::RENDERBUFFER, rb);
            let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER);
            gl::BindFramebuffer(gl::FRAMEBUFFER, 0);
            (rb, fbo, status)
        };
        let gl_cleanup = || unsafe {
            gl::DeleteFramebuffers(1, &fbo);
            gl::DeleteRenderbuffers(1, &rb);
            self.egl.DestroyImageKHR(self.dpy, image);
        };
        if status != gl::FRAMEBUFFER_COMPLETE {
            gl_cleanup();
            return Err(cleanup_bo(anyhow!("framebuffer on the {} buffer incomplete (0x{status:x})", self.format_name)));
        }
        let planes: Vec<Plane<'_>> = (0..fds.len())
            .map(|i| Plane { fd: fds[i].as_fd(), offset: offsets[i], stride: strides[i] })
            .collect();
        let made = make_wl(&planes, modifier);
        drop(planes);
        match made {
            Ok(wl) => Ok(Buffer { bo, fds, image, rb, fbo, wl, busy: false, modifier }),
            Err(e) => {
                gl_cleanup();
                Err(cleanup_bo(e))
            }
        }
    }

    /// A buffer KWin isn't holding, or None (the caller skips that frame).
    pub(crate) fn acquire(&self) -> Option<usize> {
        self.buffers.iter().position(|b| !b.busy)
    }

    pub(crate) fn buffer(&self, i: usize) -> &Buffer {
        &self.buffers[i]
    }

    pub(crate) fn mark_busy(&mut self, i: usize) {
        self.buffers[i].busy = true;
    }

    /// KWin sent wl_buffer.release for `id`.
    pub(crate) fn released(&mut self, id: &ObjectId) {
        for b in self.buffers.iter_mut().chain(self.retired.iter_mut()) {
            if &b.wl.id() == id {
                b.busy = false;
            }
        }
    }

    /// Frees retired buffers KWin no longer holds. Our GL context must be current.
    pub(crate) fn purge_retired(&mut self) {
        let (gone, keep): (Vec<Buffer>, Vec<Buffer>) = std::mem::take(&mut self.retired).into_iter().partition(|b| !b.busy);
        self.retired = keep;
        for b in gone {
            self.free(b);
        }
    }

    /// The modifier GBM chose for the current buffers (for the log).
    pub(crate) fn modifier(&self) -> u64 {
        self.buffers.first().map_or(DRM_FORMAT_MOD_INVALID, |b| b.modifier)
    }

    /// Memory planes per buffer (for the log).
    pub(crate) fn planes(&self) -> usize {
        self.buffers.first().map_or(0, |b| b.fds.len())
    }

    fn free(&self, b: Buffer) {
        // Safety: created by new_buffer; GL objects on our (current) context.
        unsafe {
            gl::DeleteFramebuffers(1, &b.fbo);
            gl::DeleteRenderbuffers(1, &b.rb);
            self.egl.DestroyImageKHR(self.dpy, b.image);
            b.wl.destroy();
            (self.gbm.bo_destroy)(b.bo);
        }
    }

    /// Frees everything. Our GL context should be current (GL deletes are
    /// no-ops otherwise — only leaked GPU objects, never someone else's).
    pub(crate) fn destroy(mut self) {
        let all: Vec<Buffer> = self.buffers.drain(..).chain(self.retired.drain(..)).collect();
        for b in all {
            self.free(b);
        }
        // Safety: created in new(); all its bos are destroyed.
        unsafe { (self.gbm.device_destroy)(self.dev) };
    }
}

/// The EGL_EXT_image_dma_buf_import(_modifiers) attribute list for a buffer
/// of `size` whose `planes` are (fd, offset, stride), NONE-terminated. The
/// modifier goes on every plane (it describes the whole layout) unless it's
/// DRM_FORMAT_MOD_INVALID (implicit).
pub(crate) fn import_attribs((w, h): (u32, u32), fourcc: u32, modifier: u64, planes: &[(i32, u32, u32)]) -> Vec<EGLint> {
    let mut a = vec![
        egl::WIDTH as EGLint, w as EGLint, egl::HEIGHT as EGLint, h as EGLint,
        EGL_LINUX_DRM_FOURCC_EXT, fourcc as EGLint,
    ];
    for (&(fd, offset, stride), [k_fd, k_off, k_pitch, k_lo, k_hi]) in planes.iter().zip(EGL_DMA_BUF_PLANE_ATTRS) {
        a.extend([k_fd, fd, k_off, offset as EGLint, k_pitch, stride as EGLint]);
        if modifier != DRM_FORMAT_MOD_INVALID {
            a.extend([k_lo, (modifier & 0xffff_ffff) as u32 as EGLint, k_hi, (modifier >> 32) as u32 as EGLint]);
        }
    }
    a.push(egl::NONE as EGLint);
    a
}

/// For the log: a DRM format modifier in hex.
pub(crate) fn modifier_text(m: u64) -> String {
    if m == DRM_FORMAT_MOD_INVALID { "implicit".into() } else { format!("0x{m:x}") }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XR30: u32 = u32::from_le_bytes(*b"XR30");
    const XB30: u32 = u32::from_le_bytes(*b"XB30");
    const XR24: u32 = u32::from_le_bytes(*b"XR24");

    #[test]
    fn prefers_xrgb_but_takes_what_gbm_can_do() {
        let kwin = vec![(XR30, Some(1)), (XR30, Some(2)), (XB30, Some(7)), (XB30, Some(DRM_FORMAT_MOD_INVALID)), (XR24, None)];
        // Both possible → XRGB with its modifiers.
        let (f, name, mods) = choose_format(&kwin, |_| true).unwrap();
        assert_eq!((f, name, mods), (XR30, "XRGB2101010", vec![1, 2]));
        // The HTPC case: GBM can't do XRGB2101010 → XBGR, INVALID filtered out.
        let (f, name, mods) = choose_format(&kwin, |f| f == XB30).unwrap();
        assert_eq!((f, name, mods), (XB30, "XBGR2101010", vec![7]));
        // KWin doesn't advertise it → not chosen even if GBM could.
        assert!(choose_format(&[(XR24, None)], |_| true).is_none());
        // Implicit-only advertisement → empty modifier list.
        assert_eq!(choose_format(&[(XB30, None)], |_| true).unwrap().2, Vec::<u64>::new());
    }

    #[test]
    fn import_attribs_describe_every_plane() {
        // AMD DCC case (2026-10-08): 2 planes, explicit modifier on both.
        let m: u64 = 0x0200_0000_28a6_bf04;
        let a = import_attribs((1920, 1200), XR30, m, &[(7, 0, 7680), (8, 9_437_184, 256)]);
        let lo = (m & 0xffff_ffff) as u32 as EGLint;
        let hi = (m >> 32) as u32 as EGLint;
        assert_eq!(a, vec![
            egl::WIDTH as EGLint, 1920, egl::HEIGHT as EGLint, 1200, 0x3271, XR30 as EGLint,
            0x3272, 7, 0x3273, 0, 0x3274, 7680, 0x3443, lo, 0x3444, hi,
            0x3275, 8, 0x3276, 9_437_184, 0x3277, 256, 0x3445, lo, 0x3446, hi,
            egl::NONE as EGLint,
        ]);
        // Implicit modifier, one plane: no modifier attributes at all.
        let a = import_attribs((64, 64), XB30, DRM_FORMAT_MOD_INVALID, &[(3, 0, 256)]);
        assert_eq!(a, vec![
            egl::WIDTH as EGLint, 64, egl::HEIGHT as EGLint, 64, 0x3271, XB30 as EGLint,
            0x3272, 3, 0x3273, 0, 0x3274, 256, egl::NONE as EGLint,
        ]);
    }

    #[test]
    fn modifier_text_names_implicit() {
        assert_eq!(modifier_text(DRM_FORMAT_MOD_INVALID), "implicit");
        assert_eq!(modifier_text(0x3000000004fe013), "0x3000000004fe013");
    }
}
