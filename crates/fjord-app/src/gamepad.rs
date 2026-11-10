// ── fjord-app · gamepad.rs ───────────────────────────────────────────────────
//   spawn         a thread reading game pads with gilrs (evdev; hot-plug) → PadState → each input
//                 delivered on the UI thread as the key it stands for, through
//                 AppState.handle-key — the same path as a key press (activity clock, screensaver
//                 wake, raw-key tiers, bindings); volume calls AppState.volume-up/-down directly
//                 (its keys are the arrows, which navigate outside the player), after the same
//                 activity/screensaver step. Only while Fjord's window has focus
//   PadInput      what a pad does: the four directions, Confirm/Back, the other buttons, volume
//   PadState      held directions repeat like a held key (REPEAT_DELAY, then REPEAT_EVERY); the
//                 left stick is a D-pad with hysteresis (STICK_ON / STICK_OFF), the right stick's
//                 up/down is volume — unit-tested
//   raw_key       directions, A (Return) and B (Escape) as keys — never through the bindings, so a
//                 text field gets Escape, not Backspace
//   action_for    the other buttons → a Fjord action, by screen (browsing / video player) — the
//                 table agreed for the Xbox 360 pad (PLAN 0.5.2 item 8); unit-tested
//   KeepAwake     KDE judges idleness by keyboard/mouse through KWin, which never sees the pad (and
//                 SimulateUserActivity doesn't reach KWin's timer on Plasma 6 Wayland): the reader
//                 thread holds playback::hold_activity (screen blanking + sleep blocked) from the
//                 first pad input until PAD_HOLD without one (release_due, unit-tested)
//   key_for       the key bound to an action (the player's bindings first while a video plays,
//                 and last otherwise — global.rs's music play/pause reads the player map; the
//                 fewest modifiers, then the shortest key — deterministic); unit-tested
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use gilrs::{Axis, Button, EventType, Gilrs};
use slint::winit_030::WinitWindowAccessor;
use slint::{ComponentHandle, Global};
use tracing::{debug, info};

use crate::activity::ActivityClock;
use crate::config::FjordState;
use crate::keys::key;
use crate::keys::{Action, KeyCombo, Keybindings};
use crate::{AppState, MainWindow};

/// A held direction or volume step repeats like a held key: first after this…
const REPEAT_DELAY: Duration = Duration::from_millis(400);
/// …then this often.
const REPEAT_EVERY: Duration = Duration::from_millis(90);
/// A stick counts as pushed past this, and as let go below STICK_OFF (no flicker in between).
const STICK_ON: f32 = 0.6;
const STICK_OFF: f32 = 0.4;
/// How long the reader waits for an event before it checks the repeats.
const POLL: Duration = Duration::from_millis(20);
/// The desktop's idle rules apply again after this long without pad input (KeepAwake).
const PAD_HOLD: Duration = Duration::from_secs(5 * 60);

fn release_due(last_input: Option<Instant>, now: Instant) -> bool {
    last_input.is_some_and(|t| now.duration_since(t) >= PAD_HOLD)
}

/// Blocks screen blanking and sleep while the pad is in use (KDE never sees pad input).
struct KeepAwake {
    hold: Option<crate::playback::ActivityHold>,
    last_input: Option<Instant>,
    failed: bool,
}

impl KeepAwake {
    fn new() -> Self {
        KeepAwake {
            hold: None,
            last_input: None,
            failed: false,
        }
    }

    fn input(&mut self, now: Instant) {
        self.last_input = Some(now);
        if self.hold.is_some() || self.failed {
            return;
        }
        self.hold = crate::playback::hold_activity("Game pad in use");
        if self.hold.is_none() {
            info!(
                "gamepad: can't keep the desktop awake while the pad is used — it may dim the screen"
            );
            self.failed = true;
        }
    }

    fn tick(&mut self, now: Instant) {
        if self.hold.is_some() && release_due(self.last_input, now) {
            debug!(
                "gamepad: no pad input for {} s — the desktop's idle rules apply again",
                PAD_HOLD.as_secs()
            );
            if let Some(hold) = self.hold.take() {
                hold.release();
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PadInput {
    Up,
    Down,
    Left,
    Right,
    Confirm,
    Back,
    ContextMenu,
    Y,
    Start,
    Select,
    Guide,
    Lb,
    Rb,
    Lt,
    Rt,
    VolumeUp,
    VolumeDown,
}

impl PadInput {
    fn repeats(self) -> bool {
        matches!(
            self,
            PadInput::Up
                | PadInput::Down
                | PadInput::Left
                | PadInput::Right
                | PadInput::VolumeUp
                | PadInput::VolumeDown
        )
    }

    fn from_button(button: Button) -> Option<Self> {
        Some(match button {
            Button::DPadUp => PadInput::Up,
            Button::DPadDown => PadInput::Down,
            Button::DPadLeft => PadInput::Left,
            Button::DPadRight => PadInput::Right,
            Button::South => PadInput::Confirm,
            Button::East => PadInput::Back,
            Button::West => PadInput::ContextMenu,
            Button::North => PadInput::Y,
            Button::Start => PadInput::Start,
            Button::Select => PadInput::Select,
            Button::Mode => PadInput::Guide,
            Button::LeftTrigger => PadInput::Lb,
            Button::RightTrigger => PadInput::Rb,
            Button::LeftTrigger2 => PadInput::Lt,
            Button::RightTrigger2 => PadInput::Rt,
            _ => return None,
        })
    }
}

/// One input to deliver: what, and whether it's a repeat of a held one.
pub(crate) type PadOut = (PadInput, bool);

#[derive(Default)]
pub(crate) struct PadState {
    /// Held repeating inputs and when each repeats next.
    held: Vec<(PadInput, Instant)>,
    left: (f32, f32),
    left_dir: Option<PadInput>,
    right_y: f32,
    right_dir: Option<PadInput>,
}

impl PadState {
    pub(crate) fn press(&mut self, input: PadInput, now: Instant) -> Vec<PadOut> {
        if input.repeats() {
            if self.held.iter().any(|(i, _)| *i == input) {
                return Vec::new();
            }
            self.held.push((input, now + REPEAT_DELAY));
        }
        vec![(input, false)]
    }

    pub(crate) fn release(&mut self, input: PadInput) {
        self.held.retain(|(i, _)| *i != input);
    }

    /// Repeats that are due.
    pub(crate) fn tick(&mut self, now: Instant) -> Vec<PadOut> {
        let mut out = Vec::new();
        for (input, next) in &mut self.held {
            if now >= *next {
                out.push((*input, true));
                *next = now + REPEAT_EVERY;
            }
        }
        out
    }

    /// The left stick as a D-pad: the stronger axis decides once past STICK_ON; the direction
    /// holds until its own axis drops below STICK_OFF. y > 0 = up (gilrs).
    pub(crate) fn left_stick(
        &mut self,
        x: Option<f32>,
        y: Option<f32>,
        now: Instant,
    ) -> Vec<PadOut> {
        if let Some(x) = x {
            self.left.0 = x;
        }
        if let Some(y) = y {
            self.left.1 = y;
        }
        let (x, y) = self.left;
        let along = |d: PadInput| match d {
            PadInput::Up => y,
            PadInput::Down => -y,
            PadInput::Left => -x,
            _ => x,
        };
        let keep = self.left_dir.filter(|d| along(*d) > STICK_OFF);
        let want = keep.or_else(|| {
            if x.abs().max(y.abs()) < STICK_ON {
                None
            } else if x.abs() > y.abs() {
                Some(if x > 0.0 {
                    PadInput::Right
                } else {
                    PadInput::Left
                })
            } else {
                Some(if y > 0.0 {
                    PadInput::Up
                } else {
                    PadInput::Down
                })
            }
        });
        self.change(want, true, now)
    }

    /// The right stick's up/down: volume.
    pub(crate) fn right_stick(&mut self, y: f32, now: Instant) -> Vec<PadOut> {
        self.right_y = y;
        let keep = self.right_dir.filter(|d| match d {
            PadInput::VolumeUp => y > STICK_OFF,
            _ => -y > STICK_OFF,
        });
        let want = keep.or_else(|| {
            if y >= STICK_ON {
                Some(PadInput::VolumeUp)
            } else if y <= -STICK_ON {
                Some(PadInput::VolumeDown)
            } else {
                None
            }
        });
        self.change(want, false, now)
    }

    fn change(&mut self, want: Option<PadInput>, left: bool, now: Instant) -> Vec<PadOut> {
        let current = if left { self.left_dir } else { self.right_dir };
        if want == current {
            return Vec::new();
        }
        if let Some(old) = current {
            self.release(old);
        }
        if left {
            self.left_dir = want;
        } else {
            self.right_dir = want;
        }
        want.map(|w| self.press(w, now)).unwrap_or_default()
    }
}

/// Directions, A and B as the keys themselves (raw-key tiers read these).
pub(crate) fn raw_key(input: PadInput) -> Option<&'static str> {
    Some(match input {
        PadInput::Up => key::UP,
        PadInput::Down => key::DOWN,
        PadInput::Left => key::LEFT,
        PadInput::Right => key::RIGHT,
        PadInput::Confirm => key::RETURN,
        PadInput::Back => key::ESCAPE,
        _ => return None,
    })
}

/// The agreed Xbox 360 table for the other buttons: browsing / video player.
pub(crate) fn action_for(input: PadInput, in_player: bool) -> Option<Action> {
    Some(match (input, in_player) {
        (PadInput::ContextMenu, _) => Action::OpenContextMenu,
        (PadInput::Y, false) => Action::SearchJump,
        (PadInput::Y, true) => Action::PanelSubtitles,
        (PadInput::Start, _) => Action::PausePlay,
        (PadInput::Select, false) => Action::ToggleNowPlaying,
        (PadInput::Select, true) => Action::ToggleStats,
        (PadInput::Lb, false) => Action::PrevTrack,
        (PadInput::Rb, false) => Action::NextTrack,
        (PadInput::Lb, true) => Action::SeekBackward,
        (PadInput::Rb, true) => Action::SeekForward,
        (PadInput::Lt, true) => Action::SeekBackwardLong,
        (PadInput::Rt, true) => Action::SeekForwardLong,
        (PadInput::Guide, false) => Action::NavHome,
        (PadInput::Guide, true) => Action::MinimizePlayer,
        _ => return None,
    })
}

/// The key bound to `action` — the player's bindings first while a video plays, as handle_key
/// looks them up, and as a last resort otherwise (Play/pause lives only there, and global.rs's
/// music_play_pause looks a key up in it); of several keys the one with the fewest modifiers,
/// then the shortest name.
pub(crate) fn key_for(
    bindings: &Keybindings,
    action: &Action,
    in_player: bool,
) -> Option<KeyCombo> {
    let best = |map: &crate::keys::KeyMap| {
        map.iter()
            .filter(|(_, a)| *a == action)
            .map(|(c, _)| c)
            .min_by_key(|c| {
                let mods = c.shift as u8 + c.ctrl as u8 + c.alt as u8;
                (mods, c.key.chars().count(), c.key.clone())
            })
            .cloned()
    };
    if in_player {
        best(&bindings.player).or_else(|| best(&bindings.normal))
    } else {
        best(&bindings.normal).or_else(|| best(&bindings.player))
    }
}

pub(crate) fn spawn(
    window: slint::Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    clock: ActivityClock,
) {
    let spawned = std::thread::Builder::new()
        .name("gamepad".into())
        .spawn(move || run(window, state, clock));
    if let Err(e) = spawned {
        debug!("gamepad: couldn't start its thread: {e}");
    }
}

fn run(window: slint::Weak<MainWindow>, state: Arc<Mutex<FjordState>>, clock: ActivityClock) {
    let mut gilrs = match Gilrs::new() {
        Ok(g) => g,
        Err(e) => {
            info!("gamepad: no game pad support ({e})");
            return;
        }
    };
    for (_, pad) in gilrs.gamepads() {
        info!("gamepad: {} connected", pad.name());
    }
    let mut pad = PadState::default();
    let mut awake = KeepAwake::new();
    loop {
        let now = Instant::now();
        let mut out = Vec::new();
        if let Some(event) = gilrs.next_event_blocking(Some(POLL)) {
            match event.event {
                EventType::ButtonPressed(button, _) => {
                    if let Some(input) = PadInput::from_button(button) {
                        out.extend(pad.press(input, now));
                    }
                }
                EventType::ButtonReleased(button, _) => {
                    if let Some(input) = PadInput::from_button(button) {
                        pad.release(input);
                    }
                }
                EventType::AxisChanged(Axis::LeftStickX, v, _) => {
                    out.extend(pad.left_stick(Some(v), None, now))
                }
                EventType::AxisChanged(Axis::LeftStickY, v, _) => {
                    out.extend(pad.left_stick(None, Some(v), now))
                }
                EventType::AxisChanged(Axis::RightStickY, v, _) => {
                    out.extend(pad.right_stick(v, now))
                }
                EventType::Connected => {
                    info!("gamepad: {} connected", gilrs.gamepad(event.id).name())
                }
                EventType::Disconnected => info!("gamepad: a pad disconnected"),
                _ => {}
            }
        }
        out.extend(pad.tick(Instant::now()));
        if out.is_empty() {
            awake.tick(Instant::now());
        } else {
            awake.input(Instant::now());
        }
        for (input, repeat) in out {
            deliver(&window, &state, &clock, input, repeat);
        }
    }
}

/// On the UI thread: the input as its key, through AppState.handle-key.
fn deliver(
    window: &slint::Weak<MainWindow>,
    state: &Arc<Mutex<FjordState>>,
    clock: &ActivityClock,
    input: PadInput,
    repeat: bool,
) {
    let window = window.clone();
    let state = Arc::clone(state);
    let clock = clock.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = window.upgrade() else { return };
        // Only Fjord's own pad input: not while another window has the keyboard.
        if !w
            .window()
            .with_winit_window(|win| win.has_focus())
            .unwrap_or(true)
        {
            return;
        }
        let g = AppState::get(&w);
        if matches!(input, PadInput::VolumeUp | PadInput::VolumeDown) {
            // What AppState.handle-key's callback does first (keys/mod.rs): count the input, and
            // a press that ends the screensaver does nothing else.
            clock.touch();
            if g.get_screensaver_active() {
                crate::screensaver::wake(&g, "game pad");
                return;
            }
            if input == PadInput::VolumeUp {
                g.invoke_volume_up();
            } else {
                g.invoke_volume_down();
            }
            return;
        }
        let combo = match raw_key(input) {
            Some(k) => KeyCombo::plain(k),
            None => {
                let in_player = g.get_is_playing();
                let Some(action) = action_for(input, in_player) else {
                    return;
                };
                let found = {
                    let s = state.lock().unwrap();
                    key_for(&s.keybindings, &action, in_player)
                };
                let Some(combo) = found else {
                    debug!("gamepad: {input:?} → {action:?}, but no key is bound to it");
                    return;
                };
                combo
            }
        };
        if !repeat {
            debug!("gamepad: {input:?} → key {:?}", combo.key);
        }
        g.invoke_handle_key(combo.key.into(), combo.shift, combo.ctrl, repeat);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_direction_repeats_like_a_held_key() {
        let mut pad = PadState::default();
        let t = Instant::now();
        assert_eq!(pad.press(PadInput::Down, t), [(PadInput::Down, false)]);
        assert_eq!(
            pad.press(PadInput::Down, t),
            [],
            "a second press of a held input is ignored"
        );
        assert!(pad.tick(t + REPEAT_DELAY / 2).is_empty());
        assert_eq!(pad.tick(t + REPEAT_DELAY), [(PadInput::Down, true)]);
        assert!(pad.tick(t + REPEAT_DELAY + REPEAT_EVERY / 2).is_empty());
        assert_eq!(
            pad.tick(t + REPEAT_DELAY + REPEAT_EVERY),
            [(PadInput::Down, true)]
        );
        pad.release(PadInput::Down);
        assert!(pad.tick(t + REPEAT_DELAY * 4).is_empty());
        // Buttons don't repeat.
        assert_eq!(
            pad.press(PadInput::Confirm, t),
            [(PadInput::Confirm, false)]
        );
        assert!(pad.tick(t + REPEAT_DELAY * 2).is_empty());
    }

    #[test]
    fn the_left_stick_is_a_dpad_with_hysteresis() {
        let mut pad = PadState::default();
        let t = Instant::now();
        assert!(
            pad.left_stick(Some(0.5), None, t).is_empty(),
            "not far enough"
        );
        assert_eq!(
            pad.left_stick(Some(0.7), None, t),
            [(PadInput::Right, false)]
        );
        assert!(
            pad.left_stick(Some(0.5), None, t).is_empty(),
            "still held above STICK_OFF"
        );
        assert!(
            pad.left_stick(Some(0.3), None, t).is_empty(),
            "let go: no output"
        );
        assert!(pad.tick(t + REPEAT_DELAY).is_empty(), "and no repeat");
        // y > 0 is up; the stronger axis wins.
        assert_eq!(
            pad.left_stick(Some(0.2), Some(-0.9), t),
            [(PadInput::Down, false)]
        );
        assert_eq!(
            pad.left_stick(Some(-0.95), Some(0.1), t),
            [(PadInput::Left, false)]
        );
    }

    #[test]
    fn the_right_stick_is_volume() {
        let mut pad = PadState::default();
        let t = Instant::now();
        assert_eq!(pad.right_stick(0.8, t), [(PadInput::VolumeUp, false)]);
        assert_eq!(pad.tick(t + REPEAT_DELAY), [(PadInput::VolumeUp, true)]);
        assert!(pad.right_stick(0.45, t).is_empty());
        assert_eq!(pad.right_stick(-0.7, t), [(PadInput::VolumeDown, false)]);
        assert!(pad.right_stick(0.0, t).is_empty());
        assert!(pad.tick(t + REPEAT_DELAY * 3).is_empty());
    }

    #[test]
    fn the_desktop_may_idle_again_5_min_after_the_last_pad_input() {
        let t = Instant::now();
        assert!(!release_due(None, t), "never used: nothing to release");
        assert!(!release_due(Some(t), t + PAD_HOLD / 2));
        assert!(release_due(Some(t), t + PAD_HOLD));
    }

    #[test]
    fn the_agreed_table() {
        assert_eq!(
            raw_key(PadInput::Back),
            Some(key::ESCAPE),
            "B is Escape, never Backspace"
        );
        assert_eq!(raw_key(PadInput::Confirm), Some(key::RETURN));
        assert_eq!(action_for(PadInput::Y, false), Some(Action::SearchJump));
        assert_eq!(action_for(PadInput::Y, true), Some(Action::PanelSubtitles));
        assert_eq!(
            action_for(PadInput::Select, false),
            Some(Action::ToggleNowPlaying)
        );
        assert_eq!(
            action_for(PadInput::Select, true),
            Some(Action::ToggleStats)
        );
        assert_eq!(action_for(PadInput::Lb, true), Some(Action::SeekBackward));
        assert_eq!(
            action_for(PadInput::Rt, true),
            Some(Action::SeekForwardLong)
        );
        assert_eq!(action_for(PadInput::Lt, false), None);
        assert_eq!(action_for(PadInput::Guide, false), Some(Action::NavHome));
        assert_eq!(
            action_for(PadInput::Guide, true),
            Some(Action::MinimizePlayer)
        );
    }

    #[test]
    fn every_action_in_the_table_has_a_default_key() {
        let bindings = crate::keys::default_keybindings();
        for input in [
            PadInput::ContextMenu,
            PadInput::Y,
            PadInput::Start,
            PadInput::Select,
            PadInput::Lb,
            PadInput::Rb,
            PadInput::Lt,
            PadInput::Rt,
            PadInput::Guide,
        ] {
            for in_player in [false, true] {
                if let Some(action) = action_for(input, in_player) {
                    assert!(
                        key_for(&bindings, &action, in_player).is_some(),
                        "{input:?} (player {in_player}) → {action:?} has no default key"
                    );
                }
            }
        }
    }
}
