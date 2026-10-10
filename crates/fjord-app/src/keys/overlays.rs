// ── fjord-app · keys/overlays.rs ─────────────────────────────────────────────
//   Raw-key handlers for overlays that show before (or on top of) any AppMode — handle_key calls
//   each one first, while its show-* flag is set:
//   handle_onscreen_keyboard_keys  the on-screen keyboard (swallows every key but Ctrl+Q while open)
//   handle_login_keys          LoginScreen zones 3-6 (Remember/Connect/Back/Quit); 0-2 are LineEdits
//   handle_profile_picker_keys 2D tile nav (section × cursor), PIN entry sub-state, Back/Quit buttons;
//                              Escape/Backspace per profile-picker-back-mode ("accounts" / "cancel")
//   handle_account_picker_keys tile row + "+ Add Account"; Escape/Backspace close only when cancelable
//   handle_sidebar_profile_menu_keys  the sidebar quick-menu rows
//   handle_connect_seerr_keys  ConnectSeerrScreen zones (✕, URL, tabs, method fields/button)
//   handle_manage_profiles_keys  tile row + "+" tile, ✕ button
//   handle_bonfire_group_keys  BonfireGroupScreen zones (profile::existing_bonfire_group_zones),
//                              join-code typing/Backspace
//   handle_offline_keys        OfflineScreen (Retry / Change Server / Quit)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// On-screen alphanumeric keyboard — checked before every screen-scoped tier: it can
// be open over any text-entry screen (Login, Profile Edit, searches, playlist name,
// Connect Seerr, join code), so it can't live inside one screen's tier. Key VALUES are
// never read here — only cursor movement, and Enter bumps kb-activate-pulse for
// QwertyKeyboard's _activate-mirror to resolve (see app_state.slint).
// Also requires settings-onscreen-keyboard-enabled: this gate swallows every key
// (except Ctrl+Q), so it must never be active for a keyboard that isn't drawn. The
// openers (open-onscreen-keyboard / open_onscreen_keyboard) refuse when it's off too.
pub(crate) fn handle_onscreen_keyboard_keys(g: &crate::AppState, key: &str, ctrl: bool) -> bool {
    // Every key reaching the gate is logged at debug, so a log shows whether a key got
    // here or a field's native focus took it first.
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
        // Physical keys still type while the keyboard is open: forwarded as a payload +
        // counter pair (see app_state.slint's onscreen-keyboard-physical-key).
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
    // Escape = the Back/Cancel button (append mode only; a first login has nothing to
    // cancel back to).
    if key == key::ESCAPE && g.get_login_append_mode() {
        g.invoke_cancel_add_account();
        return true;
    }
    // Zones 3-6 only (Remember, Connect, Back, Quit) — they hold no native focus
    // (login.slint's hooks refocus() fs when leaving zone 2). Zones 0-2 return false: the
    // LineEdit handles Tab/typing/Enter, and a `changed login-zone` tracker in login.slint
    // focuses the right field when Rust sets the zone back to 0-2.
    let zone = g.get_login_zone();
    if (3..=6).contains(&zone) {
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        let append = g.get_login_append_mode();
        // Zone 4 (Connect) has no Rust arm: do-login needs the live LineEdit texts, so Rust
        // only bumps kb-activate-pulse and login.slint's _pulse-mirror submits (like Profile
        // Edit's Save). Back (zone 5) hangs off Up from Server (handled in that field's own
        // hook) and Down returns to Server; Quit (zone 6) is Down from Connect — matching their
        // on-screen positions.
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

// ProfilePickerScreen — like the login tier, checked before active_mode() (never an
// AppMode). Raw keys, no native focus. The PIN entry sub-state captures all input
// first and follows VirtualKeyboard's 12-key row-major layout (widgets.slint), so
// keyboard and mouse agree on "cursor N".
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
        // A physical keyboard can type the PIN: digit keys add the digit (and move the
        // on-screen cursor to it, mouse-sync); Backspace deletes the last digit (Escape alone
        // closes/cancels).
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
    // Quit button focused (Down from the tile row — always there, unlike Back): Up returns,
    // Enter quits, Escape/Backspace only un-focus (quitting is never an Escape side effect).
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
    // Back button focused (Up from the tile row when the button exists), like the Back
    // button on every content screen: Down returns, Enter/Escape/Backspace activate.
    // Where it goes follows profile-picker-back-mode ("accounts" or "cancel" — see
    // app_state.slint).
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
    // Escape/Backspace go back ONE level: to the account tier, or cancel back to the live
    // session, per profile-picker-back-mode.
    if key == key::ESCAPE || key == key::BACKSPACE {
        if g.get_profile_picker_back_mode().as_str() == "accounts" {
            g.invoke_profile_picker_back_to_accounts();
        } else {
            g.invoke_profile_picker_cancel();
        }
        return true;
    }
    // No "+ Add Account" slot here (accounts are added on the account tier). 2D nav like
    // Discover's landing rows: profile-picker-section = row (household),
    // profile-picker-cursor = column. Left/Right stay clamped at the row edges (Back is
    // above the rows, Quit below — nothing to the left).
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

// Account picker — same tier and shape as the profile picker; no PIN state here
// (accounts have no PIN; a single-profile account's PIN uses the profile picker's modal).
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
    // Back focused: Enter/Escape/Backspace close the picker (always a plain cancel here),
    // Up returns to the tile row.
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

// ConnectSeerrScreen — zones from seerr_auth::existing_connect_seerr_zones (see
// app_state.slint's connect-seerr-zone), dispatched inline like login-zone. -1 = ✕ and
// 1 = the tab row are always there (Left/Right switch method, wrapping, and clear the
// error — like a tab click). A button zone is always the LAST zone of the current tab,
// so `zones.last() == Some(&zone)` identifies it. Zone 0 (URL) and in-between text
// zones are LineEdits whose own hooks handle their keys — they return false here, like
// login's 0-2. Zone order follows the visual order (URL above the tabs).
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
        // The focused zone is no longer in the list (Quick Connect's Get Code vanishes once
        // polling starts; a stale zone can survive a reopen): fall back to zones[0], or every
        // key would fall through and reach whatever is behind this modal.
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
    // Escape always closes the screen (the LineEdit zones handle it the same way in
    // connect_seerr.slint). Also clears the on-screen keyboard's state, like the screen's
    // close-screen() — a stuck keyboard flag would swallow input app-wide.
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
            // No Rust arm for Enter: submit/Get Code needs the live LineEdit texts, so Rust only
            // bumps kb-activate-pulse; connect_seerr.slint's _pulse-mirror (Get Code) or each text
            // tab's own local copy does the call.
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
    // ✕ focused (Up from the tile row): Enter/Escape/Backspace close — no quit ambiguity here.
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
    // Tile row + trailing "+" tile, like the account picker: Left/Right move, Enter activates.
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

// BonfireGroupScreen — zones vary with (is_owner, is_member, member count), so they're
// resolved live by profile::existing_bonfire_group_zones (hosting and joining are two
// independent sections); see it for the zone formula this dispatch mirrors.
pub(crate) fn handle_bonfire_group_keys(
    g: &crate::AppState,
    key: &str,
    ctrl: bool,
    window: &crate::MainWindow,
) -> bool {
    // Every key is logged at debug (zone, owner/member state), so a log shows whether a
    // key reached this screen and where it landed.
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
    // Backspace on the join-code field deletes a character (never closes — like the search
    // fields); everywhere else here it means Back. Hosting and joining are independent
    // sections; `join_base` is computed once because the Backspace check, Enter and the
    // typing fallback all need it.
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
        // Physical typing straight into the join-code field (like the search fields'
        // `is_printable` arms) — the only way to type it with the on-screen keyboard off, and it
        // works with it on too. Owners can type a join code as well (an owner may join another
        // group). Backspace is handled in the check above.
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
            // Opens the global sign-out confirmation (one of its 3 triggers). Checked before this
            // show-offline block returns, so it stays reachable.
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
