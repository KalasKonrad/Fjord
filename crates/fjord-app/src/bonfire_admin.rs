// ── fjord-app · bonfire_admin.rs ─────────────────────────────────────────────
//   Bonfire Admin (Settings → Profiles), gated on FjordState.jellyfin_is_server_admin (Jellyfin's
//   Policy.IsAdministrator — what the plugin's admin endpoints check, not household ownership).
//   Covers mappings / reset-pin / set-profile-limit / audit-logs; Bonfire's other admin/*
//   endpoints are out of scope (DEVLOG → Bonfire Phase 6).
//   build_admin_rows        AdminMappings' two lists (master_users/sub_profiles) → one flat list,
//                           each master followed by its sub-profiles (see its doc comment)
//   audit_entry_to_row      AuditLogEntry → BonfireAuditRow
//   open_bonfire_admin_screen  admin check (toast if not), reset state, fetch Mappings; Audit Logs
//                           load on first switch to that tab (ensure_audit_logs)
//   on_bonfire_admin_tab_selected  tab switch (mouse)
//   on_bonfire_admin_reset_pin  after the ConfirmDialog; patches the row in place
//   on_bonfire_admin_set_limit / on_bonfire_admin_cycle_limit  the limit stepper: next value from
//                           next_limit_step (LIMIT_STEPS), one path for keyboard and mouse
//   handle_key              AppMode::BonfireAdmin (Action-based — no text entry): confirm dialog
//                           first, then the tab switcher (cursor -1), then the active tab's rows;
//                           Left/Right between a row's two actions (default_col_for_row),
//                           Enter activates
//   wire_bonfire_admin      callbacks moved from main() (0.5.0 step 3): BonfireAdminScreen
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::keys::Action;
use crate::profile::avatar_color_for;
use crate::{AppState, BonfireAdminRow, BonfireAuditRow, MainWindow, show_toast};

fn ss(s: &str) -> SharedString {
    SharedString::from(s)
}

/// The profile-limit stepper's values: a small fixed range plus the -1 ("server default")
/// sentinel, so Enter cycles in one direction with no separate reset. Real caps are small (5
/// is Bonfire's default); 20 is a generous ceiling.
const LIMIT_STEPS: [i32; 21] = [
    -1, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19, 20,
];

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
        let initial = m
            .profile_name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default();
        rows.push(BonfireAdminRow {
            is_master: true,
            profile_user_id: ss(&m.profile_user_id),
            display_name: ss(&m.profile_name),
            avatar_color: avatar_color_for("", &m.profile_user_id),
            avatar_initial: ss(&initial),
            requires_pin: m.requires_pin,
            max_profiles: m.limit_override.map(|v| v as i32).unwrap_or(-1),
        });
        for sp in mappings
            .sub_profiles
            .iter()
            .filter(|sp| sp.master_user_id == m.profile_user_id)
        {
            let sp_initial = sp
                .profile_name
                .chars()
                .next()
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_default();
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
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    let (client, is_admin) = {
        let s = state.lock().unwrap();
        (s.client.clone(), s.jellyfin_is_server_admin)
    };
    if !is_admin {
        show_toast(
            window.as_weak(),
            "Only a Jellyfin server administrator can view this".to_string(),
        );
        return;
    }
    let Some(client) = client else { return };

    g.set_bonfire_admin_error(ss(""));
    g.set_bonfire_admin_back_focused(false);
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
                let msg = crate::short_error(&e);
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
fn ensure_audit_logs(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    if g.get_bonfire_admin_audit_fetched() {
        return;
    }
    let Some(client) = state.lock().unwrap().client.clone() else {
        return;
    };
    g.set_bonfire_admin_audit_fetched(true); // set eagerly — a failed fetch shouldn't retry every tab switch
    g.set_bonfire_admin_loading(true);
    let ww = window.as_weak();
    rt.spawn(async move {
        match client.bonfire_admin_audit_logs().await {
            Ok(entries) => {
                let rows: Vec<BonfireAuditRow> =
                    entries.into_iter().map(audit_entry_to_row).collect();
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
                let msg = crate::short_error(&e);
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

pub(crate) fn on_bonfire_admin_tab_selected(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    new_tab: i32,
) {
    let g = AppState::get(window);
    g.set_bonfire_admin_tab(new_tab);
    if new_tab == 1 {
        ensure_audit_logs(state, window, rt);
    }
}

pub(crate) fn on_bonfire_admin_reset_pin(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    profile_id: SharedString,
) {
    let Some(client) = state.lock().unwrap().client.clone() else {
        return;
    };
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
                            if let Some(mut row) = model.row_data(i)
                                && row.profile_user_id == profile_id_owned
                            {
                                row.requires_pin = false;
                                model.set_row_data(i, row);
                                break;
                            }
                        }
                        show_toast(w.as_weak(), "PIN reset".to_string());
                    }
                });
            }
            Err(e) => {
                warn!("bonfire_admin_reset_pin: {e:#}");
                let msg = format!("Couldn't reset that PIN — {}", crate::short_error(&e));
                show_toast(ww, msg);
            }
        }
    });
}

pub(crate) fn on_bonfire_admin_set_limit(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    user_id: SharedString,
    new_value: i32,
) {
    let Some(client) = state.lock().unwrap().client.clone() else {
        return;
    };
    let ww = window.as_weak();
    let user_id_owned = user_id.to_string();
    let max_profiles = if new_value < 0 {
        None
    } else {
        Some(new_value as u32)
    };
    rt.spawn(async move {
        match client
            .bonfire_admin_set_profile_limit(&user_id_owned, max_profiles)
            .await
        {
            Ok(()) => {
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        let model = g.get_bonfire_admin_rows();
                        for i in 0..model.row_count() {
                            if let Some(mut row) = model.row_data(i)
                                && row.profile_user_id == user_id_owned
                            {
                                row.max_profiles = new_value;
                                model.set_row_data(i, row);
                                break;
                            }
                        }
                    }
                });
            }
            Err(e) => {
                warn!("bonfire_admin_set_profile_limit: {e:#}");
                let msg = format!("Couldn't update the profile limit — {}", crate::short_error(&e));
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
pub(crate) fn on_bonfire_admin_cycle_limit(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    user_id: SharedString,
) {
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
    if g.get_bonfire_admin_tab() == 0 {
        g.get_bonfire_admin_rows().row_count() as i32
    } else {
        g.get_bonfire_admin_audit_rows().row_count() as i32
    }
}

/// The column a freshly-focused Mappings row should land on — 0 (Reset
/// PIN) when the row genuinely has a PIN to reset, else 1 (the limit
/// stepper) when it's a master (which always has one, PIN or not), else 0
/// as an arbitrary don't-care (a PIN-less sub-profile has no action at
/// all — see `handle_key`'s Confirm arm, which independently guards on
/// the row's own real state rather than trusting this value blindly).
/// Called on every row-change (Up/Down, and the initial Down from the tab
/// switcher) so the stored column always reflects something real on the
/// row it's landing on — without this, a master with no PIN set would
/// default to column 0 (Reset PIN, hidden on that row) and show no focus
/// ring anywhere at all, since the limit stepper only lights for column 1.
fn default_col_for_row(g: &AppState, idx: i32) -> i32 {
    let Some(row) = g.get_bonfire_admin_rows().row_data(idx.max(0) as usize) else {
        return 0;
    };
    if row.requires_pin {
        0
    } else if row.is_master {
        1
    } else {
        0
    }
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
                g.set_bonfire_admin_reset_confirm_focused(
                    1 - g.get_bonfire_admin_reset_confirm_focused(),
                );
                true
            }
            Action::Confirm => {
                let do_reset = g.get_bonfire_admin_reset_confirm_focused() == 1;
                g.set_show_bonfire_admin_reset_confirm(false);
                if do_reset {
                    g.invoke_bonfire_admin_reset_pin(g.get_bonfire_admin_reset_confirm_target());
                }
                true
            }
            Action::Back => {
                g.set_show_bonfire_admin_reset_confirm(false);
                true
            }
            _ => true,
        };
    }

    // "← Back" button — the topmost zone, matching blocklist.rs's own
    // blocklist-back-focused shape exactly. Up returns false (not true)
    // deliberately: this hands off to the shared focus_bar_on_up mechanism
    // so the mini-player bar above this screen is still reachable, the
    // same contract every other screen's own topmost zone honors.
    if g.get_bonfire_admin_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_bonfire_admin(false);
                true
            }
            Action::Down => {
                g.set_bonfire_admin_back_focused(false);
                true
            }
            Action::Up => false,
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
            Action::Up => {
                g.set_bonfire_admin_back_focused(true);
                true
            }
            Action::Down => {
                if active_rows_len(g) > 0 {
                    g.set_bonfire_admin_cursor(0);
                    g.set_bonfire_admin_col(default_col_for_row(g, 0));
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
                g.set_bonfire_admin_col(default_col_for_row(g, focused - 1));
            }
            true
        }
        Action::Down => {
            if focused + 1 < count {
                g.set_bonfire_admin_cursor(focused + 1);
                g.set_bonfire_admin_col(default_col_for_row(g, focused + 1));
            }
            true
        }
        // Mappings tab, only when the row has BOTH actions: a sub-profile has at most Reset PIN (if
        // it has a PIN), a PIN-less master only the limit stepper, a PIN-less sub-profile nothing
        // (Reset PIN is hidden without a PIN — it would do nothing visible).
        Action::Left | Action::Right if tab == 0 => {
            if let Some(row) = g.get_bonfire_admin_rows().row_data(focused as usize)
                && row.is_master
                && row.requires_pin
            {
                g.set_bonfire_admin_col(1 - g.get_bonfire_admin_col());
            }
            true
        }
        Action::Confirm if tab == 0 => {
            let Some(row) = g.get_bonfire_admin_rows().row_data(focused as usize) else {
                return true;
            };
            let col = g.get_bonfire_admin_col();
            // Explicit per-action guards, not just a col check — col is
            // always pre-set correctly by default_col_for_row on every
            // row-change, but guarding on the row's own real state here
            // too means Confirm is correct-by-construction even if col
            // somehow drifts, and a PIN-less sub-profile (neither guard
            // ever true) is a genuine, silent no-op rather than wrongly
            // opening a reset dialog for a profile with no PIN to reset.
            if col == 0 && row.requires_pin {
                g.set_bonfire_admin_reset_confirm_target(row.profile_user_id.clone());
                g.set_bonfire_admin_reset_confirm_name(row.display_name.clone());
                g.set_bonfire_admin_reset_confirm_focused(0);
                g.set_show_bonfire_admin_reset_confirm(true);
            } else if col == 1 && row.is_master {
                // The limit stepper cycles on Enter instead of adjusting with Left/Right: Left
                // inside the stepper would adjust the value and never move focus back to Reset PIN.
                // Left/Right always move between the row's two actions; Enter activates. Same
                // callback as the mouse (bonfire-admin-cycle-limit), so both compute the same next
                // value.
                g.invoke_bonfire_admin_cycle_limit(row.profile_user_id.clone());
            }
            // else: neither guard matched — a sub-profile with no PIN has
            // nothing at all to activate on this row. Silent no-op.
            true
        }
        Action::Back => {
            g.set_show_bonfire_admin(false);
            true
        }
        _ => true, // swallow all other keys while this screen is open (Audit Logs tab: read-only, no action)
    }
}

// ── wire_bonfire_admin (moved from main(), 0.5.0 step 3) ─────────────────
/// Wires BonfireAdminScreen: open_bonfire_admin, bonfire_admin_tab_selected,
/// bonfire_admin_reset_pin, bonfire_admin_set_limit, bonfire_admin_cycle_limit.
pub(crate) fn wire_bonfire_admin(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── Bonfire Admin (Phase 6, admin actions, 2026-09-04) ──────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_bonfire_admin(move || {
            if let Some(w) = window_weak.upgrade() {
                bonfire_admin::open_bonfire_admin_screen(&state, &w, &rt_handle);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_admin_tab_selected(move |new_tab| {
            if let Some(w) = window_weak.upgrade() {
                bonfire_admin::on_bonfire_admin_tab_selected(&state, &w, &rt_handle, new_tab);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_admin_reset_pin(move |profile_id| {
            if let Some(w) = window_weak.upgrade() {
                bonfire_admin::on_bonfire_admin_reset_pin(&state, &w, &rt_handle, profile_id);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_admin_set_limit(move |user_id, new_value| {
            if let Some(w) = window_weak.upgrade() {
                bonfire_admin::on_bonfire_admin_set_limit(
                    &state, &w, &rt_handle, user_id, new_value,
                );
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_admin_cycle_limit(move |user_id| {
            if let Some(w) = window_weak.upgrade() {
                bonfire_admin::on_bonfire_admin_cycle_limit(&state, &w, &rt_handle, user_id);
            }
        });
    }
}
