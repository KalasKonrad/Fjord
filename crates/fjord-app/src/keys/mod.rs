// ── fjord-app · keys/mod.rs ──────────────────────────────────────────────────
//   Submodules (re-exported here, so callers keep using crate::keys::*):
//     bindings   Action, KeyCombo, KeyMap/Keybindings, defaults, rebinding, keybinding rows + nav
//     mode       AppMode + active_mode()
//     overlays   raw-key handlers for the pre-AppMode overlays (login, pickers, Bonfire, Seerr…)
//     dashboard  dashboard dispatch, bar focus fallbacks
//     tiers      RAW_TIERS: the raw-key tiers checked before any key lookup (+ RawKey)
//     global     GLOBAL_SHORTCUTS: Quit/fullscreen/resume/queue/Now Playing/music/bar focus (+ KeyCtx)
//     library    library grid dispatch
//     player     player, queue panel and Now Playing dispatch
//     text_input on-screen keyboard opener/cursor math, drawn search fields, playlist picker
//   key          Slint key string constants
//   lookup_action  KeyCombo → Action (exact match first; with Shift held, a printable key
//                  falls back to its unshifted binding)
//   handle_key   router: RAW_TIERS (tiers.rs — on-screen keyboard, login/pickers, overlays and
//                confirms, profile edit, startup gate, search fields, playlist picker, loading guard,
//                rebind capture; first open tier owns the key) → key lookup (+ the Enter
//                kb-activate-pulse) → GLOBAL_SHORTCUTS (global.rs — Ctrl+Q, F, R, N, q, m, music,
//                bar focus; excluding overlay/detail modes) → active_mode() → per-screen handler
//                (context_menu, series, season, detail, browse, discover…, settings::dispatch_settings)
//   caret_key    Left/Right/Home/End/Delete → text_field caret for the drawn fields (Discover/
//                Browse/Library search, playlist name — Right still creates at the end — and the
//                Bonfire join code)
//   wire_key_dispatch  callbacks moved from main() (0.5.0 step 3): the global key dispatch (fs
//                FocusScope → handle_key)
// ─────────────────────────────────────────────────────────────────────────────

use serde::{Deserialize, Serialize};
use slint::{Global, Model, ModelRc, SharedString, VecModel};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tracing::{debug, warn};

use crate::config::FjordState;

// ── Slint key string constants ────────────────────────────────────────────────
// Slint encodes named keys as Unicode Private Use Area (PUA) codepoints.
// These match i-slint-common/key_codes.rs exactly.
pub mod key {
    pub const BACKSPACE: &str = "\u{0008}";
    pub const RETURN: &str = "\u{000a}";
    pub const ESCAPE: &str = "\u{001b}";
    pub const UP: &str = "\u{F700}";
    pub const DOWN: &str = "\u{F701}";
    pub const LEFT: &str = "\u{F702}";
    pub const RIGHT: &str = "\u{F703}";
    // Slint key codes (i-slint-common key_codes.rs) — text-field caret keys.
    pub const DELETE: &str = "\u{007f}";
    pub const HOME: &str = "\u{F729}";
    pub const END: &str = "\u{F72B}";
    pub const F11: &str = "\u{F70E}";
}

mod bindings;
mod dashboard;
mod global;
mod library;
mod mode;
mod overlays;
mod player;
mod text_input;
mod tiers;

pub(crate) use bindings::*;
pub(crate) use dashboard::*;
pub(crate) use global::*;
pub(crate) use library::*;
pub(crate) use mode::*;
pub(crate) use overlays::*;
pub(crate) use player::*;
pub(crate) use text_input::*;
pub(crate) use tiers::*;

// ── Dispatch ──────────────────────────────────────────────────────────────────

/// Look up `combo` in `map`. Most single-letter bindings ("f" → Fullscreen,
/// "b" → OpenBrowse, ...) are deliberately shift-insensitive — they're
/// registered once, unshifted, and meant to fire whether or not Shift was
/// held. A few (z/Z for sub-delay, x/X for audio-delay, matching mpv's own
/// convention) are deliberately shift-*sensitive* and register both an
/// unshifted and an explicit `KeyCombo::shifted(...)` entry for two
/// different actions. This function has to serve both: try the exact combo
/// first (so a shift-sensitive pair's own shifted entry always wins over
/// falling through to its unshifted sibling), then, only on a miss with
/// shift held, retry unshifted (so a shift-insensitive binding's letter
/// still fires when actually typed with Shift held, since it only ever
/// registered the unshifted form). Named keys (arrows etc., PUA codepoints)
/// never get the retry, so Shift+Left stays distinct from Left rather than
/// silently falling back to plain seeking. `KeyCombo::new` already
/// lower-cases `key` for both `combo` and everything in `map`, so this
/// never needs to reason about letter case itself — only about whether an
/// exact (key, shift) match exists.
fn lookup_action(map: &KeyMap, combo: &KeyCombo) -> Option<Action> {
    if let Some(a) = map.get(combo) {
        return Some(a.clone());
    }
    if combo.shift && is_printable(&combo.key) {
        let unshifted = KeyCombo {
            shift: false,
            ..combo.clone()
        };
        return map.get(&unshifted).cloned();
    }
    None
}

pub(crate) fn handle_key(
    key: &str,
    shift: bool,
    ctrl: bool,
    repeat: bool,
    state: &Arc<Mutex<FjordState>>,
    window: &crate::MainWindow,
    _rt: &tokio::runtime::Handle,
) -> bool {
    let g = crate::AppState::get(window);

    if key.is_empty() {
        return false;
    }

    // Raw-key tiers first (tiers.rs): overlays, confirms, the startup gate, search fields…
    let raw = RawKey { key, shift, ctrl };
    if let Some((_, _, keys)) = RAW_TIERS.iter().find(|(_, open, _)| open(&g, &raw)) {
        return keys(&g, &raw, window, state);
    }

    // Key → Action lookup. KeyCombo::new lower-cases key, so Caps Lock never
    // affects which binding this resolves to — only the physical Shift key
    // state (shift, reported separately by Slint) does.
    let combo = KeyCombo::new(key, shift, ctrl, false);
    let in_player = g.get_is_playing();
    let action: Option<Action> = {
        let s = state.lock().unwrap();
        if in_player {
            lookup_action(&s.keybindings.player, &combo)
                .or_else(|| lookup_action(&s.keybindings.normal, &combo))
        } else {
            lookup_action(&s.keybindings.normal, &combo)
        }
    };
    let mode = active_mode(&g);
    // AppState.sidebar-kb-active (app_state.slint) is a pure Slint expression
    // mirroring when these handlers move the sidebar (Dashboard/Discover with
    // focused-section < 0, Browse before its list, Settings before its section
    // list), not something pushed from here — it needs to stay correct for
    // mouse-driven screen changes too, not just keyboard ones.
    // Every focusable widgets.slint::PressPulse instance plays a brief
    // border-flash "press" cue when this bumps, gated on its own already-
    // existing focus/selection expression — this is the single centralized
    // hook for keyboard press feedback, mirroring how active_mode() itself
    // centralizes screen-priority logic instead of scattering it. Mouse press
    // feedback needs no Rust involvement (TouchArea.pressed is used directly).
    if key == key::RETURN {
        g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
    }
    drop(g);

    // Global shortcuts (global.rs), in order: Ctrl+Q, fullscreen, R, N, q, m, music, bars.
    let ctx = KeyCtx {
        action: action.clone(),
        mode: mode.clone(),
        combo: &combo,
        window,
        state,
    };
    for shortcut in GLOBAL_SHORTCUTS {
        if let Some(handled) = shortcut(&ctx) {
            return handled;
        }
    }

    // ── Per-screen dispatch ───────────────────────────────────────────────────
    // Priority is encoded once in active_mode(); each arm is exhaustive.
    match mode {
        AppMode::ContextMenu => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return true;
            }; // swallow unknown keys
            crate::context_menu::handle_key(&action, &g)
        }

        AppMode::QueuePanel => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return true;
            }; // swallow unknown keys
            handle_key_queue_panel(&action, &g)
        }

        AppMode::NowPlaying => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return true;
            }; // swallow unknown keys
            handle_key_now_playing(&action, &g)
        }

        AppMode::Person => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::person::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Season => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::season::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Series => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::series::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        // show-detail stays true during playback (hidden by !is-playing in main.slint);
        // active_mode() already routes is-playing → Player, so this arm is safe.
        AppMode::Detail => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::detail::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Artist => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::artist::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Collection => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::collection::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Album => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::album::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::RequestOptions => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return true;
            }; // swallow unknown keys, same as ContextMenu/QueuePanel
            crate::discover::handle_key_request_options(&action, &g)
        }

        AppMode::RequestDetail => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::discover::handle_key_request_detail(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Calendar => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::discover::handle_key_calendar(&action, &g)
        }

        AppMode::CalendarDayPopup => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return true;
            }; // swallow unknown keys, same as ContextMenu/QueuePanel/RequestOptions
            crate::discover::handle_key_calendar_day_popup(&action, &g)
        }

        AppMode::Blocklist => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return true;
            }; // swallow unknown keys, same as Calendar's own sibling modes
            crate::blocklist::handle_key(&action, &g)
        }
        AppMode::BonfireAdmin => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return true;
            };
            crate::bonfire_admin::handle_key(&action, &g)
        }

        AppMode::Discover => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::discover::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Player => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            // ToggleStats and PausePlay must not reveal the full controls bar.
            // Seek actions use seek accumulation + minimal bar (no full controls).
            // Confirm (Enter) activates skip/banner/panel overlays — should not reveal controls.
            let shows_controls = !matches!(
                action,
                Action::ToggleStats
                    | Action::PausePlay
                    | Action::SeekBackward
                    | Action::SeekForward
                    | Action::SeekBackwardLong
                    | Action::SeekForwardLong
                    | Action::NextChapter
                    | Action::PrevChapter
                    | Action::SubDelayIncrease
                    | Action::SubDelayDecrease
                    | Action::AudioDelayIncrease
                    | Action::AudioDelayDecrease
                    | Action::PrevTrack
                    | Action::NextTrack
                    | Action::Confirm
            );
            if shows_controls {
                g.invoke_show_controls();
            }
            drop(g);
            dispatch_player(action, window)
        }

        AppMode::Library => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            dispatch_library(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Browse => {
            let g = crate::AppState::get(window);
            let Some(action) = action else {
                return false;
            };
            crate::browse::handle_key(&action, &g)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Settings => {
            let Some(action) = action else {
                return false;
            };
            {
                let g = crate::AppState::get(window);
                if g.get_keybinding_focused() >= 0 {
                    return dispatch_keybinding_nav(action, &g);
                }
            }
            {
                let g = crate::AppState::get(window);
                if let Some(handled) = crate::settings::dispatch_settings(&action, &g) {
                    return handled;
                }
            }
            // dispatch_settings returned None: settings-section == "" (sidebar mode).
            // Let sidebar Up/Down and global shortcuts through so nav remains functional.
            dispatch_dashboard(&action, repeat, window)
                || handle_global_shortcuts(&action, window)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Dashboard => {
            let Some(action) = action else {
                return false;
            };
            if handle_global_shortcuts(&action, window) {
                return true;
            }
            dispatch_dashboard(&action, repeat, window)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

// Returns the sort-bar cursor position that lands on the currently active sort pill.
// For Music (nav=4) view pills occupy cursor 0-1, so sort pills start at offset 2.
fn sort_bar_init_cursor(g: &crate::AppState) -> i32 {
    let sort = g.get_library_sort();
    if g.get_active_nav() == 4 {
        sort + 3
    } else {
        sort
    }
}

fn nav_to(window: &crate::MainWindow, nav: i32) {
    let g = crate::AppState::get(window);
    g.set_show_browse(false);
    g.set_show_library(false);
    g.set_library_header_focused(false);
    g.set_library_scrubber_focused(false);
    g.set_focused_section(-1);
    g.set_settings_section("".into());
    g.set_settings_focused("".into());
    g.set_settings_dropdown_open(false);
    g.set_keybinding_focused(-1);
    g.set_active_nav(nav);
    g.invoke_nav_selected(nav);
}

fn sidebar_nav(g: &crate::AppState<'_>, dir: i32) {
    crate::browse::sidebar_nav(g, dir);
}

fn is_navigation_key(key: &str) -> bool {
    let Some(ch) = key.chars().next() else {
        return true;
    };
    (ch as u32) >= 0xE000 || ch.is_control()
}

fn is_printable(key: &str) -> bool {
    let Some(ch) = key.chars().next() else {
        return false;
    };
    if key.chars().count() != 1 {
        return false;
    }
    (ch as u32) < 0xE000 && !ch.is_control()
}

/// Left/Right/Home/End in a drawn text field move its caret (2026-10-05,
/// see text_field.rs). True if `key` was one of them.
fn caret_key(field: &crate::text_field::DrawnField, key: &str, g: &crate::AppState) -> bool {
    match key {
        k if k == key::LEFT => field.move_by(g, -1),
        k if k == key::RIGHT => field.move_by(g, 1),
        k if k == key::HOME => field.home(g),
        k if k == key::END => field.end(g),
        _ => return false,
    }
    true
}

// ── wire_key_dispatch (moved from main(), 0.5.0 step 3) ──────────────────
/// Wires the global key dispatch (fs FocusScope → handle_key): handle_key.
pub(crate) fn wire_key_dispatch(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    video: &std::sync::Arc<std::sync::Mutex<crate::playback::VideoState>>,
    rt: &tokio::runtime::Runtime,
    activity_clock: &crate::activity::ActivityClock,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    let video = std::sync::Arc::clone(video);
    // ── keyboard dispatch ────────────────────────────────────────────────────
    {
        let state2 = Arc::clone(&state);
        let video2k = Arc::clone(&video);
        let ww = window.as_weak();
        let rt2 = rt.handle().clone();
        let clock2 = activity_clock.clone();
        AppState::get(&window).on_handle_key(move |key, shift, ctrl, repeat| {
            let Some(w) = ww.upgrade() else {
                return false;
            };
            // Any key resets the Now Playing idle-auto-open countdown.
            video2k.lock().unwrap().music_idle_ticks = 0;
            // Any key also resets the Bonfire idle-lock clock (Phase 4,
            // 2026-08-29) — the single choke point every keypress already
            // passes through, same one music_idle_ticks resets from above.
            clock2.touch();
            keys::handle_key(key.as_str(), shift, ctrl, repeat, &state2, &w, &rt2)
        });
    }
}
