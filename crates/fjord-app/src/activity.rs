// ── fjord-app · activity.rs ───────────────────────────────────────────────────
//   ActivityClock            lock-free, Clone-able "last user activity" timestamps — touch() by the
//                            key dispatch (keys::wire_key_dispatch) and the mouse tap below,
//                            touch_playback() by profile::wire_idle_lock_timer (while media plays);
//                            idle_for() for the idle lock, input_idle_for() (real input only) for
//                            the screensaver and the Now Playing auto-open (playback/timer.rs)
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
    /// The last real input only (keys, mouse) — `touch_playback` doesn't move it. The
    /// screensaver reads it: playback keeps the idle lock's clock fresh, but not this.
    input_millis_since_start: Arc<AtomicU64>,
}

impl ActivityClock {
    /// Construct once, at the very top of `main()` — before
    /// `slint::BackendSelector::...select()` and before `MainWindow::new()`
    /// — so it can be cloned into the winit-level handler at select() time.
    pub(crate) fn new() -> Self {
        Self {
            start: Instant::now(),
            millis_since_start: Arc::new(AtomicU64::new(0)),
            input_millis_since_start: Arc::new(AtomicU64::new(0)),
        }
    }

    fn now_ms(&self) -> u64 {
        self.start.elapsed().as_millis().min(u128::from(u64::MAX)) as u64
    }

    /// Record "the user did something right now". Called from the winit-level
    /// mouse tap (CursorMoved/MouseInput/MouseWheel) and from `on_handle_key`
    /// on every keypress. Moves both readings.
    pub(crate) fn touch(&self) {
        let now = self.now_ms();
        self.millis_since_start.store(now, Ordering::Relaxed);
        self.input_millis_since_start.store(now, Ordering::Relaxed);
    }

    /// Record "media is playing" for the idle lock (`wire_idle_lock_timer`, see its doc comment
    /// for why that counts as activity) — not real input, so `input_idle_for` keeps counting.
    pub(crate) fn touch_playback(&self) {
        self.millis_since_start
            .store(self.now_ms(), Ordering::Relaxed);
    }

    /// Elapsed time since the last `touch()`/`touch_playback()` (or since construction) — the
    /// idle lock's measure.
    pub(crate) fn idle_for(&self) -> Duration {
        let last_ms = self.millis_since_start.load(Ordering::Relaxed);
        Duration::from_millis(self.now_ms().saturating_sub(last_ms))
    }

    /// Elapsed time since the last real input (`touch()`) — the screensaver's measure.
    pub(crate) fn input_idle_for(&self) -> Duration {
        let last_ms = self.input_millis_since_start.load(Ordering::Relaxed);
        Duration::from_millis(self.now_ms().saturating_sub(last_ms))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_keeps_the_idle_lock_clock_fresh_but_not_the_input_clock() {
        let clock = ActivityClock::new();
        std::thread::sleep(Duration::from_millis(30));
        clock.touch_playback();
        assert!(clock.idle_for() < Duration::from_millis(20));
        assert!(clock.input_idle_for() >= Duration::from_millis(30));
        clock.touch();
        assert!(clock.input_idle_for() < Duration::from_millis(20));
    }
}
