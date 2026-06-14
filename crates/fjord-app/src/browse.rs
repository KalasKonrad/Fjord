// ── fjord-app · browse.rs ────────────────────────────────────────────────────
//   update_library_filter  client-side filter on AppState.library-display (loaded grid)
//   populate_browse        fill media-items from all_movies + all_series (optionally filtered)
//   wire_browse            register AppState browse + library-search callbacks
//                          browse search: client-side filter over all_movies + all_series
//                          library search: client-side filter over already-loaded all-movies/all-series
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, VecModel};

use crate::config::FjordState;
use crate::AppState;
use crate::{CardItem, MainWindow, display_names, to_slint_model};

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

fn populate_browse(w: &MainWindow, state: &Mutex<FjordState>, query: &str) {
    let lock = state.lock().unwrap();
    let all: Vec<_> = lock.all_movies.iter().chain(lock.all_series.iter()).cloned().collect();
    drop(lock);

    let filtered: Vec<_> = if query.is_empty() {
        all
    } else {
        let q = query.to_lowercase();
        all.into_iter().filter(|i| i.display_name().to_lowercase().contains(&q)).collect()
    };

    let names = display_names(&filtered);
    state.lock().unwrap().filtered_items = filtered;
    AppState::get(w).set_media_items(to_slint_model(names));
}

pub(crate) fn wire_browse(
    window:    &MainWindow,
    state:     Arc<Mutex<FjordState>>,
    rt_handle: tokio::runtime::Handle,
) {
    let _ = rt_handle; // no longer used; kept for call-site compatibility

    // ── Browse list: client-side filter over all_movies + all_series ─────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_filter_changed(move |query| {
            let Some(w) = ww.upgrade() else { return };
            populate_browse(&w, &state, query.as_str());
        });
    }
    // ── Browse search: keyboard-driven append / backspace / clear ────────────
    {
        let ww = window.as_weak();
        AppState::get(window).on_browse_search_append(move |ch| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let mut q = g.get_browse_query().to_string();
            q.push_str(ch.as_str());
            g.set_browse_query(q.as_str().into());
            g.invoke_filter_changed(q.as_str().into());
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_browse_search_backspace(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let mut q = g.get_browse_query().to_string();
            q.pop();
            g.set_browse_query(q.as_str().into());
            g.invoke_filter_changed(q.as_str().into());
        });
    }
    {
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        AppState::get(window).on_browse_search_clear(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_browse_query("".into());
            g.set_current_item(-1);
            populate_browse(&w, &state, "");
        });
    }
    // ── Library grid: client-side filter over loaded movies/series ───────────
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
    // ── Nav selected: clear browse results (skip when nav=3 — browse is opening) ─
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_nav_selected(move |nav| {
            if nav == 3 { return; }
            state.lock().unwrap().filtered_items.clear();
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_media_items(to_slint_model(vec![]));
                g.set_current_item(-1);
                g.set_browse_query("".into());
            }
        });
    }
}
