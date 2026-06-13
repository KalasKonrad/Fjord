// ── fjord-app · browse.rs ────────────────────────────────────────────────────
//   update_library_filter  client-side filter on AppState.library-display (loaded grid)
//   wire_browse            register AppState browse + library-search callbacks
//                          browse search: server-side GET /Items?searchTerm= with 300 ms debounce
//                          library search: client-side filter over already-loaded all-movies/all-series
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};
use std::time::Duration;

use slint::{ComponentHandle, Global, Model, ModelRc, VecModel};
use tracing::warn;

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

pub(crate) fn wire_browse(
    window:    &MainWindow,
    state:     Arc<Mutex<FjordState>>,
    rt_handle: tokio::runtime::Handle,
) {
    // ── Browse list: server-side search with 300 ms debounce ─────────────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_filter_changed(move |query| {
            let query = query.to_string();
            let client = state.lock().unwrap().client.as_ref().map(Arc::clone);
            let Some(client) = client else { return };

            state.lock().unwrap().text_query = query.clone();

            if query.is_empty() {
                state.lock().unwrap().filtered_items.clear();
                if let Some(w) = ww.upgrade() {
                    AppState::get(&w).set_media_items(to_slint_model(vec![]));
                }
                return;
            }

            let state2 = Arc::clone(&state);
            let ww2    = ww.clone();
            rt_handle.spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                if state2.lock().unwrap().text_query != query { return; }

                match client.search_items(&query, 100).await {
                    Ok(items) => {
                        let names = display_names(&items);
                        state2.lock().unwrap().filtered_items = items;
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww2.upgrade() {
                                AppState::get(&w).set_media_items(to_slint_model(names));
                            }
                        });
                    }
                    Err(e) => warn!("search_items: {:#}", e),
                }
            });
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
    // ── Nav selected: clear browse results ───────────────────────────────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_nav_selected(move |_nav| {
            state.lock().unwrap().text_query.clear();
            state.lock().unwrap().filtered_items.clear();
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_media_items(to_slint_model(vec![]));
                AppState::get(&w).set_current_item(-1);
            }
        });
    }
}
