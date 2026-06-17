// ── fjord-app · context_menu.rs ──────────────────────────────────────────────
//   wire_context_menu   register all AppState context-menu callbacks:
//     open-context-menu             set menu state from CardItem fields
//     open-context-menu-browse      resolve browse index → MediaItem → set state
//     open-context-menu-series-ep   set menu state for a series episode
//     context-mark-played           POST/DELETE /Users/{id}/PlayedItems/{itemId}
//     context-toggle-fav            POST/DELETE /Users/{id}/FavoriteItems/{itemId}
//     context-play-from-start       series → get_next_up_for_series (from start); movie/ep → start_position_secs = None
//   update_card_in_all_models       patch has-played / is-favorite across every model
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::playback::{VideoState, start_playback};
use crate::series::open_series_screen;
use crate::{AppState, CardItem, EpisodeEntry, MainWindow};

// Patch every dashboard row, library grid, and episode list; called after a successful API toggle.
fn update_card_in_all_models(w: &MainWindow, id: &str, played: Option<bool>, fav: Option<bool>) {
    let patch_cards = |model: ModelRc<CardItem>| -> ModelRc<CardItem> {
        let mut hit = false;
        let items: Vec<CardItem> = (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .map(|mut c| {
                if c.id.as_str() == id {
                    if let Some(p) = played { c.has_played  = p; }
                    if let Some(f) = fav    { c.is_favorite = f; }
                    hit = true;
                }
                c
            })
            .collect();
        if hit { ModelRc::new(VecModel::from(items)) } else { model }
    };

    let patch_episodes = |model: ModelRc<EpisodeEntry>| -> ModelRc<EpisodeEntry> {
        let mut hit = false;
        let items: Vec<EpisodeEntry> = (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .map(|mut e| {
                if e.id.as_str() == id {
                    if let Some(p) = played { e.has_played  = p; }
                    if let Some(f) = fav    { e.is_favorite = f; }
                    hit = true;
                }
                e
            })
            .collect();
        if hit { ModelRc::new(VecModel::from(items)) } else { model }
    };

    let g = AppState::get(w);
    g.set_continue_watching(patch_cards(g.get_continue_watching()));
    g.set_next_up(patch_cards(g.get_next_up()));
    g.set_recently_added(patch_cards(g.get_recently_added()));
    g.set_recently_added_movies(patch_cards(g.get_recently_added_movies()));
    g.set_continue_watching_movies(patch_cards(g.get_continue_watching_movies()));
    g.set_not_watched_movies(patch_cards(g.get_not_watched_movies()));
    g.set_continue_watching_tv(patch_cards(g.get_continue_watching_tv()));
    g.set_recently_added_tv(patch_cards(g.get_recently_added_tv()));
    g.set_not_watched_tv(patch_cards(g.get_not_watched_tv()));
    g.set_all_movies(patch_cards(g.get_all_movies()));
    g.set_all_series(patch_cards(g.get_all_series()));
    g.set_library_display(patch_cards(g.get_library_display()));
    g.set_series_episodes(patch_episodes(g.get_series_episodes()));
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
        AppState::get(window).on_open_context_menu(move |id, has_played, is_fav, resume_pct, item_type| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_context_menu_item_id(id);
            g.set_context_menu_item_type(item_type);
            g.set_context_menu_has_played(has_played);
            g.set_context_menu_is_favorite(is_fav);
            g.set_context_menu_resume_pct(resume_pct);
            g.set_context_menu_focused(if resume_pct > 0.0 && !has_played { 0 } else { 1 });
            g.set_show_context_menu(true);
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
            drop(s);
            let g = AppState::get(&w);
            g.set_context_menu_item_id(id);
            g.set_context_menu_item_type(item_type);
            g.set_context_menu_has_played(played);
            g.set_context_menu_is_favorite(is_fav);
            g.set_context_menu_resume_pct(resume_pct);
            g.set_context_menu_focused(if resume_pct > 0.0 && !played { 0 } else { 1 });
            g.set_show_context_menu(true);
        });
    }

    // ── open-context-menu-series-ep: episode C-key context menu ─────────────
    {
        let ww = window.as_weak();
        AppState::get(window).on_open_context_menu_series_ep(move |id, has_played, is_fav, resume_pct| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_context_menu_item_id(id);
            g.set_context_menu_item_type("Episode".into());
            g.set_context_menu_has_played(has_played);
            g.set_context_menu_is_favorite(is_fav);
            g.set_context_menu_resume_pct(resume_pct);
            g.set_context_menu_focused(if resume_pct > 0.0 && !has_played { 0 } else { 1 });
            g.set_show_context_menu(true);
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
            let id2    = id.to_string();
            let ww2    = ww.clone();
            let state2 = Arc::clone(&state);
            rt.spawn(async move {
                let result = if currently_played {
                    client.mark_unplayed(&id2).await
                } else {
                    client.mark_played(&id2).await
                };
                if let Err(e) = result {
                    warn!("mark played/unplayed failed: {e}");
                } else {
                    let new_played = !currently_played;
                    state2.lock().unwrap().update_item_user_state(&id2, Some(new_played), None);
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() {
                            AppState::get(&w).set_context_menu_has_played(new_played);
                            update_card_in_all_models(&w, &id2, Some(new_played), None);
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
                } else {
                    let new_fav = !currently_fav;
                    state2.lock().unwrap().update_item_user_state(&id2, None, Some(new_fav));
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() {
                            AppState::get(&w).set_context_menu_is_favorite(new_fav);
                            update_card_in_all_models(&w, &id2, None, Some(new_fav));
                        }
                    });
                }
            });
        });
    }

    // ── context-play-from-start: play with no resume position ────────────────
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
                        let series_id = next.series_id.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            start_playback(url, ep_id, "Episode", title, config, cli2,
                                           series_id, &video2, &ww2, &rt2);
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
                let detail    = client.get_item_detail(&id).await.ok();
                let item_type = detail.as_ref().map(|i| i.item_type.clone()).unwrap_or_default();
                let series_id = detail.as_ref().and_then(|i| i.series_id.clone());
                let title     = detail.map(|i| i.display_name()).unwrap_or_else(|| id.clone());
                let _ = slint::invoke_from_event_loop(move || {
                    start_playback(play_url, id, &item_type, title, config, client,
                                   series_id, &video2, &ww2, &rt2);
                });
            });
        });
    }
}
