// ── fjord-app · context_menu.rs ──────────────────────────────────────────────
//   wire_context_menu   register all AppState context-menu callbacks:
//     open-context-menu             set menu state from CardItem fields
//     open-context-menu-browse      resolve browse index → MediaItem → set state
//     open-context-menu-series-ep   set menu state for a series episode
//     context-mark-played           POST/DELETE /Users/{id}/PlayedItems/{itemId};
//                                   on success: update all models; if played→remove from dynamic rows;
//                                   always call refresh_series_next_up (both mark-played and unplayed)
//     context-toggle-fav            POST/DELETE /Users/{id}/FavoriteItems/{itemId}
//     context-play-from-start       series → get_next_up_for_series (from start); movie/ep → start_position_secs = None;
//                                   BoxSet toasts instead of playing a dead /Videos/{id}/stream URL (CR11-1)
//     context-jf-toggle-watchlist   Watchlist row 8 on the JELLYFIN menu family (2026-07-19,
//                                   real gap live-reported — "you cant add anyting from the
//                                   library to the watchlist"); reuses discover_toggle_watchlist
//                                   (discover.rs) verbatim, sourced from context-menu-jf-tmdb-id/
//                                   -jf-media-type (resolved at open time, see below)
//   resolve_tmdb_for_jellyfin_item  local item id + item_type (Movie/Series only) -> TMDB id +
//                                   "movie"/"tv", via MediaItem.provider_ids["Tmdb"] — the reverse
//                                   of discover.rs::find_local_item's own TMDB-id -> local-item lookup
//   existing_jellyfin_menu_rows      "gaps are fine" row-index list (0=Resume conditional, 1-6
//                                   always, 7=Add to Playlist conditional, 8=Watchlist conditional) —
//                                   replaced a plain min/max range once a SECOND independent optional
//                                   row (8) joined row 7 at the end; a min/max pair can't skip an
//                                   interior gap (7 absent, 8 present)
//   open_context_menu_state         set all context-menu AppState fields incl. series-id + the
//                                   resolved Watchlist fields above (shared by all three open handlers,
//                                   now also takes `state: &Arc<Mutex<FjordState>>` for the TMDB lookup)
//   update_series_unplayed_count    ±1 unplayed-count on the parent series card after mark-played (also called from main.rs)
//   remove_item_from_all_models     rebuild-filter every CardItem model to drop a deleted id (WS ItemsRemoved,
//                                   purge_deleted_item); also clamps library-focused/series-focused-ep/
//                                   season-focused-ep (§0 focus safety) if the removed row was the focused one
//   update_card_in_all_models       patch has-played / is-favorite across every model (incl. series-next-up-cards
//                                   and the album-tracks TrackItem model — track ♥ indicator)
//   remove_from_dynamic_rows        remove item from Next Up/Continue Watching/Not Watched rows;
//                                   matches card.id==id (item) OR card.series_id==id (series → all its episodes);
//                                   does NOT touch series-next-up-cards (refresh_series_next_up handles that)
//   remove_from_favorites           remove item from the three favorite-X rows only (WS IsFavorite=false)
//   remove_from_continue_watching   remove item from Next Up/Continue Watching only (NOT Not Watched — opposite
//                                   membership rule); used for a position-reset-to-0 that isn't also played=true
//   reanchor_focus                  find an item's new index by id after a model mutation, so a keyboard-focus
//                                   index (library-focused/season-focused-ep/series-focused-ep) can follow the
//                                   same logical item instead of pointing at whatever now sits at the old index
//   upsert_cards_in_model           insert/replace items by id into a CardItem model (WS delta-sync merge —
//                                   the upsert counterpart to remove_item_from_all_models); poster-preserving
//   find_title_in_state             scan FjordState media lists by item id → display name
//   enqueue_item                    insert into playlist (play-next) or append to queue
//   queue_from_context_menu         shared add/play-next body; Series resolved to next-up episode (CR10-7);
//                                   MusicAlbum/MusicArtist expanded to their Audio tracks; BoxSet toasts;
//                                   title from context-menu-title (set by every open site), state/model scans as fallback
//   wire_queue_callbacks            on_queue_add_item / on_queue_play_next_item
//   wire_playlist_picker            open-playlist-picker (populate + bg refresh) /
//                                   playlist-picker-select (add to existing) /
//                                   playlist-picker-create (POST /Playlists);
//                                   resolve_music_ids expands MusicAlbum → track ids (empty result toasts, CR11-14);
//                                   refresh_playlists updates state/cache/models after change, and reopens the
//                                   playlist detail screen if it's showing the just-mutated playlist (CR11-7)
//   handle_key                      keyboard dispatch for the context-menu overlay
//                                   (row 7 = Add to Playlist, music items only); branches
//                                   entirely to handle_key_discover_menu when
//                                   context-menu-item-type is Discover* (2026-07-18) — a
//                                   completely different row family, see context_menu.slint
//   existing_discover_menu_rows/handle_key_discover_menu  Discover context menu's own
//                                   Up/Down/Confirm — fixed index scheme (0=View Details,
//                                   5=View Request [requested only — bypasses the
//                                   find_local_item redirect, traversed 2nd despite the
//                                   high index, see the function's own doc comment],
//                                   1=Request/Edit Request, 2=Cancel, 3=Approve, 4=Decline),
//                                   existing_discover_menu_rows resolves which indices exist
//                                   for the current card's request state (same "gaps are
//                                   fine" idiom as the Jellyfin menu's own Resume row) —
//                                   gated against Seerr's REAL per-endpoint permission
//                                   checks (edit/approve/decline need no pending status,
//                                   only cancel does for non-admins; fixed 2026-07-18 after
//                                   a blanket `pending` requirement hid every action on any
//                                   auto-approved request); Confirm dispatches to
//                                   discover.rs's on_context_discover_* handlers, which each
//                                   close the menu themselves (2026-07-18); row 6 = Watchlist
//                                   toggle (2026-07-18, Watchlist + Release Calendar) — always
//                                   visible, unlike Request/Edit/Cancel/Approve/Decline which
//                                   are gated on request state
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, warn};

use fjord_api::JellyfinClient;
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
    // The album/playlist tracklist is TrackItem, not CardItem — patch it too so
    // the row's ♥ indicator / played dimming update without reopening the screen.
    {
        let tracks = g.get_album_tracks();
        for i in 0..tracks.row_count() {
            if let Some(mut t) = tracks.row_data(i) {
                if t.id.as_str() == id {
                    if let Some(p) = played { t.has_played  = p; }
                    if let Some(f) = fav    { t.is_favorite = f; }
                    tracks.set_row_data(i, t);
                    break;
                }
            }
        }
    }
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
    patch_cards(g.get_music_playlists());
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

// Remove an item from EVERY visible CardItem model — used when the server
// reports the item deleted (ws LibraryChanged ItemsRemoved) or a fetch 404s.
// Rebuild-filter (not patch) so the card disappears immediately.
pub(crate) fn remove_item_from_all_models(w: &MainWindow, id: &str) {
    let filter = |model: ModelRc<CardItem>| -> ModelRc<CardItem> {
        let kept: Vec<CardItem> = (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .filter(|c| c.id.as_str() != id)
            .collect();
        ModelRc::new(VecModel::from(kept))
    };
    let g = AppState::get(w);

    // §0 focus safety: capture whether an index-based focus in a currently open
    // screen points at the row about to disappear, so it can be clamped into the
    // new bounds below instead of silently landing on whatever now occupies that
    // slot (library-focused/series-focused-ep/season-focused-ep are all plain
    // integer indices with this exact risk).
    let lib_focus_hit = g.get_show_library()
        && g.get_library_display().row_data(g.get_library_focused().max(0) as usize).is_some_and(|c| c.id.as_str() == id);
    let series_ep_hit = g.get_show_series() && !g.get_series_in_season_row()
        && g.get_series_episode_cards().row_data(g.get_series_focused_ep().max(0) as usize).is_some_and(|c| c.id.as_str() == id);
    let season_ep_hit = g.get_show_season()
        && g.get_series_episode_cards().row_data(g.get_season_focused_ep().max(0) as usize).is_some_and(|c| c.id.as_str() == id);

    g.set_continue_watching(filter(g.get_continue_watching()));
    g.set_next_up(filter(g.get_next_up()));
    g.set_recently_added(filter(g.get_recently_added()));
    g.set_recently_added_movies(filter(g.get_recently_added_movies()));
    g.set_continue_watching_movies(filter(g.get_continue_watching_movies()));
    g.set_not_watched_movies(filter(g.get_not_watched_movies()));
    g.set_continue_watching_tv(filter(g.get_continue_watching_tv()));
    g.set_recently_added_tv(filter(g.get_recently_added_tv()));
    g.set_not_watched_tv(filter(g.get_not_watched_tv()));
    g.set_recently_added_collections(filter(g.get_recently_added_collections()));
    g.set_unwatched_collections(filter(g.get_unwatched_collections()));
    g.set_recently_added_albums(filter(g.get_recently_added_albums()));
    g.set_recently_played_albums(filter(g.get_recently_played_albums()));
    g.set_favorite_movies(filter(g.get_favorite_movies()));
    g.set_favorite_series(filter(g.get_favorite_series()));
    g.set_favorite_albums(filter(g.get_favorite_albums()));
    g.set_music_playlists(filter(g.get_music_playlists()));
    g.set_all_movies(filter(g.get_all_movies()));
    g.set_all_series(filter(g.get_all_series()));
    g.set_all_collections(filter(g.get_all_collections()));
    g.set_all_artists(filter(g.get_all_artists()));
    g.set_all_albums(filter(g.get_all_albums()));
    g.set_all_playlists(filter(g.get_all_playlists()));
    g.set_library_display(filter(g.get_library_display()));
    g.set_series_episode_cards(filter(g.get_series_episode_cards()));
    g.set_series_next_up_cards(filter(g.get_series_next_up_cards()));

    if lib_focus_hit {
        let len = g.get_library_display().row_count() as i32;
        g.set_library_focused(g.get_library_focused().clamp(0, (len - 1).max(0)));
    }
    if series_ep_hit {
        let len = g.get_series_episode_cards().row_count() as i32;
        g.set_series_focused_ep(g.get_series_focused_ep().clamp(0, (len - 1).max(0)));
    }
    if season_ep_hit {
        let len = g.get_series_episode_cards().row_count() as i32;
        g.set_season_focused_ep(g.get_season_focused_ep().clamp(0, (len - 1).max(0)));
    }
    g.set_collection_items(filter(g.get_collection_items()));
    g.set_detail_similar(filter(g.get_detail_similar()));
    g.set_detail_collection(filter(g.get_detail_collection()));
    g.set_series_similar(filter(g.get_series_similar()));
    g.set_person_filmography(filter(g.get_person_filmography()));
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

/// Remove an id from the four "in progress" rows (Next Up/Continue Watching) only — used
/// when playback position resets to 0 WITHOUT the item being marked played. Deliberately
/// distinct from remove_from_dynamic_rows: Not Watched rows have the OPPOSITE membership
/// rule from Continue Watching (untouched = position 0, vs in-progress = position > 0), so
/// folding a position-reset into remove_from_dynamic_rows's shared played-or-position==0
/// condition incorrectly stripped a freshly-favorited-but-never-watched item out of Not
/// Watched purely because its position already happens to be 0 — not because anything
/// about its watch state actually changed (found while investigating a WS delta-sync
/// dashboard-flash report, Phase 89 follow-up).
pub(crate) fn remove_from_continue_watching(w: &MainWindow, id: &str) {
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
}

/// Remove an id from the three favorites rows only — used when a WS UserDataChanged
/// event reports IsFavorite=false, distinct from remove_from_dynamic_rows (which is
/// about played/resume state, not favorite status; a played favorite should stay put).
pub(crate) fn remove_from_favorites(w: &MainWindow, id: &str) {
    let filter = |model: ModelRc<CardItem>| -> ModelRc<CardItem> {
        let kept: Vec<CardItem> = (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .filter(|c| c.id.as_str() != id)
            .collect();
        ModelRc::new(VecModel::from(kept))
    };
    let g = AppState::get(w);
    g.set_favorite_movies(filter(g.get_favorite_movies()));
    g.set_favorite_series(filter(g.get_favorite_series()));
    g.set_favorite_albums(filter(g.get_favorite_albums()));
}

// Find the new index of the item with a given id after a model mutation, so a
// keyboard-focus index can follow the same logical item instead of silently
// pointing at whatever now occupies its old position (library-focused,
// season-focused-ep, series-focused-ep are all plain integer indices with
// this exact risk). Returns None if the item is no longer present — callers
// should fall back to a screen-appropriate safe reset, not a stale index.
pub(crate) fn reanchor_focus(model: &ModelRc<CardItem>, focused_id: &str) -> Option<usize> {
    (0..model.row_count()).find(|&i| model.row_data(i).is_some_and(|c| c.id.as_str() == focused_id))
}

/// Insert/replace `items` by id into a CardItem model — the WS delta-sync counterpart to
/// remove_item_from_all_models's rebuild-filter, upserting instead of removing. `posters`
/// supplies already-decoded art for the delta (from poster::fetch_posters_for_delta, keyed by
/// item id); a miss falls back to whatever poster the row already had rather than flashing to
/// no-poster (fetch_posters_for_delta re-resolves every item in the batch including unchanged
/// ones, so a miss here should be rare — only on a fetch failure — not the common case). The
/// actual model apply is delegated to crate::apply_cards_preserving_identity (Phase 96, shared
/// with poster.rs/movies.rs/home.rs) so an upsert-only batch (no new rows) mutates in place.
pub(crate) fn upsert_cards_in_model(
    model:   ModelRc<CardItem>,
    items:   &[fjord_api::models::MediaItem],
    posters: &std::collections::HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
) -> ModelRc<CardItem> {
    let mut rows: Vec<CardItem> = (0..model.row_count()).filter_map(|i| model.row_data(i)).collect();
    for item in items {
        let mut card = crate::item_to_card_item(item);
        if let Some(buf) = posters.get(&item.id) {
            card.poster = slint::Image::from_rgba8(buf.clone());
            card.has_poster = true;
        }
        match rows.iter_mut().find(|c| c.id.as_str() == item.id.as_str()) {
            Some(existing) => {
                if !card.has_poster && existing.has_poster {
                    card.poster     = existing.poster.clone();
                    card.has_poster = true;
                }
                *existing = card;
            }
            None => rows.push(card),
        }
    }
    // Delegate the apply to the shared primitive (Phase 96): when nothing was
    // appended, `rows` has the exact same ids in the exact same order as `model`
    // already had, so it mutates in place instead of destroying/recreating every
    // OTHER card's poster Image too — a WS delta batch usually only touches one
    // or two items in a grid of hundreds.
    crate::apply_cards_preserving_identity(&model, rows)
}

/// Movie/Series only, matching Seerr's own Watchlist mediaType enum (movie|tv)
/// — an Episode/BoxSet/MusicAlbum/etc has no sensible Seerr counterpart.
/// Scans the same `all_movies`/`all_series` lists `discover.rs::find_local_item`
/// already scans in the opposite direction (TMDB id -> local item); this is
/// the reverse (local item -> TMDB id), via the same `provider_ids["Tmdb"]`
/// field. Real gap, live-reported 2026-07-19 ("you cant add anyting from the
/// library to the watchlist") — an already-in-library item redirects
/// straight to this Jellyfin-flavored menu, which never had a Watchlist row
/// at all; Discover's own Watchlist toggle only ever lived on the separate
/// Discover-card menu family, unreachable once `find_local_item` redirects.
fn resolve_tmdb_for_jellyfin_item(s: &FjordState, id: &str, item_type: &str) -> Option<(String, &'static str)> {
    let (list, media_type): (&[fjord_api::models::MediaItem], &'static str) = match item_type {
        "Movie" => (&s.all_movies, "movie"),
        "Series" => (&s.all_series, "tv"),
        _ => return None,
    };
    list.iter()
        .find(|m| m.id == id)
        .and_then(|m| m.provider_ids.get("Tmdb"))
        .map(|tmdb_id| (tmdb_id.clone(), media_type))
}

// Bundled to keep open_context_menu_state under clippy's too-many-arguments
// threshold (8 > 7) once the state param joined the original 6 CardItem-ish
// fields, 2026-07-19 — same "group loose scalars into one struct" fix this
// codebase already applied to movie_details_to_meta/tv_details_to_meta
// (discover.rs) for the identical reason.
struct OpenMenuArgs {
    id: SharedString,
    item_type: SharedString,
    played: bool,
    is_fav: bool,
    resume_pct: f32,
    series_id: SharedString,
}

fn open_context_menu_state(g: &AppState, state: &Arc<Mutex<FjordState>>, args: OpenMenuArgs) {
    let OpenMenuArgs { id, item_type, played, is_fav, resume_pct, series_id } = args;
    g.set_context_menu_item_id(id.clone());
    g.set_context_menu_item_type(item_type.clone());
    g.set_context_menu_series_id(series_id);
    g.set_context_menu_has_played(played);
    g.set_context_menu_is_favorite(is_fav);
    g.set_context_menu_resume_pct(resume_pct);
    {
        let s = state.lock().unwrap();
        match resolve_tmdb_for_jellyfin_item(&s, id.as_str(), item_type.as_str()) {
            Some((tmdb_id, media_type)) => {
                let on_watchlist = s.discover_watchlist_ids.contains(&(if media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" }, tmdb_id.clone()));
                g.set_context_menu_jf_tmdb_id(tmdb_id.into());
                g.set_context_menu_jf_media_type(media_type.into());
                g.set_context_menu_on_watchlist(on_watchlist);
            }
            None => {
                g.set_context_menu_jf_tmdb_id("".into());
                g.set_context_menu_jf_media_type("".into());
                g.set_context_menu_on_watchlist(false);
            }
        }
    }
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
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        AppState::get(window).on_open_context_menu(move |id, has_played, is_fav, resume_pct, item_type, series_id| {
            let Some(w) = ww.upgrade() else { return };
            open_context_menu_state(&AppState::get(&w), &state, OpenMenuArgs { id, item_type, played: has_played, is_fav, resume_pct, series_id });
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
            let title      = SharedString::from(item.display_name());
            drop(s);
            let g = AppState::get(&w);
            g.set_context_menu_title(title);
            open_context_menu_state(&g, &state, OpenMenuArgs { id, item_type, played, is_fav, resume_pct, series_id });
        });
    }

    // ── open-context-menu-series-ep: episode C-key context menu ─────────────
    {
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        AppState::get(window).on_open_context_menu_series_ep(move |id, has_played, is_fav, resume_pct, series_id| {
            let Some(w) = ww.upgrade() else { return };
            open_context_menu_state(&AppState::get(&w), &state, OpenMenuArgs { id, item_type: "Episode".into(), played: has_played, is_fav, resume_pct, series_id });
        });
    }

    // ── context-jf-toggle-watchlist: Watchlist row on the JELLYFIN menu
    // family (row 8, 2026-07-19) — resolved at open time into
    // context-menu-jf-tmdb-id/-jf-media-type by open_context_menu_state
    // above; reuses discover_toggle_watchlist (discover.rs) verbatim, the
    // same function the Discover-card menu's own Watchlist row calls —
    // watchlisting is a plain TMDB-id action with no Jellyfin-vs-Discover
    // distinction once the id is known.
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        AppState::get(window).on_context_jf_toggle_watchlist(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let tmdb_id_str = g.get_context_menu_jf_tmdb_id();
            let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else { return };
            let media_type = g.get_context_menu_jf_media_type().to_string();
            let adding = !g.get_context_menu_on_watchlist();
            let title = g.get_context_menu_title().to_string();
            crate::discover::discover_toggle_watchlist(Arc::clone(&state), ww.clone(), rt.clone(), tmdb_id, media_type, title, adding);
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

            // Music containers: a raw album/artist/playlist id has no stream —
            // handing it to mpv opened the VIDEO player on a dead URL. Build the
            // track playlist and start the music player instead (Play All).
            let ctype = ww.upgrade()
                .map(|w| AppState::get(&w).get_context_menu_item_type().to_string())
                .unwrap_or_default();
            // BoxSets have no stream of their own — the "Add to Queue" path already
            // guards against this (CR11-1); this entry point never got the same check,
            // so Enter on a fresh context menu (defaults to this row) played a dead URL.
            if ctype == "BoxSet" {
                crate::show_toast(ww.clone(), "Open the collection to play an item".to_string());
                return;
            }
            if matches!(ctype.as_str(), "MusicAlbum" | "MusicArtist" | "Playlist") {
                let mut config = s.player_config();
                drop(s);
                config.start_position_secs = None;
                let video2 = Arc::clone(&video);
                let ww2    = ww.clone();
                let rt2    = rt.clone();
                rt.spawn(async move {
                    let tracks = match music_container_tracks(&client, &id, &ctype).await {
                        Ok(v) if !v.is_empty() => v,
                        Ok(_)  => { crate::show_toast(ww2, "No tracks to play".to_string()); return; }
                        Err(e) => {
                            warn!("play-from-start container tracks: {e:#}");
                            crate::show_toast(ww2, "Couldn't play — check your server connection".to_string());
                            return;
                        }
                    };
                    let first = tracks[0].clone();
                    let url   = client.direct_play_url(&first.id);
                    let _ = slint::invoke_from_event_loop(move || {
                        {
                            let mut vs = video2.lock().unwrap();
                            // Rebuild the playlist but keep vs.queue (Phase 56).
                            vs.playlist       = tracks;
                            vs.playlist_index = 0;
                            vs.shuffle_order.clear();
                            crate::playback::rebuild_shuffle_order(&mut vs);
                            if let Some(w) = ww2.upgrade() {
                                crate::push_queue_display(&vs, &AppState::get(&w));
                            }
                        }
                        start_playback(url, first.id.clone(), "Audio", first.title.clone(),
                                       config, client, None, first.audio_meta.clone(),
                                       &video2, &ww2, &rt2);
                    });
                });
                return;
            }

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
    // Upcoming order changes — drop any gapless-preloaded entry first.
    crate::playback::invalidate_preload(vs);
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

    // The open site always sets context-menu-title from the card/track it was
    // opened on — the state/model scans are only a fallback (album tracks and
    // dashboard albums are in neither, which used to surface the raw GUID).
    // MusicAlbum / MusicArtist: expand to their Audio tracks — a raw album or
    // artist id has no stream, so enqueueing it verbatim produced an unplayable
    // row (and its GUID as the title). Same class of bug as Series (CR10-7).
    if item_type == "MusicAlbum" || item_type == "MusicArtist" || item_type == "Playlist" {
        let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
        let video2    = Arc::clone(video);
        let ww2       = ww.clone();
        let itype2 = item_type.clone();
        rt.spawn(async move {
            let items = match music_container_tracks(&client, &id, &itype2).await {
                Ok(v)  => v,
                Err(e) => {
                    warn!("queue container tracks failed: {e:#}");
                    crate::show_toast(ww2, "Couldn't queue — check your server connection".to_string());
                    return;
                }
            };
            if items.is_empty() {
                crate::show_toast(ww2, "No tracks to queue".to_string());
                return;
            }
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww2.upgrade() else { return };
                let mut vs = video2.lock().unwrap();
                if play_next {
                    // enqueue_item(play_next) inserts each track right after the
                    // current position — reverse iteration keeps album order.
                    for item in items.into_iter().rev() { enqueue_item(&mut vs, item, true); }
                } else {
                    for item in items { enqueue_item(&mut vs, item, false); }
                }
                crate::push_queue_display(&vs, &AppState::get(&w));
            });
        });
        return;
    }

    if item_type == "BoxSet" {
        crate::show_toast(ww.clone(), "Collections can't be queued".to_string());
        return;
    }

    let title = {
        let t = g.get_context_menu_title().to_string();
        if !t.is_empty() {
            t
        } else {
            let t = find_title_in_state(&state.lock().unwrap(), &id);
            if t == id {
                find_title_in_models(g, &id).unwrap_or(t)
            } else {
                t
            }
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

/// Which row indices exist for the current Discover card's request state —
/// fixed index scheme, same "some indices can be absent" idiom the Jellyfin
/// menu already uses for Resume (row 0 only when resumable): 0=View
/// Details (always), 5=View Request (requested — see below), 1=Request (not
/// yet requested) OR Edit Request (requested, and mine or admin),
/// 2=Cancel Request (requested, and admin or (mine and pending)),
/// 3=Approve/4=Decline (requested and admin). Row 5 sits between 0 and 1 in
/// the returned Vec (Up/Down visits it right after View Details) even
/// though its numeric index is highest — index values are pure keyboard-
/// focus identities matched against `context_menu.slint`'s own `row-index`,
/// not a visual-order or Up/Down-order constraint, so it can be appended at
/// the end of the enum-ish numbering (avoiding renumbering 1-4) while still
/// being traversed second.
///
/// Gating was originally tied to `pending` across the board — wrong, and a
/// real live-reported bug (2026-07-18: "I don't get the remove request on a
/// requested item" / "on requested 4k items I only got detail"). Re-checked
/// against Seerr's actual route source (`server/routes/request.ts`) rather
/// than re-guessing: `PUT /request/:id` (edit) requires only ownership or
/// `MANAGE_REQUESTS` — no status check at all; `POST /request/:id/approve|
/// decline` requires only `MANAGE_REQUESTS` — also no status check; only
/// `DELETE /request/:id` (cancel) actually restricts a non-admin to
/// `status == PENDING`. Once a request auto-approves (a very common Seerr
/// config — and evidently this user's own 4K setup), it leaves Pending
/// within seconds, so the old blanket `pending` requirement silently hid
/// Edit/Cancel/Approve/Decline almost immediately after every request,
/// even for the connected account's own `MANAGE_REQUESTS` admin, who the
/// server would have allowed to act on it regardless of status.
///
/// Row 5 (View Request) is a separate later fix (2026-07-18): a card can be
/// `requested` AND also (partially) present in the local Jellyfin library
/// (e.g. a series missing some seasons) — View Details/Request/Edit Request
/// all redirect to the real Jellyfin item in that case (unchanged, by the
/// user's own choice — see `open_discover_item_ex`'s doc comment), which
/// left no way back to Seerr's own Request Detail screen. View Request is
/// the dedicated escape hatch: shown whenever a request exists, regardless
/// of local-library presence, and always opens the Seerr side.
fn existing_discover_menu_rows(g: &AppState) -> Vec<i32> {
    let requested = !g.get_context_menu_request_id().as_str().is_empty();
    let pending = g.get_context_menu_request_pending();
    let mine = g.get_context_menu_request_mine();
    let admin = g.get_seerr_is_admin();
    let mut rows = vec![0];
    if requested {
        rows.push(5); // View Request
    }
    if !requested || mine || admin {
        rows.push(1); // Request (not yet requested) or Edit Request
    }
    if requested && (admin || (mine && pending)) {
        rows.push(2); // Cancel Request
    }
    if requested && admin {
        rows.push(3); // Approve
        rows.push(4); // Decline
    }
    rows.push(6); // Watchlist (2026-07-18) — always visible
    debug!(
        "seerr: discover menu rows requested={requested} pending={pending} mine={mine} admin={admin} -> {rows:?}"
    );
    rows
}

fn handle_key_discover_menu(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;
    let rows = existing_discover_menu_rows(g);
    match action {
        Action::Back | Action::OpenContextMenu => {
            g.set_show_context_menu(false);
            true
        }
        Action::Up => {
            let pos = rows.iter().position(|&r| r == g.get_context_menu_focused()).unwrap_or(0);
            g.set_context_menu_focused(rows[if pos == 0 { rows.len() - 1 } else { pos - 1 }]);
            true
        }
        Action::Down => {
            let pos = rows.iter().position(|&r| r == g.get_context_menu_focused()).unwrap_or(0);
            g.set_context_menu_focused(rows[if pos + 1 >= rows.len() { 0 } else { pos + 1 }]);
            true
        }
        Action::Confirm => {
            // Each invoke_context_discover_* handler (discover.rs) closes
            // the menu itself on activation — not repeated here.
            match g.get_context_menu_focused() {
                0 => g.invoke_context_discover_view_details(),
                1 => {
                    if g.get_context_menu_request_id().as_str().is_empty() {
                        g.invoke_context_discover_request();
                    } else {
                        g.invoke_context_discover_edit_request();
                    }
                }
                2 => g.invoke_context_discover_cancel_request(),
                3 => g.invoke_context_discover_approve_request(),
                4 => g.invoke_context_discover_decline_request(),
                5 => g.invoke_context_discover_view_request(),
                _ => {}
            }
            true
        }
        _ => true, // swallow all other keys while the menu is open
    }
}

/// Fixed row indices, "gaps are fine" idiom (same shape as Discover's own
/// `existing_discover_menu_rows`): 0=Resume(conditional on resume-pct>0 &&
/// !played) 1=Play from Start 2=Play Next 3=Add to Queue 4=Mark Played
/// 5=Favourite 6=View Details 7=Add to Playlist(conditional, music items
/// only) 8=Watchlist(conditional, resolvable TMDB id + Seerr connected —
/// 2026-07-19, real gap live-reported: an already-in-library item redirects
/// to this Jellyfin-flavored menu, which never had a Watchlist row before).
/// A simple min/max range (the original shape) stopped being correct once a
/// SECOND independent optional row joined row 7 at the end — row 7 absent
/// with row 8 present is a genuine interior gap a min/max pair can't skip.
fn existing_jellyfin_menu_rows(g: &AppState) -> Vec<i32> {
    let mut rows = Vec::with_capacity(9);
    if g.get_context_menu_resume_pct() > 0.0 && !g.get_context_menu_has_played() {
        rows.push(0);
    }
    rows.extend([1, 2, 3, 4, 5, 6]);
    if is_music_type(g.get_context_menu_item_type().as_str()) {
        rows.push(7);
    }
    if !g.get_context_menu_jf_tmdb_id().is_empty() && g.get_seerr_connected() {
        rows.push(8);
    }
    rows
}

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    if g.get_context_menu_item_type().as_str().starts_with("Discover") {
        return handle_key_discover_menu(action, g);
    }
    use crate::keys::Action;
    match action {
        Action::Back | Action::OpenContextMenu => {
            g.set_show_context_menu(false); true
        }
        Action::Up => {
            let rows = existing_jellyfin_menu_rows(g);
            let f = g.get_context_menu_focused();
            let pos = rows.iter().position(|&r| r == f).unwrap_or(0);
            g.set_context_menu_focused(rows[if pos == 0 { rows.len() - 1 } else { pos - 1 }]);
            true
        }
        Action::Down => {
            let rows = existing_jellyfin_menu_rows(g);
            let f = g.get_context_menu_focused();
            let pos = rows.iter().position(|&r| r == f).unwrap_or(0);
            g.set_context_menu_focused(rows[if pos + 1 >= rows.len() { 0 } else { pos + 1 }]);
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
                6 => g.invoke_open_detail(id, itype),
                7 => g.invoke_open_playlist_picker(),
                8 => g.invoke_context_jf_toggle_watchlist(),
                _ => {}
            }
            g.set_show_context_menu(false);
            true
        }
        _ => true, // swallow all other keys while the menu is open
    }
}

// ── Add-to-playlist picker ────────────────────────────────────────────────────

fn is_music_type(t: &str) -> bool {
    matches!(t, "Audio" | "MusicAlbum")
}

// Flatten a music container (MusicAlbum / MusicArtist / Playlist) into playable
// Audio QueueItems in play order. Errors on the container-level fetch bubble up;
// a single failing album inside an artist is warned and skipped.
async fn music_container_tracks(
    client:    &JellyfinClient,
    id:        &str,
    item_type: &str,
) -> anyhow::Result<Vec<QueueItem>> {
    if item_type == "Playlist" {
        return Ok(client.get_playlist_items(id).await?
            .into_iter()
            .filter(|t| t.item_type == "Audio")
            .map(|t| QueueItem {
                id:         t.id.clone(),
                item_type:  "Audio".into(),
                series_id:  None,
                title:      t.name.clone(),
                audio_meta: Some((t.album_artist.clone().unwrap_or_default(),
                                  t.album_id.clone().unwrap_or_default())),
            })
            .collect());
    }
    let album_ids: Vec<String> = if item_type == "MusicArtist" {
        client.get_artist_albums(id).await?.into_iter().map(|a| a.id).collect()
    } else {
        vec![id.to_string()]
    };
    let mut items = Vec::new();
    for album_id in &album_ids {
        match client.get_album_tracks(album_id).await {
            Ok(tracks) => {
                for t in tracks {
                    items.push(QueueItem {
                        id:         t.id.clone(),
                        item_type:  "Audio".into(),
                        series_id:  None,
                        title:      t.display_name(),
                        audio_meta: Some((t.album_artist.clone().unwrap_or_default(),
                                          album_id.clone())),
                    });
                }
            }
            Err(e) => warn!("container tracks({album_id}): {e:#}"),
        }
    }
    Ok(items)
}

// Target items for playlist add: a track is itself; an album is its tracks.
async fn resolve_music_ids(
    client:    &JellyfinClient,
    id:        String,
    item_type: &str,
) -> anyhow::Result<Vec<String>> {
    if item_type == "MusicAlbum" {
        Ok(client.get_album_tracks(&id).await?.into_iter().map(|t| t.id).collect())
    } else {
        Ok(vec![id])
    }
}

fn playlist_items_model(playlists: &[fjord_api::models::MediaItem]) -> ModelRc<CardItem> {
    let items: Vec<CardItem> = playlists.iter().map(|p| {
        CardItem {
            id:        p.id.as_str().into(),
            item_type: "Playlist".into(),
            title:     p.name.as_str().into(),
            subtitle:  p.card_subtitle().as_str().into(),
            ..Default::default()
        }
    }).collect();
    ModelRc::new(VecModel::from(items))
}

// Background refresh of the playlist list after create/add (ChildCount changed,
// new playlist appeared): updates FjordState + disk cache + all-playlists model
// + the open library grid + the picker list if it is still open.
fn refresh_playlists(
    state:      Arc<Mutex<FjordState>>,
    ww:         slint::Weak<MainWindow>,
    rt:         tokio::runtime::Handle,
    mutated_id: Option<String>,
) {
    let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
    let rt_task = rt.clone();
    rt.spawn(async move {
        match client.get_all_playlists().await {
            Ok(playlists) => {
                {
                    let mut s = state.lock().unwrap();
                    s.all_playlists     = playlists.clone();
                    s.playlists_fetched = true;
                }
                crate::home::save_playlists_cache(&playlists);
                let state2 = Arc::clone(&state);
                let ww2    = ww.clone();
                let rt2    = rt_task.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_all_playlists(crate::items_to_model(&playlists));
                    if g.get_show_playlist_picker() {
                        g.set_playlist_picker_items(playlist_items_model(&playlists));
                    }
                    if g.get_show_library() && g.get_active_nav() == 4 && g.get_library_music_view() == 2 {
                        crate::browse::refresh_library_display(&w);
                    }
                    // The open playlist detail screen isn't covered by any model
                    // above — reopen it so its tracklist reflects the change (CR11-7).
                    if let Some(id) = mutated_id {
                        if g.get_show_album() && g.get_album_is_playlist() && g.get_album_id() == id.as_str() {
                            let title = g.get_album_title().to_string();
                            // Invalidate container_tracks_cache first — without this the
                            // "reopen" below just re-serves the pre-mutation cached
                            // tracklist (open_music_screen skips its network fetch on a
                            // cache hit), silently defeating the whole point of this
                            // reopen: the track the user just added stays invisible on
                            // the exact screen they're looking at.
                            state2.lock().unwrap().container_tracks_cache.remove(&id);
                            crate::album::open_playlist_screen(id, title, state2, ww2, rt2);
                        }
                    }
                });
            }
            Err(e) => warn!("refresh_playlists: {e:#}"),
        }
    });
}

pub(crate) fn wire_playlist_picker(
    window:    &MainWindow,
    state:     Arc<Mutex<FjordState>>,
    rt_handle: tokio::runtime::Handle,
) {
    // ── open: populate from FjordState, refresh in background ────────────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        AppState::get(window).on_open_playlist_picker(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let playlists = state.lock().unwrap().all_playlists.clone();
            g.set_playlist_picker_items(playlist_items_model(&playlists));
            g.set_playlist_picker_cursor(if playlists.is_empty() { 0 } else { 1 });
            g.set_playlist_picker_naming(false);
            g.set_playlist_picker_name("".into());
            g.set_show_playlist_picker(true);
            // The list may be stale (grid never opened this session) — refresh.
            refresh_playlists(Arc::clone(&state), ww.clone(), rt.clone(), None);
        });
    }

    // ── select: add the context-menu item to an existing playlist ────────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        AppState::get(window).on_playlist_picker_select(move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Some(pl) = g.get_playlist_picker_items().row_data(idx as usize) else { return };
            let pl_id     = pl.id.to_string();
            let pl_name   = pl.title.to_string();
            let target    = g.get_context_menu_item_id().to_string();
            let item_type = g.get_context_menu_item_type().to_string();
            g.set_show_playlist_picker(false);
            let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
            let state2 = Arc::clone(&state);
            let ww2    = ww.clone();
            let rt2    = rt.clone();
            rt.spawn(async move {
                let ids = match resolve_music_ids(&client, target, &item_type).await {
                    Ok(v) if !v.is_empty() => v,
                    Ok(_)  => { crate::show_toast(ww2, "Nothing to add".to_string()); return; }
                    Err(e) => {
                        warn!("playlist add resolve: {e:#}");
                        crate::show_toast(ww2, "Couldn't add to playlist — check your server connection".to_string());
                        return;
                    }
                };
                match client.add_to_playlist(&pl_id, &ids).await {
                    Ok(())  => {
                        crate::show_toast(ww2.clone(), format!("Added to {pl_name}"));
                        refresh_playlists(state2, ww2, rt2, Some(pl_id));
                    }
                    Err(e) => {
                        warn!("add_to_playlist: {e:#}");
                        crate::show_toast(ww2, "Couldn't add to playlist — check your server connection".to_string());
                    }
                }
            });
        });
    }

    // ── create: new playlist named playlist-picker-name with the item ────────
    {
        let state = Arc::clone(&state);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        AppState::get(window).on_playlist_picker_create(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let name = g.get_playlist_picker_name().trim().to_string();
            if name.is_empty() {
                crate::show_toast(ww.clone(), "Enter a playlist name".to_string());
                return;
            }
            let target    = g.get_context_menu_item_id().to_string();
            let item_type = g.get_context_menu_item_type().to_string();
            g.set_show_playlist_picker(false);
            let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
            let state2 = Arc::clone(&state);
            let ww2    = ww.clone();
            let rt2    = rt.clone();
            rt.spawn(async move {
                let ids = match resolve_music_ids(&client, target, &item_type).await {
                    Ok(v) if !v.is_empty() => v,
                    Ok(_)  => { crate::show_toast(ww2, "Nothing to add".to_string()); return; }
                    Err(e) => {
                        warn!("playlist create resolve: {e:#}");
                        crate::show_toast(ww2, "Couldn't create playlist — check your server connection".to_string());
                        return;
                    }
                };
                match client.create_playlist(&name, &ids).await {
                    Ok(_)  => {
                        crate::show_toast(ww2.clone(), format!("Created playlist {name}"));
                        refresh_playlists(state2, ww2, rt2, None);
                    }
                    Err(e) => {
                        warn!("create_playlist: {e:#}");
                        crate::show_toast(ww2, "Couldn't create playlist — check your server connection".to_string());
                    }
                }
            });
        });
    }
}
