// ── fjord-app · browse.rs ────────────────────────────────────────────────────
//   populate_browse_async  filter all_movies + all_series off the UI thread
//   update_library_filter  client-side filter on AppState.library-display (loaded grid)
//   wire_browse            register AppState browse + library-search callbacks
//                          browse search: client-side filter over all_movies + all_series
//                          library search: client-side filter over already-loaded all-movies/all-series
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use slint::{ComponentHandle, Global, Model, ModelRc, VecModel};

use crate::config::FjordState;
use crate::AppState;
use crate::{CardItem, MainWindow, display_names, to_slint_model};

// Snapshot → tokio task (filter + display_names) → invoke_from_event_loop (set model).
// A generation counter discards results from superseded queries.
fn populate_browse_async(
    ww:        slint::Weak<MainWindow>,
    state:     Arc<Mutex<FjordState>>,
    query:     String,
    gen:       Arc<AtomicU64>,
    rt_handle: &tokio::runtime::Handle,
) {
    let my_gen = gen.fetch_add(1, Ordering::Relaxed) + 1;

    let all: Vec<_> = {
        let lock = state.lock().unwrap();
        lock.all_movies.iter().chain(lock.all_series.iter()).cloned().collect()
    };

    rt_handle.spawn(async move {
        let filtered: Vec<_> = if query.is_empty() {
            all
        } else {
            let q = query.to_lowercase();
            all.into_iter()
                .filter(|i| i.display_name().to_lowercase().contains(&q))
                .collect()
        };
        let names = display_names(&filtered);

        slint::invoke_from_event_loop(move || {
            if gen.load(Ordering::Relaxed) != my_gen { return; }
            state.lock().unwrap().filtered_items = filtered;
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_media_items(to_slint_model(names));
            }
        }).ok();
    });
}

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
    let browse_gen = Arc::new(AtomicU64::new(0));

    // ── Browse list: client-side filter over all_movies + all_series ─────────
    {
        let state     = Arc::clone(&state);
        let gen       = Arc::clone(&browse_gen);
        let rt        = rt_handle.clone();
        let ww        = window.as_weak();
        AppState::get(window).on_filter_changed(move |query| {
            populate_browse_async(ww.clone(), Arc::clone(&state), query.to_string(), Arc::clone(&gen), &rt);
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
        let state     = Arc::clone(&state);
        let gen       = Arc::clone(&browse_gen);
        let rt        = rt_handle.clone();
        let ww        = window.as_weak();
        AppState::get(window).on_browse_search_clear(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_browse_query("".into());
            g.set_current_item(-1);
            populate_browse_async(ww.clone(), Arc::clone(&state), String::new(), Arc::clone(&gen), &rt);
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
