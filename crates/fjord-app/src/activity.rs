// ── fjord-app · activity.rs ───────────────────────────────────────────────────
//   ActivityClock            lock-free, Clone-able "last user activity" timestamp — touched by the
//                            key dispatch (keys::wire_key_dispatch), the mouse tap below and
//                            profile::wire_idle_lock_timer (while media plays); read by the idle
//                            lock
//   FjordApplicationHandler  the one CustomApplicationHandler Slint allows (registered via
//                            slint::BackendSelector before MainWindow::new()), so it carries every
//                            concern needing that hook: (1) the mouse-activity tap — sees every
//                            raw CursorMoved/MouseInput/MouseWheel before Slint's hit-testing, so
//                            mouse activity over any element counts; (2) a one-time capture of the
//                            real wl_display/wl_surface once the winit Window exists, handed to
//                            hdr::spawn_worker and video_surface::set_wayland_handles. Pure
//                            observation: always EventResult::Propagate.
// ─────────────────────────────────────────────────────────────────────────────

use slint::winit_030::winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use slint::winit_030::{CustomApplicationHandler, EventResult, winit};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// A fixed start `Instant` plus an `Arc<AtomicU64>` storing milliseconds-
/// since-start. Deliberately NOT part of `FjordState`/behind its `Mutex`:
/// the winit hook below fires on every raw `CursorMoved`, a high-frequency
/// event, and contending the whole app mutex for that on every pixel of
/// mouse movement would be wasteful and risks stalling unrelated UI-thread
/// work that also needs that lock. `Clone` is cheap (one `Arc` refcount
/// bump), so every consumer (the handler, `on_handle_key`,
/// `wire_idle_lock_timer`) just holds its own copy — never a channel, never
/// a second lock.
#[derive(Clone)]
pub(crate) struct ActivityClock {
    start: Instant,
    millis_since_start: Arc<AtomicU64>,
}

impl ActivityClock {
    /// Construct once, at the very top of `main()` — before
    /// `slint::BackendSelector::...select()` and before `MainWindow::new()`
    /// — so it can be cloned into the winit-level handler at select() time.
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            millis_since_start: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Record "activity happened right now". Called from the winit-level
    /// mouse tap (CursorMoved/MouseInput/MouseWheel), from `on_handle_key`
    /// on every keypress, and from `wire_idle_lock_timer` itself while media
    /// plays (see that function's own doc comment for why that counts too).
    pub(crate) fn touch(&self) {
        let elapsed_ms = self.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        self.millis_since_start.store(elapsed_ms, Ordering::Relaxed);
    }

    /// Elapsed time since the last `touch()` (or since construction, if
    /// `touch()` was never called at all).
    pub(crate) fn idle_for(&self) -> Duration {
        let last_ms = self.millis_since_start.load(Ordering::Relaxed);
        let now_ms = self.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
        Duration::from_millis(now_ms.saturating_sub(last_ms))
    }
}

/// See this file's own header for the full rationale. Registered exactly
/// once, from `main()`, via `BackendSelector::with_winit_custom_application_handler`.
pub(crate) struct FjordApplicationHandler {
    pub(crate) clock: ActivityClock,
    /// Guards the one-time wl_display/wl_surface capture below. A plain bool: window_event takes
    /// `&mut self` and runs only on the winit event-loop thread.
    pub(crate) hdr_handles_captured: bool,
}

impl CustomApplicationHandler for FjordApplicationHandler {
    fn window_event(
        &mut self,
        _event_loop: &winit::event_loop::ActiveEventLoop,
        _window_id: winit::window::WindowId,
        winit_window: Option<&winit::window::Window>,
        _slint_window: Option<&slint::Window>,
        event: &winit::event::WindowEvent,
    ) -> EventResult {
        use winit::event::WindowEvent;
        if matches!(
            event,
            WindowEvent::CursorMoved { .. }
                | WindowEvent::MouseInput { .. }
                | WindowEvent::MouseWheel { .. }
        ) {
            self.clock.touch();
        }

        // The real winit Window (and its wl_surface/wl_display) only exists once the event loop has
        // run past show()/run() (WinitWindowAdapter's ensure_window needs a live ActiveEventLoop) —
        // this is the first point it can be Some. Fires once per process.
        if !self.hdr_handles_captured
            && let Some(w) = winit_window
        {
            self.hdr_handles_captured = true;
            if let (Ok(dh), Ok(wh)) = (w.display_handle(), w.window_handle()) {
                if let (RawDisplayHandle::Wayland(wdh), RawWindowHandle::Wayland(wwh)) =
                    (dh.as_raw(), wh.as_raw())
                {
                    crate::hdr::spawn_worker(wdh.display, wwh.surface);
                    crate::video_surface::set_wayland_handles(wdh.display, wwh.surface);
                } else {
                    tracing::debug!(
                        "not running under Wayland — skipping HDR color-management worker"
                    );
                }
            } else {
                tracing::debug!(
                    "winit window handle unavailable on first window_event — skipping HDR color-management worker"
                );
            }
        }

        EventResult::Propagate
    }
}
