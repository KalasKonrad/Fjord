// ── fjord-app · ui_watch.rs ──────────────────────────────────────────────────
//   wire_ui_watch   debug log when the UI thread was busy: a 100 ms Timer that notices when it
//                   fires late (≥ 250 ms over), with the key mode and sidebar position — keys
//                   pressed meanwhile queue up and run all at once afterwards
//   note_frame      debug log for a frame slower than 150 ms (called by the rendering notifier);
//                   and for TRACE_FOR after every screen change (key mode or sidebar position):
//                   each frame that was slow-ish, came after a gap (work between frames) or drew a
//                   new Discover row (AppState.discover-rows-mounted), then a summary — to see
//                   which frame of a screen's first draw is the slow one
// ─────────────────────────────────────────────────────────────────────────────
// Why: on the HTPC the first visit to Discover froze the UI ~1.4 s (three queued Downs then ran
// within 9 ms and overshot to Quit), and the log couldn't say whether drawing or work on the UI
// thread was the cause. Both are logged here, at debug level only.
use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Global, Timer, TimerMode};
use tracing::debug;

use crate::keys::AppMode;
use crate::{AppState, MainWindow};

const TICK: Duration = Duration::from_millis(100);
const STALL: Duration = Duration::from_millis(250);
const SLOW_FRAME: Duration = Duration::from_millis(150);
/// After a screen change the frames are traced this long (Discover's first visit still had one
/// 690 ms frame after its rows were staggered, 2026-10-21 — which one?)…
const TRACE_FOR: Duration = Duration::from_secs(2);
/// …logging a frame only when it took this long, came after this long a gap, or drew a new
/// Discover row; the others count in the summary.
const TRACE_FRAME: Duration = Duration::from_millis(20);
const TRACE_GAP: Duration = Duration::from_millis(50);

struct Trace {
    screen: Option<(AppMode, i32)>,
    since: Instant,
    last_end: Option<Instant>,
    frames: u32,
    slowest: Duration,
    rows: i32,
    active: bool,
}

thread_local! {
    static WATCH: RefCell<Option<Timer>> = const { RefCell::new(None) };
    static TRACE: RefCell<Trace> = RefCell::new(Trace {
        screen: None,
        since: Instant::now(),
        last_end: None,
        frames: 0,
        slowest: Duration::ZERO,
        rows: 0,
        active: false,
    });
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
    if tracing::enabled!(tracing::Level::DEBUG) {
        trace_frame(window, took);
    }
}

fn trace_frame(window: &MainWindow, took: Duration) {
    let g = AppState::get(window);
    let now = Instant::now();
    let start = now.checked_sub(took).unwrap_or(now);
    let screen = (crate::keys::active_mode(&g), g.get_active_nav());
    let rows = g.get_discover_rows_mounted();
    TRACE.with(|t| {
        let mut t = t.borrow_mut();
        let gap = t
            .last_end
            .map(|end| start.saturating_duration_since(end))
            .unwrap_or_default();
        t.last_end = Some(now);
        if t.screen.as_ref() != Some(&screen) {
            summary(&t);
            debug!("ui: trace → {:?}, nav {}", screen.0, screen.1);
            *t = Trace {
                screen: Some(screen.clone()),
                since: start,
                last_end: Some(now),
                frames: 0,
                slowest: Duration::ZERO,
                rows,
                active: true,
            };
        }
        if !t.active {
            return;
        }
        if start.duration_since(t.since) > TRACE_FOR {
            summary(&t);
            t.active = false;
            return;
        }
        t.frames += 1;
        t.slowest = t.slowest.max(took);
        let new_row = rows != t.rows;
        t.rows = rows;
        if t.frames == 1 || took >= TRACE_FRAME || gap >= TRACE_GAP || new_row {
            let rows_note = if screen.0 == AppMode::Discover {
                format!(", Discover rows built: {rows}")
            } else {
                String::new()
            };
            debug!(
                "ui: frame #{} at +{} ms: {} ms, {} ms since the previous one{rows_note}",
                t.frames,
                start.duration_since(t.since).as_millis(),
                took.as_millis(),
                gap.as_millis()
            );
        }
    });
}

fn summary(t: &Trace) {
    if let (true, Some((mode, nav))) = (t.active, t.screen.as_ref()) {
        debug!(
            "ui: trace done ({mode:?}, nav {nav}): {} frames, slowest {} ms",
            t.frames,
            t.slowest.as_millis()
        );
    }
}
