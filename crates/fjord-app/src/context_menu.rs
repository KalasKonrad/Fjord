// ── fjord-app · context_menu.rs ──────────────────────────────────────────────
//   wire_context_menu   register all AppState context-menu callbacks:
//     open-context-menu             set menu state from CardItem fields
//     open-context-menu-browse      resolve browse index → MediaItem → set state
//     open-context-menu-series-ep   set menu state for a series episode
//     context-mark-played           POST/DELETE /Users/{id}/PlayedItems/{itemId};
//                                   on success: update all models; if played→remove from dynamic rows;
//                                   always call refresh_series_next_up (both mark-played and unplayed)
//     context-toggle-fav            POST/DELETE /Users/{id}/FavoriteItems/{itemId}
//     context-play-from-start       series → get_next_up_for_series (from start); movie/ep → start_position_secs = None
//   open_context_menu_state         set all 8 context-menu AppState fields incl. series-id (shared by all three open handlers)
//   update_series_unplayed_count    ±1 unplayed-count on the parent series card after mark-played (also called from main.rs)
//   update_card_in_all_models       patch has-played / is-favorite across every model (incl. series-next-up-cards)
//   remove_from_dynamic_rows        remove item from Next Up/Continue Watching/Not Watched rows;
//                                   matches card.id==id (item) OR card.series_id==id (series → all its episodes);
//                                   does NOT touch series-next-up-cards (refresh_series_next_up handles that)
//   find_title_in_state             scan FjordState media lists by item id → display name
//   enqueue_item                    insert into playlist (play-next) or append to queue
//   queue_from_context_menu         shared add/play-next body; Series resolved to next-up episode (CR10-7)
//   wire_queue_callbacks            on_queue_add_item / on_queue_play_next_item
//   handle_key                      keyboard dispatch for the context-menu overlay
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::playback::{QueueItem, VideoState, start_playback};
use crate::series::open_series_screen;
use crate::{AppState, CardItem, MainWindow};

// Patch every dashboard row, library grid, and episode list; called after a successful API toggle.
// Uses set_row_data to mutate rows in place — preserves poster images and fires per-row
// change notifications without rebuilding the whole model.
pub(crate) fn update_card_in_all_models(w: &MainWindow, id: &str, played: Option<bool>, fav: Option<bool>) {
    let patch_cards = |model: ModelRc<CardItem>| {
        for i in 0..model.row_count() {
            if let Some(mut c) = model.row_data(i) {
                if c.id.as_str() == id {
                    if let Some(p) = played { c.has_played  = p; }
                    if let Some(f) = fav    { c.is_favorite = f; }
                    model.set_row_data(i, c);
                    break;
                }
            }
        }
    };

    let g = AppState::get(w);
    patch_cards(g.get_continue_watching());
    patch_cards(g.get_next_up());
    patch_cards(g.get_recently_added());
    patch_cards(g.get_recently_added_movies());
    patch_cards(g.get_continue_watching_movies());
    patch_cards(g.get_not_watched_movies());
    patch_cards(g.get_continue_watching_tv());
    patch_cards(g.get_recently_added_tv());
    patch_cards(g.get_not_watched_tv());
    patch_cards(g.get_recently_added_collections());
    patch_cards(g.get_unwatched_collections());
    patch_cards(g.get_recently_added_albums());
    patch_cards(g.get_recently_played_albums());
    patch_cards(g.get_favorite_movies());
    patch_cards(g.get_favorite_series());
    patch_cards(g.get_favorite_albums());
    patch_cards(g.get_all_movies());
    patch_cards(g.get_all_series());
    patch_cards(g.get_library_display());
    patch_cards(g.get_series_episode_cards());
    patch_cards(g.get_series_next_up_cards());
    patch_cards(g.get_collection_items());
    patch_cards(g.get_detail_similar());
    patch_cards(g.get_detail_collection());
    patch_cards(g.get_series_similar());
    patch_cards(g.get_person_filmography());
}

// Remove cards from curated rows (Next Up, Continue Watching, Not Watched) when an item is
// marked as played. Matches on card.id == id (the item itself) OR card.series_id == id (all
// episodes of a series that was marked played as a whole). Rebuilds the model rather than
// patching in-place so that removed rows collapse from the UI immediately.
pub(crate) fn remove_from_dynamic_rows(w: &MainWindow, id: &str) {
    let filter = |model: ModelRc<CardItem>| -> ModelRc<CardItem> {
        let kept: Vec<CardItem> = (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .filter(|c| c.id.as_str() != id && c.series_id.as_str() != id)
            .collect();
        ModelRc::new(VecModel::from(kept))
    };
    let g = AppState::get(w);
    g.set_next_up(filter(g.get_next_up()));
    g.set_continue_watching(filter(g.get_continue_watching()));
    g.set_continue_watching_movies(filter(g.get_continue_watching_movies()));
    g.set_continue_watching_tv(filter(g.get_continue_watching_tv()));
    g.set_not_watched_movies(filter(g.get_not_watched_movies()));
    g.set_not_watched_tv(filter(g.get_not_watched_tv()));
    g.set_unwatched_collections(filter(g.get_unwatched_collections()));
    // Do NOT touch series_next_up_cards here — leave the old card visible while
    // refresh_series_next_up fetches the replacement. update_card_in_all_models already
    // applied the ✓ badge. refresh_series_next_up will either replace the card or clear
    // the row (and redirect focus) once the server response arrives.
}

fn open_context_menu_state(
    g: &AppState,
    id: SharedString,
    item_type: SharedString,
    played: bool,
    is_fav: bool,
    resume_pct: f32,
    series_id: SharedString,
) {
    g.set_context_menu_item_id(id);
    g.set_context_menu_item_type(item_type);
    g.set_context_menu_series_id(series_id);
    g.set_context_menu_has_played(played);
    g.set_context_menu_is_favorite(is_fav);
    g.set_context_menu_resume_pct(resume_pct);
    g.set_context_menu_focused(if resume_pct > 0.0 && !played { 0 } else { 1 });
    g.set_show_context_menu(true);
}

// Adjust a series card's unplayed_count by delta after an episode is marked played/unplayed.
pub(crate) fn update_series_unplayed_count(w: &MainWindow, series_id: &str, delta: i32) {
    let patch = |model: slint::ModelRc<crate::CardItem>| {
        for i in 0..model.row_count() {
            if let Some(mut c) = model.row_data(i) {
                if c.id.as_str() == series_id {
                    c.unplayed_count = (c.unplayed_count + delta).max(0);
                    model.set_row_data(i, c);
                    break;
                }
            }
        }
    };
    let g = AppState::get(w);
    patch(g.get_continue_watching());
    patch(g.get_next_up());
    patch(g.get_recently_added());
    patch(g.get_recently_added_movies());
    patch(g.get_continue_watching_movies());
    patch(g.get_not_watched_movies());
    patch(g.get_continue_watching_tv());
    patch(g.get_recently_added_tv());
    patch(g.get_not_watched_tv());
    patch(g.get_all_movies());
    patch(g.get_all_series());
    patch(g.get_library_display());
    // Also update the series screen header badge if this series is currently open.
    if g.get_series_id().as_str() == series_id {
        g.set_series_unplayed_count((g.get_series_unplayed_count() + delta).max(0));
    }
}

pub(crate) fn wire_context_menu(
    window:    &MainWindow,
    state:     Arc<Mutex<FjordState>>,
    video:     Arc<Mutex<VideoState>>,
    rt_handle: tokio::runtime::Handle,
) {
    // ── open-context-menu: called with full card data from Slint ─────────────
    {
        let ww = window.as_weak();
        AppState::get(window).on_open_context_menu(move |id, has_played, is_fav, resume_pct, item_type, series_id| {
            let Some(w) = ww.upgrade() else { return };
            open_context_menu_state(&AppState::get(&w), id, item_type, has_played, is_fav, resume_pct, series_id);
        });
    }

    // ── open-context-menu-browse: Rust resolves index into filtered_items ────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        AppState::get(window).on_open_context_menu_browse(move |index| {
            let Some(w) = ww.upgrade() else { return };
            let s = state.lock().unwrap();
            let Some(item) = s.filtered_items.get(index as usize) else { return };
            let id         = SharedString::from(item.id.as_str());
            let played     = item.user_data.played;
            let is_fav     = item.user_data.is_favorite;
            let resume_pct = item.resume_pct();
            let item_type  = SharedString::from(item.item_type.as_str());
            let series_id  = SharedString::from(item.series_id.as_deref().unwrap_or(""));
            drop(s);
            open_context_menu_state(&AppState::get(&w), id, item_type, played, is_fav, resume_pct, series_id);
        });
    }

    // ── open-context-menu-series-ep: episode C-key context menu ─────────────
    {
        let ww = window.as_weak();
        AppState::get(window).on_open_context_menu_series_ep(move |id, has_played, is_fav, resume_pct, series_id| {
            let Some(w) = ww.upgrade() else { return };
            open_context_menu_state(&AppState::get(&w), id, "Episode".into(), has_played, is_fav, resume_pct, series_id);
        });
    }

    // ── context-mark-played: toggle played state ──────────────────────────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        AppState::get(window).on_context_mark_played(move |id, currently_played| {
            let s  = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let id2     = id.to_string();
            // Capture series_id now (CR2-4): re-reading inside invoke_from_event_loop
            // would see whatever item the menu is open for at response time, not this one.
            let sid2    = ww.upgrade()
                .map(|w| AppState::get(&w).get_context_menu_series_id().to_string())
                .unwrap_or_default();
            let ww2     = ww.clone();
            let state2  = Arc::clone(&state);
            let client2 = Arc::clone(&client); // for refresh_series_next_up
            let rt2     = rt.clone();           // for refresh_series_next_up
            rt.spawn(async move {
                let result = if currently_played {
                    client.mark_unplayed(&id2).await
                } else {
                    client.mark_played(&id2).await
                };
                if let Err(e) = result {
                    warn!("mark played/unplayed failed: {e}");
                    crate::show_toast(ww2.clone(), "Couldn't update watch status".to_string());
                } else {
                    let new_played = !currently_played;
                    state2.lock().unwrap().update_item_user_state(&id2, Some(new_played), None);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() {
                            // Only update the menu display if it's still open for this item (CR-7).
                            if AppState::get(&w).get_context_menu_item_id().as_str() == id2 {
                                let g = AppState::get(&w);
                                g.set_context_menu_has_played(new_played);
                                // Resume row (0) disappears when item becomes fully played.
                                // Move focus to Play from Start (1) so Enter doesn't invoke resume.
                                if new_played && g.get_context_menu_focused() == 0 {
                                    g.set_context_menu_focused(1);
                                }
                            }
                            update_card_in_all_models(&w, &id2, Some(new_played), None);
                            if new_played {
                                // Remove from curated rows (Next Up, Continue Watching, Not Watched,
                                // series-next-up-cards). Matches card.id==id (episode/movie) and
                                // card.series_id==id (all episodes when a whole series is marked played).
                                remove_from_dynamic_rows(&w, &id2);
                            }
                            // Refresh the series Next Up row on any played-state change (mark-played
                            // OR unplayed) so the new first-unwatched episode appears immediately.
                            if !sid2.is_empty() {
                                crate::series::refresh_series_next_up(
                                    sid2.clone(), client2, ww2.clone(), rt2
                                );
                            }
                            // Adjust unplayed badge on the parent series card if this is an episode.
                            if !sid2.is_empty() {
                                let delta = if new_played { -1 } else { 1 };
                                update_series_unplayed_count(&w, &sid2, delta);
                            }
                        }
                    });
                }
            });
        });
    }

    // ── context-toggle-fav: toggle favourite state ────────────────────────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        AppState::get(window).on_context_toggle_fav(move |id, currently_fav| {
            let s  = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
            drop(s);
            let id2    = id.to_string();
            let ww2    = ww.clone();
            let state2 = Arc::clone(&state);
            rt.spawn(async move {
                let result = if currently_fav {
                    client.unset_favorite(&id2).await
                } else {
                    client.set_favorite(&id2).await
                };
                if let Err(e) = result {
                    warn!("toggle favourite failed: {e}");
                    crate::show_toast(ww2, "Couldn't update favourite".to_string());
                    return;
                }
                let new_fav = !currently_fav;
                state2.lock().unwrap().update_item_user_state(&id2, None, Some(new_fav));
                let ww3 = ww2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        // Only update the menu display if it's still open for this item (CR-7).
                        if AppState::get(&w).get_context_menu_item_id().as_str() == id2 {
                            AppState::get(&w).set_context_menu_is_favorite(new_fav);
                        }
                        update_card_in_all_models(&w, &id2, None, Some(new_fav));
                    }
                });
                let rt2 = tokio::runtime::Handle::current();
                crate::home::refresh_favorites(client, ww3, rt2);
            });
        });
    }

    // ── context-play-from-start: play with no resume position ───────────────
    {
        let state  = Arc::clone(&state);
        let video  = Arc::clone(&video);
        let ww     = window.as_weak();
        let rt     = rt_handle.clone();
        AppState::get(window).on_context_play_from_start(move |id| {
            let id = id.to_string();
            let s  = state.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else { return };

            // Series: find next-up episode and play it from the start
            if s.all_series.iter().any(|i| i.id == id) {
                let state2 = Arc::clone(&state);
                let video2 = Arc::clone(&video);
                let ww2    = ww.clone();
                let rt2    = rt.clone();
                drop(s);
                rt.spawn(async move {
                    let next = client.get_next_up_for_series(&id).await.ok().flatten();
                    if let Some(next) = next {
                        let mut config = state2.lock().unwrap().player_config();
                        config.start_position_secs = None; // play from start of this episode
                        let cli2      = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                        let Some(cli2) = cli2 else {
                            let _ = slint::invoke_from_event_loop(move || {
                                open_series_screen(id, state2, ww2, rt2);
                            });
                            return;
                        };
                        let url       = cli2.direct_play_url(&next.id);
                        let title     = next.display_name();
                        let ep_id     = next.id.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            start_playback(url, ep_id, "Episode", title, config, cli2,
                                           Some(id), None, &video2, &ww2, &rt2);
                        });
                    } else {
                        let _ = slint::invoke_from_event_loop(move || {
                            open_series_screen(id, state2, ww2, rt2);
                        });
                    }
                });
                return;
            }

            let mut config = s.player_config();
            drop(s);
            config.start_position_secs = None;
            let play_url = client.direct_play_url(&id);
            let video2   = Arc::clone(&video);
            let ww2      = ww.clone();
            let rt2      = rt.clone();
            rt.spawn(async move {
                let detail    = client.get_item_detail(&id).await
                    .inspect_err(|e| warn!("play-from-start: get_item_detail({id}) failed: {e:#}"))
                    .ok();
                let item_type = detail.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
                let series_id = detail.as_ref().and_then(|i| i.series_id.clone());
                if item_type == "Episode" && series_id.is_none() {
                    warn!("play-from-start: episode {} has no SeriesId — Up Next will be disabled for this session", id);
                }
                let title     = detail.map(|i| i.display_name()).unwrap_or_else(|| id.clone());
                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(play_url, id, &item_type, title, config, client,
                                   series_id, None, &video2, &ww2, &rt2);
                });
            });
        });
    }
}

// ── Queue helpers ─────────────────────────────────────────────────────────────

fn find_title_in_state(s: &FjordState, id: &str) -> String {
    for item in s.all_movies.iter()
        .chain(s.all_series.iter())
        .chain(s.all_albums.iter())
        .chain(s.all_artists.iter())
    {
        if item.id == id { return item.display_name(); }
    }
    for eps in s.series_episode_cache.values() {
        for ep in eps {
            if ep.id == id { return ep.display_name(); }
        }
    }
    id.to_string()
}

// Fallback title lookup across the visible Slint card models — episodes queued
// from home rows (Next Up, Continue Watching, …) aren't in any FjordState vec,
// so find_title_in_state fell through to the raw GUID (CR10-18).
fn find_title_in_models(g: &AppState, id: &str) -> Option<String> {
    let find = |model: ModelRc<CardItem>| -> Option<String> {
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .find(|c| c.id.as_str() == id)
            .map(|c| c.title.to_string())
    };
    find(g.get_continue_watching())
        .or_else(|| find(g.get_next_up()))
        .or_else(|| find(g.get_recently_added()))
        .or_else(|| find(g.get_recently_added_movies()))
        .or_else(|| find(g.get_continue_watching_movies()))
        .or_else(|| find(g.get_not_watched_movies()))
        .or_else(|| find(g.get_continue_watching_tv()))
        .or_else(|| find(g.get_recently_added_tv()))
        .or_else(|| find(g.get_not_watched_tv()))
        .or_else(|| find(g.get_library_display()))
        .or_else(|| find(g.get_series_episode_cards()))
        .or_else(|| find(g.get_series_next_up_cards()))
        .or_else(|| find(g.get_collection_items()))
        .or_else(|| find(g.get_detail_similar()))
        .or_else(|| find(g.get_detail_collection()))
        .or_else(|| find(g.get_series_similar()))
        .or_else(|| find(g.get_person_filmography()))
}

// Insert `item` into the playback collections. play_next=true inserts right
// after the current playlist position (or at the queue front when there is no
// playlist); play_next=false appends to the context-menu queue.
fn enqueue_item(vs: &mut VideoState, item: QueueItem, play_next: bool) {
    if play_next {
        if !vs.playlist.is_empty() {
            // Insert after the current playlist position (plays next within album)
            let insert_at = (vs.playlist_index + 1).min(vs.playlist.len());
            vs.playlist.insert(insert_at, item);
            // Keep shuffle_order valid: shift indices >= insert_at up by one
            for idx in vs.shuffle_order.iter_mut() {
                if *idx >= insert_at { *idx += 1; }
            }
            // Insert the new position right after the CURRENT item's slot in
            // shuffle_order. Slot 1 (pre-CR10-8) was only correct immediately
            // after toggling shuffle — once playback advanced to shuffle
            // position k, anything inserted at slot 1 was behind the cursor
            // and never played.
            if vs.shuffle && !vs.shuffle_order.is_empty() {
                let cur_pos = vs.shuffle_order.iter()
                    .position(|&i| i == vs.playlist_index)
                    .unwrap_or(0);
                vs.shuffle_order.insert(cur_pos + 1, insert_at);
            }
        } else {
            vs.queue.insert(0, item);
        }
    } else {
        vs.queue.push(item);
    }
}

// Shared body for queue-add-item / queue-play-next-item. Series cards are
// resolved to their next unwatched episode first (CR10-7) — a raw series id
// has no stream, so enqueueing it verbatim produced an unplayable item.
fn queue_from_context_menu(
    g:         &AppState,
    state:     &Arc<Mutex<FjordState>>,
    video:     &Arc<Mutex<VideoState>>,
    ww:        &slint::Weak<MainWindow>,
    rt:        &tokio::runtime::Handle,
    play_next: bool,
) {
    let id        = g.get_context_menu_item_id().to_string();
    let item_type = g.get_context_menu_item_type().to_string();
    let sid_str   = g.get_context_menu_series_id().to_string();
    let series_id = if sid_str.is_empty() { None } else { Some(sid_str) };

    if item_type == "Series" {
        let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
        let video2 = Arc::clone(video);
        let ww2    = ww.clone();
        rt.spawn(async move {
            match client.get_next_up_for_series(&id).await {
                Ok(Some(ep)) => {
                    let item = QueueItem {
                        id:         ep.id.clone(),
                        item_type:  "Episode".into(),
                        series_id:  ep.series_id.clone().or(Some(id)),
                        title:      ep.display_name(),
                        audio_meta: None,
                    };
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(w) = ww2.upgrade() else { return };
                        let mut vs = video2.lock().unwrap();
                        enqueue_item(&mut vs, item, play_next);
                        crate::push_queue_display(&vs, &AppState::get(&w));
                    });
                }
                Ok(None) => {
                    crate::show_toast(ww2, "No unwatched episodes to queue".to_string());
                }
                Err(e) => {
                    warn!("queue series next-up failed: {e:#}");
                    crate::show_toast(ww2, "Couldn't queue series — check your server connection".to_string());
                }
            }
        });
        return;
    }

    let title = {
        let t = find_title_in_state(&state.lock().unwrap(), &id);
        if t == id {
            // Not in any FjordState vec (e.g. episode from a home row) — try the
            // visible card models before falling back to the raw GUID (CR10-18).
            find_title_in_models(g, &id).unwrap_or(t)
        } else {
            t
        }
    };
    let mut vs = video.lock().unwrap();
    enqueue_item(&mut vs, QueueItem { id, item_type, series_id, title, audio_meta: None }, play_next);
    crate::push_queue_display(&vs, g); // also updates queue-count (CR10-6)
}

pub(crate) fn wire_queue_callbacks(
    window:    &MainWindow,
    state:     Arc<Mutex<FjordState>>,
    video:     Arc<Mutex<VideoState>>,
    rt_handle: tokio::runtime::Handle,
) {
    {
        let state = Arc::clone(&state);
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        AppState::get(window).on_queue_add_item(move || {
            let Some(w) = ww.upgrade() else { return };
            queue_from_context_menu(&AppState::get(&w), &state, &video, &ww, &rt, false);
        });
    }
    {
        let state = Arc::clone(&state);
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        AppState::get(window).on_queue_play_next_item(move || {
            let Some(w) = ww.upgrade() else { return };
            queue_from_context_menu(&AppState::get(&w), &state, &video, &ww, &rt, true);
        });
    }
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;
    match action {
        Action::Back | Action::OpenContextMenu => {
            g.set_show_context_menu(false); true
        }
        Action::Up => {
            let f       = g.get_context_menu_focused();
            let min_row = if g.get_context_menu_resume_pct() > 0.0 && !g.get_context_menu_has_played() { 0 } else { 1 };
            g.set_context_menu_focused(if f <= min_row { 6 } else { f - 1 });
            true
        }
        Action::Down => {
            let f       = g.get_context_menu_focused();
            let min_row = if g.get_context_menu_resume_pct() > 0.0 && !g.get_context_menu_has_played() { 0 } else { 1 };
            g.set_context_menu_focused(if f >= 6 { min_row } else { f + 1 });
            true
        }
        Action::Confirm => {
            let id     = g.get_context_menu_item_id();
            let played = g.get_context_menu_has_played();
            let fav    = g.get_context_menu_is_favorite();
            let itype  = g.get_context_menu_item_type();
            match g.get_context_menu_focused() {
                0 => g.invoke_item_play(id),
                1 => g.invoke_context_play_from_start(id),
                2 => g.invoke_queue_play_next_item(),
                3 => g.invoke_queue_add_item(),
                4 => g.invoke_context_mark_played(id, played),
                5 => g.invoke_context_toggle_fav(id, fav),
                _ => g.invoke_open_detail(id, itype),
            }
            g.set_show_context_menu(false);
            true
        }
        _ => true, // swallow all other keys while the menu is open
    }
}
