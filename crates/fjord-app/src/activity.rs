// ── fjord-app · activity.rs ───────────────────────────────────────────────
//   ActivityClock            lock-free, Clone-able "when did the user last
//                            do anything" timestamp — shared by the keyboard
//                            path (main.rs::on_handle_key), the winit-level
//                            mouse tap below, and profile::wire_idle_lock_timer's
//                            own idle check.
//   FjordApplicationHandler  CustomApplicationHandler impl, registered once
//                            via slint::BackendSelector at the top of
//                            main(), before MainWindow::new() — Slint only
//                            ever allows ONE such handler (confirmed
//                            directly from i-slint-backend-selector's own
//                            source: a single `Option<Box<dyn
//                            CustomApplicationHandler>>`, not composable),
//                            so this struct carries every concern that needs
//                            this one hook, not just its original one. Two
//                            responsibilities today: (1) the mouse-activity
//                            tap — observes every raw CursorMoved/MouseInput/
//                            MouseWheel winit event BEFORE Slint's own
//                            hit-testing/dispatch, so unlike the old
//                            AppState.record-activity() mechanism (fed from
//                            exactly 3 Slint TouchAreas, replaced by this
//                            same-day, event-loop branch) it sees mouse
//                            activity anywhere on the window — including
//                            over a MediaCard/FjordButton/NavItem that would
//                            otherwise swallow it; (2) hdr branch (2026-09-10)
//                            — a one-time capture of the real winit Window's
//                            raw wl_display/wl_surface handles, the instant
//                            they first become available (a real winit
//                            Window doesn't exist at all until the event
//                            loop has run at least one iteration past
//                            show()/run() — this callback is the first point
//                            that's ever true), handed off to hdr.rs for its
//                            own Wayland color-management capability
//                            diagnostic. Pure observation either way: always
//                            returns EventResult::Propagate, Slint's own
//                            dispatch is completely unaffected.
// ───────────────────────────────────────────────────────────────────────────

use slint::winit_030::winit::raw_window_handle::{
    HasDisplayHandle, HasWindowHandle, RawDisplayHandle, RawWindowHandle,
};
use slint::winit_030::{winit, CustomApplicationHandler, EventResult};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
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
        Self { start: Instant::now(), millis_since_start: Arc::new(AtomicU64::new(0)) }
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
    /// hdr branch (2026-09-10) — guards the one-time wl_display/wl_surface
    /// capture below. Plain `bool`, no `Arc`/atomics needed:
    /// `CustomApplicationHandler::window_event` takes `&mut self`
    /// (confirmed directly from i-slint-backend-winit's own trait
    /// definition) and only the winit event loop's own single thread ever
    /// calls it.
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
            WindowEvent::CursorMoved { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. }
        ) {
            self.clock.touch();
        }

        // hdr branch (2026-09-10), Stage 1 — the real winit Window (and
        // therefore a real wl_surface/wl_display) doesn't exist at all
        // until the event loop has run at least one iteration past
        // show()/run() (confirmed directly against WinitWindowAdapter's own
        // source: ensure_window() needs a live &ActiveEventLoop, which only
        // exists inside a running event-loop callback like this one) — so
        // this is the first point in the whole app where `winit_window` can
        // ever be `Some`. Fires at most once per process, regardless of how
        // many more window_event calls follow.
        if !self.hdr_handles_captured {
            if let Some(w) = winit_window {
                self.hdr_handles_captured = true;
                if let (Ok(dh), Ok(wh)) = (w.display_handle(), w.window_handle()) {
                    if let (RawDisplayHandle::Wayland(wdh), RawWindowHandle::Wayland(wwh)) =
                        (dh.as_raw(), wh.as_raw())
                    {
                        crate::hdr::spawn_capability_diagnostic(wdh.display, wwh.surface);
                    } else {
                        tracing::debug!(
                            "not running under Wayland — skipping HDR capability diagnostic"
                        );
                    }
                } else {
                    tracing::debug!(
                        "winit window handle unavailable on first window_event — skipping HDR capability diagnostic"
                    );
                }
            }
        }

        EventResult::Propagate
    }
}
