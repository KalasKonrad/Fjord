// ── fjord-app · browse.rs ────────────────────────────────────────────────────
//   refresh_library_display  apply current sort + filter + query → library-display + alpha-offsets;
//                            #[track_caller] logs caller file:line (Phase 99 diagnostic)
//   build_alpha_offsets      [i32; 27] first flat-index for A-Z+# in the display model
//   pseudo_shuffle           deterministic Fisher-Yates using LCG seed
//   update_library_filter    update library-query then call refresh_library_display;
//                            #[track_caller] too (Phase 99 diagnostic)
//   populate_browse_async    filter all_movies + all_series off the UI thread
//   wire_browse              register AppState browse + library-search + sort + jump callbacks
//   handle_key               keyboard dispatch for the browse list / sidebar
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use slint::{ComponentHandle, Global, Model, ModelRc, VecModel};

use crate::config::FjordState;
use crate::AppState;
use crate::{CardItem, MainWindow, display_names, to_slint_model};

// ── Sort helpers ──────────────────────────────────────────────────────────────

fn pseudo_shuffle(items: &mut [CardItem], seed: u64) {
    let n = items.len();
    if n <= 1 { return; }
    let mut rng = seed;
    for i in (1..n).rev() {
        rng = rng.wrapping_mul(6364136223846793005u64).wrapping_add(1442695040888963407u64);
        let j = (rng >> 33) as usize % (i + 1);
        items.swap(i, j);
    }
}

// Returns a 27-element Vec: index 0=#, 1=A..26=Z.
// Value = flat item index of the first title starting with that letter/symbol; -1 if none.
pub(crate) fn build_alpha_offsets(model: &ModelRc<CardItem>) -> Vec<i32> {
    let mut offsets = vec![-1i32; 27];
    for i in 0..model.row_count() {
        let card  = model.row_data(i).unwrap();
        let first = card.title.to_lowercase().chars().next().unwrap_or(' ');
        let bucket: usize = if first.is_ascii_alphabetic() {
            (first as u8 - b'a') as usize + 1  // A=1..Z=26
        } else {
            0  // # = non-alpha/numeric, at top
        };
        if offsets[bucket] < 0 { offsets[bucket] = i as i32; }
    }
    offsets
}

// ── Core refresh ─────────────────────────────────────────────────────────────

/// Rebuild library-display from current sort/filter/query and update alpha offsets.
/// Must be called on the UI thread.
/// `#[track_caller]`: the diagnostic log below needs to know which of the ~15 call
/// sites triggered a given refresh, to trace an intermittent post-open flash that
/// isn't explained by any single obviously-guilty caller (investigation ongoing).
#[track_caller]
pub(crate) fn refresh_library_display(w: &MainWindow) {
    let g     = AppState::get(w);
    let nav   = g.get_active_nav();
    let sort  = g.get_library_sort();
    let fw    = g.get_library_filter_unwatched();
    let ff    = g.get_library_filter_favorites();
    let query = g.get_library_query().to_string();

    let source: ModelRc<CardItem> = match nav {
        2 => g.get_all_movies(),
        1 => g.get_all_series(),
        3 => g.get_all_collections(),
        4 => match g.get_library_music_view() {
            1 => g.get_all_albums(),
            2 => g.get_all_playlists(),
            _ => g.get_all_artists(),
        },
        _ => g.get_all_series(),
    };
    if source.row_count() == 0 {
        // Nothing loaded yet — set empty alpha offsets and bail.
        g.set_library_alpha_offsets(ModelRc::new(VecModel::from(vec![-1i32; 27])));
        return;
    }

    let mut items: Vec<CardItem> = (0..source.row_count())
        .filter_map(|i| source.row_data(i))
        .collect();

    // Filters (not applicable for Collections or Artists)
    if nav != 3 && nav != 4 {
        if fw { items.retain(|c| !c.has_played); }
        if ff { items.retain(|c| c.is_favorite); }
    }

    // Sort
    match sort {
        0 => items.sort_by(|a, b| a.title.as_str().to_lowercase().cmp(&b.title.as_str().to_lowercase())),
        1 => items.sort_by(|a, b| b.title.as_str().to_lowercase().cmp(&a.title.as_str().to_lowercase())),
        2 => items.sort_by(|a, b| b.year.cmp(&a.year).then(a.title.as_str().to_lowercase().cmp(&b.title.as_str().to_lowercase()))),
        3 => items.sort_by(|a, b| a.year.cmp(&b.year).then(a.title.as_str().to_lowercase().cmp(&b.title.as_str().to_lowercase()))),
        4 => {
            let seed = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64)
                .unwrap_or(42);
            pseudo_shuffle(&mut items, seed);
        }
        _ => {}
    }

    // Search query on top of sort
    let final_items: Vec<CardItem> = if query.is_empty() {
        items
    } else {
        let q = query.to_lowercase();
        items.into_iter().filter(|c| c.title.as_str().to_lowercase().contains(q.as_str())).collect()
    };

    // Apply preserving identity (Phase 97): this function runs unconditionally on
    // every grid-open, every network-fetch-landing, and every poster-decode
    // completion — a plain rebuild here would flash every card even after
    // apply_cards_preserving_identity already correctly preserved all_movies/etc
    // underneath, since library-display (not all_movies) is what the grid actually
    // renders. Only genuinely different content/order (e.g. sort==4's Shuffle,
    // where a fresh random order is the point) falls back to a real rebuild.
    let caller = std::panic::Location::caller();
    tracing::debug!(
        "refresh_library_display[nav={nav} sort={sort}]: applying {} card(s), called from {}:{}",
        final_items.len(), caller.file(), caller.line()
    );
    let display = crate::apply_cards_preserving_identity(&g.get_library_display(), final_items);

    // Alpha offsets: only meaningful for Name A-Z sort with no active query/filter
    let alpha = if sort == 0 && query.is_empty() && !fw && !ff {
        build_alpha_offsets(&display)
    } else {
        vec![-1i32; 27]
    };

    g.set_library_display(display);
    g.set_library_alpha_offsets(ModelRc::new(VecModel::from(alpha)));
}

#[track_caller]
fn update_library_filter(w: &MainWindow, query: &str) {
    let caller = std::panic::Location::caller();
    tracing::debug!("update_library_filter: query={query:?}, called from {}:{}", caller.file(), caller.line());
    AppState::get(w).set_library_query(query.into());
    refresh_library_display(w);
}

// ── Browse async populate ─────────────────────────────────────────────────────

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

// ── Wire callbacks ────────────────────────────────────────────────────────────

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
    // ── Library sort: apply new sort/filter, persist to Config ───────────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_library_sort_apply(move |sort, fw, ff| {
            let Some(w) = ww.upgrade() else { return };
            let g   = AppState::get(&w);
            let nav = g.get_active_nav();
            g.set_library_sort(sort);
            g.set_library_filter_unwatched(fw);
            g.set_library_filter_favorites(ff);
            g.set_library_focused(0);
            g.set_library_focused_row(0);
            {
                let mut s = state.lock().unwrap();
                match nav {
                    2 => s.config.library_movies_sort      = sort.clamp(0, 4) as u8,
                    1 => s.config.library_series_sort      = sort.clamp(0, 4) as u8,
                    3 => s.config.library_collections_sort = sort.clamp(0, 4) as u8,
                    4 => match g.get_library_music_view() {
                        1 => s.config.library_albums_sort    = sort.clamp(0, 4) as u8,
                        2 => s.config.library_playlists_sort = sort.clamp(0, 4) as u8,
                        _ => s.config.library_artists_sort   = sort.clamp(0, 4) as u8,
                    },
                    _ => {}
                }
                crate::config::save_config(&s.config);
            }
            refresh_library_display(&w);
        });
    }
    // ── Library alpha-jump: set focused card to first item for that letter ────
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_jump_to_letter(move |letter_idx| {
            let Some(w) = ww.upgrade() else { return };
            let g       = AppState::get(&w);
            let cols    = g.get_library_cols();
            let offsets = g.get_library_alpha_offsets();
            if let Some(flat_idx) = offsets.row_data(letter_idx as usize) {
                if flat_idx >= 0 {
                    g.set_library_focused(flat_idx);
                    g.set_library_focused_row(flat_idx / cols);
                }
            }
        });
    }
    // ── Library grid scroll: update scrubber cursor to reflect visible letter ─
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_grid_scrolled(move |top_card| {
            let Some(w) = ww.upgrade() else { return };
            let g       = AppState::get(&w);
            let offsets = g.get_library_alpha_offsets();
            let mut letter = 0i32;
            for i in 0..27usize {
                if let Some(off) = offsets.row_data(i) {
                    if off >= 0 && off <= top_card { letter = i as i32; }
                }
            }
            g.set_library_scrubber_cursor(letter);
        });
    }
    // ── Nav selected: clear browse results (skip when nav=5 — browse is opening) ─
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_nav_selected(move |nav| {
            if nav == 5 { return; }
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
            if g.get_active_nav() == 5 { g.set_active_nav(0); }
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
                true
            } else {
                false // at last item — let focus_bar_on_down handle it
            }
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

    // Up from the topmost sidebar item: focus the mini-player bar when it is visible.
    if dir < 0 && nav == 0 && g.get_has_background_player() && !g.get_is_playing() {
        g.set_float_card_focused(0);
        return;
    }

    let next = if dir < 0 {
        match nav { 0 => 11, 11 => 10, 10 => 5, 5 => 4, 4 => 3, 3 => 2, 2 => 1, _ => 0 }
    } else {
        match nav { 0 => 1, 1 => 2, 2 => 3, 3 => 4, 4 => 5, 5 => 10, 10 => 11, _ => 0 }
    };
    g.set_active_nav(next);
    if next == 5 { g.set_show_browse(true); g.invoke_browse_search_clear(); }
    g.invoke_nav_selected(next);
}
