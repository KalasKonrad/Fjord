// ── fjord-app · hdr.rs ────────────────────────────────────────────────────
//   spawn_capability_diagnostic  entry point, called once from
//                                activity::FjordApplicationHandler the
//                                instant real wl_display/wl_surface handles
//                                first become available — spawns a dedicated
//                                OS thread and returns immediately, never
//                                touching the Slint/UI thread.
//   run_diagnostic               the actual unsafe FFI: attaches a SECOND,
//                                independent wayland-client connection to
//                                the same Wayland socket winit already
//                                owns (the smithay-clipboard-proven
//                                Backend::from_foreign_display pattern),
//                                binds wp_color_manager_v1 (the staging
//                                color-management-v1 protocol) if the
//                                compositor advertises it, logs every
//                                capability it reports, and separately
//                                attempts to wrap Fjord's own existing
//                                wl_surface as a typed proxy on that new
//                                connection. Pure diagnostic — never calls
//                                get_surface/creates an image description/
//                                touches playback in any way. hdr branch,
//                                Stage 1+2 (2026-09-10); Stage 3 (real
//                                negotiation) and Stage 4 (mpv-side real HDR
//                                output) are deliberately not built here —
//                                see CLAUDE.md's HDR section.
//   DiagState                    minimal Dispatch target for the one-shot
//                                registry + wp_color_manager_v1 capability
//                                roundtrip above; consumed nowhere else.
// ───────────────────────────────────────────────────────────────────────────

use std::ffi::c_void;
use std::ptr::NonNull;

use wayland_backend::client::{Backend, ObjectId};
use wayland_client::globals::{registry_queue_init, BindError, GlobalListContents};
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::color_management::v1::client::wp_color_manager_v1::{self, WpColorManagerV1};

/// Called once, from `activity::FjordApplicationHandler::window_event`, the
/// first time a real winit `Window` (and therefore real wl_display/
/// wl_surface handles) exists. Spawns a dedicated thread and returns
/// immediately — never blocks the caller, never touches Slint state.
/// Best-effort: a spawn failure is logged and otherwise harmless, matching
/// this codebase's established `slint::set_xdg_app_id`/`BackendSelector::select()`
/// non-fatal pattern.
pub(crate) fn spawn_capability_diagnostic(display: NonNull<c_void>, surface: NonNull<c_void>) {
    // Raw pointers aren't `Send`, so they can't cross the thread::spawn
    // boundary directly — carry them as plain addresses and cast back to
    // pointers on the other side. Sound for the same reason spelled out in
    // the safety comment below: both addresses stay valid for the whole
    // (well-under-a-second) lifetime of this diagnostic.
    let display_addr = display.as_ptr() as usize;
    let surface_addr = surface.as_ptr() as usize;
    if let Err(e) = std::thread::Builder::new()
        .name("fjord-hdr-diag".into())
        .spawn(move || {
            // Safety: `display_addr`/`surface_addr` come from a
            // `RawDisplayHandle::Wayland`/`RawWindowHandle::Wayland` pair
            // obtained from winit's own `Window::display_handle()`/
            // `window_handle()` the instant this thread was spawned — both
            // are guaranteed valid, non-null, live Wayland objects for at
            // least the lifetime of the window (per raw-window-handle's own
            // safety contract), and this thread runs to completion in well
            // under a second, long before the window could plausibly close.
            unsafe { run_diagnostic(display_addr as *mut c_void, surface_addr as *mut c_void) };
        })
    {
        tracing::warn!("couldn't spawn HDR capability diagnostic thread: {e}");
    }
}

/// The actual unsafe FFI. Every fallible step logs and returns early rather
/// than panicking — this thread must be structurally incapable of
/// destabilizing the app, regardless of what it discovers about the
/// compositor. Does one pass and exits; no persistent connection or retry
/// loop (compositor protocol support can't change mid-session, and a
/// one-shot diagnostic has nothing further to do — Stage 3, when built,
/// will need a genuinely persistent connection instead, since it fires
/// per-playback).
unsafe fn run_diagnostic(display_ptr: *mut c_void, surface_ptr: *mut c_void) {
    // Safety: see spawn_capability_diagnostic's own safety comment.
    let backend = unsafe { Backend::from_foreign_display(display_ptr.cast()) };
    let connection = Connection::from_backend(backend);

    let (globals, mut event_queue) = match registry_queue_init::<DiagState>(&connection) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!("hdr diagnostic: registry_queue_init failed: {e}");
            return;
        }
    };
    let qh = event_queue.handle();

    let mut state = DiagState::default();

    match globals.bind::<WpColorManagerV1, DiagState, ()>(
        &qh,
        1..=WpColorManagerV1::interface().version,
        (),
    ) {
        Ok(_manager) => {
            // The done event marks the end of the capability-advertisement
            // batch (per the protocol's own spec) — one roundtrip is enough
            // to receive the whole burst for a freshly-bound global.
            if let Err(e) = event_queue.roundtrip(&mut state) {
                tracing::warn!("hdr diagnostic: roundtrip after binding wp_color_manager_v1 failed: {e}");
            } else if state.done {
                tracing::info!(
                    "hdr diagnostic: compositor advertises wp_color_manager_v1 (color-management-v1) \
                     — intents={:?} features={:?} tf_named={:?} primaries_named={:?}",
                    state.supported_intents,
                    state.supported_features,
                    state.supported_tf_named,
                    state.supported_primaries_named,
                );
            } else {
                tracing::warn!(
                    "hdr diagnostic: bound wp_color_manager_v1 but never received its 'done' event \
                     — capability list may be incomplete"
                );
            }
        }
        Err(BindError::NotPresent) => {
            tracing::info!(
                "hdr diagnostic: compositor does not advertise wp_color_manager_v1 \
                 (staging color-management-v1) — real HDR passthrough unavailable this session"
            );
        }
        Err(e) => {
            tracing::warn!("hdr diagnostic: binding wp_color_manager_v1 failed: {e}");
        }
    }

    // Independent of whether the bind above succeeded — this exercises the
    // OTHER unverified mechanism Stage 3 will need: wrapping the surface
    // winit already created as a typed proxy on THIS (new, second)
    // connection. `ObjectId::from_ptr` cross-checks the real interface name
    // against the pointer's own `wl_proxy_get_class` before ever succeeding,
    // so this can't silently wrap the wrong kind of object.
    // Safety: `surface_ptr` is the same already-validated Wayland pointer
    // passed into this function; `WlSurface::interface()` is the correct,
    // matching interface descriptor for it.
    match unsafe { ObjectId::from_ptr(WlSurface::interface(), surface_ptr.cast()) } {
        Ok(object_id) => match Proxy::from_id(&connection, object_id) {
            Ok(surface) => {
                let surface: WlSurface = surface;
                tracing::info!(
                    "hdr diagnostic: wrapped Fjord's own wl_surface as a typed WlSurface proxy \
                     on the new connection ({:?})",
                    surface.id()
                );
            }
            Err(e) => {
                tracing::warn!("hdr diagnostic: Proxy::from_id for the existing wl_surface failed: {e}");
            }
        },
        Err(e) => {
            tracing::warn!("hdr diagnostic: ObjectId::from_ptr for the existing wl_surface failed: {e}");
        }
    }
}

/// Minimal `Dispatch` target for the one-shot registry-init + capability
/// roundtrip above. Not consumed anywhere else — Stage 3 will need a
/// genuinely different, longer-lived state shape once it exists.
#[derive(Default)]
struct DiagState {
    // `WEnum<T>` (not bare `T`) is what wayland-scanner actually generates
    // for an enum-typed event argument — `Value(T)` for a recognized wire
    // value, `Unknown(u32)` otherwise. Kept as-is (not unwrapped) rather
    // than filtered/converted: for a diagnostic log, an "Unknown(N)" entry
    // is itself useful information (a compositor advertising a value this
    // crate's own vendored protocol version doesn't yet know the name of),
    // not a case to discard.
    supported_intents: Vec<WEnum<wp_color_manager_v1::RenderIntent>>,
    supported_features: Vec<WEnum<wp_color_manager_v1::Feature>>,
    supported_tf_named: Vec<WEnum<wp_color_manager_v1::TransferFunction>>,
    supported_primaries_named: Vec<WEnum<wp_color_manager_v1::Primaries>>,
    done: bool,
}

// Required by `registry_queue_init` — this diagnostic only ever needs the
// registry's INITIAL global list (a single well-known, always-present
// global, not a dynamic multi-instance one like wl_output/wl_seat), so
// dynamic add/remove events are deliberately ignored.
impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for DiagState {
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

impl Dispatch<WpColorManagerV1, ()> for DiagState {
    fn event(
        state: &mut Self,
        _proxy: &WpColorManagerV1,
        event: wp_color_manager_v1::Event,
        _data: &(),
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
        use wp_color_manager_v1::Event;
        match event {
            Event::SupportedIntent { render_intent } => state.supported_intents.push(render_intent),
            Event::SupportedFeature { feature } => state.supported_features.push(feature),
            Event::SupportedTfNamed { tf } => state.supported_tf_named.push(tf),
            Event::SupportedPrimariesNamed { primaries } => {
                state.supported_primaries_named.push(primaries)
            }
            Event::Done => state.done = true,
            // `Event` is #[non_exhaustive] — a future protocol version could
            // add more events; nothing here needs them.
            _ => {}
        }
    }
}
