// ── fjord-app · ui_watch.rs ──────────────────────────────────────────────────
//   wire_ui_watch   debug log when the UI thread was busy: a 100 ms Timer that notices when it
//                   fires late (≥ 250 ms over), with the key mode and sidebar position — keys
//                   pressed meanwhile queue up and run all at once afterwards
//   note_frame      debug log for a frame slower than 150 ms (called by the rendering notifier)
// ─────────────────────────────────────────────────────────────────────────────
// Why: on the HTPC the first visit to Discover froze the UI ~1.4 s (three queued Downs then ran
// within 9 ms and overshot to Quit), and the log couldn't say whether drawing or work on the UI
// thread was the cause. Both are logged here, at debug level only.
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Global, Timer, TimerMode};
use tracing::debug;

use crate::{AppState, MainWindow};

const TICK: Duration = Duration::from_millis(100);
const STALL: Duration = Duration::from_millis(250);
const SLOW_FRAME: Duration = Duration::from_millis(150);

thread_local! {
    static WATCH: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

pub(crate) fn wire_ui_watch(window: &MainWindow) {
    let ww = window.as_weak();
    let last = Cell::new(Instant::now());
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, TICK, move || {
        let now = Instant::now();
        let late = now.duration_since(last.replace(now)).saturating_sub(TICK);
        if late >= STALL
            && let Some(w) = ww.upgrade()
        {
            let g = AppState::get(&w);
            debug!(
                "ui: event loop busy ~{} ms (mode {:?}, nav {})",
                late.as_millis(),
                crate::keys::active_mode(&g),
                g.get_active_nav()
            );
        }
    });
    WATCH.with(|w| *w.borrow_mut() = Some(timer));
}

/// A frame (BeforeRendering → AfterRendering, mpv's own render included) that took `took`.
pub(crate) fn note_frame(window: &MainWindow, took: Duration) {
    if took >= SLOW_FRAME {
        let g = AppState::get(window);
        debug!(
            "ui: slow frame {} ms (mode {:?}, nav {})",
            took.as_millis(),
            crate::keys::active_mode(&g),
            g.get_active_nav()
        );
    }
}
