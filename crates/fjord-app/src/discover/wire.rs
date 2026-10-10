// ── fjord-app · discover/wire.rs ─────────────────────────────────────────────
//   wire_discover              registers every Discover/RequestDetail AppState callback (search
//                              append/backspace/clear, load-more, open-discover-item, request
//                              detail/options, context-menu actions, filters, Missing Seasons …)
//   on_nav_selected            (in wire_discover) the ONE nav-selected handler: logs it, calls
//                              browse.rs's per-nav logic, closes the on-screen keyboard on every tab
//                              switch, resets Discover's popup/filter bar and Settings' keybinding
//                              state when leaving them; on arriving at Discover: landing rows (once),
//                              all_movies metadata refresh (for find_local_item), refresh_seerr_admin_status
//   on_discover_filter_changed  shared tail of every filter-pill change: save Config, recompute
//                              discover-filters-active, then filtered browse (query empty + active),
//                              clear discover-results (empty + inactive), or apply_search_filters
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// Shared tail of every filter-change callback below (2026-07-18):
/// persists the change, then re-triggers whichever view is actually
/// relevant right now — a fresh filtered-browse fetch (query empty,
/// filters now active), a client-side re-filter (query non-empty), or
/// nothing extra (query empty, filters now all default — the landing rows
/// are already loaded and untouched; the Slint side's own view switch just
/// shows them again once `discover-results` is cleared).
fn on_discover_filter_changed(
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    generation: &Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    let (active, cfg) = {
        let s = state.lock().unwrap();
        (discover_filters_active(s.config.active()), s.config.clone())
    };
    save_config(&cfg);
    g.set_discover_filters_active(active);
    if g.get_discover_query().as_str().is_empty() {
        if active {
            spawn_discover_filtered_browse(
                ww.clone(),
                Arc::clone(state),
                Arc::clone(generation),
                rt,
            );
        } else {
            g.set_discover_results(ModelRc::new(VecModel::from(Vec::<CardItem>::new())));
        }
    } else {
        apply_search_filters(state, ww);
    }
}

// ── Wiring ───────────────────────────────────────────────────────────────────

pub(crate) fn wire_discover(
    window: &MainWindow,
    state: Arc<Mutex<FjordState>>,
    rt: tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    let discover_gen = Arc::new(AtomicU64::new(0));

    // Discover's landing rows are fetched once per session, on first arrival.
    // nav-selected fires from both the sidebar click and browse::sidebar_nav's keyboard
    // cycle, so this one registration covers both. It also refreshes the movie list
    // (metadata only, `with_posters: false`): unlike `all_series` (refreshed at every
    // login), `all_movies` is only fetched when the Movies grid opens, and
    // `find_local_item` needs its fresh `ProviderIds` for the in-library redirect.
    g.on_nav_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |nav| {
            debug!("nav-selected({nav})"); // sidebar click or key (2026-10-08 diagnostics)
            if nav == 6 {
                ensure_discover_landing(Arc::clone(&state), ww.clone(), rt.clone());
                spawn_movies_list_fetch(Arc::clone(&state), ww.clone(), rt.clone(), false);
                ensure_discover_filter_options(
                    Arc::clone(&state),
                    ww.clone(),
                    Arc::clone(&generation),
                    rt.clone(),
                );
                // Watchlist + Release Calendar, 2026-07-18 — same
                // once-per-session guard shape as ensure_discover_landing.
                ensure_discover_watchlist(Arc::clone(&state), ww.clone(), rt.clone());
                // Non-blocking: the menu opens with the cached permission; this makes the next open
                // reflect a server-side change (see refresh_seerr_admin_status).
                refresh_seerr_admin_status(Arc::clone(&state), ww.clone(), rt.clone());
            }
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);

            // This is the ONE `on_nav_selected` registration (Slint callbacks are single-handler),
            // so browse.rs's per-nav logic is called from here.
            crate::browse::clear_browse_results(&state, &g, nav);

            // Close the on-screen keyboard on every tab switch: Browse/LibraryGrid/Discover stay
            // mounted (only `visible:` toggles), so a keyboard opened from one of their search
            // fields would vanish with the screen while `show-onscreen-keyboard` stayed true —
            // and keys.rs's keyboard gate would then swallow every key app-wide. Every sidebar
            // switch (mouse and keyboard) comes through here.
            g.set_show_onscreen_keyboard(false);
            g.set_onscreen_keyboard_target("".into());
            g.set_onscreen_keyboard_cursor(0);

            if nav != 6 {
                // Leaving Discover: close an open filter popup and deactivate the filter bar, or
                // they reappear on return. Every tab switch passes this hook, so it's a better reset
                // point than each NavItem handler.
                g.set_discover_popup_open("".into());
                g.set_discover_filter_bar_active(false);
            }

            if nav != 10 {
                // Leaving Settings: clear a focused keybinding row — keys.rs's Settings routing
                // checks `keybinding-focused >= 0` first, so a stale value would hijack keys on any
                // screen (Enter could arm a rebind and the next key rebind an action unseen). Also
                // clears the two ConfirmDialog flags and the pending rebind, as sign-out does.
                g.set_keybinding_focused(-1);
                g.set_keybinding_rebinding(false);
                g.set_show_keybinding_reset_confirm(false);
                g.set_show_keybinding_collision_confirm(false);
                state.lock().unwrap().pending_keybind_rebind = None;
                // Disconnect-Seerr confirm: a stale true would reopen the dialog the next time
                // Settings shows (it renders on the flag alone).
                g.set_show_seerr_disconnect_confirm(false);
            }
        }
    });

    // ── Discover filters (2026-07-18) ──────────────────────────────────────
    g.on_discover_filter_type_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |desc| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let key = discover_type_key(desc.as_str());
            {
                let mut s = state.lock().unwrap();
                s.config.active_mut().discover_filter_type = key.to_string();
            }
            g.set_discover_filter_type_desc(discover_type_desc(key).into());
            // Genre/Provider's own selectable list depends on Type (a
            // movie-only or TV-only genre shouldn't be pickable while the
            // other type is excluded) — rebuild both from the already-
            // cached raw lists, no re-fetch needed.
            {
                let s = state.lock().unwrap();
                refresh_discover_filter_models(&g, &s);
            }
            on_discover_filter_changed(&state, &ww, &generation, &rt);
        }
    });

    g.on_discover_filter_sort_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |desc| {
            let key = discover_sort_key(desc.as_str());
            {
                let mut s = state.lock().unwrap();
                s.config.active_mut().discover_filter_sort = key.to_string();
            }
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_discover_filter_sort_desc(discover_sort_desc(key).into());
            }
            on_discover_filter_changed(&state, &ww, &generation, &rt);
        }
    });

    g.on_discover_filter_rating_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |desc| {
            let value = discover_rating_value(desc.as_str());
            {
                let mut s = state.lock().unwrap();
                s.config.active_mut().discover_filter_min_rating = value;
            }
            if let Some(w) = ww.upgrade() {
                AppState::get(&w)
                    .set_discover_filter_rating_desc(discover_rating_desc(value).into());
            }
            on_discover_filter_changed(&state, &ww, &generation, &rt);
        }
    });

    g.on_discover_filter_year_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |desc| {
            let value = discover_year_value(desc.as_str());
            {
                let mut s = state.lock().unwrap();
                s.config.active_mut().discover_filter_min_year = value;
            }
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_discover_filter_year_desc(discover_year_desc(value).into());
            }
            on_discover_filter_changed(&state, &ww, &generation, &rt);
        }
    });

    // Genre/Provider: multi-select, toggled by row index — the row's own
    // `selected` flips in the already-mounted model in place (cheap,
    // matches TagItem's own toggle pattern in RequestOptionsOverlay), then
    // Config's persisted name/id list is rebuilt from whichever rows ended
    // up selected.
    g.on_discover_filter_genre_toggle({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_discover_filter_genres();
            let Some(mut item) = model.row_data(idx as usize) else {
                return;
            };
            item.selected = !item.selected;
            model.set_row_data(idx as usize, item);
            let names: Vec<String> = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .filter(|g| g.selected)
                .map(|g| g.name.to_string())
                .collect();
            g.set_discover_filter_genre_count(names.len() as i32);
            state
                .lock()
                .unwrap()
                .config
                .active_mut()
                .discover_filter_genre_names = names;
            on_discover_filter_changed(&state, &ww, &generation, &rt);
        }
    });

    g.on_discover_filter_provider_toggle({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_discover_filter_providers();
            let Some(mut item) = model.row_data(idx as usize) else {
                return;
            };
            item.selected = !item.selected;
            model.set_row_data(idx as usize, item);
            let ids: Vec<i64> = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .filter(|p| p.selected)
                .map(|p| p.id as i64)
                .collect();
            g.set_discover_filter_provider_count(ids.len() as i32);
            state
                .lock()
                .unwrap()
                .config
                .active_mut()
                .discover_filter_provider_ids = ids;
            on_discover_filter_changed(&state, &ww, &generation, &rt);
        }
    });

    g.on_discover_filter_clear({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            {
                let mut s = state.lock().unwrap();
                let cp = s.config.active_mut();
                cp.discover_filter_type = String::new();
                cp.discover_filter_genre_names.clear();
                cp.discover_filter_sort = String::new();
                cp.discover_filter_min_rating = 0.0;
                cp.discover_filter_min_year = 0;
                cp.discover_filter_provider_ids.clear();
            }
            g.set_discover_filter_type_desc(discover_type_desc("").into());
            g.set_discover_filter_sort_desc(discover_sort_desc("").into());
            g.set_discover_filter_rating_desc(discover_rating_desc(0.0).into());
            g.set_discover_filter_year_desc(discover_year_desc(0).into());
            {
                let s = state.lock().unwrap();
                refresh_discover_filter_models(&g, &s);
            }
            on_discover_filter_changed(&state, &ww, &generation, &rt);
        }
    });

    // Mouse-click equivalents of keyboard Confirm — reuse the exact same
    // dispatch functions as the keyboard path (see their own doc comments
    // above) rather than a second, independently-written click handler, so
    // mouse and keyboard can never disagree about what a pill/option/chip
    // does. discover.slint's click handlers set -bar-focused/-popup-cursor
    // to the clicked index first, then invoke these.
    g.on_discover_filter_bar_confirm({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            handle_key_discover_filter_bar(&Action::Confirm, &AppState::get(&w));
        }
    });
    g.on_discover_popup_confirm({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            handle_key_discover_popup(&Action::Confirm, &AppState::get(&w));
        }
    });

    g.on_discover_search_append({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |ch| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let was_landing = g.get_discover_query().is_empty();
            let q = crate::text_field::DISCOVER_SEARCH.insert(&g, ch.as_str());
            if was_landing {
                // First character typed: the landing rows give way to the flat results grid, where
                // only focused-section 0 means "grid" — reset it in case the search field was clicked
                // while parked on another landing row.
                g.set_focused_section(0);
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
            }
            spawn_discover_search(
                ww.clone(),
                Arc::clone(&state),
                q,
                Arc::clone(&generation),
                &rt,
            );
        }
    });
    g.on_discover_search_backspace({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if let Some(q) = crate::text_field::DISCOVER_SEARCH.backspace(&g) {
                spawn_discover_search(
                    ww.clone(),
                    Arc::clone(&state),
                    q,
                    Arc::clone(&generation),
                    &rt,
                );
            }
        }
    });
    // Delete key: removes the letter after the caret (2026-10-04).
    g.on_discover_search_delete({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if let Some(q) = crate::text_field::DISCOVER_SEARCH.delete(&g) {
                spawn_discover_search(
                    ww.clone(),
                    Arc::clone(&state),
                    q,
                    Arc::clone(&generation),
                    &rt,
                );
            }
        }
    });
    g.on_discover_search_clear({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_discover_query("".into()); // caret: past the end = end, nothing to reset
            g.set_discover_focused(0);
            g.set_discover_focused_row(0);
            spawn_discover_search(
                ww.clone(),
                Arc::clone(&state),
                String::new(),
                Arc::clone(&generation),
                &rt,
            );
        }
    });
    g.on_discover_load_more({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let generation = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let query = AppState::get(&w).get_discover_query().to_string();
            if query.is_empty() {
                // Filtered-browse's own pagination (2026-07-18) — landing
                // rows (no filters active) have nothing to load more of.
                if discover_filters_active(state.lock().unwrap().config.active()) {
                    spawn_discover_filtered_browse_more(
                        ww.clone(),
                        Arc::clone(&state),
                        Arc::clone(&generation),
                        &rt,
                    );
                }
                return;
            }
            spawn_discover_search_more(
                ww.clone(),
                Arc::clone(&state),
                query,
                Arc::clone(&generation),
                &rt,
            );
        }
    });

    g.on_open_discover_item({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |media_type, tmdb_id| {
            open_discover_item(
                media_type.to_string(),
                tmdb_id.to_string(),
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
            );
        }
    });

    // Wired here, not in keys.rs: handle_key has no state/rt for the async TMDB
    // resolution + request-detail open (same as on_open_discover_item).
    g.on_series_missing_season_activate({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            crate::series::activate_missing_season(
                &g,
                idx as usize,
                &state,
                ww.clone(),
                rt.clone(),
            );
        }
    });

    g.on_request_detail_toggle_season({
        let ww = window.as_weak();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_request_detail_seasons();
            if let Some(mut s) = model.row_data(idx as usize) {
                s.selected = !s.selected;
                model.set_row_data(idx as usize, s);
            }
        }
    });

    g.on_request_detail_toggle_tag({
        let ww = window.as_weak();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_request_detail_tags();
            if let Some(mut t) = model.row_data(idx as usize) {
                t.selected = !t.selected;
                model.set_row_data(idx as usize, t);
            }
        }
    });

    g.on_request_detail_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            if AppState::get(&w).get_request_options_editing() {
                submit_edit_request(Arc::clone(&state), ww.clone(), rt.clone());
            } else {
                submit_request(Arc::clone(&state), ww.clone(), rt.clone());
            }
        }
    });

    g.on_open_request_options({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            open_request_options_modal(&AppState::get(&w));
        }
    });

    g.on_request_detail_set_quality({
        let ww = window.as_weak();
        move |want_4k| {
            let Some(w) = ww.upgrade() else { return };
            set_quality(&AppState::get(&w), want_4k);
        }
    });

    // ── Discover context menu (2026-07-18) ────────────────────────────────
    g.on_open_context_menu_discover({
        let ww = window.as_weak();
        move |item| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_context_menu_item_id(item.id.clone());
            g.set_context_menu_item_type(item.item_type.clone());
            g.set_context_menu_title(item.title.clone());
            g.set_context_menu_request_id(item.request_id.clone());
            g.set_context_menu_availability(item.availability.clone());
            g.set_context_menu_request_pending(item.request_pending);
            g.set_context_menu_request_mine(item.request_mine);
            g.set_context_menu_on_watchlist(item.on_watchlist);
            debug!(
                "seerr: discover context menu opened for {} ({}) request_id={:?} pending={} mine={} seerr-is-admin={}",
                item.id, item.item_type, item.request_id, item.request_pending, item.request_mine, g.get_seerr_is_admin(),
            );
            g.set_context_menu_focused(0);
            g.set_show_context_menu(true);
        }
    });

    // RequestDetailScreen's ⋮ More: the same context-menu-* population as
    // on_open_context_menu_discover, sourced from request-detail-* (no CardItem here);
    // the request id/pending/mine come from discover::pick_primary_request.
    g.on_open_discover_menu_from_detail({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let item_type = if g.get_request_detail_media_type().as_str() == "movie" { "DiscoverMovie" } else { "DiscoverTv" };
            g.set_context_menu_item_id(g.get_request_detail_tmdb_id().to_string().as_str().into());
            g.set_context_menu_item_type(item_type.into());
            g.set_context_menu_title(g.get_request_detail_title());
            g.set_context_menu_request_id(g.get_request_detail_request_id());
            g.set_context_menu_request_pending(g.get_request_detail_request_pending());
            g.set_context_menu_request_mine(g.get_request_detail_request_mine());
            g.set_context_menu_on_watchlist(g.get_request_detail_on_watchlist());
            // Forward availability too — the Blocklist row reads it.
            g.set_context_menu_availability(g.get_request_detail_availability());
            debug!(
                "seerr: discover menu opened from detail page for {} ({}) request_id={:?} pending={} mine={}",
                g.get_request_detail_tmdb_id(), item_type, g.get_request_detail_request_id(),
                g.get_request_detail_request_pending(), g.get_request_detail_request_mine(),
            );
            g.set_context_menu_focused(0);
            g.set_show_context_menu(true);
        }
    });

    g.on_context_discover_view_details({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" {
                "movie"
            } else {
                "tv"
            };
            let tmdb_id = g.get_context_menu_item_id().to_string();
            g.set_show_context_menu(false);
            open_discover_item(
                media_type.into(),
                tmdb_id,
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
            );
        }
    });

    // "View Request" (shown when context-menu-request-id is set) skips the
    // find_local_item redirect, so a partly-owned item's request stays reachable — see
    // open_discover_item_ex.
    g.on_context_discover_view_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" {
                "movie"
            } else {
                "tv"
            };
            let tmdb_id = g.get_context_menu_item_id().to_string();
            g.set_show_context_menu(false);
            open_discover_item_ex(
                media_type.into(),
                tmdb_id,
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                PostOpenAction::None,
                false,
            );
        }
    });

    g.on_context_discover_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" {
                "movie"
            } else {
                "tv"
            };
            let tmdb_id = g.get_context_menu_item_id().to_string();
            g.set_show_context_menu(false);
            open_discover_item_ex(
                media_type.into(),
                tmdb_id,
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                PostOpenAction::OpenRequestOptions,
                true,
            );
        }
    });

    g.on_context_discover_edit_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_context_menu_request_id().parse::<i64>() else {
                return;
            };
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" {
                "movie"
            } else {
                "tv"
            };
            let tmdb_id = g.get_context_menu_item_id().to_string();
            g.set_show_context_menu(false);
            open_discover_item_ex(
                media_type.into(),
                tmdb_id,
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                PostOpenAction::EditRequest(request_id),
                true,
            );
        }
    });

    g.on_context_discover_cancel_request({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_context_menu_request_id().parse::<i64>() else {
                return;
            };
            g.set_show_context_menu(false);
            // Opens the global cancel-request confirmation (app_state.slint): DELETE /request
            // has no undo. The delete runs in on_cancel_request_confirmed.
            g.set_cancel_request_confirm_id(request_id.to_string().into());
            g.set_cancel_request_confirm_focused(0);
            g.set_show_cancel_request_confirm(true);
        }
    });

    g.on_cancel_request_confirmed({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_cancel_request_confirm_id().parse::<i64>() else {
                return;
            };
            discover_request_action(
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                request_id,
                "cancel",
                true,
            );
        }
    });

    g.on_context_discover_approve_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_context_menu_request_id().parse::<i64>() else {
                return;
            };
            g.set_show_context_menu(false);
            discover_request_action(
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                request_id,
                "approve",
                false,
            );
        }
    });

    g.on_context_discover_decline_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_context_menu_request_id().parse::<i64>() else {
                return;
            };
            g.set_show_context_menu(false);
            discover_request_action(
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                request_id,
                "decline",
                true,
            );
        }
    });

    // Watchlist + Release Calendar (2026-07-18) — always visible in the
    // Discover context menu, unlike Request's own availability gating.
    g.on_context_discover_toggle_watchlist({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" { "movie" } else { "tv" };
            let raw_id = g.get_context_menu_item_id();
            let Ok(tmdb_id) = raw_id.parse::<i64>() else {
                warn!("seerr: on_context_discover_toggle_watchlist: bad tmdb id {raw_id:?}, item_type={:?}", g.get_context_menu_item_type());
                return;
            };
            let adding = !g.get_context_menu_on_watchlist();
            let title = g.get_context_menu_title().to_string();
            g.set_show_context_menu(false);
            discover_toggle_watchlist(Arc::clone(&state), ww.clone(), rt.clone(), tmdb_id, media_type.into(), title, adding, None);
        }
    });

    // RequestDetailScreen's own Watchlist button (2026-07-18) — same toggle,
    // sourced from request-detail-* state since this page has no CardItem.
    g.on_request_detail_toggle_watchlist({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = g.get_request_detail_media_type().to_string();
            let tmdb_id = g.get_request_detail_tmdb_id() as i64;
            let adding = !g.get_request_detail_on_watchlist();
            let title = g.get_request_detail_title().to_string();
            discover_toggle_watchlist(
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                tmdb_id,
                media_type,
                title,
                adding,
                None,
            );
        }
    });

    // Discover context menu's Blocklist row: like Watchlist, but "adding" = not
    // currently blocklisted (blocklisted is an availability value — see availability_tag).
    g.on_context_discover_toggle_blocklist({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" { "movie" } else { "tv" };
            let raw_id = g.get_context_menu_item_id();
            let Ok(tmdb_id) = raw_id.parse::<i64>() else {
                warn!("seerr: on_context_discover_toggle_blocklist: bad tmdb id {raw_id:?}, item_type={:?}", g.get_context_menu_item_type());
                return;
            };
            let adding = g.get_context_menu_availability().as_str() != "blocklisted";
            let title = g.get_context_menu_title().to_string();
            g.set_show_context_menu(false);
            discover_toggle_blocklist(Arc::clone(&state), ww.clone(), rt.clone(), tmdb_id, media_type.into(), title, adding);
        }
    });

    // RequestDetailScreen's own Blocklist button (2026-08-06) — same shape
    // as its Watchlist sibling above.
    g.on_request_detail_toggle_blocklist({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = g.get_request_detail_media_type().to_string();
            let tmdb_id = g.get_request_detail_tmdb_id() as i64;
            let adding = g.get_request_detail_availability().as_str() != "blocklisted";
            let title = g.get_request_detail_title().to_string();
            discover_toggle_blocklist(
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                tmdb_id,
                media_type,
                title,
                adding,
            );
        }
    });

    // ── Calendar screen (2026-07-18, Watchlist + Release Calendar) ──────────
    g.on_open_calendar({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let today = chrono::Local::now().date_naive();
            g.set_calendar_year(chrono::Datelike::year(&today));
            g.set_calendar_month(chrono::Datelike::month(&today) as i32);
            g.set_calendar_cursor_row(-1);
            g.set_calendar_cursor_col(0);
            g.set_show_calendar_day_popup(false);
            {
                let s = state.lock().unwrap();
                push_calendar_view(&g, &s);
            }
            g.set_show_calendar(true);
        }
    });

    g.on_calendar_prev_month({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let (mut y, mut m) = (g.get_calendar_year(), g.get_calendar_month());
            m -= 1;
            if m < 1 {
                m = 12;
                y -= 1;
            }
            g.set_calendar_year(y);
            g.set_calendar_month(m);
            g.set_calendar_cursor_row(0);
            g.set_calendar_cursor_col(0);
            push_calendar_view(&g, &state.lock().unwrap());
        }
    });

    g.on_calendar_next_month({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let (mut y, mut m) = (g.get_calendar_year(), g.get_calendar_month());
            m += 1;
            if m > 12 {
                m = 1;
                y += 1;
            }
            g.set_calendar_year(y);
            g.set_calendar_month(m);
            g.set_calendar_cursor_row(0);
            g.set_calendar_cursor_col(0);
            push_calendar_view(&g, &state.lock().unwrap());
        }
    });

    // Mouse click on a day cell — mirrors handle_key_calendar's Confirm arm
    // for the day-grid case, so mouse and keyboard can't diverge.
    g.on_calendar_day_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        move |day| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_calendar_cursor_row((day - 1 + g.get_calendar_leading_blanks()) / 7);
            g.set_calendar_cursor_col((day - 1 + g.get_calendar_leading_blanks()) % 7);
            open_calendar_day_popup(&g, &state, day);
        }
    });

    // Mouse click on a day-popup entry — mirrors
    // handle_key_calendar_day_popup's Confirm arm.
    g.on_calendar_day_popup_entry_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Some(entry) = g.get_calendar_day_popup_entries().row_data(idx as usize) else {
                return;
            };
            g.set_calendar_day_popup_cursor(idx);
            let media_type = if entry.item_type.as_str() == "DiscoverMovie" {
                "movie"
            } else {
                "tv"
            };
            g.set_show_calendar_day_popup(false);
            g.set_show_calendar(false);
            open_discover_item(
                media_type.into(),
                entry.id.to_string(),
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
            );
        }
    });
}
