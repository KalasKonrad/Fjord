// ── fjord-app · browse.rs ────────────────────────────────────────────────────
//   refresh_library_display  current sort + filter + query → library-display (preserving
//                            identity) + alpha-offsets; #[track_caller] logs the caller
//   build_alpha_offsets      [i32; 27] first flat index for A–Z + # in the display model
//   pseudo_shuffle           deterministic Fisher-Yates with an LCG seed
//   update_library_filter    set library-query, then refresh_library_display (#[track_caller])
//   populate_browse_async    filter all_movies + all_series off the UI thread
//   wire_browse              browse + library-search + sort + jump callbacks (edits at the caret
//                            via text_field.rs); Browse All built once per session
//                            (browse_populated), first build debounced
//   clear_browse_results     on leaving Browse All (from discover's on_nav_selected): rebuild only
//                            after a search, reset the search field focus
//   handle_key               keyboard dispatch for the browse list / sidebar
//   sidebar_nav              sidebar Up/Down cycle (Discover only with Seerr enabled; Profile
//                            always, before Settings)
//   wire_play_item           callbacks moved from main() (0.5.0 step 3): play from the Browse list
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use slint::{ComponentHandle, Global, Model, ModelRc, VecModel};
use tracing::debug;

use crate::AppState;
use crate::config::FjordState;
use crate::{CardItem, MainWindow, display_names, to_slint_model};

// ── Sort helpers ──────────────────────────────────────────────────────────────

fn pseudo_shuffle(items: &mut [CardItem], seed: u64) {
    let n = items.len();
    if n <= 1 {
        return;
    }
    let mut rng = seed;
    for i in (1..n).rev() {
        rng = rng
            .wrapping_mul(6364136223846793005u64)
            .wrapping_add(1442695040888963407u64);
        let j = (rng >> 33) as usize % (i + 1);
        items.swap(i, j);
    }
}

// Returns a 27-element Vec: index 0=#, 1=A..26=Z.
// Value = flat item index of the first title starting with that letter/symbol; -1 if none.
pub(crate) fn build_alpha_offsets(model: &ModelRc<CardItem>) -> Vec<i32> {
    let mut offsets = vec![-1i32; 27];
    for i in 0..model.row_count() {
        let card = model.row_data(i).unwrap();
        let first = card.title.to_lowercase().chars().next().unwrap_or(' ');
        let bucket: usize = if first.is_ascii_alphabetic() {
            (first as u8 - b'a') as usize + 1 // A=1..Z=26
        } else {
            0 // # = non-alpha/numeric, at top
        };
        if offsets[bucket] < 0 {
            offsets[bucket] = i as i32;
        }
    }
    offsets
}

// ── Core refresh ─────────────────────────────────────────────────────────────

/// Rebuild library-display from current sort/filter/query and update alpha offsets.
/// Must be called on the UI thread.
/// `#[track_caller]`: the diagnostic log below names which of the ~15 call sites triggered a
/// refresh (an intermittent post-open flash is still being traced).
#[track_caller]
pub(crate) fn refresh_library_display(w: &MainWindow) {
    let g = AppState::get(w);
    let nav = g.get_active_nav();
    let sort = g.get_library_sort();
    let fw = g.get_library_filter_unwatched();
    let ff = g.get_library_filter_favorites();
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
        if fw {
            items.retain(|c| !c.has_played);
        }
        if ff {
            items.retain(|c| c.is_favorite);
        }
    }

    // Sort
    match sort {
        0 => items.sort_by_key(|a| a.title.as_str().to_lowercase()),
        1 => items.sort_by_key(|b| std::cmp::Reverse(b.title.as_str().to_lowercase())),
        2 => items.sort_by(|a, b| {
            b.year.cmp(&a.year).then(
                a.title
                    .as_str()
                    .to_lowercase()
                    .cmp(&b.title.as_str().to_lowercase()),
            )
        }),
        3 => items.sort_by(|a, b| {
            a.year.cmp(&b.year).then(
                a.title
                    .as_str()
                    .to_lowercase()
                    .cmp(&b.title.as_str().to_lowercase()),
            )
        }),
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
        items
            .into_iter()
            .filter(|c| c.title.as_str().to_lowercase().contains(q.as_str()))
            .collect()
    };

    // Applied preserving identity: this runs on every grid open, fetch landing and poster decode,
    // and library-display (what the grid renders) would otherwise flash every card. Only really
    // different content/order (e.g. Shuffle) rebuilds.
    let caller = std::panic::Location::caller();
    tracing::debug!(
        "refresh_library_display[nav={nav} sort={sort}]: applying {} card(s), called from {}:{}",
        final_items.len(),
        caller.file(),
        caller.line()
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
    tracing::debug!(
        "update_library_filter: query={query:?}, called from {}:{}",
        caller.file(),
        caller.line()
    );
    AppState::get(w).set_library_query(query.into());
    refresh_library_display(w);
}

// ── Browse async populate ─────────────────────────────────────────────────────

// Snapshot → tokio task (filter + display_names) → invoke_from_event_loop (set model).
// A generation counter discards results from superseded queries.
fn populate_browse_async(
    ww: slint::Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    query: String,
    generation: Arc<AtomicU64>,
    rt_handle: &tokio::runtime::Handle,
) {
    let my_gen = generation.fetch_add(1, Ordering::Relaxed) + 1;
    let started = std::time::Instant::now();

    let all: Vec<_> = {
        let lock = state.lock().unwrap();
        lock.all_movies
            .iter()
            .chain(lock.all_series.iter())
            .cloned()
            .collect()
    };
    debug!(
        "populate_browse_async: generation={my_gen} starting with {} source item(s) (query={query:?})",
        all.len()
    );

    let is_full_list = query.is_empty();
    rt_handle.spawn(async move {
        let filtered: Vec<_> = if is_full_list {
            all
        } else {
            let q = query.to_lowercase();
            all.into_iter()
                .filter(|i| i.display_name().to_lowercase().contains(&q))
                .collect()
        };
        let names = display_names(&filtered);

        slint::invoke_from_event_loop(move || {
            debug!("populate_browse_async: generation={my_gen} landed after {:.3}s, current_gen={} (stale={})",
                started.elapsed().as_secs_f64(), generation.load(Ordering::Relaxed), generation.load(Ordering::Relaxed) != my_gen);
            if generation.load(Ordering::Relaxed) != my_gen { return; }
            {
                let mut s = state.lock().unwrap();
                s.filtered_items = filtered;
                // Only the unfiltered build marks Browse All as populated for the session (a search
                // result is a transient view).
                if is_full_list { s.browse_populated = true; }
            }
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_media_items(to_slint_model(names));
            }
        }).ok();
    });
}

// ── Wire callbacks ────────────────────────────────────────────────────────────

pub(crate) fn wire_browse(
    window: &MainWindow,
    state: Arc<Mutex<FjordState>>,
    rt_handle: tokio::runtime::Handle,
) {
    let browse_gen = Arc::new(AtomicU64::new(0));

    // ── Browse list: client-side filter over all_movies + all_series ─────────
    {
        let state = Arc::clone(&state);
        let generation = Arc::clone(&browse_gen);
        let rt = rt_handle.clone();
        let ww = window.as_weak();
        AppState::get(window).on_filter_changed(move |query| {
            populate_browse_async(
                ww.clone(),
                Arc::clone(&state),
                query.to_string(),
                Arc::clone(&generation),
                &rt,
            );
        });
    }
    // ── Browse search: keyboard-driven append / backspace / clear ────────────
    {
        let ww = window.as_weak();
        AppState::get(window).on_browse_search_append(move |ch| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let q = crate::text_field::BROWSE_SEARCH.insert(&g, ch.as_str());
            g.invoke_filter_changed(q.as_str().into());
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_browse_search_backspace(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if let Some(q) = crate::text_field::BROWSE_SEARCH.backspace(&g) {
                g.invoke_filter_changed(q.as_str().into());
            }
        });
    }
    // Delete key (2026-10-05): the letter after the caret.
    {
        let ww = window.as_weak();
        AppState::get(window).on_browse_search_delete(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if let Some(q) = crate::text_field::BROWSE_SEARCH.delete(&g) {
                g.invoke_filter_changed(q.as_str().into());
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let generation = Arc::clone(&browse_gen);
        let rt = rt_handle.clone();
        let ww = window.as_weak();
        AppState::get(window).on_browse_search_clear(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_browse_query("".into());
            g.set_current_item(-1);
            // Built once per session (until a WS LibraryChanged invalidates it, see ws.rs):
            // media_items already holds the list, so arriving again rebuilds nothing (like
            // discover_landing_fetched).
            if state.lock().unwrap().browse_populated {
                return;
            }
            // Debounced: this fires whenever the sidebar cursor lands on Browse All, also when just
            // passing through. Rebuilding the ~800-item list model lands on the UI thread and
            // stuttered the next key's transition; waiting for the cursor to settle means a
            // pass-through never starts it. Only the first arrival per session gets here (see the
            // check above).
            let ww2 = ww.clone();
            let state2 = Arc::clone(&state);
            let gen2 = Arc::clone(&generation);
            let rt2 = rt.clone();
            rt.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(120)).await;
                // ww.upgrade() only succeeds on the Slint UI thread (silently
                // returns None otherwise, per this codebase's own documented
                // history) — the settle check and the populate_browse_async
                // call both have to happen inside invoke_from_event_loop, not
                // out here on the Tokio worker thread.
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww2.upgrade() else { return };
                    let g = AppState::get(&w);
                    if g.get_active_nav() != 5 || !g.get_show_browse() {
                        return;
                    }
                    populate_browse_async(ww2, state2, String::new(), gen2, &rt2);
                });
            });
        });
    }
    // ── Library grid: client-side filter over loaded movies/series ───────────
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_search_append(move |ch| {
            let Some(w) = ww.upgrade() else { return };
            let q = crate::text_field::LIBRARY_SEARCH.insert(&AppState::get(&w), ch.as_str());
            update_library_filter(&w, &q);
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_search_backspace(move || {
            let Some(w) = ww.upgrade() else { return };
            if let Some(q) = crate::text_field::LIBRARY_SEARCH.backspace(&AppState::get(&w)) {
                update_library_filter(&w, &q);
            }
        });
    }
    // Delete key (2026-10-05): the letter after the caret.
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_search_delete(move || {
            let Some(w) = ww.upgrade() else { return };
            if let Some(q) = crate::text_field::LIBRARY_SEARCH.delete(&AppState::get(&w)) {
                update_library_filter(&w, &q);
            }
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
        let ww = window.as_weak();
        AppState::get(window).on_library_sort_apply(move |sort, fw, ff| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let nav = g.get_active_nav();
            g.set_library_sort(sort);
            g.set_library_filter_unwatched(fw);
            g.set_library_filter_favorites(ff);
            g.set_library_focused(0);
            g.set_library_focused_row(0);
            let cfg = {
                let mut s = state.lock().unwrap();
                let cp = s.config.active_mut();
                match nav {
                    2 => cp.library_movies_sort = sort.clamp(0, 4) as u8,
                    1 => cp.library_series_sort = sort.clamp(0, 4) as u8,
                    3 => cp.library_collections_sort = sort.clamp(0, 4) as u8,
                    4 => match g.get_library_music_view() {
                        1 => cp.library_albums_sort = sort.clamp(0, 4) as u8,
                        2 => cp.library_playlists_sort = sort.clamp(0, 4) as u8,
                        _ => cp.library_artists_sort = sort.clamp(0, 4) as u8,
                    },
                    _ => {}
                }
                s.config.clone()
            };
            crate::config::save_config(&cfg);
            refresh_library_display(&w);
        });
    }
    // ── Library alpha-jump: set focused card to first item for that letter ────
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_jump_to_letter(move |letter_idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let cols = g.get_library_cols();
            let offsets = g.get_library_alpha_offsets();
            if let Some(flat_idx) = offsets.row_data(letter_idx as usize)
                && flat_idx >= 0
            {
                g.set_library_focused(flat_idx);
                g.set_library_focused_row(flat_idx / cols);
            }
        });
    }
    // ── Library grid scroll: update scrubber cursor to reflect visible letter ─
    {
        let ww = window.as_weak();
        AppState::get(window).on_library_grid_scrolled(move |top_card| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let offsets = g.get_library_alpha_offsets();
            let mut letter = 0i32;
            for i in 0..27usize {
                if let Some(off) = offsets.row_data(i)
                    && off >= 0
                    && off <= top_card
                {
                    letter = i as i32;
                }
            }
            g.set_library_scrubber_cursor(letter);
        });
    }
    // Nav-selected clearing lives in `clear_browse_results` below, called
    // from `discover::wire_discover`'s own `on_nav_selected` registration —
    // see that function's doc comment for why a second registration here
    // would just be silently overwritten rather than adding a second listener.
}

// Clear browse results on nav change (skipped for nav 5 — browse is opening). Called from
// discover's on_nav_selected handler: a Slint callback has one handler, so a second
// registration would replace the first.
pub(crate) fn clear_browse_results(state: &Arc<Mutex<FjordState>>, g: &AppState, nav: i32) {
    if nav == 5 {
        return;
    }
    // Leave media-items/filtered_items alone in the common case: the full list is built only
    // once per session (browse_populated), so wiping it here left Browse All empty on every
    // later visit. Only after leaving mid-search, rebuild the full list synchronously (cheap)
    // from all_movies/all_series — the query resets to "" below and a filtered subset would
    // stay.
    if !g.get_browse_query().is_empty() {
        let mut s = state.lock().unwrap();
        let all: Vec<_> = s
            .all_movies
            .iter()
            .chain(s.all_series.iter())
            .cloned()
            .collect();
        let names = display_names(&all);
        s.filtered_items = all;
        drop(s);
        g.set_media_items(to_slint_model(names));
    }
    g.set_current_item(-1);
    g.set_browse_query("".into());
    // Reset browse-header-focused on every way of leaving Browse All (mouse or keys): a stale
    // true made keys.rs's raw-key pre-dispatch send arrows to handle_browse_search on the next
    // visit, so Up/Left/Right did nothing.
    g.set_browse_header_focused(false);
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;
    let ci = g.get_current_item();
    debug!(
        "browse::handle_key: action={action:?} current_item={ci} media_items_len={} active_nav={}",
        g.get_media_items().row_count(),
        g.get_active_nav()
    );
    match action {
        Action::Back => {
            g.set_browse_header_focused(false);
            g.set_current_item(-1);
            g.set_show_browse(false);
            g.invoke_browse_search_clear();
            if g.get_active_nav() == 5 {
                g.set_active_nav(0);
            }
            g.invoke_refocus();
            true
        }
        Action::Confirm if ci < 0 => {
            if g.get_media_items().row_count() > 0 {
                g.set_current_item(0);
            }
            true
        }
        Action::SearchJump if ci >= 0 => {
            g.set_browse_header_focused(true);
            true
        }
        Action::Up if ci < 0 => {
            sidebar_nav(g, -1);
            true
        }
        Action::Down if ci < 0 => {
            sidebar_nav(g, 1);
            true
        }
        Action::Up if ci >= 0 => {
            if ci > 0 {
                g.set_current_item(ci - 1);
            } else {
                g.set_browse_header_focused(true);
            }
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
        Action::Left if ci >= 0 => {
            g.set_current_item(-1);
            true
        }
        Action::Right if ci < 0 => {
            if g.get_media_items().row_count() > 0 {
                g.set_current_item(0);
            }
            true
        }
        Action::Confirm if ci >= 0 => {
            g.invoke_play_item(ci);
            true
        }
        Action::OpenContextMenu if ci >= 0 => {
            g.invoke_open_context_menu_browse(ci);
            true
        }
        _ => false,
    }
}

pub(crate) fn sidebar_nav(g: &AppState, dir: i32) {
    g.set_show_library(false);
    g.set_show_browse(false);
    g.set_settings_section("".into());
    g.set_settings_focused("".into());
    g.set_settings_dropdown_open(false);
    g.set_keybinding_focused(-1);
    let nav = g.get_active_nav();

    // Up from the topmost sidebar item: focus the mini-player bar when it is visible.
    if dir < 0 && nav == 0 && g.get_has_background_player() && !g.get_is_playing() {
        g.set_float_card_focused(0);
        return;
    }

    // Discover (nav 6) is in the cycle only when Seerr is enabled (a hidden tab can't take the
    // cursor). Profile (nav 7) always is, right before Settings.
    let seerr_on = g.get_settings_seerr_enabled();
    let next = if dir < 0 {
        match nav {
            0 => 11,
            11 => 10,
            10 => 7,
            7 => {
                if seerr_on {
                    6
                } else {
                    5
                }
            }
            6 => 5,
            5 => 4,
            4 => 3,
            3 => 2,
            2 => 1,
            _ => 0,
        }
    } else {
        match nav {
            0 => 1,
            1 => 2,
            2 => 3,
            3 => 4,
            4 => 5,
            5 => {
                if seerr_on {
                    6
                } else {
                    7
                }
            }
            6 => 7,
            7 => 10,
            10 => 11,
            _ => 0,
        }
    };
    debug!(
        "sidebar_nav: dir={dir} nav={nav} -> next={next} seerr_on={seerr_on} current_item={} show_browse_before={}",
        g.get_current_item(),
        g.get_show_browse()
    );
    g.set_active_nav(next);
    if next == 5 {
        g.set_show_browse(true);
        g.invoke_browse_search_clear();
    }
    g.invoke_nav_selected(next);
}

// ── wire_play_item (moved from main(), 0.5.0 step 3) ─────────────────────
/// Wires play from the Browse list: play_item.
pub(crate) fn wire_play_item(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    video: &std::sync::Arc<std::sync::Mutex<crate::playback::VideoState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    let video = std::sync::Arc::clone(video);
    // ── play from browse list ─────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let video2 = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();

        AppState::get(&window).on_play_item(move |idx| {
            let s = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else {
                return;
            };
            let Some(item) = s.filtered_items.get(idx as usize) else {
                return;
            };
            let item_id = item.id.clone();
            let item_title = item.display_name();
            if item.item_type == "Series" {
                let state2 = state.clone();
                let ww2 = window_weak.clone();
                let rt_handle2 = rt_handle.clone();
                drop(s);
                open_series_screen(item_id, state2, ww2, rt_handle2);
                return;
            }
            let play_url = client.direct_play_url(&item_id);
            let mut config = s.player_config();
            let item_type = item.item_type.clone();
            let series_id = item.series_id.clone();
            drop(s);
            let video2b = Arc::clone(&video2);
            let ww2 = window_weak.clone();
            let rth2 = rt_handle.clone();
            let state2b = Arc::clone(&state);
            rt_handle.spawn(async move {
                let detail = client.get_item_detail(&item_id).await.ok();
                let video_info = detail.as_ref().and_then(|i| i.video_stream_info());
                config.start_position_secs = detail.and_then(|i| i.resume_position_secs());
                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(
                        play_url, item_id, &item_type, item_title, config, client, series_id, None,
                        &video2b, &ww2, &rth2, &state2b, video_info,
                    );
                });
            });
        });
    }
}
