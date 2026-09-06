// ── fjord-app · bonfire_admin.rs ─────────────────────────────────────────────
//   Bonfire Phase 6 (admin actions, 2026-09-04) — Settings -> Profiles -> "Bonfire Admin",
//   gated on the genuinely-new FjordState.jellyfin_is_server_admin flag (Jellyfin's own core
//   Policy.IsAdministrator, verified directly against the real plugin controller source — NOT
//   Bonfire's household-master concept). Scoped to just the 4 already-modeled admin methods
//   (mappings/reset-pin/set-profile-limit/audit-logs); the other 14 real admin/* endpoints
//   this same investigation turned up (session control, config export/import, avatar-folder
//   scanning, a "panic code" duress feature, global plugin settings) are explicitly out of
//   scope for this pass — see CLAUDE.md's own dated section.
//   build_admin_rows        merges AdminMappings' two separate lists (master_users/sub_profiles)
//                           into one flat, grouped Vec<BonfireAdminRow> — see its own doc
//                           comment for why this is a real merge, not a naive concatenation,
//                           and why the whole screen is a flat list rather than a
//                           lazily-expanding tree
//   open_bonfire_admin_screen  gates on jellyfin_is_server_admin (toast+return if not — same
//                           authority-check shape as open_bonfire_group_screen/
//                           open_manage_profiles_screen), resets transient state, fetches
//                           Mappings (the default tab) — Audit Logs are fetched lazily, only
//                           on first switch to that tab (see handle_key's Left/Right arm)
//   on_bonfire_admin_reset_pin  ConfirmDialog-gated (a real, if easily-reversed, destructive
//                           action — forcibly clears someone's PIN); patches the row in place
//                           on success rather than re-fetching the whole list
//   on_bonfire_admin_set_limit  applies one exact profile-limit value (patches the row, toasts
//                           on failure) — the shared write path callers below funnel through
//   on_bonfire_admin_cycle_limit  the profile-limit "stepper"'s single source of truth for
//                           "what's the next value" (next_limit_step off the row's current
//                           one) — reached identically by keyboard Confirm (handle_key) and a
//                           mouse click on the control, so the two can never disagree; see
//                           handle_key's own doc comment for why this is Enter/click-cycles-
//                           through-values, not Left/Right-adjusts-a-value (a genuine D-pad
//                           dead end for col 0 -> col 1 -> "how do I get back")
//   handle_key              genuine AppMode::BonfireAdmin dispatch (Action-based, like
//                           blocklist.rs — this screen has no text entry at all, so it doesn't
//                           need BonfireGroupScreen's raw-key-tier shape): the reset-confirm
//                           dialog intercepts first, then the tab switcher (cursor == -1,
//                           the same sentinel every other "Up from row 0" zone in this app
//                           uses), then Up/Down/Left/Right/Confirm within the active tab's
//                           own flat row list
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::keys::Action;
use crate::profile::avatar_color_for;
use crate::{show_toast, AppState, BonfireAdminRow, BonfireAuditRow, MainWindow};

fn ss(s: &str) -> SharedString { SharedString::from(s) }

/// Profile-limit stepper's own value sequence — a small fixed range plus
/// the -1 ("use the server's own default") sentinel at both ends, so Enter
/// cycles through it in one direction with no separate "reset" control
/// needed. Real per-master caps observed in this project's own prior
/// Bonfire research have all been small (5 is the plugin's own documented
/// default), so 20 is a generous ceiling, not a tight one.
const LIMIT_STEPS: [i32; 21] = [-1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20];

fn next_limit_step(current: i32) -> i32 {
    let idx = LIMIT_STEPS.iter().position(|&v| v == current).unwrap_or(0);
    LIMIT_STEPS[(idx + 1) % LIMIT_STEPS.len()]
}

/// Merges `AdminMappings`' two separate wire-format lists into one flat,
/// grouped `Vec<BonfireAdminRow>` — a real merge, not a naive
/// concatenation. The response returns `master_users` and `sub_profiles`
/// as two independent lists (the latter carrying its own `master_user_id`
/// linking it back to its master); simply appending one after the other
/// would put every master first and every sub-profile last, not grouped
/// under its own master the way the UI actually wants. Real-world counts
/// are small enough that a plain `iter().filter(...)` per master (rather
/// than building an index/HashMap first) is perfectly fine.
fn build_admin_rows(mappings: &fjord_api::models::AdminMappings) -> Vec<BonfireAdminRow> {
    let mut rows = Vec::new();
    for m in &mappings.master_users {
        let initial = m.profile_name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
        rows.push(BonfireAdminRow {
            is_master: true,
            profile_user_id: ss(&m.profile_user_id),
            display_name: ss(&m.profile_name),
            avatar_color: avatar_color_for("", &m.profile_user_id),
            avatar_initial: ss(&initial),
            requires_pin: m.requires_pin,
            max_profiles: m.limit_override.map(|v| v as i32).unwrap_or(-1),
        });
        for sp in mappings.sub_profiles.iter().filter(|sp| sp.master_user_id == m.profile_user_id) {
            let sp_initial = sp.profile_name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default();
            rows.push(BonfireAdminRow {
                is_master: false,
                profile_user_id: ss(&sp.profile_user_id),
                display_name: ss(&sp.profile_name),
                avatar_color: avatar_color_for("", &sp.profile_user_id),
                avatar_initial: ss(&sp_initial),
                requires_pin: sp.requires_pin,
                max_profiles: -1, // meaningless on a sub-profile row
            });
        }
    }
    rows
}

fn audit_entry_to_row(e: fjord_api::models::AuditLogEntry) -> BonfireAuditRow {
    BonfireAuditRow {
        timestamp: ss(&e.timestamp),
        master_username: ss(&e.master_username),
        target_username: ss(&e.target_username),
        device_name: ss(&e.device_name),
        client: ss(&e.client),
        ip_address: ss(&e.ip_address),
    }
}

pub(crate) fn open_bonfire_admin_screen(
    state:  &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt:     &tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    let (client, is_admin) = {
        let s = state.lock().unwrap();
        (s.client.clone(), s.jellyfin_is_server_admin)
    };
    if !is_admin {
        show_toast(window.as_weak(), "Only a Jellyfin server administrator can view this".to_string());
        return;
    }
    let Some(client) = client else { return };

    g.set_bonfire_admin_error(ss(""));
    g.set_bonfire_admin_tab(0);
    g.set_bonfire_admin_cursor(-1);
    g.set_bonfire_admin_col(0);
    g.set_bonfire_admin_audit_fetched(false);
    g.set_bonfire_admin_audit_rows(ModelRc::new(VecModel::from(Vec::<BonfireAuditRow>::new())));
    g.set_show_bonfire_admin_reset_confirm(false);
    g.set_bonfire_admin_loading(true);
    g.set_show_bonfire_admin(true);
    window.invoke_grab_keyboard_focus();

    let ww = window.as_weak();
    rt.spawn(async move {
        match client.bonfire_admin_mappings().await {
            Ok(mappings) => {
                let rows = build_admin_rows(&mappings);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_bonfire_admin_rows(ModelRc::new(VecModel::from(rows)));
                        g.set_bonfire_admin_loading(false);
                    }
                });
            }
            Err(e) => {
                warn!("bonfire_admin_mappings: {e:#}");
                let msg = format!("{e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_bonfire_admin_error(ss(&msg));
                        g.set_bonfire_admin_loading(false);
                    }
                });
            }
        }
    });
}

/// Fetched lazily, only on first switch to the Audit Logs tab — skips the
/// extra request for the common case of someone who only ever checks
/// Mappings. The real endpoint has no pagination at all (returns
/// everything, sorted newest-first, server-side) — cached for the rest of
/// this screen-open's lifetime once fetched, not re-fetched on every tab
/// switch back to it.
fn ensure_audit_logs(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    let g = AppState::get(window);
    if g.get_bonfire_admin_audit_fetched() { return; }
    let Some(client) = state.lock().unwrap().client.clone() else { return };
    g.set_bonfire_admin_audit_fetched(true); // set eagerly — a failed fetch shouldn't retry every tab switch
    g.set_bonfire_admin_loading(true);
    let ww = window.as_weak();
    rt.spawn(async move {
        match client.bonfire_admin_audit_logs().await {
            Ok(entries) => {
                let rows: Vec<BonfireAuditRow> = entries.into_iter().map(audit_entry_to_row).collect();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_bonfire_admin_audit_rows(ModelRc::new(VecModel::from(rows)));
                        g.set_bonfire_admin_loading(false);
                    }
                });
            }
            Err(e) => {
                warn!("bonfire_admin_audit_logs: {e:#}");
                let msg = format!("{e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_bonfire_admin_error(ss(&msg));
                        g.set_bonfire_admin_loading(false);
                    }
                });
            }
        }
    });
}

pub(crate) fn on_bonfire_admin_tab_selected(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle, new_tab: i32) {
    let g = AppState::get(window);
    g.set_bonfire_admin_tab(new_tab);
    if new_tab == 1 { ensure_audit_logs(state, window, rt); }
}

pub(crate) fn on_bonfire_admin_reset_pin(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle, profile_id: SharedString) {
    let Some(client) = state.lock().unwrap().client.clone() else { return };
    let ww = window.as_weak();
    let profile_id_owned = profile_id.to_string();
    rt.spawn(async move {
        match client.bonfire_admin_reset_pin(&profile_id_owned).await {
            Ok(()) => {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        let model = g.get_bonfire_admin_rows();
                        for i in 0..model.row_count() {
                            if let Some(mut row) = model.row_data(i) {
                                if row.profile_user_id == profile_id_owned {
                                    row.requires_pin = false;
                                    model.set_row_data(i, row);
                                    break;
                                }
                            }
                        }
                        show_toast(w.as_weak(), "PIN reset".to_string());
                    }
                });
            }
            Err(e) => {
                warn!("bonfire_admin_reset_pin: {e:#}");
                let msg = format!("Couldn't reset that PIN: {e:#}");
                show_toast(ww, msg);
            }
        }
    });
}

pub(crate) fn on_bonfire_admin_set_limit(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle, user_id: SharedString, new_value: i32) {
    let Some(client) = state.lock().unwrap().client.clone() else { return };
    let ww = window.as_weak();
    let user_id_owned = user_id.to_string();
    let max_profiles = if new_value < 0 { None } else { Some(new_value as u32) };
    rt.spawn(async move {
        match client.bonfire_admin_set_profile_limit(&user_id_owned, max_profiles).await {
            Ok(()) => {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        let model = g.get_bonfire_admin_rows();
                        for i in 0..model.row_count() {
                            if let Some(mut row) = model.row_data(i) {
                                if row.profile_user_id == user_id_owned {
                                    row.max_profiles = new_value;
                                    model.set_row_data(i, row);
                                    break;
                                }
                            }
                        }
                    }
                });
            }
            Err(e) => {
                warn!("bonfire_admin_set_profile_limit: {e:#}");
                let msg = format!("Couldn't update the profile limit: {e:#}");
                show_toast(ww, msg);
            }
        }
    });
}

/// Looks up the row's CURRENT `max_profiles` and applies `next_limit_step`
/// to it — the one place that computation happens, reached identically by
/// a keyboard Confirm (`handle_key`'s col-1 arm) and a mouse click on the
/// stepper control (`bonfire_admin.slint`), so the two input paths can
/// never disagree on what "next" means for a given row.
pub(crate) fn on_bonfire_admin_cycle_limit(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle, user_id: SharedString) {
    let g = AppState::get(window);
    let model = g.get_bonfire_admin_rows();
    let current = (0..model.row_count())
        .filter_map(|i| model.row_data(i))
        .find(|row| row.profile_user_id == user_id)
        .map(|row| row.max_profiles)
        .unwrap_or(-1);
    on_bonfire_admin_set_limit(state, window, rt, user_id, next_limit_step(current));
}

fn active_rows_len(g: &AppState) -> i32 {
    if g.get_bonfire_admin_tab() == 0 { g.get_bonfire_admin_rows().row_count() as i32 }
    else { g.get_bonfire_admin_audit_rows().row_count() as i32 }
}

/// Dispatched via `AppMode::BonfireAdmin` in `keys.rs`'s own `match mode`
/// (a genuine mode, not a raw-key pre-tier — this screen has no text
/// entry at all, unlike BonfireGroupScreen/ProfileEditScreen, so it
/// doesn't need that shape; matches `blocklist::handle_key`'s own 2-
/// parameter signature exactly, since that dispatch site only ever has
/// `window`/`action`/`g` in scope, not `state`/`rt`). Any action that
/// needs actual network access (tab-switching's lazy audit-log fetch,
/// Reset PIN, Set Limit) is routed through a Slint callback instead of
/// called directly from here — the same decoupling blocklist.rs's own
/// Confirm/DeleteItem arm already uses for its own remove action, with
/// the real work living in `on_bonfire_admin_*` functions registered
/// against those callbacks in main.rs (which DOES have `state`/`rt`).
pub(crate) fn handle_key(action: &Action, g: &AppState) -> bool {
    if g.get_show_bonfire_admin_reset_confirm() {
        return match action {
            Action::Left | Action::Right => {
                g.set_bonfire_admin_reset_confirm_focused(1 - g.get_bonfire_admin_reset_confirm_focused());
                true
            }
            Action::Confirm => {
                let do_reset = g.get_bonfire_admin_reset_confirm_focused() == 1;
                g.set_show_bonfire_admin_reset_confirm(false);
                if do_reset { g.invoke_bonfire_admin_reset_pin(g.get_bonfire_admin_reset_confirm_target()); }
                true
            }
            Action::Back => {
                g.set_show_bonfire_admin_reset_confirm(false);
                true
            }
            _ => true,
        };
    }

    // Tab switcher — cursor == -1, the same sentinel every other "Up from
    // row 0" zone in this app uses.
    if g.get_bonfire_admin_cursor() < 0 {
        return match action {
            Action::Left | Action::Right => {
                g.invoke_bonfire_admin_tab_selected(1 - g.get_bonfire_admin_tab());
                true
            }
            Action::Down => {
                if active_rows_len(g) > 0 {
                    g.set_bonfire_admin_cursor(0);
                    g.set_bonfire_admin_col(0);
                }
                true
            }
            Action::Back => {
                g.set_show_bonfire_admin(false);
                true
            }
            _ => true,
        };
    }

    let count = active_rows_len(g);
    let focused = g.get_bonfire_admin_cursor().clamp(0, (count - 1).max(0));
    let tab = g.get_bonfire_admin_tab();

    match action {
        Action::Up => {
            if focused == 0 {
                g.set_bonfire_admin_cursor(-1);
            } else {
                g.set_bonfire_admin_cursor(focused - 1);
                g.set_bonfire_admin_col(0);
            }
            true
        }
        Action::Down => {
            if focused + 1 < count {
                g.set_bonfire_admin_cursor(focused + 1);
                g.set_bonfire_admin_col(0);
            }
            true
        }
        // Mappings tab only, and only meaningful on a master row (a
        // sub-profile has just the one action, Reset PIN — Left/Right is a
        // no-op there, matching how this app's other single-action rows
        // already behave).
        Action::Left | Action::Right if tab == 0 => {
            if let Some(row) = g.get_bonfire_admin_rows().row_data(focused as usize) {
                if row.is_master {
                    g.set_bonfire_admin_col(1 - g.get_bonfire_admin_col());
                }
            }
            true
        }
        Action::Confirm if tab == 0 => {
            let Some(row) = g.get_bonfire_admin_rows().row_data(focused as usize) else { return true };
            let col = if row.is_master { g.get_bonfire_admin_col() } else { 0 };
            if col == 0 {
                g.set_bonfire_admin_reset_confirm_target(row.profile_user_id.clone());
                g.set_bonfire_admin_reset_confirm_name(row.display_name.clone());
                g.set_bonfire_admin_reset_confirm_focused(0);
                g.set_show_bonfire_admin_reset_confirm(true);
            } else {
                // The profile-limit "stepper" — deliberately an
                // Enter-cycles-through-values control, not a Left/Right-
                // adjusts-a-value one. The latter was the original design
                // but turned out to be a genuine D-pad dead end: once
                // Left/Right moved focus INTO the stepper column, there
                // was no way for Left to ever move focus back OUT to
                // Reset PIN again, since Left there would be consumed by
                // the value-adjust instead. Enter avoids the ambiguity
                // entirely — Left/Right always just move between the 2
                // actions on this row, and Enter always means "activate
                // whatever's focused," consistent for both columns.
                // Routed through the same callback a mouse click on this
                // control uses (bonfire-admin-cycle-limit), so keyboard and
                // mouse can never compute a different "next" value.
                g.invoke_bonfire_admin_cycle_limit(row.profile_user_id.clone());
            }
            true
        }
        Action::Back => {
            g.set_show_bonfire_admin(false);
            true
        }
        _ => true, // swallow all other keys while this screen is open (Audit Logs tab: read-only, no action)
    }
}
