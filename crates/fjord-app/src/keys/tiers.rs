// ── fjord-app · keys/tiers.rs ────────────────────────────────────────────────
//   The raw-key tiers handle_key checks first, before any key-map lookup:
//   RAW_TIERS                 (name, is it open, its key handler), top to bottom — the first open
//                             tier owns the key and its handler's result is handle_key's. Order is
//                             priority: on-screen keyboard, login/pickers, overlays and modal
//                             confirms, profile edit, the startup gate, search fields, the playlist
//                             picker, the loading guard, rebind capture, the library sort-bar Tab
//   RawKey                    one key press as the tiers see it (key, Shift, Ctrl)
//   confirm_dialog_keys       the global ConfirmDialogs (Sign Out, Cancel Request, Delete Profile):
//                             Left/Right pick, Enter confirms the focused button, Back closes
//   remember_login_confirm_keys / profile_edit_keys / playlist_picker_keys / loading_keys /
//   rebind_keys / library_tab_keys  the tiers that aren't a one-line call into overlays.rs etc.
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// One raw key press, as the raw-key tiers see it (before any key-map lookup).
#[derive(Clone, Copy)]
pub(crate) struct RawKey<'a> {
    pub key: &'a str,
    pub shift: bool,
    pub ctrl: bool,
}

type TierOpen = fn(&crate::AppState<'_>, &RawKey<'_>) -> bool;
type TierKeys =
    fn(&crate::AppState<'_>, &RawKey<'_>, &crate::MainWindow, &Arc<Mutex<FjordState>>) -> bool;

/// Checked top to bottom; the first open tier owns the key.
pub(crate) const RAW_TIERS: &[(&str, TierOpen, TierKeys)] = &[
    // First: the keyboard can be open over any text-entry screen; only Ctrl+Q passes.
    (
        "on-screen keyboard",
        |g, _| g.get_show_onscreen_keyboard() && g.get_settings_onscreen_keyboard_enabled(),
        |g, k, _, _| handle_onscreen_keyboard_keys(g, k.key, k.ctrl),
    ),
    (
        "login",
        |g, _| g.get_show_login(),
        |g, k, _, _| handle_login_keys(g, k.key, k.ctrl),
    ),
    (
        "profile picker",
        |g, _| g.get_show_profile_picker(),
        |g, k, _, _| handle_profile_picker_keys(g, k.key, k.ctrl),
    ),
    (
        "account picker",
        |g, _| g.get_show_account_picker(),
        |g, k, w, _| handle_account_picker_keys(g, k.key, k.ctrl, w),
    ),
    (
        "sidebar profile menu",
        |g, _| g.get_show_sidebar_profile_menu(),
        |g, k, w, _| handle_sidebar_profile_menu_keys(g, k.key, k.ctrl, w),
    ),
    (
        "Connect Seerr",
        |g, _| g.get_show_connect_seerr(),
        |g, k, w, _| handle_connect_seerr_keys(g, k.key, k.ctrl, w),
    ),
    (
        "remember-login confirm",
        |g, _| g.get_show_remember_login_confirm(),
        |g, k, _, _| remember_login_confirm_keys(g, k),
    ),
    // The global confirms (main.slint) win wherever they were opened from.
    (
        "Sign Out confirm",
        |g, _| g.get_show_sign_out_confirm(),
        |g, k, _, _| {
            confirm_dialog_keys(
                g,
                k,
                |g| g.get_sign_out_confirm_focused(),
                |g, v| g.set_sign_out_confirm_focused(v),
                |g| g.invoke_sign_out(),
                |g| g.set_show_sign_out_confirm(false),
            )
        },
    ),
    (
        "Cancel Request confirm",
        |g, _| g.get_show_cancel_request_confirm(),
        |g, k, _, _| {
            confirm_dialog_keys(
                g,
                k,
                |g| g.get_cancel_request_confirm_focused(),
                |g, v| g.set_cancel_request_confirm_focused(v),
                |g| g.invoke_cancel_request_confirmed(),
                |g| g.set_show_cancel_request_confirm(false),
            )
        },
    ),
    (
        "Manage Profiles",
        |g, _| g.get_show_manage_profiles(),
        |g, k, w, _| handle_manage_profiles_keys(g, k.key, k.ctrl, w),
    ),
    (
        "Dashboard rows editor",
        |g, _| g.get_show_dashboard_rows(),
        |g, k, w, _| crate::dashboard_rows::handle_keys(g, k.key, k.ctrl, w),
    ),
    (
        "Bonfire group",
        |g, _| g.get_show_bonfire_group(),
        |g, k, w, _| handle_bonfire_group_keys(g, k.key, k.ctrl, w),
    ),
    (
        "profile edit",
        |g, _| g.get_show_profile_edit(),
        |g, k, _, _| profile_edit_keys(g, k),
    ),
    // Startup connectivity gate: Ctrl+Q quits from both screens (each tier below returns
    // unconditionally, so the global Quit would never run); Connecting has nothing to focus.
    (
        "quit from the startup gate",
        |g, k| (g.get_show_connecting() || g.get_show_offline()) && k.ctrl && is_quit_key(k.key),
        |g, _, _, _| {
            g.invoke_quit();
            true
        },
    ),
    (
        "connecting",
        |g, _| g.get_show_connecting(),
        |_, _, _, _| true,
    ),
    (
        "offline",
        |g, _| g.get_show_offline(),
        |g, k, _, _| handle_offline_keys(g, k.key),
    ),
    // Search fields: typing bypasses the key map.
    (
        "library search",
        |g, _| g.get_show_library() && g.get_library_header_focused(),
        |_, k, w, _| handle_library_search(k.key, k.ctrl, w),
    ),
    (
        "Browse All search",
        |g, _| g.get_show_browse() && g.get_browse_header_focused(),
        |_, k, w, _| handle_browse_search(k.key, k.ctrl, w),
    ),
    (
        "Discover search",
        |g, _| {
            g.get_active_nav() == 6
                && !g.get_show_request_detail()
                && g.get_discover_header_focused()
        },
        |_, k, w, _| handle_discover_search(k.key, k.ctrl, w),
    ),
    (
        "playlist picker",
        |g, _| g.get_show_playlist_picker(),
        |g, k, w, _| playlist_picker_keys(g, k, w),
    ),
    (
        "loading",
        |g, _| g.get_app_content_loading(),
        |g, k, _, _| loading_keys(g, k),
    ),
    (
        "rebind capture",
        |g, _| g.get_keybinding_rebinding(),
        |g, k, w, s| rebind_keys(g, k, w, s),
    ),
    (
        "library sort-bar Tab",
        |g, k| k.key == "\t" && g.get_show_library() && !g.get_library_header_focused(),
        |g, _, _, _| library_tab_keys(g),
    ),
];

fn is_quit_key(key: &str) -> bool {
    key == "q" || key == "Q"
}

/// A ConfirmDialog with no key handling of its own (Cancel = 0, Confirm = 1): Ctrl+Q quits,
/// Left/Right pick, Enter runs `confirm` when Confirm is focused and closes, Escape/Backspace close.
fn confirm_dialog_keys(
    g: &crate::AppState<'_>,
    k: &RawKey<'_>,
    focused: fn(&crate::AppState<'_>) -> i32,
    set_focused: fn(&crate::AppState<'_>, i32),
    confirm: fn(&crate::AppState<'_>),
    close: fn(&crate::AppState<'_>),
) -> bool {
    if k.ctrl && is_quit_key(k.key) {
        g.invoke_quit();
        return true;
    }
    if k.key == key::LEFT {
        set_focused(g, 0);
    } else if k.key == key::RIGHT {
        set_focused(g, 1);
    } else if k.key == key::RETURN {
        if focused(g) == 1 {
            confirm(g);
        }
        close(g);
    } else if k.key == key::ESCAPE || k.key == key::BACKSPACE {
        close(g);
    }
    true
}

/// "Remember this login" confirm-password modal: no zones — one password field; Cancel/Confirm
/// through its accepted=>/Escape.
fn remember_login_confirm_keys(g: &crate::AppState<'_>, k: &RawKey<'_>) -> bool {
    if k.ctrl && is_quit_key(k.key) {
        g.invoke_quit();
        return true;
    }
    if k.key == key::ESCAPE {
        g.invoke_remember_login_confirm_cancel();
        return true;
    }
    if k.key == key::RETURN {
        g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
    }
    true
}

/// ProfileEditScreen — full D-pad dispatch (zones: profile-edit-zone in app_state.slint). Escape is
/// handled here (it must tell "close the dropdown" from "cancel the screen") and skipped while a
/// LineEdit has focus (profile-edit-text-editing) — the field's own key-pressed consumes it then.
fn profile_edit_keys(g: &crate::AppState<'_>, k: &RawKey<'_>) -> bool {
    if k.ctrl && is_quit_key(k.key) {
        g.invoke_quit();
        return true;
    }
    // Delete-profile confirmation: checked before Escape-closes-screen, so Escape/Backspace cancel
    // the dialog first.
    if g.get_show_profile_edit_delete_confirm() {
        return confirm_dialog_keys(
            g,
            k,
            |g| g.get_profile_edit_delete_confirm_focused(),
            |g, v| g.set_profile_edit_delete_confirm_focused(v),
            |g| g.invoke_profile_edit_delete(),
            |g| g.set_show_profile_edit_delete_confirm(false),
        );
    }
    if k.key == key::RETURN {
        g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
    }
    if k.key == key::ESCAPE && !g.get_profile_edit_text_editing() {
        if g.get_profile_edit_dropdown_open() {
            g.set_profile_edit_dropdown_open(false);
        } else {
            g.invoke_profile_edit_cancel();
        }
        return true;
    }
    crate::profile_edit::handle_key_profile_edit(k.key, g)
}

/// The playlist picker returns before handle_key's own Enter pulse, so it bumps it itself — its
/// PressPulse rows (new-pulse/p-pulse in context_menu.slint) would never flash on Enter otherwise.
fn playlist_picker_keys(g: &crate::AppState<'_>, k: &RawKey<'_>, w: &crate::MainWindow) -> bool {
    if k.key == key::RETURN {
        g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
    }
    handle_playlist_picker(k.key, k.ctrl, w)
}

/// While a detail/series page is loading (app-content-loading), every key is swallowed except
/// Back/Escape (cancel the pending load) and Quit.
fn loading_keys(g: &crate::AppState<'_>, k: &RawKey<'_>) -> bool {
    let cancel = k.key == key::ESCAPE || k.key == key::BACKSPACE;
    let quit = k.ctrl && is_quit_key(k.key);
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
    true
}

/// Key-binding rebind capture: Escape cancels, any other key becomes the binding.
fn rebind_keys(
    g: &crate::AppState<'_>,
    k: &RawKey<'_>,
    w: &crate::MainWindow,
    state: &Arc<Mutex<FjordState>>,
) -> bool {
    if k.key == key::ESCAPE {
        debug!("keybindings: rebind cancelled (Escape)");
        g.set_keybinding_rebinding(false);
    } else {
        let fi = g.get_keybinding_focused();
        debug!(
            "keybindings: rebind capture key={:?} shift={} ctrl={} for row {fi}",
            k.key, k.shift, k.ctrl
        );
        rebind_action(fi, k.key, k.shift, k.ctrl, state, w);
    }
    true
}

/// Tab in the library grid: toggle the sort bar's focus.
fn library_tab_keys(g: &crate::AppState<'_>) -> bool {
    let focused = g.get_library_sort_focused();
    g.set_library_sort_focused(!focused);
    if !focused {
        g.set_library_sort_cursor(sort_bar_init_cursor(g));
    }
    true
}
