// ── fjord-app · dashboard_rows.rs ────────────────────────────────────────────
//   DASHBOARDS        the five dashboards: config key + default row numbers (indexes into
//                     AppState.<key>-rows, which hold titles and cards; must match its
//                     *-row-order defaults). The editor reads titles from those lists (title).
//   rows_for          a profile's saved rows for one dashboard merged with the defaults
//                     (unknown/duplicate ids dropped, rows missing from the save appended, shown)
//   push_row_orders   ProfileSettings → AppState.*-row-order (shown rows, in order) — called by
//                     settings::apply_profile_settings_to_window
//   apply_action      one editor action on a row list: toggle (never the last shown row) / up / down
//   wire              Settings → Interface → Dashboard rows… editor: dash-rows-open / -select-tab /
//                     -action (saves the profile, re-pushes the dashboards' orders)
//   handle_keys       raw-key tier while the editor is open (keys/mod.rs)
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use tracing::debug;

use crate::config::{DashboardRow, FjordState, ProfileSettings, save_config};
use crate::keys::key;
use crate::{AppState, DashRowItem, MainWindow, show_toast};

pub(crate) struct Dashboard {
    pub key: &'static str,
    /// Row numbers in their default order (indexes into AppState.<key>-rows, which holds the
    /// titles and cards). Movies has no row 1 (Next Up).
    pub ids: &'static [u8],
}

/// Tab order = AppState.dash-rows-tab (0 Home … 4 Music).
pub(crate) const DASHBOARDS: [Dashboard; 5] = [
    Dashboard {
        key: "home",
        ids: &[0, 1, 2, 3, 4, 5, 6],
    },
    Dashboard {
        key: "tv",
        ids: &[0, 1, 2, 3, 4, 5, 6],
    },
    Dashboard {
        key: "movies",
        ids: &[0, 2, 3, 4, 5, 6],
    },
    Dashboard {
        key: "collections",
        ids: &[0, 1],
    },
    Dashboard {
        key: "music",
        ids: &[0, 1, 2, 3],
    },
];

pub(crate) fn rows_for(cp: &ProfileSettings, dash: &Dashboard) -> Vec<DashboardRow> {
    let known = |id: u8| dash.ids.contains(&id);
    let mut out: Vec<DashboardRow> = Vec::new();
    if let Some(saved) = cp.dashboard_rows.get(dash.key) {
        for r in saved {
            if known(r.id) && !out.iter().any(|o| o.id == r.id) {
                out.push(r.clone());
            }
        }
    }
    for id in dash.ids {
        if !out.iter().any(|o| o.id == *id) {
            out.push(DashboardRow {
                id: *id,
                shown: true,
            });
        }
    }
    out
}

fn shown_order(cp: &ProfileSettings, dash: &Dashboard) -> ModelRc<i32> {
    let ids: Vec<i32> = rows_for(cp, dash)
        .into_iter()
        .filter(|r| r.shown)
        .map(|r| r.id as i32)
        .collect();
    ModelRc::new(VecModel::from(ids))
}

pub(crate) fn push_row_orders(g: &AppState<'_>, cp: &ProfileSettings) {
    g.set_home_row_order(shown_order(cp, &DASHBOARDS[0]));
    g.set_tv_row_order(shown_order(cp, &DASHBOARDS[1]));
    g.set_movies_row_order(shown_order(cp, &DASHBOARDS[2]));
    g.set_collections_row_order(shown_order(cp, &DASHBOARDS[3]));
    g.set_music_row_order(shown_order(cp, &DASHBOARDS[4]));
}

/// A row's title, from the same AppState.<key>-rows list the dashboard draws.
fn title(g: &AppState<'_>, tab: usize, id: u8) -> SharedString {
    let rows = match tab {
        0 => g.get_home_rows(),
        1 => g.get_tv_rows(),
        2 => g.get_movies_rows(),
        3 => g.get_collections_rows(),
        _ => g.get_music_rows(),
    };
    rows.row_data(id as usize)
        .map(|r| r.title)
        .unwrap_or_default()
}

fn push_editor_rows(g: &AppState<'_>, cp: &ProfileSettings) {
    let tab = g.get_dash_rows_tab().clamp(0, 4) as usize;
    let items: Vec<DashRowItem> = rows_for(cp, &DASHBOARDS[tab])
        .iter()
        .map(|r| DashRowItem {
            title: title(g, tab, r.id),
            shown: r.shown,
        })
        .collect();
    g.set_dash_rows_items(ModelRc::new(VecModel::from(items)));
}

/// One editor action on row `idx`: 0 toggles shown (refused for the last shown row — an empty
/// dashboard would look broken), 1 moves it up, 2 down. Ok(the row's new index).
pub(crate) fn apply_action(
    rows: &mut [DashboardRow],
    idx: usize,
    action: i32,
) -> Result<usize, &'static str> {
    match action {
        0 => {
            if rows[idx].shown && rows.iter().filter(|r| r.shown).count() == 1 {
                return Err("At least one row stays on the dashboard");
            }
            rows[idx].shown = !rows[idx].shown;
            Ok(idx)
        }
        1 if idx > 0 => {
            rows.swap(idx, idx - 1);
            Ok(idx - 1)
        }
        2 if idx + 1 < rows.len() => {
            rows.swap(idx, idx + 1);
            Ok(idx + 1)
        }
        _ => Ok(idx),
    }
}

pub(crate) fn wire(window: &MainWindow, state: &Arc<Mutex<FjordState>>) {
    let g = AppState::get(window);
    {
        let state = Arc::clone(state);
        let ww = window.as_weak();
        g.on_dash_rows_open(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_dash_rows_tab(0);
            g.set_dash_rows_cursor(-1);
            g.set_dash_rows_col(0);
            push_editor_rows(&g, state.lock().unwrap().config.active());
            g.set_show_dashboard_rows(true);
            debug!("dashboard rows: editor opened");
        });
    }
    {
        let state = Arc::clone(state);
        let ww = window.as_weak();
        g.on_dash_rows_select_tab(move |tab| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_dash_rows_tab(tab.clamp(0, 4));
            push_editor_rows(&g, state.lock().unwrap().config.active());
        });
    }
    {
        let state = Arc::clone(state);
        let ww = window.as_weak();
        g.on_dash_rows_action(move |row, action| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let dash = &DASHBOARDS[g.get_dash_rows_tab().clamp(0, 4) as usize];
            let (result, cfg) = {
                let mut s = state.lock().unwrap();
                let mut rows = rows_for(s.config.active(), dash);
                if row < 0 || row as usize >= rows.len() {
                    return;
                }
                let result = apply_action(&mut rows, row as usize, action);
                if result.is_ok() {
                    s.config
                        .active_mut()
                        .dashboard_rows
                        .insert(dash.key.to_string(), rows);
                }
                (result, s.config.clone())
            };
            match result {
                Err(msg) => show_toast(ww.clone(), msg.into()),
                Ok(new_idx) => {
                    save_config(&cfg);
                    push_row_orders(&g, cfg.active());
                    push_editor_rows(&g, cfg.active());
                    g.set_dash_rows_cursor(new_idx as i32);
                    debug!(
                        "dashboard rows: {} row {row} action {action} → {:?}",
                        dash.key,
                        rows_for(cfg.active(), dash)
                    );
                }
            }
        });
    }
}

/// Raw-key tier while the editor is open: Up/Down move between ✕ (-2), the tabs (-1) and the
/// rows; Left/Right switch tabs or the ✓/▲/▼ button; Enter presses; Back/Escape closes.
pub(crate) fn handle_keys(g: &AppState<'_>, k: &str, ctrl: bool, window: &MainWindow) -> bool {
    if ctrl && (k == "q" || k == "Q") {
        g.invoke_quit();
        return true;
    }
    let close = || {
        g.set_show_dashboard_rows(false);
        window.invoke_grab_keyboard_focus();
    };
    let cur = g.get_dash_rows_cursor();
    let last = g.get_dash_rows_items().row_count() as i32 - 1;
    let tab = g.get_dash_rows_tab();
    let col = g.get_dash_rows_col();
    match k {
        key::ESCAPE | key::BACKSPACE => close(),
        key::UP if cur > -2 => g.set_dash_rows_cursor(cur - 1),
        key::DOWN if cur < last => g.set_dash_rows_cursor(cur + 1),
        key::LEFT if cur == -1 && tab > 0 => g.invoke_dash_rows_select_tab(tab - 1),
        key::RIGHT if cur == -1 && tab < 4 => g.invoke_dash_rows_select_tab(tab + 1),
        key::LEFT if cur >= 0 => g.set_dash_rows_col((col - 1).max(0)),
        key::RIGHT if cur >= 0 => g.set_dash_rows_col((col + 1).min(2)),
        key::RETURN => {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
            if cur == -2 {
                close();
            } else if cur >= 0 {
                g.invoke_dash_rows_action(cur, col);
            }
        }
        _ => {}
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(id: u8, shown: bool) -> DashboardRow {
        DashboardRow { id, shown }
    }

    #[test]
    fn saved_rows_merge_with_the_defaults() {
        let mut cp = ProfileSettings::default();
        // Saved: 1 before 0, 0 hidden, an unknown id 9 and a duplicate 1.
        cp.dashboard_rows.insert(
            "collections".into(),
            vec![row(1, true), row(9, true), row(1, false), row(0, false)],
        );
        assert_eq!(
            rows_for(&cp, &DASHBOARDS[3]),
            vec![row(1, true), row(0, false)]
        );
        // Nothing saved for Music: the defaults, all shown.
        let music: Vec<u8> = rows_for(&cp, &DASHBOARDS[4]).iter().map(|r| r.id).collect();
        assert_eq!(music, vec![0, 1, 2, 3]);
        // A row missing from an old save is appended, shown.
        cp.dashboard_rows
            .insert("music".into(), vec![row(2, true), row(0, false)]);
        assert_eq!(
            rows_for(&cp, &DASHBOARDS[4]),
            vec![row(2, true), row(0, false), row(1, true), row(3, true)]
        );
    }

    #[test]
    fn actions_toggle_and_move_but_keep_one_row_shown() {
        let mut rows = vec![row(0, true), row(1, false), row(2, false)];
        assert!(
            apply_action(&mut rows, 0, 0).is_err(),
            "the last shown row stays"
        );
        assert_eq!(apply_action(&mut rows, 1, 0), Ok(1));
        assert!(rows[1].shown);
        assert_eq!(apply_action(&mut rows, 1, 1), Ok(0));
        assert_eq!(rows[0].id, 1);
        assert_eq!(
            apply_action(&mut rows, 2, 2),
            Ok(2),
            "the bottom row can't move down"
        );
        assert_eq!(
            apply_action(&mut rows, 0, 1),
            Ok(0),
            "the top row can't move up"
        );
    }
}
