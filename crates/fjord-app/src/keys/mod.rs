// ── fjord-app · keys/mod.rs ──────────────────────────────────────────────────
//   Submodules (re-exported here, so callers keep using crate::keys::*):
//     bindings   Action, KeyCombo, KeyMap/Keybindings, defaults, rebinding, keybinding rows + nav
//     mode       AppMode + active_mode()
//     overlays   raw-key handlers for the pre-AppMode overlays (login, pickers, Bonfire, Seerr…)
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
//   caret_key          Left/Right/Home/End → text_field caret for the drawn fields (Discover/Browse/
//                      Library search, new-playlist name — Right still creates when the caret is
//                      at the end — and the Bonfire join code), 2026-10-05; Delete per field
//   Settings dispatch → crate::settings (dispatch_settings, settings_row_action)
//   Per-screen key handlers live in their own modules:
//     context_menu::handle_key, series::handle_key, season::handle_key,
//     detail::handle_key, browse::handle_key,
//     discover::handle_key (Discover grid), discover::handle_key_request_detail (Seerr detail/Request)
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
mod overlays;
mod player;
mod text_input;

pub(crate) use bindings::*;
pub(crate) use dashboard::*;
pub(crate) use library::*;
pub(crate) use mode::*;
pub(crate) use overlays::*;
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

    if g.get_show_onscreen_keyboard() && g.get_settings_onscreen_keyboard_enabled() {
        return handle_onscreen_keyboard_keys(&g, key, ctrl);
    }

    if g.get_show_login() {
        return handle_login_keys(&g, key, ctrl);
    }

    if g.get_show_profile_picker() {
        return handle_profile_picker_keys(&g, key, ctrl);
    }

    if g.get_show_account_picker() {
        return handle_account_picker_keys(&g, key, ctrl, window);
    }

    if g.get_show_sidebar_profile_menu() {
        return handle_sidebar_profile_menu_keys(&g, key, ctrl, window);
    }

    if g.get_show_connect_seerr() {
        return handle_connect_seerr_keys(&g, key, ctrl, window);
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

    if g.get_show_manage_profiles() {
        return handle_manage_profiles_keys(&g, key, ctrl, window);
    }
    if g.get_show_bonfire_group() {
        return handle_bonfire_group_keys(&g, key, ctrl, window);
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
        return handle_offline_keys(&g, key);
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
