// ── fjord-app · browse.rs ────────────────────────────────────────────────────
//   update_library_filter  filter AppState.library-display by query string
//   wire_browse            register AppState browse + library-search callbacks
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, VecModel};

use crate::config::FjordState;
use crate::AppState;
use crate::{CardItem, MainWindow};

fn update_library_filter(w: &MainWindow, query: &str) {
    let g   = AppState::get(w);
    let nav = g.get_active_nav();
    g.set_library_query(query.into());
    let full: ModelRc<CardItem> = if nav == 1 { g.get_all_movies() } else { g.get_all_series() };
    if query.is_empty() {
        AppState::get(w).set_library_display(full);
        return;
    }
    let q = query.to_lowercase();
    let filtered: Vec<CardItem> = (0..full.row_count())
        .filter_map(|i| full.row_data(i))
        .filter(|item| item.title.to_lowercase().contains(q.as_str()))
        .collect();
    AppState::get(w).set_library_display(ModelRc::new(VecModel::from(filtered)));
}

pub(crate) fn wire_browse(window: &MainWindow, state: Arc<Mutex<FjordState>>) {
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_filter_changed(move |query| {
            let mut s = state.lock().unwrap();
            s.apply_filter(&query);
            let names = crate::display_names(&s.filtered_items);
            drop(s);
            if let Some(w) = ww.upgrade() { AppState::get(&w).set_media_items(crate::to_slint_model(names)); }
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_search_append(move |ch| {
            let Some(w) = ww.upgrade() else { return };
            let mut q = AppState::get(&w).get_library_query().to_string();
            q.push_str(ch.as_str());
            update_library_filter(&w, &q);
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_search_backspace(move || {
            let Some(w) = ww.upgrade() else { return };
            let mut q = AppState::get(&w).get_library_query().to_string();
            q.pop();
            update_library_filter(&w, &q);
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_search_clear(move || {
            let Some(w) = ww.upgrade() else { return };
            update_library_filter(&w, "");
        });
    }
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_nav_selected(move |nav| {
            let mut s = state.lock().unwrap();
            s.apply_nav(nav as usize);
            let names = crate::display_names(&s.filtered_items);
            drop(s);
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_media_items(crate::to_slint_model(names));
                AppState::get(&w).set_current_item(-1);
            }
        });
    }
}
