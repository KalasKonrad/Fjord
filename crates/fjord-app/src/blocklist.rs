// ── fjord-app · blocklist.rs ─────────────────────────────────────────────────
//   open_blocklist_screen  fetches page 1 (take=25,skip=0) via SeerrClient::get_blocklist,
//                          builds BlocklistRow rows directly (no two-phase Send dance needed —
//                          BlocklistItem carries no image field, unlike CardItem), shows the
//                          screen, re-grabs keyboard focus (Settings-launched overlay
//                          convention — Settings rows are a different Slint focus scope)
//   load_more_blocklist    skip += BLOCKLIST_PAGE_SIZE, appends via VecModel::extend (true
//                          incremental append, same downcast-and-extend idiom this session's
//                          own Discover search-flash fix established) rather than a fresh
//                          ModelRc swap; no-ops when already loading or no more pages exist
//                          (per blocklist_total_results, the true count from page_info.results)
//   remove_blocklist_row   resolves tmdb_id/media_type from the row at `index`, calls
//                          discover::discover_toggle_blocklist(...,adding:false) (reuses its
//                          all-model availability patching for free), removes the row from the
//                          local list, toasts
//   handle_key             Up/Down navigate rows; Up at row 0 focuses Back; Confirm/DeleteItem
//                          remove the focused row; Back closes; Down past the last row triggers
//                          load-more
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::{debug, warn};

use crate::config::FjordState;
use crate::discover::{discover_toggle_blocklist, format_date_pretty, handle_seerr_error};
use crate::keys::Action;
use crate::{show_toast, AppState, BlocklistRow, MainWindow};

const BLOCKLIST_PAGE_SIZE: u32 = 25;

fn blocklisted_by_pretty(item: &fjord_seerr::BlocklistItem) -> String {
    item.user.as_ref().map(|u| u.label()).unwrap_or_default()
}

fn blocklisted_at_pretty(item: &fjord_seerr::BlocklistItem) -> String {
    item.created_at.as_deref().map(|s| format_date_pretty(&s[..s.len().min(10)])).unwrap_or_default()
}

fn item_to_row(item: fjord_seerr::BlocklistItem) -> BlocklistRow {
    BlocklistRow {
        tmdb_id: item.tmdb_id as i32,
        media_type: item.media_type.as_str().into(),
        title: item.title.clone().unwrap_or_else(|| format!("tmdb #{}", item.tmdb_id)).into(),
        blocklisted_by: blocklisted_by_pretty(&item).into(),
        blocklisted_at: blocklisted_at_pretty(&item).into(),
    }
}

pub(crate) fn open_blocklist_screen(state: Arc<Mutex<FjordState>>, ww: slint::Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    {
        let mut s = state.lock().unwrap();
        s.blocklist_skip = 0;
        s.blocklist_total_results = 0;
        s.blocklist_loading_more = false;
    }
    let _ = slint::invoke_from_event_loop({
        let ww = ww.clone();
        move || {
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_blocklist_items(ModelRc::new(VecModel::from(Vec::<BlocklistRow>::new())));
                g.set_blocklist_focused(0);
                g.set_blocklist_back_focused(false);
                g.set_blocklist_loading_more(false);
                g.set_show_blocklist(true);
                w.invoke_grab_keyboard_focus();
            }
        }
    });

    rt.spawn(async move {
        match client.get_blocklist(BLOCKLIST_PAGE_SIZE, 0).await {
            Ok(resp) => {
                debug!("seerr: get_blocklist page 1 -> {} of {} total", resp.results.len(), resp.page_info.results);
                {
                    let mut s = state.lock().unwrap();
                    s.blocklist_skip = resp.results.len() as u32;
                    s.blocklist_total_results = resp.page_info.results;
                }
                let rows: Vec<BlocklistRow> = resp.results.into_iter().map(item_to_row).collect();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_blocklist_items(ModelRc::new(VecModel::from(rows)));
                    }
                });
            }
            Err(e) => {
                warn!("seerr: get_blocklist failed: {e:#}");
                handle_seerr_error(&state, &ww, is_session_auth, "Couldn't load blocklist", &e);
            }
        }
    });
}

pub(crate) fn load_more_blocklist(state: Arc<Mutex<FjordState>>, ww: slint::Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let (client, skip) = {
        let mut s = state.lock().unwrap();
        if s.blocklist_loading_more { return; }
        if s.blocklist_skip >= s.blocklist_total_results && s.blocklist_total_results > 0 { return; }
        let Some(client) = s.seerr_client.clone() else { return };
        s.blocklist_loading_more = true;
        (client, s.blocklist_skip)
    };
    let is_session_auth = client.is_session_auth();
    let _ = slint::invoke_from_event_loop({
        let ww = ww.clone();
        move || {
            if let Some(w) = ww.upgrade() { AppState::get(&w).set_blocklist_loading_more(true); }
        }
    });

    rt.spawn(async move {
        let result = client.get_blocklist(BLOCKLIST_PAGE_SIZE, skip).await;
        state.lock().unwrap().blocklist_loading_more = false;
        match result {
            Ok(resp) => {
                debug!("seerr: get_blocklist skip={skip} -> {} more, {} total", resp.results.len(), resp.page_info.results);
                {
                    let mut s = state.lock().unwrap();
                    s.blocklist_skip = skip + resp.results.len() as u32;
                    s.blocklist_total_results = resp.page_info.results;
                }
                let rows: Vec<BlocklistRow> = resp.results.into_iter().map(item_to_row).collect();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_blocklist_loading_more(false);
                        let existing = g.get_blocklist_items();
                        // True incremental append (same reasoning as this
                        // session's Discover search-flash fix): the Manage
                        // Blocklist list is always constructed as a
                        // VecModel elsewhere in this file, so downcasting
                        // back to it and extending appends onto the SAME
                        // live model instance — no already-shown row is
                        // torn down/reconstructed just to add a few more.
                        if let Some(vm) = existing.as_any().downcast_ref::<VecModel<BlocklistRow>>() {
                            vm.extend(rows);
                        } else {
                            let mut all: Vec<BlocklistRow> = (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();
                            all.extend(rows);
                            g.set_blocklist_items(ModelRc::new(VecModel::from(all)));
                        }
                    }
                });
            }
            Err(e) => {
                warn!("seerr: get_blocklist (load more) failed: {e:#}");
                let _ = slint::invoke_from_event_loop({
                    let ww = ww.clone();
                    move || {
                        if let Some(w) = ww.upgrade() { AppState::get(&w).set_blocklist_loading_more(false); }
                    }
                });
                handle_seerr_error(&state, &ww, is_session_auth, "Couldn't load more of the blocklist", &e);
            }
        }
    });
}

pub(crate) fn remove_blocklist_row(state: Arc<Mutex<FjordState>>, ww: slint::Weak<MainWindow>, rt: tokio::runtime::Handle, index: i32) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    let model = g.get_blocklist_items();
    let Some(row) = model.row_data(index as usize) else { return };
    let tmdb_id = row.tmdb_id as i64;
    let media_type = row.media_type.to_string();
    let title = row.title.to_string();
    // Removes locally immediately (this screen's own list is the only
    // model showing this row — unlike the Discover-card family,
    // discover_toggle_blocklist's own patch below has nothing here to
    // find), then dispatches the real DELETE.
    if let Some(vm) = model.as_any().downcast_ref::<VecModel<BlocklistRow>>() {
        vm.remove(index as usize);
    } else {
        // Defensive fallback — this model is always constructed as a
        // VecModel by open_blocklist_screen/load_more_blocklist above, so
        // this should never actually trigger.
        let kept: Vec<BlocklistRow> =
            (0..model.row_count()).filter(|&i| i != index as usize).filter_map(|i| model.row_data(i)).collect();
        g.set_blocklist_items(ModelRc::new(VecModel::from(kept)));
    }
    {
        let mut s = state.lock().unwrap();
        s.blocklist_total_results = s.blocklist_total_results.saturating_sub(1);
        s.blocklist_skip = s.blocklist_skip.saturating_sub(1);
    }
    discover_toggle_blocklist(state, ww, rt, tmdb_id, media_type, title, false);
}

pub(crate) fn handle_key(action: &Action, g: &AppState) -> bool {
    if g.get_blocklist_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_blocklist(false);
                true
            }
            Action::Down => {
                if g.get_blocklist_items().row_count() > 0 {
                    g.set_blocklist_back_focused(false);
                    g.set_blocklist_focused(0);
                }
                true
            }
            Action::Up => false, // let focus_bar_on_up handle the mini-player bar
            _ => true,
        };
    }

    let count = g.get_blocklist_items().row_count() as i32;
    let focused = g.get_blocklist_focused().clamp(0, (count - 1).max(0));
    match action {
        Action::Up => {
            if focused == 0 {
                g.set_blocklist_back_focused(true);
            } else {
                g.set_blocklist_focused(focused - 1);
            }
            true
        }
        Action::Down => {
            if focused + 1 < count {
                g.set_blocklist_focused(focused + 1);
            } else {
                g.invoke_blocklist_load_more();
            }
            true
        }
        Action::Confirm | Action::DeleteItem => {
            if count > 0 {
                g.invoke_blocklist_remove_item(focused);
            }
            true
        }
        Action::Back => {
            g.set_show_blocklist(false);
            true
        }
        _ => true, // swallow all other keys while this screen is open
    }
}
