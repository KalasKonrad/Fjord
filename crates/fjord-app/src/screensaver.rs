// ── fjord-app · screensaver.rs ───────────────────────────────────────────────
//   wire_screensaver   500 ms Timer: while music plays (is-audio-playing — paused too — and not the
//                      video player), Settings → Audio → "Screensaver while music plays" isn't Off
//                      and the ☾ button hasn't paused it, turns the window black
//                      (AppState.screensaver-active, main.slint's overlay) after that many minutes
//                      without real input (ActivityClock::input_idle_for — playback doesn't count);
//                      ends it on input (the mouse, through the clock; keys and clicks end it
//                      directly) or when that stops being true; the ☾ pause lasts until the music
//                      stops
//   wake               end it (keys::wire_key_dispatch: the waking key does nothing else)
// ─────────────────────────────────────────────────────────────────────────────
// The screen itself is never turned off: a TV losing its HDMI signal mid-session is what the user
// wants to avoid, and playback keeps inhibiting power saving (playback/inhibit.rs) the whole time.
use std::cell::RefCell;
use std::time::Duration;

use slint::{ComponentHandle, Global, Timer, TimerMode};
use tracing::debug;

use crate::activity::ActivityClock;
use crate::{AppState, MainWindow};

const TICK: Duration = Duration::from_millis(500);

thread_local! {
    static TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

pub(crate) fn wire_screensaver(window: &MainWindow, clock: ActivityClock) {
    let ww = window.as_weak();
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, TICK, move || {
        let Some(w) = ww.upgrade() else { return };
        let g = AppState::get(&w);
        // The ☾ pause is for this playback only.
        if !g.get_is_audio_playing() && g.get_screensaver_paused() {
            g.set_screensaver_paused(false);
        }
        let mins = g.get_settings_screensaver_mins();
        let armed = g.get_is_audio_playing()
            && !g.get_is_playing()
            && mins > 0
            && !g.get_screensaver_paused();
        let idle = clock.input_idle_for();
        match step(g.get_screensaver_active(), armed, idle, mins) {
            Some(true) => {
                debug!("screensaver: on after {} s without input", idle.as_secs());
                g.set_screensaver_active(true);
            }
            Some(false) if armed => wake(&g, "input"),
            Some(false) => wake(&g, "music stopped or screensaver turned off"),
            None => {}
        }
    });
    TIMER.with(|t| *t.borrow_mut() = Some(timer));
}

/// One tick's decision: Some(true) turn it on, Some(false) end it, None leave it. `armed` = music
/// plays, a time is set and the ☾ button hasn't paused it; `idle` = time since the last real input.
fn step(active: bool, armed: bool, idle: Duration, mins: i32) -> Option<bool> {
    if active {
        (!armed || idle < 2 * TICK).then_some(false)
    } else {
        (armed && idle >= Duration::from_secs(u64::from(mins.unsigned_abs()) * 60)).then_some(true)
    }
}

/// Ends the screensaver (no-op when it's off).
pub(crate) fn wake(g: &AppState, why: &str) {
    if g.get_screensaver_active() {
        debug!("screensaver: off ({why})");
        g.set_screensaver_active(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIN: Duration = Duration::from_secs(60);

    #[test]
    fn comes_on_after_the_set_time_without_input() {
        assert_eq!(step(false, true, 5 * MIN - Duration::from_secs(1), 5), None);
        assert_eq!(step(false, true, 5 * MIN, 5), Some(true));
        // Not armed (no music, setting Off, or paused by ☾): never.
        assert_eq!(step(false, false, 60 * MIN, 5), None);
    }

    #[test]
    fn ends_on_input_or_when_no_longer_armed() {
        assert_eq!(
            step(true, true, 6 * MIN, 5),
            None,
            "still idle: stays black"
        );
        assert_eq!(
            step(true, true, Duration::from_millis(300), 5),
            Some(false),
            "mouse moved"
        );
        assert_eq!(
            step(true, false, 6 * MIN, 5),
            Some(false),
            "music stopped / ☾ pressed"
        );
    }
}
