// ── fjord-app · browse.rs ────────────────────────────────────────────────────
//   populate_browse_async  filter all_movies + all_series off the UI thread
//   update_library_filter  client-side filter on AppState.library-display (loaded grid)
//   wire_browse            register AppState browse + library-search callbacks
//                          browse search: client-side filter over all_movies + all_series
//                          library search: client-side filter over already-loaded all-movies/all-series
//   handle_key             keyboard dispatch for the browse list / sidebar
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

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;
    let ci = g.get_current_item();
    match action {
        Action::Back => {
            g.set_browse_header_focused(false);
            g.set_current_item(-1);
            g.set_show_browse(false);
            g.invoke_browse_search_clear();
            if g.get_active_nav() == 3 { g.set_active_nav(0); }
            g.invoke_refocus();
            true
        }
        Action::Confirm if ci < 0 => {
            if g.get_media_items().row_count() > 0 { g.set_current_item(0); }
            true
        }
        Action::SearchJump if ci >= 0 => {
            g.set_browse_header_focused(true);
            true
        }
        Action::Up if ci < 0   => { sidebar_nav(g, -1); true }
        Action::Down if ci < 0 => { sidebar_nav(g, 1);  true }
        Action::Up if ci >= 0 => {
            if ci > 0 { g.set_current_item(ci - 1); }
            else { g.set_browse_header_focused(true); }
            true
        }
        Action::Down if ci >= 0 => {
            if ci < g.get_media_items().row_count() as i32 - 1 {
                g.set_current_item(ci + 1);
            }
            true
        }
        Action::Left if ci >= 0  => { g.set_current_item(-1); true }
        Action::Right if ci < 0  => {
            if g.get_media_items().row_count() > 0 { g.set_current_item(0); }
            true
        }
        Action::Confirm if ci >= 0 => { g.invoke_play_item(ci); true }
        Action::OpenContextMenu if ci >= 0 => { g.invoke_open_context_menu_browse(ci); true }
        _ => false,
    }
}

pub(crate) fn sidebar_nav(g: &AppState, dir: i32) {
    g.set_show_library(false);
    g.set_show_browse(false);
    g.set_settings_section(-1);
    g.set_settings_focused(-1);
    g.set_settings_dropdown_open(false);
    g.set_keybinding_focused(-1);
    let nav = g.get_active_nav();
    let next = if dir < 0 {
        match nav { 0 => 11, 11 => 10, 10 => 3, 3 => 2, 2 => 1, _ => 0 }
    } else {
        match nav { 0 => 1, 1 => 2, 2 => 3, 3 => 10, 10 => 11, _ => 0 }
    };
    g.set_active_nav(next);
    if next == 3 { g.set_show_browse(true); g.invoke_browse_search_clear(); }
    g.invoke_nav_selected(next);
}
