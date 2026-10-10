// ── fjord-app · keys/overlays.rs ─────────────────────────────────────────────
//   Raw-key handlers for overlays that show before (or on top of) any AppMode —
//   handle_key calls each one first, while its show-* flag is set:
//   handle_onscreen_keyboard_keys, handle_login_keys, handle_profile_picker_keys,
//   handle_account_picker_keys, handle_sidebar_profile_menu_keys, handle_connect_seerr_keys,
//   handle_manage_profiles_keys, handle_bonfire_group_keys, handle_offline_keys
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
//                        app_state.slint for the real bug this distinction fixes, 2026-08-19)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

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
pub(crate) fn handle_onscreen_keyboard_keys(g: &crate::AppState, key: &str, ctrl: bool) -> bool {
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
        if key.chars().count() == 1 && !c.is_control() && !('\u{E000}'..='\u{F8FF}').contains(&c) {
            g.set_onscreen_keyboard_physical_key(key.into());
            g.set_onscreen_keyboard_physical_key_seq(
                g.get_onscreen_keyboard_physical_key_seq().wrapping_add(1),
            );
        }
    }
    true
}

// LoginScreen: return false below to let LineEdit handle normal typing/
// tabbing, but Ctrl+Q must be carved out first — it would otherwise never
// reach the global Quit pre-dispatch further down, same class of bug just
// fixed for the connectivity-gate screens below.
pub(crate) fn handle_login_keys(g: &crate::AppState, key: &str, ctrl: bool) -> bool {
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
    false
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
pub(crate) fn handle_profile_picker_keys(g: &crate::AppState, key: &str, ctrl: bool) -> bool {
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
                if digit.len() == 1 && digit.chars().next().is_some_and(|c| c.is_ascii_digit()) =>
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
    true
}

// Account picker (2026-08-14, the 2-tier account/profile redesign) —
// same tier and shape as ProfilePickerScreen just above (checked
// before active_mode() ever runs); no PIN sub-state at this tier at
// all (accounts aren't PIN-protected, only profiles within them are —
// picking a single-profile account either switches directly or opens
// ProfilePickerScreen's own PIN modal, never one here).
pub(crate) fn handle_account_picker_keys(
    g: &crate::AppState,
    key: &str,
    ctrl: bool,
    window: &crate::MainWindow,
) -> bool {
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
        key::RIGHT => g.set_account_picker_cursor((g.get_account_picker_cursor() + 1).min(count)),
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
    true
}

// Sidebar profile quick-menu (2026-08-14) — dim-backdrop overlay opened
// from the sidebar's own profile row; same raw-key-tier shape as the
// profile picker just above (checked before active_mode() ever runs).
pub(crate) fn handle_sidebar_profile_menu_keys(
    g: &crate::AppState,
    key: &str,
    ctrl: bool,
    window: &crate::MainWindow,
) -> bool {
    if ctrl && (key == "q" || key == "Q") {
        g.invoke_quit();
        return true;
    }
    let count = g.get_sidebar_profile_menu_rows().row_count() as i32;
    if key == key::RETURN {
        g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
    }
    match key {
        key::UP => {
            g.set_sidebar_profile_menu_focused((g.get_sidebar_profile_menu_focused() - 1).max(0))
        }
        key::DOWN => g.set_sidebar_profile_menu_focused(
            (g.get_sidebar_profile_menu_focused() + 1).min(count - 1),
        ),
        key::RETURN => g.invoke_sidebar_profile_menu_action(g.get_sidebar_profile_menu_focused()),
        key::ESCAPE | key::BACKSPACE => {
            g.set_show_sidebar_profile_menu(false);
            window.invoke_grab_keyboard_focus();
        }
        _ => {}
    }
    true
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
pub(crate) fn handle_connect_seerr_keys(
    g: &crate::AppState,
    key: &str,
    ctrl: bool,
    window: &crate::MainWindow,
) -> bool {
    if ctrl && (key == "q" || key == "Q") {
        g.invoke_quit();
        return true;
    }
    let zones = crate::seerr_auth::existing_connect_seerr_zones(g);
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
    true
}

// ManageProfilesScreen (Bonfire Phase 2, 2026-08-09).
pub(crate) fn handle_manage_profiles_keys(
    g: &crate::AppState,
    key: &str,
    ctrl: bool,
    window: &crate::MainWindow,
) -> bool {
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
    true
}

// BonfireGroupScreen (Bonfire Phase 5, cross-household groups,
// 2026-08-09; restructured 2026-08-29 from 3 mutually-exclusive states
// to 2 independent, always-rendered sections — hosting and joining can
// now both be active at once, matching Bonfire's own official UI) —
// zone count varies with (is_owner, is_member, member count), so
// navigation is resolved live via profile::existing_bonfire_group_zones
// rather than a fixed enum; see that function's own doc comment for the
// exact host/join/toggle zone-base formula this dispatch mirrors.
pub(crate) fn handle_bonfire_group_keys(
    g: &crate::AppState,
    key: &str,
    ctrl: bool,
    window: &crate::MainWindow,
) -> bool {
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

    let zones = crate::profile::existing_bonfire_group_zones(g);
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
                    if open_onscreen_keyboard(g, "bonfire-group-join-code") {
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
        k if !is_member && zone == join_base && caret_key(&crate::text_field::JOIN_CODE, k, g) => {}
        k if !is_member && zone == join_base && k == key::DELETE => {
            crate::text_field::JOIN_CODE.delete(g);
        }
        _ => {}
    }
    true
}

pub(crate) fn handle_offline_keys(g: &crate::AppState, key: &str) -> bool {
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
    true
}
