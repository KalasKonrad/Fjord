// ── fjord-app · keys/mod.rs ──────────────────────────────────────────────────
//   Submodules (re-exported here, so callers keep using crate::keys::*):
//     bindings   Action, KeyCombo, KeyMap/Keybindings, defaults, rebinding, keybinding rows + nav
//     mode       AppMode + active_mode()
//     dashboard  dashboard dispatch, global shortcuts, bar focus fallbacks
//     library    library grid dispatch
//     player     player, queue panel and Now Playing dispatch
//     text_input on-screen keyboard opener/cursor math, drawn search fields, playlist picker
//   key          Slint key string constants
//   lookup_action  KeyCombo → Action (case-insensitive fallback)
//   handle_key         router: show-onscreen-keyboard gate (Bonfire Phase 3 — checked before
//                        EVERYTHING else, including show-login, since the keyboard can be open
//                        on any wired-up screen; Left/Right/Up/Down move the flat cursor via
//                        onscreen_keyboard_move_row, Enter bumps kb-activate-pulse, Ctrl+Q quits)
//                        → show-login bypass → startup connectivity gate (show-connecting
//                        swallows all keys; show-offline: Enter → retry-connection, OfflineScreen's
//                        only interactive element has no native widget focus) → show-profile-picker
//                        / show-account-picker raw-key tiers (both pre-AppMode, same reason
//                        show-login is — can show before any session/AppMode-relevant state
//                        exists yet; 2026-08-14, 2-tier account/profile redesign) → search
//                        bypasses → loading-guard (app-content-loading) → rebind capture →
//                        key lookup → active_mode() → match per-screen arm
//   show-account-picker tier  Left/Right move the tile cursor (count == "+ Add Account" tile's
//                        own cursor value); Enter on a real tile → account-picker-select,
//                        on the trailing tile → account-picker-add-account; Escape/Backspace
//                        closes only when account-picker-cancelable (the startup-gate open has
//                        nothing to cancel back to)
//   show-profile-picker tier  same shape one tier down, always account-scoped; Escape/Backspace
//                        dispatches on profile-picker-back-mode ("accounts" → profile-picker-
//                        back-to-accounts, genuinely came from there; "cancel" →
//                        profile-picker-cancel, closes back to a live session without switching
//                        — the sidebar's own "Switch Profile" action, which never went through
//                        the account tier at all; see that property's own doc comment in
//   caret_key          Left/Right/Home/End → text_field caret for the drawn fields (Discover/Browse/
//                      Library search, new-playlist name — Right still creates when the caret is
//                      at the end — and the Bonfire join code), 2026-10-05; Delete per field
//   dispatch_dashboard  content grid nav + item actions
//   Settings dispatch → crate::settings (dispatch_settings, settings_row_action)
//   Per-screen key handlers live in their own modules:
//     context_menu::handle_key, series::handle_key, season::handle_key,
//     detail::handle_key, browse::handle_key,
//                      no Up handler at all (unlike handle_library_search), so Escape was the
//                      ONLY way out of an empty/cleared search field
//   ── Keyboard-navigation fixes (2026-07-18, see discover.rs's own header block for
//      the full investigation this came from) ── AppMode::RequestDetail/RequestOptions
//      added to 3 global pre-dispatch exclusion lists (ResumePlayer, music-bar-focused,
//      mini-player-bar-focused) that already excluded their peer group
//      (Person/Detail/Season/.../Album) but were missing these two — real bug: 'r' could
//      yank the user into the fullscreen player mid-request-flow, and a stale
//      music-bar-focused/float-card-focused left over from earlier keyboard nav could
//      hijack these screens' own arrow keys after a mouse-driven screen switch.
//      active_mode()'s RequestOptions arm also gained the same !is_playing guard every
//      sibling overlay already had (real bug: the modal could get stuck rendered on top
//      of a resumed fullscreen video).
//   ── Watchlist + Release Calendar (2026-07-18, see discover.rs's own header block) ──
//      AppMode::Calendar/CalendarDayPopup added to active_mode() (own !is_playing guard,
//      same as every sibling overlay) and to the same 3 global exclusion lists
//      (ResumePlayer, music-bar-focused, mini-player-bar-focused) RequestDetail/
//      RequestOptions were added to above — CalendarScreen/its day popup dispatch to
//      discover::handle_key_calendar/handle_key_calendar_day_popup.
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
mod library;
mod mode;
mod player;
mod text_input;

pub(crate) use bindings::*;
pub(crate) use dashboard::*;
pub(crate) use library::*;
pub(crate) use mode::*;
pub(crate) use player::*;
pub(crate) use text_input::*;

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

    // On-screen alphanumeric keyboard (Bonfire Phase 3, 2026-08-22; full
    // rollout beyond Login, 2026-08-23) — checked before show_login (and
    // every other screen-scoped gate below), same shape as show-sign-out-
    // confirm: this mechanism is opened from several different screens
    // (Login, ProfileEditScreen, Discover search, Browse search,
    // PlaylistPicker naming, ConnectSeerr — every text-entry surface in the
    // app as of 2026-08-23), so it can't be nested inside any one screen's
    // own tier the way show_profile_pin_entry is nested inside
    // show_profile_picker (PIN entry only ever happens on that one screen —
    // this keyboard doesn't have that luxury). Key VALUES are never read
    // here — only cursor movement and Enter, which just bumps
    // kb-activate-pulse and lets QwertyKeyboard's own _activate-mirror
    // (widgets.slint) resolve what that means; see app_state.slint's own
    // doc comment on show-onscreen-keyboard for why.
    //
    // Also requires settings-onscreen-keyboard-enabled (2026-08-27, the new
    // Settings → UI toggle) — deliberately in ADDITION to every
    // QwertyKeyboard's own mount condition also checking it, not instead
    // of. The mount check alone stops the widget from ever rendering when
    // the setting is off, but says nothing about THIS gate — which runs
    // before every other input tier and unconditionally consumes any key
    // (only Ctrl+Q escapes it) — so if any trigger site (present or
    // future) ever left show-onscreen-keyboard stuck true while the
    // setting is off, this gate alone could still turn into a silent,
    // no-visible-cause input lockout with nothing on screen to explain it.
    // Checking it here too means that failure mode is structurally
    // impossible regardless of what any individual trigger site does.
    if g.get_show_onscreen_keyboard() && g.get_settings_onscreen_keyboard_enabled() {
        // Debug logging, 2026-08-25 — this whole gate had none at all,
        // which left the ProfileEditScreen focus-race bug undiagnosable
        // from a log alone (see profile_edit.rs's own zone 0/5/6 doc
        // comment for the bug this exists to catch a recurrence of): the
        // next log capture will show directly whether a given keypress
        // ever reached this gate at all, or whether some field's native
        // focus swallowed it first.
        debug!(
            "onscreen-kb: key={key:?} target={:?} cursor={}",
            g.get_onscreen_keyboard_target(),
            g.get_onscreen_keyboard_cursor()
        );
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
            return true;
        }
        let row_lens: Vec<i32> = g.get_onscreen_keyboard_row_lens().iter().collect();
        let total: i32 = row_lens.iter().sum();
        let cursor = g.get_onscreen_keyboard_cursor();
        if key == key::LEFT {
            g.set_onscreen_keyboard_cursor((cursor - 1).max(0));
        } else if key == key::RIGHT {
            g.set_onscreen_keyboard_cursor((cursor + 1).min(total - 1));
        } else if key == key::UP {
            g.set_onscreen_keyboard_cursor(onscreen_keyboard_move_row(&row_lens, cursor, -1));
        } else if key == key::DOWN {
            g.set_onscreen_keyboard_cursor(onscreen_keyboard_move_row(&row_lens, cursor, 1));
        } else if key == key::BACKSPACE {
            // Physical-keyboard passthrough, 2026-08-23 — live feedback
            // ("i want it to still work to type on the keybord even if
            // its open"). See app_state.slint's own doc comment on
            // onscreen-keyboard-physical-key for why this is a
            // payload+counter pair, not a direct callback.
            g.set_onscreen_keyboard_physical_key("backspace".into());
            g.set_onscreen_keyboard_physical_key_seq(
                g.get_onscreen_keyboard_physical_key_seq().wrapping_add(1),
            );
        } else if let Some(c) = key.chars().next() {
            // Any other genuinely printable single character — real
            // Shift/AltGr effects are already baked into `key` by Slint
            // (it delivers the resolved text, not a raw keycode), so this
            // needs no separate uppercase handling of its own. Excludes
            // the private-use-area codepoints Slint uses for every other
            // named key (arrows, F11, Delete, Home/End, etc. — all in the
            // same U+E000-U+F8FF block as key::RIGHT's own \u{F703}) as
            // well as plain C0/C1 control characters (Tab, etc.) — neither
            // is a real character a text field should ever receive.
            if key.chars().count() == 1
                && !c.is_control()
                && !('\u{E000}'..='\u{F8FF}').contains(&c)
            {
                g.set_onscreen_keyboard_physical_key(key.into());
                g.set_onscreen_keyboard_physical_key_seq(
                    g.get_onscreen_keyboard_physical_key_seq().wrapping_add(1),
                );
            }
        }
        return true;
    }

    // LoginScreen: return false below to let LineEdit handle normal typing/
    // tabbing, but Ctrl+Q must be carved out first — it would otherwise never
    // reach the global Quit pre-dispatch further down, same class of bug just
    // fixed for the connectivity-gate screens below.
    if g.get_show_login() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        // Real bug, live-reported 2026-08-17: "there is no cancel/back only
        // quit witch will quit jellyfin" — LoginScreen's own "← Back to
        // Profiles"/"Cancel" button (append mode only — see
        // login-append-mode's own doc comment) was mouse-only, with no
        // keyboard path to it at all; Ctrl+Q (quit the whole app) was
        // genuinely the only reachable keyboard action. Escape now invokes
        // the identical cancel-add-account() the button's own click handler
        // does, matching this app's universal Escape=Back convention.
        // Never fires in a genuine first-login (no append mode, nothing to
        // cancel back to) — unchanged there, Escape still does nothing.
        if key == key::ESCAPE && g.get_login_append_mode() {
            g.invoke_cancel_add_account();
            return true;
        }
        // Full D-pad nav, 2026-08-19 (zones 3/4), extended 2026-08-21 (zones
        // 5/6, Back/Quit reachability — see login.slint's own header doc
        // comment for the full design). Only reached for zones 3-6, none of
        // which hold native LineEdit focus (login.slint's own key-pressed
        // hooks call AppState.refocus() when leaving zone 2, specifically so
        // this tier starts seeing keys again) — zones 0-2 fall straight
        // through to `return false` below, since Tab/typing/Enter there are
        // all handled by the LineEdit itself, and a `changed login-zone`
        // tracker in login.slint calls the right field's own .focus()
        // whenever Rust sets this back down to 0-2 (Rust can't call a named
        // Slint element's method directly).
        let zone = g.get_login_zone();
        if (3..=6).contains(&zone) {
            if key == key::RETURN {
                g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
            }
            let append = g.get_login_append_mode();
            // zone 4 (Connect)'s RETURN has no Rust-side arm at all — the
            // actual do-login call needs live LineEdit.text values Rust
            // can't read directly, same "Rust can only bump
            // kb-activate-pulse, a Slint-side changed tracker does the
            // real work" pattern ProfileEditScreen's own Save button
            // already uses. Handled by login.slint's _pulse-mirror tracker.
            // Zones 5 (Back)/6 (Quit) are two INDEPENDENT entry points off
            // opposite ends of the chain, not chained through each other —
            // real bug, live-reported 2026-08-21 ("the back button is down
            // from connect witch feels wrong as it is top left so it shuld
            // be up from the server right?"): the first version reached
            // Back via Down-from-Connect, requiring a full pass through
            // every field to reach a button sitting top-left, visually
            // ABOVE all of them. Back is now reached via Up from Server
            // (zone 0, handled in that field's own key-pressed hook, since
            // it holds native LineEdit focus and never reaches this match
            // at all) — zone 5's own Down returns to Server the same way.
            // Quit (bottom-right) keeps its original Down-from-Connect
            // reachability, matching its actual on-screen position.
            match key {
                key::UP => g.set_login_zone(match zone {
                    3 => 2,
                    4 => 3,
                    6 => 4,
                    _ => zone, // zone == 5 (Back) — nothing above it, stays put
                }),
                key::DOWN => match zone {
                    3 => g.set_login_zone(4),
                    4 => g.set_login_zone(6),
                    5 => g.set_login_zone(0),
                    _ => {} // zone == 6, bottom of the chain
                },
                key::RETURN if zone == 3 => g.set_login_remember(!g.get_login_remember()),
                key::RETURN if zone == 5 => g.invoke_cancel_add_account(),
                key::RETURN if zone == 6 => g.invoke_quit(),
                // Quit-focused Escape/Backspace un-focuses rather than
                // quitting (same convention as the picker screens' own
                // quit-focused blocks) — only reachable here at all when
                // !append_mode, since the unconditional append-mode Escape
                // handler above already intercepts the key first, matching
                // how Escape already behaves at every OTHER zone in append
                // mode (always cancels, not zone-specific).
                key::ESCAPE | key::BACKSPACE if zone == 6 && !append => g.set_login_zone(4),
                _ => {}
            }
            return true;
        }
        return false;
    }

    // ProfilePickerScreen (Bonfire Phase 1, step 6, 2026-08-09) — same tier
    // as show-login above (checked before active_mode() ever runs, never
    // appears as an AppMode value). Raw-key handling, same shape as
    // OfflineScreen below: no native widget focus path, so Left/Right/Enter
    // are matched directly rather than going through the Action/KeyMap
    // layer. PIN entry is a layered sub-state that captures all input first
    // when open — mirrors VirtualKeyboard's own 12-key row-major layout
    // (widgets.slint) exactly, so keyboard and mouse activation always
    // agree on what "cursor N" means.
    if g.get_show_profile_picker() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if g.get_show_profile_pin_entry() {
            if key == key::RETURN {
                g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
            }
            // Cancel button focused (see profile-pin-cancel-focused's own
            // doc comment) — its own small dispatch tier, checked ahead of
            // the grid's own match below.
            if g.get_profile_pin_cancel_focused() {
                match key {
                    key::UP => g.set_profile_pin_cancel_focused(false),
                    key::RETURN | key::ESCAPE | key::BACKSPACE => {
                        g.set_profile_pin_cancel_focused(false);
                        g.set_show_profile_pin_entry(false);
                        g.set_profile_pin_error("".into());
                    }
                    _ => {}
                }
                return true;
            }
            const PIN_VALS: [&str; 12] = [
                "1",
                "2",
                "3",
                "4",
                "5",
                "6",
                "7",
                "8",
                "9",
                "backspace",
                "0",
                "confirm",
            ];
            // Real bug, live-reported 2026-08-17: "cant use numpad or
            // numbers if you have a real keybord and backspace dont work."
            // Two gaps, both fixed together: (1) no arm at all accepted a
            // raw digit character — a physical-keyboard user had no way to
            // type a PIN except D-pad-navigating the on-screen 12-key grid
            // one key at a time; (2) Backspace CLOSED the whole PIN screen
            // instead of deleting the last digit, the opposite of what
            // Backspace means on every other text-entry surface in this
            // app. Escape alone now closes/cancels; Backspace forwards to
            // the same "backspace" value the on-screen key already sends.
            // Digit keys sync the cursor to the matching on-screen key too,
            // same mouse-sync discipline as everywhere else in this app.
            match key {
                key::LEFT => g.set_profile_pin_cursor((g.get_profile_pin_cursor() - 1).max(0)),
                key::RIGHT => g.set_profile_pin_cursor((g.get_profile_pin_cursor() + 1).min(11)),
                key::UP => g.set_profile_pin_cursor((g.get_profile_pin_cursor() - 3).max(0)),
                key::DOWN => {
                    let cursor = g.get_profile_pin_cursor();
                    if cursor >= 9 {
                        g.set_profile_pin_cancel_focused(true);
                    } else {
                        g.set_profile_pin_cursor((cursor + 3).min(11));
                    }
                }
                key::RETURN => {
                    if let Some(v) = PIN_VALS.get(g.get_profile_pin_cursor() as usize) {
                        g.invoke_profile_pin_key((*v).into());
                    }
                }
                key::BACKSPACE => {
                    g.set_profile_pin_cursor(9);
                    g.invoke_profile_pin_key("backspace".into());
                }
                key::ESCAPE => {
                    g.set_show_profile_pin_entry(false);
                    g.set_profile_pin_error("".into());
                }
                digit
                    if digit.len() == 1
                        && digit.chars().next().is_some_and(|c| c.is_ascii_digit()) =>
                {
                    let d = digit.chars().next().unwrap();
                    let cursor = if d == '0' {
                        10
                    } else {
                        d.to_digit(10).unwrap() as i32 - 1
                    };
                    g.set_profile_pin_cursor(cursor);
                    g.invoke_profile_pin_key(digit.into());
                }
                _ => {}
            }
            return true;
        }
        // 2026-08-16, direct follow-up to the Back-button fix immediately
        // below ("quit it not also reacheble by keybord navigation"): the
        // on-screen Quit button had the identical gap — Ctrl+Q already
        // quits from any screen, but there was no keyboard CURSOR path
        // onto the button itself. Down from the tile row (below) sets
        // this — always reachable, unlike the conditional Back button;
        // Up returns to the tile row, Enter activates, Escape/Backspace
        // un-focuses it without quitting (quitting is a terminal action,
        // not something Escape should trigger as a side effect).
        if g.get_profile_picker_quit_focused() {
            match key {
                key::UP => g.set_profile_picker_quit_focused(false),
                key::RETURN => {
                    g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
                    g.set_profile_picker_quit_focused(false);
                    g.invoke_quit();
                }
                key::ESCAPE | key::BACKSPACE => g.set_profile_picker_quit_focused(false),
                _ => {}
            }
            return true;
        }
        // 2026-08-16, real bug ("the button shows but i cant navigate to
        // it with keybord and press enter"): the "← Back to Accounts"
        // button was mouse-only — visible and clickable, but with no
        // keyboard CURSOR path onto it at all; only the Escape/Backspace
        // shortcut below reached the same action. Handled as its own
        // focus state, mirroring the "Back button focused" convention
        // every other content-style screen in this app already
        // establishes (Detail/Season/Collection/Album/Artist: Up from the
        // top of content focuses Back, Down returns to content, Enter
        // activates) — Up from the tile row below sets this when the
        // button exists; here, Down returns to the tile row and
        // Enter/Escape/Backspace all activate it, same destination the
        // pre-existing shortcut already reaches.
        //
        // 2026-08-19, real bug ("if you was in fjord and pressed switch
        // profile you shuld go back to fjord as the same profile you
        // was"): this used to unconditionally call
        // invoke_profile_picker_back_to_accounts() — now dispatches on
        // profile-picker-back-mode ("accounts" vs "cancel"), matching
        // whichever of the two buttons is actually shown (see that
        // property's own doc comment in app_state.slint for the full bug).
        if g.get_profile_picker_back_focused() {
            match key {
                key::DOWN => g.set_profile_picker_back_focused(false),
                key::RETURN => {
                    g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
                    g.set_profile_picker_back_focused(false);
                    if g.get_profile_picker_back_mode().as_str() == "accounts" {
                        g.invoke_profile_picker_back_to_accounts();
                    } else {
                        g.invoke_profile_picker_cancel();
                    }
                }
                key::ESCAPE | key::BACKSPACE => {
                    g.set_profile_picker_back_focused(false);
                    if g.get_profile_picker_back_mode().as_str() == "accounts" {
                        g.invoke_profile_picker_back_to_accounts();
                    } else {
                        g.invoke_profile_picker_cancel();
                    }
                }
                _ => {}
            }
            return true;
        }
        // 2026-08-14, the 2-tier redesign: Escape/Backspace goes back ONE
        // level at a time — either to the account tier or by cancelling
        // straight back to a live session, per profile-picker-back-mode
        // (see its own doc comment).
        if key == key::ESCAPE || key == key::BACKSPACE {
            if g.get_profile_picker_back_mode().as_str() == "accounts" {
                g.invoke_profile_picker_back_to_accounts();
            } else {
                g.invoke_profile_picker_cancel();
            }
            return true;
        }
        // No trailing "+ Add Account" cursor slot anywhere in here
        // (2026-08-14) — this screen is always scoped to one account's own
        // profiles (plus any Bonfire-linked ones), and adding a brand-new,
        // unrelated account lives on the account tier instead.
        //
        // 2026-08-31, Bonfire Phase 5 follow-up ("but what i shuld still be
        // able to switch to a bonfire master profile with out needing to
        // switch 'accaunt'...") — 2D nav, modeled directly on Discover's
        // own landing-row pattern (discover.rs::handle_key_landing), not
        // BonfireGroupScreen's flat 1D zone list (which has no vocabulary
        // for a second axis at all): profile-picker-section picks the ROW
        // (which household has focus), profile-picker-cursor picks the
        // COLUMN within that section's own tile row.
        //
        // Left/Right stay clamped at the row's own edges — no escape to
        // Back/Quit, unlike Discover's own Left-at-column-0 escape (which
        // exists because Discover's sidebar sits physically to its left);
        // there's no analogous "thing to the left" here — Back sits above
        // the tile rows, Quit below, matching the already column-
        // independent Up/Down bindings this screen already had before this
        // change (now just scoped to "section 0"/"the last section"
        // instead of "the only row").
        let sections = g.get_profile_picker_sections();
        let section_count = sections.row_count() as i32;
        let section = g
            .get_profile_picker_section()
            .clamp(0, (section_count - 1).max(0));
        let tile_count = sections
            .row_data(section as usize)
            .map(|s| s.tiles.row_count() as i32)
            .unwrap_or(0);
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            key::UP => {
                if section == 0 {
                    g.set_profile_picker_back_focused(true);
                } else {
                    let new_section = section - 1;
                    let new_len = sections
                        .row_data(new_section as usize)
                        .map(|s| s.tiles.row_count() as i32)
                        .unwrap_or(0);
                    g.set_profile_picker_section(new_section);
                    // Preserve the current column, only pull it down if the
                    // section being landed on is shorter — Discover's own
                    // reclamp formula (`.min(...)`), not an unconditional
                    // jump to the last tile, which would discard the
                    // user's column on every vertical move.
                    g.set_profile_picker_cursor(
                        g.get_profile_picker_cursor().min((new_len - 1).max(0)),
                    );
                }
            }
            key::DOWN => {
                if section + 1 >= section_count {
                    g.set_profile_picker_quit_focused(true);
                } else {
                    let new_section = section + 1;
                    let new_len = sections
                        .row_data(new_section as usize)
                        .map(|s| s.tiles.row_count() as i32)
                        .unwrap_or(0);
                    g.set_profile_picker_section(new_section);
                    g.set_profile_picker_cursor(
                        g.get_profile_picker_cursor().min((new_len - 1).max(0)),
                    );
                }
            }
            key::LEFT => g.set_profile_picker_cursor((g.get_profile_picker_cursor() - 1).max(0)),
            key::RIGHT => g.set_profile_picker_cursor(
                (g.get_profile_picker_cursor() + 1).min((tile_count - 1).max(0)),
            ),
            key::RETURN => {
                let cursor = g.get_profile_picker_cursor();
                if let Some(sec) = sections.row_data(section as usize)
                    && let Some(t) = sec.tiles.row_data(cursor as usize)
                {
                    g.invoke_profile_picker_select(t.user_id);
                }
            }
            _ => {}
        }
        return true;
    }

    // Account picker (2026-08-14, the 2-tier account/profile redesign) —
    // same tier and shape as ProfilePickerScreen just above (checked
    // before active_mode() ever runs); no PIN sub-state at this tier at
    // all (accounts aren't PIN-protected, only profiles within them are —
    // picking a single-profile account either switches directly or opens
    // ProfilePickerScreen's own PIN modal, never one here).
    if g.get_show_account_picker() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        // 2026-08-16, same Quit-keyboard-reachability fix as the profile
        // picker's own quit-focused branch just above — see that block's
        // doc comment for the full reasoning.
        if g.get_account_picker_quit_focused() {
            match key {
                key::UP => g.set_account_picker_quit_focused(false),
                key::RETURN => {
                    g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
                    g.set_account_picker_quit_focused(false);
                    g.invoke_quit();
                }
                key::ESCAPE | key::BACKSPACE => g.set_account_picker_quit_focused(false),
                _ => {}
            }
            return true;
        }
        // 2026-08-21, real gap — see account-picker-back-focused's own doc
        // comment in app_state.slint. Same shape as the quit-focused block
        // above, and as profile_picker.slint's own back-focused dispatch:
        // Enter/Escape/Backspace all close the picker (this variant never
        // has a destination to distinguish, unlike the profile tier's own
        // "accounts" vs "cancel" split — an account picker Back always just
        // cancels), Up returns to the tile row.
        if g.get_account_picker_back_focused() {
            match key {
                key::DOWN => g.set_account_picker_back_focused(false),
                key::RETURN => {
                    g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
                    g.set_account_picker_back_focused(false);
                    g.set_show_account_picker(false);
                    window.invoke_grab_keyboard_focus();
                }
                key::ESCAPE | key::BACKSPACE => {
                    g.set_account_picker_back_focused(false);
                    g.set_show_account_picker(false);
                    window.invoke_grab_keyboard_focus();
                }
                _ => {}
            }
            return true;
        }
        if (key == key::ESCAPE || key == key::BACKSPACE) && g.get_account_picker_cancelable() {
            g.set_show_account_picker(false);
            window.invoke_grab_keyboard_focus();
            return true;
        }
        let count = g.get_account_picker_accounts().row_count() as i32; // == "+ Add Account" tile's cursor value
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            // Only reachable when the Back button actually exists — see
            // account-picker-cancelable's own doc comment for why it's
            // deliberately absent at cold startup.
            key::UP if g.get_account_picker_cancelable() => g.set_account_picker_back_focused(true),
            key::DOWN => g.set_account_picker_quit_focused(true),
            key::LEFT => g.set_account_picker_cursor((g.get_account_picker_cursor() - 1).max(0)),
            key::RIGHT => {
                g.set_account_picker_cursor((g.get_account_picker_cursor() + 1).min(count))
            }
            key::RETURN => {
                let cursor = g.get_account_picker_cursor();
                if cursor == count {
                    g.invoke_account_picker_add_account();
                } else if let Some(t) = g.get_account_picker_accounts().row_data(cursor as usize) {
                    g.invoke_account_picker_select(t.root_id);
                }
            }
            _ => {}
        }
        return true;
    }

    // Sidebar profile quick-menu (2026-08-14) — dim-backdrop overlay opened
    // from the sidebar's own profile row; same raw-key-tier shape as the
    // profile picker just above (checked before active_mode() ever runs).
    if g.get_show_sidebar_profile_menu() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        let count = g.get_sidebar_profile_menu_rows().row_count() as i32;
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            key::UP => g.set_sidebar_profile_menu_focused(
                (g.get_sidebar_profile_menu_focused() - 1).max(0),
            ),
            key::DOWN => g.set_sidebar_profile_menu_focused(
                (g.get_sidebar_profile_menu_focused() + 1).min(count - 1),
            ),
            key::RETURN => {
                g.invoke_sidebar_profile_menu_action(g.get_sidebar_profile_menu_focused())
            }
            key::ESCAPE | key::BACKSPACE => {
                g.set_show_sidebar_profile_menu(false);
                window.invoke_grab_keyboard_focus();
            }
            _ => {}
        }
        return true;
    }

    // ConnectSeerrScreen — full D-pad zone system, 2026-08-23 (was: same
    // native-LineEdit-focus shape as LoginScreen but with no zone nav at
    // all, letting typing/tabbing pass through untouched and only handling
    // Ctrl+Q/Enter-pulse/Escape). See connect_seerr.slint's own header doc
    // comment and app_state.slint's connect-seerr-zone doc comment for the
    // full design — mirrors login-zone's INLINE dispatch shape (not
    // ProfileEditScreen's delegate-to-a-separate-function one), since this
    // screen's zone count, while variable across tabs, stays small enough
    // not to need its own file. Zones -1 (close-✕) and 1 (tab row) are
    // always reachable; zone 1's Left/Right cycle connect-seerr-method
    // directly (wrapping) and clear connect-seerr-error, matching each
    // MethodTab's own mouse click handler exactly. Zones >= 2 that resolve
    // to a plain button (never a LineEdit) are always the LAST zone in
    // existing_connect_seerr_zones' own list for whichever tab is active —
    // see that function's own doc comment for why this holds across every
    // method/polling combination — so `zones.last() == Some(&zone)` is
    // enough to tell a button zone apart from an in-between text-field zone
    // with no need to also check connect-seerr-method here. Zone 0 (url-
    // input) and any in-between zone (2/3 when NOT last) are real LineEdits
    // and never actually reach this tier in practice — native focus
    // intercepts first, each field's own key-pressed hook handles its
    // Up/Down/Enter/Escape — so those fall through to `return false`, same
    // as login-zone's own zones 0-2. `zone` self-heals to `zones[0]`
    // whenever it's not actually present in the current list (2026-08-26,
    // code review — Quick Connect's own zone 2 vanishes the instant
    // qc-polling flips true, and a stale zone can also survive a screen
    // close/reopen; without this, `dispatchable` below is false for the
    // stranded zone and every key fell through to `_ => return false`,
    // leaking input to whatever's rendered behind this modal).
    //
    // Zone 0/1 numbering, fixed 2026-08-26 (real bug, live-reported: "the
    // keybord nav on seerr connect seams off it do not go where you are
    // expekting") — url-input and the tab row were originally numbered 1
    // and 0 respectively, the REVERSE of their actual visual top-to-bottom
    // order (url-field-wrap is declared, and renders, ABOVE the tab row's
    // HorizontalLayout in connect_seerr.slint). Since `next_zone`/
    // `prev_zone` walk the `zones` list purely by list position — with no
    // idea which physical screen element a given number represents —
    // Down from the tab row (list position after 0) landed on url-input,
    // which sits VISUALLY ABOVE it, and Down from url-input's own
    // key-pressed hook jumped straight past the tab row to zone 2,
    // skipping it entirely on the way back down. Renumbered so 0 = url-
    // input (topmost navigable field, right below Close) and 1 = the tab
    // row (matching visual order exactly) — see connect_seerr.slint's own
    // header doc comment for the full before/after zone map.
    if g.get_show_connect_seerr() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        let zones = crate::seerr_auth::existing_connect_seerr_zones(&g);
        let mut zone = g.get_connect_seerr_zone();
        if !zones.contains(&zone) {
            // Self-heal (code review, 2026-08-26): the previously-focused
            // zone vanished out from under us — Quick Connect's zone 2
            // ("Get Code") disappears the instant qc-polling flips true, or
            // a stale non-zero zone survived a screen reopen. Without this,
            // `dispatchable` (below) is false for a zone not in the list,
            // and every key silently hits `_ => return false`, leaking to
            // whatever's rendered behind this modal for as long as the
            // stale zone persists — a real, confirmed lockout, not
            // hypothetical (verified by tracing the exact Quick Connect
            // polling transition).
            zone = zones[0];
            g.set_connect_seerr_zone(zone);
        }
        let zone_pos = zones.iter().position(|&z| z == zone).unwrap_or(0);
        let prev_zone = || zone_pos.checked_sub(1).and_then(|i| zones.get(i)).copied();
        let next_zone = || zones.get(zone_pos + 1).copied();
        let dispatchable = zone == -1 || zone == 1 || (zone >= 2 && zones.last() == Some(&zone));
        if dispatchable && key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        // Escape always closes the whole screen, regardless of zone —
        // matches every zone's own key-pressed Escape branch in
        // connect_seerr.slint (this tier only ever sees Escape at zones
        // -1/0/a button zone; the LineEdit zones handle it themselves,
        // identically, before it can ever reach here). Also clears the
        // on-screen keyboard's 3 properties, mirroring connect_seerr.slint's
        // own close-screen() function — that gate runs before every other
        // screen's own tier, so leaving it stuck true here would silently
        // swallow all subsequent input app-wide, not just on this screen
        // (Bonfire Phase 3's original code review, Finding 1).
        if key == key::ESCAPE {
            g.set_show_connect_seerr(false);
            g.set_show_onscreen_keyboard(false);
            g.set_onscreen_keyboard_target("".into());
            g.set_onscreen_keyboard_cursor(0);
            window.invoke_grab_keyboard_focus();
            return true;
        }
        match zone {
            -1 => match key {
                key::DOWN => {
                    if let Some(n) = next_zone() {
                        g.set_connect_seerr_zone(n);
                    }
                }
                key::RETURN => {
                    g.set_show_connect_seerr(false);
                    g.set_show_onscreen_keyboard(false);
                    g.set_onscreen_keyboard_target("".into());
                    g.set_onscreen_keyboard_cursor(0);
                    window.invoke_grab_keyboard_focus();
                }
                _ => {}
            },
            1 => match key {
                key::UP => {
                    if let Some(p) = prev_zone() {
                        g.set_connect_seerr_zone(p);
                    }
                }
                key::DOWN => {
                    if let Some(n) = next_zone() {
                        g.set_connect_seerr_zone(n);
                    }
                }
                key::LEFT => {
                    let m = g.get_connect_seerr_method();
                    g.set_connect_seerr_method((m + 3) % 4);
                    g.set_connect_seerr_error("".into());
                }
                key::RIGHT => {
                    let m = g.get_connect_seerr_method();
                    g.set_connect_seerr_method((m + 1) % 4);
                    g.set_connect_seerr_error("".into());
                }
                _ => {}
            },
            z if z >= 2 && zones.last() == Some(&z) => match key {
                key::UP => {
                    if let Some(p) = prev_zone() {
                        g.set_connect_seerr_zone(p);
                    }
                }
                key::DOWN => {
                    if let Some(n) = next_zone() {
                        g.set_connect_seerr_zone(n);
                    }
                }
                // Enter has no Rust-side arm here — the actual submit/get-
                // code call needs live LineEdit.text values Rust can't read
                // directly, same "Rust can only bump kb-activate-pulse, a
                // Slint-side changed tracker does the real work" pattern
                // login-zone's own zone 4 (Connect) already uses. Handled
                // by connect_seerr.slint's own _pulse-mirror (Quick
                // Connect's Get Code) or, for the 3 text-field tabs, their
                // own local copy of it (see that file's header doc comment
                // for why each tab needs its own).
                _ => {}
            },
            _ => return false,
        }
        return true;
    }

    // "Remember this login" confirm-password modal (2026-08-17, see
    // app_state.slint's own doc comment for the full design). No grid/zone
    // nav needed — a single password field, Cancel/Confirm reachable via
    // the password LineEdit's own accepted=>/Escape.
    if g.get_show_remember_login_confirm() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if key == key::ESCAPE {
            g.invoke_remember_login_confirm_cancel();
            return true;
        }
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        return true;
    }

    // Sign Out confirmation (2026-08-22, see show-sign-out-confirm's own
    // doc comment in app_state.slint) — reachable from 3 genuinely
    // different contexts (Settings' Profiles row, the sidebar quick-menu,
    // OfflineScreen's Change Server button), so this dialog is GLOBAL
    // (main.slint top-level), not owned by any one screen. Checked here,
    // before active_mode() ever runs, so it takes priority regardless of
    // which of those 3 contexts triggered it — same shape as show-
    // remember-login-confirm right above.
    if g.get_show_sign_out_confirm() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if key == key::LEFT {
            g.set_sign_out_confirm_focused(0);
        } else if key == key::RIGHT {
            g.set_sign_out_confirm_focused(1);
        } else if key == key::RETURN {
            if g.get_sign_out_confirm_focused() == 1 {
                g.invoke_sign_out();
            }
            g.set_show_sign_out_confirm(false);
        } else if key == key::ESCAPE || key == key::BACKSPACE {
            g.set_show_sign_out_confirm(false);
        }
        return true;
    }

    // Cancel-Seerr-request confirmation (2026-08-22, see show-cancel-
    // request-confirm's own doc comment in app_state.slint) — reachable
    // from 2 screens (the Discover grid's own context menu, and
    // RequestDetailScreen's ⋮ More menu, which reuses that same overlay),
    // so this is also global, same shape as show-sign-out-confirm above.
    if g.get_show_cancel_request_confirm() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if key == key::LEFT {
            g.set_cancel_request_confirm_focused(0);
        } else if key == key::RIGHT {
            g.set_cancel_request_confirm_focused(1);
        } else if key == key::RETURN {
            if g.get_cancel_request_confirm_focused() == 1 {
                g.invoke_cancel_request_confirmed();
            }
            g.set_show_cancel_request_confirm(false);
        } else if key == key::ESCAPE || key == key::BACKSPACE {
            g.set_show_cancel_request_confirm(false);
        }
        return true;
    }

    // ManageProfilesScreen (Bonfire Phase 2, 2026-08-09).
    if g.get_show_manage_profiles() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        // Real gap, live-reported 2026-08-21 ("when in the manage profile
        // picker you cant go back without pressing escape, it has a x for
        // the mouse but cant get to it with keybord nav or dpad") — see
        // manage-profiles-close-focused's own doc comment in
        // app_state.slint. Same shape as AccountPickerScreen's own
        // quit-focused block: Enter/Escape/Backspace all close (there's no
        // "quit the app" ambiguity to worry about here, unlike a real Quit
        // button, so Escape closing is fine, not a terminal-action risk).
        if g.get_manage_profiles_close_focused() {
            match key {
                key::DOWN => g.set_manage_profiles_close_focused(false),
                key::RETURN => {
                    g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
                    g.set_manage_profiles_close_focused(false);
                    g.set_show_manage_profiles(false);
                    window.invoke_grab_keyboard_focus();
                }
                key::ESCAPE | key::BACKSPACE => {
                    g.set_manage_profiles_close_focused(false);
                    g.set_show_manage_profiles(false);
                    window.invoke_grab_keyboard_focus();
                }
                _ => {}
            }
            return true;
        }
        if key == key::ESCAPE {
            g.set_show_manage_profiles(false);
            window.invoke_grab_keyboard_focus();
            return true;
        }
        // Real bug, code-review 2026-08-16: this screen previously had no
        // keyboard navigation at all beyond Escape/Ctrl+Q — a dead end for
        // a D-pad/remote user. Mirrors AccountPickerScreen's own tile-row +
        // trailing "+" tile dispatch exactly (Left/Right cursor, Enter
        // activates); AppState.manage-profiles-cursor was already declared
        // for exactly this, just never wired.
        let list_count = g.get_manage_profiles_list().row_count() as i32;
        // The real (server-reported) cap, not a hardcoded 5 — see
        // profile_edit.rs::open_manage_profiles_screen's own doc comment on
        // manage-profiles-max-sub-profiles for why "5" alone was wrong.
        let add_shown = list_count < g.get_manage_profiles_max_sub_profiles();
        let max_cursor = if add_shown {
            list_count
        } else {
            (list_count - 1).max(0)
        };
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            // 2026-08-21, part of the close-button reachability fix above.
            key::UP => g.set_manage_profiles_close_focused(true),
            key::LEFT => g.set_manage_profiles_cursor((g.get_manage_profiles_cursor() - 1).max(0)),
            key::RIGHT => {
                g.set_manage_profiles_cursor((g.get_manage_profiles_cursor() + 1).min(max_cursor))
            }
            key::RETURN => {
                let cursor = g.get_manage_profiles_cursor();
                if add_shown && cursor == list_count {
                    g.invoke_manage_profiles_add();
                } else if let Some(t) = g.get_manage_profiles_list().row_data(cursor as usize) {
                    g.invoke_manage_profiles_select(t.user_id);
                }
            }
            _ => {}
        }
        return true;
    }
    // BonfireGroupScreen (Bonfire Phase 5, cross-household groups,
    // 2026-08-09; restructured 2026-08-29 from 3 mutually-exclusive states
    // to 2 independent, always-rendered sections — hosting and joining can
    // now both be active at once, matching Bonfire's own official UI) —
    // zone count varies with (is_owner, is_member, member count), so
    // navigation is resolved live via profile::existing_bonfire_group_zones
    // rather than a fixed enum; see that function's own doc comment for the
    // exact host/join/toggle zone-base formula this dispatch mirrors.
    if g.get_show_bonfire_group() {
        // Debug logging, 2026-08-29 — added while investigating a live
        // "can't write the join code" report; this whole tier had no
        // per-keypress trace at all, so there was no way to tell from a log
        // whether a keypress reached this screen, and if so which zone it
        // landed on (D-pad-focusing the join-code field is a separate step
        // from actually opening the on-screen keyboard for it — Enter is
        // needed for that, matching every other on-screen-keyboard consumer
        // in this app; a raw letter key typed before that is silently
        // swallowed by this tier's own unconditional `return true`).
        debug!(
            "bonfire_group: key={key:?} zone={} is_owner={} is_member={} onscreen_kb_open={}",
            g.get_bonfire_group_zone(),
            g.get_bonfire_group_is_owner(),
            g.get_bonfire_group_is_member(),
            g.get_show_onscreen_keyboard()
        );
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        // 4 ConfirmDialog gates — same "screen owns Left/Right/Confirm/Back,
        // dialog itself is keyboard-dumb" shape as ProfileEditScreen's own
        // delete-confirm gate; checked before everything else in this tier.
        if g.get_show_bonfire_kick_confirm() {
            match key {
                key::LEFT => g.set_bonfire_kick_confirm_focused(0),
                key::RIGHT => g.set_bonfire_kick_confirm_focused(1),
                key::RETURN => {
                    if g.get_bonfire_kick_confirm_focused() == 1 {
                        g.invoke_bonfire_group_kick(g.get_bonfire_kick_target_id());
                    }
                    g.set_show_bonfire_kick_confirm(false);
                }
                key::ESCAPE | key::BACKSPACE => g.set_show_bonfire_kick_confirm(false),
                _ => {}
            }
            return true;
        }
        if g.get_show_bonfire_leave_confirm() {
            match key {
                key::LEFT => g.set_bonfire_leave_confirm_focused(0),
                key::RIGHT => g.set_bonfire_leave_confirm_focused(1),
                key::RETURN => {
                    if g.get_bonfire_leave_confirm_focused() == 1 {
                        g.invoke_bonfire_group_leave();
                    }
                    g.set_show_bonfire_leave_confirm(false);
                }
                key::ESCAPE | key::BACKSPACE => g.set_show_bonfire_leave_confirm(false),
                _ => {}
            }
            return true;
        }
        if g.get_show_bonfire_delete_group_confirm() {
            match key {
                key::LEFT => g.set_bonfire_delete_group_confirm_focused(0),
                key::RIGHT => g.set_bonfire_delete_group_confirm_focused(1),
                key::RETURN => {
                    if g.get_bonfire_delete_group_confirm_focused() == 1 {
                        g.invoke_bonfire_group_delete();
                    }
                    g.set_show_bonfire_delete_group_confirm(false);
                }
                key::ESCAPE | key::BACKSPACE => g.set_show_bonfire_delete_group_confirm(false),
                _ => {}
            }
            return true;
        }
        if g.get_show_bonfire_lan_bypass_confirm() {
            match key {
                key::LEFT => g.set_bonfire_lan_bypass_confirm_focused(0),
                key::RIGHT => g.set_bonfire_lan_bypass_confirm_focused(1),
                key::RETURN => {
                    if g.get_bonfire_lan_bypass_confirm_focused() == 1 {
                        g.invoke_bonfire_group_settings_changed(
                            g.get_bonfire_group_hide_my_sub_profiles(),
                            g.get_bonfire_group_hide_others_sub_profiles(),
                            true,
                        );
                    }
                    g.set_show_bonfire_lan_bypass_confirm(false);
                }
                key::ESCAPE | key::BACKSPACE => g.set_show_bonfire_lan_bypass_confirm(false),
                _ => {}
            }
            return true;
        }

        if key == key::ESCAPE {
            g.set_show_bonfire_group(false);
            window.invoke_grab_keyboard_focus();
            return true;
        }
        // Backspace, real bug live-reported 2026-08-29 ("backspace wont
        // remove what have been typeded it will just close it"): this used
        // to be lumped in with Escape above (both unconditionally closed
        // the screen), which meant the join-code-field backspace arm added
        // for the on-screen-keyboard-disabled fix just below was dead
        // code — this check ran first and returned before that arm was
        // ever reached. Split apart: on the join-code field specifically,
        // Backspace deletes a character (a no-op on an already-empty
        // buffer, never a close — matching handle_browse_search's own
        // established "Backspace never means exit" convention for this
        // exact field shape); everywhere else in this screen it still
        // means Back, unchanged.
        // 2026-08-29 restructure: hosting and join are now two INDEPENDENT,
        // always-rendered sections rather than 3 mutually-exclusive states
        // (see existing_bonfire_group_zones' own doc comment in profile.rs
        // for the full formula and why — a real screenshot of Bonfire's own
        // official UI showed both sections together unconditionally).
        // Computed once here since both the BACKSPACE check and the
        // printable-char fallback below need `join_base` too, not just the
        // RETURN dispatch.
        let is_owner = g.get_bonfire_group_is_owner();
        let is_member = g.get_bonfire_group_is_member();
        let n_members = g.get_bonfire_group_owned_members().row_count() as i32;
        let host_count = if is_owner { n_members + 2 } else { 1 };
        let join_base = host_count;
        let join_count = if is_member { 1 } else { 2 };
        let toggle_base = join_base + join_count;

        if key == key::BACKSPACE {
            // The join-code field only exists at all while !is_member (once
            // a member, that zone is "Leave Group" instead) — dropped the
            // old `!is_owner` requirement here too, matching the RETURN
            // dispatch and the printable-char fallback below: an owner can
            // now also type a join code, exactly like the real product.
            let on_join_code_field = !is_member && g.get_bonfire_group_zone() == join_base;
            if on_join_code_field {
                if !g.get_bonfire_group_join_code().is_empty() {
                    g.invoke_bonfire_group_join_code_backspace();
                }
            } else {
                g.set_show_bonfire_group(false);
                window.invoke_grab_keyboard_focus();
            }
            return true;
        }
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }

        let zones = crate::profile::existing_bonfire_group_zones(&g);
        let zone = g.get_bonfire_group_zone();
        match key {
            // -1 (the floating "✕" button) sits above zone 0 in every
            // state, matching every other master-only screen's own
            // Back-button convention.
            key::UP => match zones.iter().position(|&z| z == zone) {
                Some(0) | None => g.set_bonfire_group_zone(-1),
                Some(pos) => g.set_bonfire_group_zone(zones[pos - 1]),
            },
            key::DOWN => {
                if zone == -1 {
                    if let Some(&first) = zones.first() {
                        g.set_bonfire_group_zone(first);
                    }
                } else if let Some(pos) = zones.iter().position(|&z| z == zone)
                    && pos + 1 < zones.len()
                {
                    g.set_bonfire_group_zone(zones[pos + 1]);
                }
            }
            key::RETURN => {
                if zone == -1 {
                    g.set_show_bonfire_group(false);
                    window.invoke_grab_keyboard_focus();
                } else if zone < host_count {
                    // ── Hosting section ──
                    if is_owner {
                        if zone == 0 {
                            // Code display — purely informational.
                        } else if zone <= n_members {
                            if let Some(m) = g
                                .get_bonfire_group_owned_members()
                                .row_data((zone - 1) as usize)
                            {
                                g.set_bonfire_kick_target_id(m.user_id);
                                g.set_bonfire_kick_target_name(m.username);
                                g.set_bonfire_kick_confirm_focused(0);
                                g.set_show_bonfire_kick_confirm(true);
                            }
                        } else {
                            // zone == host_count - 1: Delete Group.
                            g.set_bonfire_delete_group_confirm_focused(0);
                            g.set_show_bonfire_delete_group_confirm(true);
                        }
                    } else {
                        // zone == 0: Generate Join Code.
                        g.invoke_bonfire_group_generate();
                    }
                } else if zone < toggle_base {
                    // ── Join section ──
                    if is_member {
                        // zone == join_base: Leave Group.
                        g.set_bonfire_leave_confirm_focused(0);
                        g.set_show_bonfire_leave_confirm(true);
                    } else if zone == join_base {
                        if open_onscreen_keyboard(&g, "bonfire-group-join-code") {
                            window.invoke_grab_keyboard_focus();
                        }
                    } else {
                        // zone == join_base + 1: Join button.
                        g.invoke_bonfire_group_join_code_submit();
                    }
                } else {
                    // ── Toggles section — shared, shown regardless of
                    // hosting/membership state ──
                    match zone - toggle_base {
                        0 => g.invoke_bonfire_group_settings_changed(
                            !g.get_bonfire_group_hide_my_sub_profiles(),
                            g.get_bonfire_group_hide_others_sub_profiles(),
                            g.get_bonfire_group_allow_lan_bypass(),
                        ),
                        1 => g.invoke_bonfire_group_settings_changed(
                            g.get_bonfire_group_hide_my_sub_profiles(),
                            !g.get_bonfire_group_hide_others_sub_profiles(),
                            g.get_bonfire_group_allow_lan_bypass(),
                        ),
                        _ => {
                            // zone - toggle_base == 2: allow-lan-bypass.
                            if g.get_bonfire_group_allow_lan_bypass() {
                                g.invoke_bonfire_group_settings_changed(
                                    g.get_bonfire_group_hide_my_sub_profiles(),
                                    g.get_bonfire_group_hide_others_sub_profiles(),
                                    false,
                                );
                            } else {
                                g.set_bonfire_lan_bypass_confirm_focused(0);
                                g.set_show_bonfire_lan_bypass_confirm(true);
                            }
                        }
                    }
                }
            }
            // Direct physical typing into the join-code field, real gap
            // live-reported 2026-08-29 ("i have the on screen keybord
            // disabled" — "cant write thje joine code"). Unlike every other
            // hand-drawn field this app already had before the on-screen-
            // keyboard rollout (Discover/Browse/Library search — see e.g.
            // handle_browse_search's own `is_printable(k) => append` arm a
            // few hundred lines below), this field was BUILT entirely
            // within that rollout and had no independent typing path of its
            // own at all: with the setting off, Enter (above) still arms
            // show-onscreen-keyboard, but the widget never mounts and the
            // top-level onscreen-kb dispatch gate never runs (both
            // correctly also gate on settings-onscreen-keyboard-enabled),
            // so every subsequent letter fell straight into this tier's own
            // catch-all and was silently swallowed — the field was
            // completely untypeable with the on-screen keyboard disabled.
            // Fixed by adding the same direct-typing fallback those other
            // fields already have, scoped to the one zone/state it applies
            // to; RETURN's own "open the on-screen keyboard" behavior above
            // is untouched, so D-pad/on-screen-keyboard users keep that
            // path too — both now coexist, matching every sibling field.
            // (Backspace's own equivalent fallback lives in the dedicated
            // check above, not here — it needs to run before this whole
            // match, since Escape/Backspace used to be handled together as
            // a single "close the screen" case at that same earlier point.)
            // `!is_owner` was dropped from this guard the same day it was
            // added — that's precisely what made "an owner types a join
            // code" impossible, the exact gap the 2026-08-29 restructure
            // above exists to fix.
            k if is_printable(k) && !is_member && zone == join_base => {
                g.invoke_bonfire_group_join_code_append(k.into());
            }
            // Caret keys in the join-code field (2026-10-05) — this screen
            // only moves between zones with Up/Down, so Left/Right are free.
            k if !is_member
                && zone == join_base
                && caret_key(&crate::text_field::JOIN_CODE, k, &g) => {}
            k if !is_member && zone == join_base && k == key::DELETE => {
                crate::text_field::JOIN_CODE.delete(&g);
            }
            _ => {}
        }
        return true;
    }

    // ProfileEditScreen — full D-pad dispatch, 2026-08-17 (was Escape/
    // Ctrl+Q/press-pulse only; see app_state.slint's profile-edit-zone doc
    // comment for the full 12-zone design). Escape is special-cased here
    // rather than inside handle_key_profile_edit itself, since it needs to
    // distinguish "close the dropdown popup" from "cancel the whole
    // screen" — and is skipped entirely while a LineEdit holds real focus
    // (profile-edit-text-editing), since that key never reaches this tier
    // at all in that state (consumed by the field's own key-pressed(event)
    // hook in profile_edit.slint).
    if g.get_show_profile_edit() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        // Delete-profile confirmation (2026-08-21) — same "screen owns
        // Left/Right/Confirm/Back, ConfirmDialog itself is keyboard-dumb"
        // shape as dispatch_keybinding_nav's own Reset/rebind-collision
        // gates, just expressed against raw keys since this whole tier is
        // a pre-active_mode() raw-key dispatch, not an Action/KeyMap one.
        // Checked before the plain Escape-closes-screen handler below so
        // Escape/Backspace cancel the dialog first, not the whole screen.
        if g.get_show_profile_edit_delete_confirm() {
            if key == key::LEFT {
                g.set_profile_edit_delete_confirm_focused(0);
            } else if key == key::RIGHT {
                g.set_profile_edit_delete_confirm_focused(1);
            } else if key == key::RETURN {
                if g.get_profile_edit_delete_confirm_focused() == 1 {
                    g.invoke_profile_edit_delete();
                }
                g.set_show_profile_edit_delete_confirm(false);
            } else if key == key::ESCAPE || key == key::BACKSPACE {
                g.set_show_profile_edit_delete_confirm(false);
            }
            return true;
        }
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        if key == key::ESCAPE && !g.get_profile_edit_text_editing() {
            if g.get_profile_edit_dropdown_open() {
                g.set_profile_edit_dropdown_open(false);
            } else {
                g.invoke_profile_edit_cancel();
            }
            return true;
        }
        return crate::profile_edit::handle_key_profile_edit(key, &g);
    }

    // Startup connectivity gate: ConnectingScreen has nothing to focus; on
    // OfflineScreen Left/Right cycle the 3 buttons (0=Retry 1=Change Server
    // 2=Quit — a permanent failure needs a way out besides retrying forever)
    // and Enter activates the focused one, since neither screen has a native
    // widget focus path the way LoginScreen's LineEdits do. Ctrl+Q still
    // quits directly regardless of focus, matching the global Quit shortcut
    // used everywhere else — both branches below return unconditionally, so
    // without this the later global Quit pre-dispatch would never run here.
    if (g.get_show_connecting() || g.get_show_offline()) && ctrl && (key == "q" || key == "Q") {
        g.invoke_quit();
        return true;
    }
    if g.get_show_connecting() {
        return true;
    }
    if g.get_show_offline() {
        // Bump the same central press-pulse counter as the main RETURN handler
        // below (which this block returns before ever reaching) — otherwise
        // OfflineScreen's Retry/Change Server/Quit FjordButtons, which DO react
        // to kb-activate-pulse via their own built-in PressPulse, never flash on
        // keyboard Enter, only on mouse click.
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            key::LEFT => g.set_offline_focused((g.get_offline_focused() + 2) % 3),
            key::RIGHT => g.set_offline_focused((g.get_offline_focused() + 1) % 3),
            key::RETURN => match g.get_offline_focused() {
                0 => g.invoke_retry_connection(),
                // Confirmation dialog, 2026-08-22 — see show-sign-out-
                // confirm's own doc comment in app_state.slint (this is
                // one of its 3 trigger sites). Checked BEFORE this whole
                // if-show-offline block returns, so once open it stays
                // reachable regardless of show-offline's own value.
                1 => {
                    g.set_sign_out_confirm_focused(0);
                    g.set_show_sign_out_confirm(true);
                }
                _ => g.invoke_quit(),
            },
            _ => {}
        }
        return true;
    }

    // Search field text-input modes bypass the KeyMap
    if g.get_show_library() && g.get_library_header_focused() {
        return handle_library_search(key, ctrl, window);
    }
    if g.get_show_browse() && g.get_browse_header_focused() {
        return handle_browse_search(key, ctrl, window);
    }
    if g.get_active_nav() == 6 && !g.get_show_request_detail() && g.get_discover_header_focused() {
        return handle_discover_search(key, ctrl, window);
    }
    if g.get_show_playlist_picker() {
        // Same reason as the show_offline block above: this returns before the
        // main RETURN handler's kb-activate-pulse bump, so PlaylistPicker's own
        // PressPulse-driven rows (new-pulse/p-pulse in context_menu.slint) never
        // flashed on keyboard Enter.
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        return handle_playlist_picker(key, ctrl, window);
    }

    // While a detail/series page is loading (app-content-loading), block all keys except
    // Back/Escape (cancel the pending load) and Quit.
    if g.get_app_content_loading() {
        let cancel = key == key::ESCAPE || key == key::BACKSPACE;
        let quit = ctrl && (key == "q" || key == "Q");
        if cancel || quit {
            g.set_app_content_loading(false);
            g.set_app_loading_progress(0.0);
            // Clear both IDs so any still-running fetch tasks see a stale check and exit.
            g.set_detail_id("".into());
            g.set_series_id("".into());
            if quit {
                g.invoke_quit();
            }
        }
        return true; // swallow all keys during loading
    }

    // Keybinding rebind capture
    if g.get_keybinding_rebinding() {
        if key == key::ESCAPE {
            debug!("keybindings: rebind cancelled (Escape)");
            g.set_keybinding_rebinding(false);
        } else {
            let fi = g.get_keybinding_focused();
            debug!(
                "keybindings: rebind capture key={key:?} shift={shift} ctrl={ctrl} for row {fi}"
            );
            drop(g);
            rebind_action(fi, key, shift, ctrl, state, window);
        }
        return true;
    }

    // Tab in library grid mode: toggle sort bar focus
    if key == "\t" && g.get_show_library() && !g.get_library_header_focused() {
        let focused = g.get_library_sort_focused();
        g.set_library_sort_focused(!focused);
        if !focused {
            g.set_library_sort_cursor(sort_bar_init_cursor(&g));
        }
        return true;
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
    // mirroring this same active_mode()==Dashboard condition, not something
    // pushed from here — it needs to stay correct for mouse-driven screen
    // changes too, not just keyboard ones.
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

    // Ctrl+Q quits from ANY mode. Quit has no per-screen meaning, so it is
    // handled here once instead of per-mode — the old per-screen arms only
    // covered dashboard/settings/series/season/detail/person, which is why
    // q/Q never worked in the library grid, browse, player, or the music
    // screens (CR10-4 follow-up).
    if action == Some(Action::Quit) {
        crate::AppState::get(window).invoke_quit();
        return true;
    }

    // F / F11 toggles fullscreen from any non-player mode. Several focus states
    // (album/artist/collection button rows, queue panel, context menu) swallowed
    // it with catch-all arms; like Quit it has no per-screen meaning. The Player
    // arm keeps its own handling so the controls-reveal behaviour is unchanged.
    if action == Some(Action::Fullscreen) && mode != AppMode::Player {
        crate::AppState::get(window).invoke_toggle_fullscreen();
        return true;
    }

    // Global R: resume background player from any non-fullscreen, non-detail, non-overlay mode.
    // RequestDetail/RequestOptions added 2026-07-18 (real bug) — they're the
    // same class of detail/overlay screen as Person/Detail/.../Album above but
    // were missing from this list, so 'r' could yank the user into the
    // fullscreen player mid-request-flow.
    if action == Some(Action::ResumePlayer)
        && !matches!(
            mode,
            AppMode::Player
                | AppMode::Person
                | AppMode::Season
                | AppMode::Detail
                | AppMode::Artist
                | AppMode::Collection
                | AppMode::Album
                | AppMode::ContextMenu
                | AppMode::QueuePanel
                | AppMode::NowPlaying
                | AppMode::RequestDetail
                | AppMode::RequestOptions
                | AppMode::Calendar
                | AppMode::CalendarDayPopup
                | AppMode::Blocklist
                | AppMode::BonfireAdmin
        )
    {
        let g = crate::AppState::get(window);
        if g.get_has_background_player() {
            g.invoke_resume_player();
            return true;
        }
    }

    // N: focus the mini-player bar from any non-player screen.
    if action == Some(Action::FocusFloatCard)
        && mode != AppMode::Player
        && mode != AppMode::ContextMenu
    {
        let g = crate::AppState::get(window);
        if g.get_has_background_player() && !g.get_is_playing() {
            g.set_float_card_focused(0);
            return true;
        }
    }

    // q: the queue panel opens from any non-ContextMenu mode whenever audio is
    // playing OR the queue has content — a queue built while idle stays reachable
    // (Phase 56). Player mode keeps its own arm in dispatch_player.
    if action == Some(Action::OpenQueuePanel)
        && !matches!(mode, AppMode::ContextMenu | AppMode::Player)
    {
        let g = crate::AppState::get(window);
        if g.get_show_queue_panel() {
            g.set_show_queue_panel(false);
        } else if g.get_is_audio_playing() || g.get_queue_count() > 0 {
            g.invoke_refresh_queue_display();
            // Cursor: current item when one is playing, else the first row.
            g.set_queue_panel_cursor(0);
            let items = g.get_queue_items();
            for i in 0..items.row_count() {
                if let Some(e) = items.row_data(i)
                    && e.is_current
                {
                    g.set_queue_panel_cursor(i as i32);
                    break;
                }
            }
            g.set_show_queue_panel(true);
        } else {
            crate::show_toast(
                slint::ComponentHandle::as_weak(window),
                "Queue is empty".to_string(),
            );
        }
        return true;
    }

    // m: fullscreen Now Playing screen — toggles from any non-ContextMenu/Player
    // mode while audio is playing; opening resets focus to the transport row.
    if action == Some(Action::ToggleNowPlaying)
        && !matches!(mode, AppMode::ContextMenu | AppMode::Player)
    {
        let g = crate::AppState::get(window);
        if g.get_is_audio_playing() {
            if g.get_show_now_playing() {
                g.set_show_now_playing(false);
            } else {
                g.invoke_open_now_playing();
            }
        }
        return true;
    }

    // Global playlist controls when audio is playing (fire from any mode except ContextMenu).
    if mode != AppMode::ContextMenu {
        let g = crate::AppState::get(window);
        if g.get_is_audio_playing()
            && let Some(ref a) = action
        {
            match a {
                Action::PrevTrack => {
                    g.invoke_queue_prev_track();
                    return true;
                }
                Action::NextTrack => {
                    g.invoke_queue_next_track();
                    return true;
                }
                Action::ToggleShuffle => {
                    g.invoke_toggle_shuffle();
                    return true;
                }
                Action::CycleRepeat => {
                    g.invoke_cycle_repeat();
                    return true;
                }
                Action::ToggleLyrics => {
                    g.invoke_toggle_lyrics();
                    return true;
                }
                _ => {}
            }
        }
    }

    // Music bar keyboard focus: intercept nav keys when a button is focused.
    // Layout: [art (0)] | [⏸/▶ (1)] [⏹ (2)] | [timeline (3)] | [⏮ (4)] [⏭ (5)] [⇌ (6)] [↺ (7)] [⋮ (8)] [♪ (9)] [🔉 (10)] [🔊 (11)]
    //         Left zone   Centre zone            Below buttons   Right zone (9 only when lyrics-available; 10/11 always)
    // Left/Right: 0↔1↔2 → 4↔5↔6↔7↔8↔(9)↔10↔11 (skip over 3, and over 9 when lyrics unavailable); Down from any button→3; Up from 3→1.
    // RequestDetail/RequestOptions added 2026-07-18 (real bug, same class as
    // the QueuePanel/NowPlaying exclusions already here) — a stale
    // music-bar-focused >= 0 left over from earlier keyboard navigation
    // survives a mouse-driven screen switch (mouse clicks bypass handle_key
    // entirely) and would otherwise hijack this screen's own arrow keys/Enter.
    if !matches!(
        mode,
        AppMode::Player
            | AppMode::ContextMenu
            | AppMode::QueuePanel
            | AppMode::NowPlaying
            | AppMode::RequestDetail
            | AppMode::RequestOptions
            | AppMode::Calendar
            | AppMode::CalendarDayPopup
            | AppMode::Blocklist
            | AppMode::BonfireAdmin
    ) {
        let mf = crate::AppState::get(window).get_music_bar_focused();
        if mf >= 0 {
            let g = crate::AppState::get(window);
            if g.get_is_audio_playing() {
                let Some(ref action) = action else {
                    return false;
                };
                match action {
                    Action::Left => {
                        match mf {
                            3 => {
                                g.invoke_music_bar_seek_rel(-10.0);
                            }
                            4 => {
                                g.set_music_bar_focused(2);
                            } // ⏮ ← ⏹ (skip timeline)
                            // Slot 10 (🔉) steps back to 9 (♪) only when lyrics are available.
                            10 => {
                                g.set_music_bar_focused(if g.get_lyrics_available() {
                                    9
                                } else {
                                    8
                                });
                            }
                            1 | 2 | 5 | 6 | 7 | 8 | 9 | 11 => {
                                g.set_music_bar_focused(mf - 1);
                            }
                            _ => {} // 0: absorbed
                        }
                        return true;
                    }
                    Action::Right => {
                        match mf {
                            3 => {
                                g.invoke_music_bar_seek_rel(10.0);
                            }
                            2 => {
                                g.set_music_bar_focused(4);
                            } // ⏹ → ⏮ (skip timeline)
                            // Slot 8 (⋮) advances to 9 (♪) when available, else straight to 10 (🔉).
                            8 => {
                                g.set_music_bar_focused(if g.get_lyrics_available() {
                                    9
                                } else {
                                    10
                                });
                            }
                            0 | 1 | 4 | 5 | 6 | 7 | 9 | 10 => {
                                g.set_music_bar_focused(mf + 1);
                            }
                            _ => {} // 11: absorbed
                        }
                        return true;
                    }
                    Action::Down => {
                        if mf != 3 {
                            g.set_music_bar_focused(3);
                        }
                        return true;
                    }
                    Action::Up => {
                        if mf == 3 {
                            g.set_music_bar_focused(1);
                        } else {
                            g.set_music_bar_focused(-1);
                        }
                        return true;
                    }
                    Action::Confirm => {
                        match mf {
                            0 => {
                                g.invoke_open_now_playing();
                            }
                            2 => {
                                g.set_music_bar_focused(-1);
                                g.invoke_music_bar_stop();
                            }
                            4 => {
                                g.invoke_queue_prev_track();
                            }
                            5 => {
                                g.invoke_queue_next_track();
                            }
                            6 => {
                                g.invoke_toggle_shuffle();
                            }
                            7 => {
                                g.invoke_cycle_repeat();
                            }
                            8 => {
                                // ⋮ Queue button: open queue panel
                                g.invoke_refresh_queue_display();
                                let items = g.get_queue_items();
                                for i in 0..items.row_count() {
                                    if let Some(e) = items.row_data(i)
                                        && e.is_current
                                    {
                                        g.set_queue_panel_cursor(i as i32);
                                        break;
                                    }
                                }
                                g.set_show_queue_panel(true);
                            }
                            9 => {
                                g.invoke_toggle_lyrics();
                            } // ♪ Lyrics
                            10 => {
                                g.invoke_volume_down();
                            } // 🔉
                            11 => {
                                g.invoke_volume_up();
                            } // 🔊
                            _ => {
                                g.invoke_music_bar_play_pause();
                            } // 1 or 3
                        }
                        return true;
                    }
                    Action::Back => {
                        g.set_music_bar_focused(-1);
                        return true;
                    }
                    _ => {}
                }
            } else {
                crate::AppState::get(window).set_music_bar_focused(-1);
            }
        }
    }

    // Mini-player bar focused: intercept nav keys before the underlying screen sees them.
    // RequestDetail/RequestOptions added 2026-07-18 — same stale-focus-survives-
    // a-mouse-click reasoning as the music-bar block above.
    if !matches!(
        mode,
        AppMode::Player
            | AppMode::ContextMenu
            | AppMode::NowPlaying
            | AppMode::QueuePanel
            | AppMode::RequestDetail
            | AppMode::RequestOptions
            | AppMode::Calendar
            | AppMode::CalendarDayPopup
            | AppMode::Blocklist
            | AppMode::BonfireAdmin
    ) {
        let fc = crate::AppState::get(window).get_float_card_focused();
        if fc >= 0 {
            let g = crate::AppState::get(window);
            if g.get_has_background_player() && !g.get_is_playing() {
                let Some(ref action) = action else {
                    return false;
                };
                match action {
                    Action::Left | Action::Right => {
                        g.set_float_card_focused(1 - fc);
                        return true;
                    }
                    Action::Confirm => {
                        g.set_float_card_focused(-1);
                        if fc == 0 {
                            g.invoke_resume_player();
                        } else {
                            g.invoke_stop_playback();
                        }
                        return true;
                    }
                    Action::Up | Action::Back => {
                        g.set_float_card_focused(-1);
                        return true;
                    }
                    Action::Down => {
                        return true; // already at bottom, absorb
                    }
                    _ => {}
                }
            } else {
                crate::AppState::get(window).set_float_card_focused(-1);
            }
        }
    }

    // Music bar: Space/K/P pause/play during audio-only from any non-player mode.
    // PausePlay lives in the player map; look it up directly when is-audio-playing.
    if !matches!(mode, AppMode::Player | AppMode::ContextMenu) {
        let g = crate::AppState::get(window);
        if g.get_is_audio_playing() {
            let player_action = lookup_action(&state.lock().unwrap().keybindings.player, &combo);
            if let Some(Action::PausePlay) = player_action {
                g.invoke_music_bar_play_pause();
                return true;
            }
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
