// ── fjord-app · collection.rs ─────────────────────────────────────────────────
//   open_collection_screen  reset AppState collection props; increment collection-open-gen;
//                           checks boxset_items_cache + item_detail_cache (Part 2) — only sets
//                           app-content-loading=true when either is a miss; spawn async: fetch
//                           BoxSet items + poster + item-detail in parallel (cached ones skip
//                           their network call); sets collection-overview,
//                           collection-is-favorite, collection-has-played from detail;
//                           backdrop only when backdrop_image_tags non-empty;
//                           stale-request guard (gen check, handles same-ID re-opens) +
//                           early-return-on-error with toast;
//                           single invoke_from_event_loop sets all data then shows page;
//                           also spawns spawn_missing_items (independent task)
//   spawn_missing_items     "Missing From This Collection" row (2026-07-29, Deep Seerr
//                           integration) — resolves this BoxSet's TMDB collection id (free path:
//                           the BoxSet's own ProviderIds; guaranteed fallback: first member
//                           movie's TMDB id + get_movie's new .collection field), fetches the
//                           full TMDB collection, shows whatever isn't owned anywhere locally
//                           (resolve_and_fetch_discovery_row's own filter already covers "not a
//                           member of THIS boxset" for free — every boxset member is by
//                           definition a local item); first SectionRow on this screen (previously
//                           a plain grid only)
//   handle_key              keyboard dispatch for the collection screen:
//                           grid nav (Up/Down/Left/Right + Enter → open-detail + C → ctx-menu);
//                           Back button focus (Up from row 0); Down from grid's last row enters
//                           the Missing Items row (2026-07-29) if non-empty; Back → close
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::AppState;
use crate::detail::{fetch_card_posters, items_to_cards};
use crate::poster::{decode_backdrop_buffer, decode_poster_buffer, fetch_backdrop_cached_tagged, fetch_poster_cached};
use crate::MainWindow;

// ── open_collection_screen ────────────────────────────────────────────────────

pub(crate) fn open_collection_screen(
    id:    String,
    title: String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    // Screen-open cache (Part 2): skip the loading spinner when both the item
    // list and detail are cached — the remaining work (poster/backdrop fetch)
    // is disk-cached and fast enough to feel instant.
    let (client, cached_items, cached_detail) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        (c, s.boxset_items_cache.get(&id), s.item_detail_cache.get(&id))
    };
    let is_cache_hit = cached_items.is_some() && cached_detail.is_some();
    tracing::debug!("open_collection_screen({id}): cache_hit={is_cache_hit}");

    // Increment the open-generation counter and capture it so async tasks can
    // detect when they've been superseded (even by a re-open of the same collection).
    let gen = if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_collection_id(id.as_str().into());
        g.set_collection_title(title.as_str().into());
        g.set_collection_overview("".into());
        g.set_collection_is_favorite(false);
        g.set_collection_has_played(false);
        g.set_collection_btn_focused(-1);
        g.set_collection_has_poster(false);
        g.set_collection_has_backdrop(false);
        g.set_collection_items(ModelRc::new(VecModel::default()));
        g.set_collection_focused(0);
        g.set_collection_back_focused(false);
        g.set_collection_missing(ModelRc::new(VecModel::default()));
        g.set_collection_missing_focused(-1);
        g.set_app_loading_progress(0.0);
        if !is_cache_hit {
            g.set_app_content_loading(true);
        }
        // show-collection is deferred until the async task has all data ready
        let next = g.get_collection_open_gen() + 1;
        g.set_collection_open_gen(next);
        next
    } else {
        -1  // window gone; async task will abort on the gen check
    };

    let id2    = id.clone();
    let title2 = title.clone();
    let ww_task = ww.clone();
    let state_missing = Arc::clone(&state);
    let state_task = state;
    rt.spawn(async move {
        // Fetch items + poster in parallel; backdrop only if the BoxSet has backdrop tags.
        // Cached items/detail (if any) skip their respective network call.
        let items_fut = async {
            if let Some(v) = cached_items { return Ok(v); }
            client.get_boxset_items(&id2).await
        };
        let detail_fut = async {
            if let Some(d) = cached_detail { return Ok(d); }
            client.get_item_detail(&id2).await
        };
        let (items_res, poster_bytes, detail_res) = tokio::join!(
            items_fut,
            fetch_poster_cached(&client, &id2),
            detail_fut,
        );
        if let Ok(v) = &items_res  { state_task.lock().unwrap().boxset_items_cache.insert(id2.clone(), v.clone()); }
        if let Ok(d) = &detail_res { state_task.lock().unwrap().item_detail_cache.insert(id2.clone(), d.clone()); }

        // Deleted BoxSet: the ParentId item query returns an empty 200, so the
        // ghost is only visible on the detail fetch's 404 — purge and bail (S4).
        if let Err(e) = &detail_res {
            if crate::is_not_found(e) {
                let ww_err = ww_task.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww_err.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_collection_open_gen() == gen {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                crate::purge_deleted_item(&state_task, &ww_task, &id2);
                return;
            }
        }
        let backdrop_bytes = match &detail_res {
            Ok(d) if !d.backdrop_image_tags.is_empty() =>
                fetch_backdrop_cached_tagged(&client, &id2, d.backdrop_image_tags.first().map(String::as_str)).await,
            _ => None,
        };

        let items = match items_res {
            Ok(v) => v,
            Err(e) => {
                warn!("open_collection_screen get_boxset_items({}): {:#}", id2, e);
                let ww_err = ww_task.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww_err.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_collection_open_gen() == gen {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                crate::show_toast(ww_task, "Couldn't load collection — check your server connection".into());
                return;
            }
        };

        // Fetch all item posters in parallel before showing the screen.
        let bufs = fetch_card_posters(&client, &items).await;

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_task.upgrade() else { return };
            let g = AppState::get(&w);

            // Stale-request guard: abort if superseded by any newer open (same or different collection).
            if g.get_collection_open_gen() != gen { return; }

            // Overview + user state from detail fetch
            if let Ok(d) = &detail_res {
                g.set_collection_overview(d.overview.clone().unwrap_or_default().trim().into());
                g.set_collection_is_favorite(d.user_data.is_favorite);
                g.set_collection_has_played(d.user_data.played);
            }

            // Collection poster
            if let Some(bytes) = poster_bytes {
                if let Some(spb) = decode_poster_buffer(&bytes) {
                    g.set_collection_poster(slint::Image::from_rgba8(spb));
                    g.set_collection_has_poster(true);
                }
            }

            // Backdrop
            if let Some(bytes) = backdrop_bytes {
                if let Some(spb) = decode_backdrop_buffer(&bytes) {
                    g.set_collection_backdrop(slint::Image::from_rgba8(spb));
                    g.set_collection_has_backdrop(true);
                }
            }

            let cards = items_to_cards(&items, bufs);
            g.set_collection_items(crate::apply_cards_preserving_identity(&g.get_collection_items(), cards));
            g.set_collection_focused(0);
            g.set_collection_back_focused(false);
            g.set_collection_title(title2.as_str().into());
            g.set_app_content_loading(false);
            g.set_show_collection(true);
            w.invoke_grab_keyboard_focus();
        });
    });

    spawn_missing_items(id, state_missing, ww, rt);
}

/// "Missing From This Collection" row (2026-07-29, Deep Seerr integration) —
/// resolves this BoxSet's TMDB collection id (free path via the BoxSet
/// item's own `ProviderIds`, falling back to its first eligible member
/// movie's own TMDB id + the already-used `get_movie` call's `.collection`
/// field — a movie belongs to at most one TMDB collection, so any one
/// member is as good a source as any; not worth iterating every member),
/// fetches the full TMDB collection membership, and shows whatever isn't
/// already owned anywhere in the local library. `resolve_and_fetch_discovery_row`'s
/// own "not owned anywhere" filter already covers "not a member of THIS
/// boxset specifically" for free, since every boxset member is by
/// definition a local item — no separate boxset-membership diff needed.
/// Independent task, same reasoning as `person.rs::spawn_other_work` — this
/// row's resolution can fail outright (no TMDB collection at all, e.g. a
/// user-curated folder that isn't a real franchise) or take an extra
/// network round trip, and shouldn't hold up the main grid from showing.
fn spawn_missing_items(
    id:    String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    let (client, seerr) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        let Some(sr) = s.seerr_client.clone() else { return };
        (c, sr)
    };
    rt.spawn(async move {
        let cached_items = state.lock().unwrap().boxset_items_cache.get(&id);
        let items = match cached_items {
            Some(v) => v,
            None => match client.get_boxset_items(&id).await {
                Ok(v)  => v,
                Err(e) => { warn!("spawn_missing_items get_boxset_items({id}): {:#}", e); return; }
            }
        };

        let cached_detail = state.lock().unwrap().item_detail_cache.get(&id);
        let detail = match cached_detail {
            Some(d) => Some(d),
            None => client.get_item_detail(&id).await.ok(),
        };
        let mut collection_id = detail
            .as_ref()
            .and_then(|d| d.provider_ids.get("TmdbCollection"))
            .and_then(|s| s.parse::<i64>().ok());

        if collection_id.is_none() {
            let first_member_tmdb_id = items
                .iter()
                .find(|m| m.item_type == "Movie")
                .and_then(|m| m.provider_ids.get("Tmdb"))
                .and_then(|s| s.parse::<i64>().ok());
            if let Some(tmdb_id) = first_member_tmdb_id {
                match seerr.get_movie(tmdb_id).await {
                    Ok(m)  => collection_id = m.collection.map(|c| c.id),
                    Err(e) => warn!("spawn_missing_items get_movie({tmdb_id}): {:#}", e),
                }
            }
        }
        let Some(collection_id) = collection_id else { return };

        let collection = match seerr.get_collection(collection_id).await {
            Ok(c)  => c,
            Err(e) => { warn!("seerr: get_collection({collection_id}): {e:#}"); return; }
        };
        let metas = crate::discover::build_filtered_metas(&collection.parts);
        let ready = crate::discover::resolve_and_fetch_discovery_row(&state, metas, 20).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_collection_id().as_str() != id { return; }
            let cards = crate::discover::discover_cards_from(ready);
            g.set_collection_missing(crate::apply_cards_preserving_identity(&g.get_collection_missing(), cards));
        });
    });
}

// ── handle_key ────────────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;

    // ── Back button focused ────────────────────────────────────────────────────
    if g.get_collection_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_collection(false);
                true
            }
            Action::Down => {
                g.set_collection_back_focused(false);
                g.set_collection_btn_focused(0);
                true
            }
            Action::Up => false, // let focus_bar_on_up reach the mini-player
            _ => true,
        };
    }

    // ── ♥/✓ button row focused ─────────────────────────────────────────────────
    let btn = g.get_collection_btn_focused();
    if btn >= 0 {
        return match action {
            Action::Left  => { g.set_collection_btn_focused((btn - 1).max(0)); true }
            Action::Right => { g.set_collection_btn_focused((btn + 1).min(1)); true }
            Action::Confirm => {
                if btn == 0 { g.invoke_toggle_collection_fav(); }
                else        { g.invoke_toggle_collection_played(); }
                true
            }
            Action::Up => {
                g.set_collection_btn_focused(-1);
                g.set_collection_back_focused(true);
                true
            }
            Action::Down => {
                g.set_collection_btn_focused(-1);
                g.set_collection_focused(0);
                true
            }
            Action::Back => {
                g.set_collection_btn_focused(-1);
                g.set_show_collection(false);
                true
            }
            _ => true,
        };
    }

    // ── Missing-from-collection row focused (2026-07-29, Deep Seerr integration) ─
    let missing_focused = g.get_collection_missing_focused();
    if missing_focused >= 0 {
        let missing_len = g.get_collection_missing().row_count() as i32;
        return match action {
            Action::Left => {
                if missing_focused > 0 { g.set_collection_missing_focused(missing_focused - 1); }
                true
            }
            Action::Right => {
                if missing_focused < missing_len - 1 { g.set_collection_missing_focused(missing_focused + 1); }
                true
            }
            Action::Up => {
                g.set_collection_missing_focused(-1); // back to grid; collection_focused unchanged
                true
            }
            Action::Confirm => {
                if let Some(card) = g.get_collection_missing().row_data(missing_focused as usize) {
                    let media_type = if card.item_type == "DiscoverMovie" { "movie" } else { "tv" };
                    g.invoke_open_discover_item(media_type.into(), card.id);
                }
                true
            }
            Action::OpenContextMenu => {
                if let Some(card) = g.get_collection_missing().row_data(missing_focused as usize) {
                    g.invoke_open_context_menu_discover(card);
                }
                true
            }
            Action::Back => {
                g.set_collection_missing_focused(-1);
                g.set_show_collection(false);
                true
            }
            _ => true,
        };
    }

    // ── Grid navigation ────────────────────────────────────────────────────────
    let f    = g.get_collection_focused();
    let cols = g.get_library_cols();
    let len  = g.get_collection_items().row_count() as i32;

    match action {
        Action::Back => {
            g.set_show_collection(false);
            true
        }
        Action::Up => {
            if f >= cols {
                g.set_collection_focused(f - cols);
            } else {
                // Enter button row at ♥
                g.set_collection_btn_focused(0);
            }
            true
        }
        Action::Down => {
            if f + cols < len {
                g.set_collection_focused(f + cols);
            } else if g.get_collection_missing().row_count() > 0 {
                g.set_collection_missing_focused(0);
            }
            true
        }
        Action::Left => {
            if f > 0 { g.set_collection_focused(f - 1); }
            true
        }
        Action::Right => {
            if f < len - 1 { g.set_collection_focused(f + 1); }
            true
        }
        Action::Confirm => {
            if f < len {
                let card = g.get_collection_items().row_data(f as usize).unwrap();
                g.invoke_open_detail(card.id, card.item_type);
            }
            true
        }
        Action::OpenContextMenu => {
            if f < len {
                let card = g.get_collection_items().row_data(f as usize).unwrap();
                g.set_context_menu_title(card.title.clone());
                g.invoke_open_context_menu(
                    card.id, card.has_played, card.is_favorite,
                    card.resume_pct, card.item_type, card.series_id,
                );
            }
            true
        }
        _ => false,
    }
}
