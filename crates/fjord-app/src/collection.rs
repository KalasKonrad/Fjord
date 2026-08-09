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
//   resolve_and_blocklist_collection  bulk-blocklists every part of this BoxSet's TMDB
//                           collection at once (POST /blocklist/collection/{id}, 2026-08-06,
//                           Seerr Blocklist support) — reuses resolve_missing_items_collection_id
//                           for the same id resolution the Missing Items row already does; on
//                           success re-invokes spawn_missing_items rather than hand-patching
//                           member cards, so the row reflects Seerr's own real server-side result
//   handle_key              keyboard dispatch for the collection screen:
//                           grid nav (Up/Down/Left/Right + Enter → open-detail + C → ctx-menu);
//                           Back button focus (Up from row 0); button row now 3-wide (♥/✓/⛔,
//                           2026-08-06 — ⛔ opens the blocklist-collection confirm dialog rather
//                           than acting directly, checked first in this function since it's an
//                           overlay on top of the screen); Down from grid's last row enters
//                           the Missing Items row (2026-07-29) if non-empty; Back → close
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::{debug, warn};

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
    let id_revalidate    = id.clone();
    let state_revalidate = Arc::clone(&state);
    let ww_revalidate    = ww.clone();
    let rt_revalidate    = rt.clone();
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
            // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09): the
            // gen counter above only catches a SAME-TYPE re-open — nothing
            // increments it on sign-out or a profile switch, so a stale
            // fetch from a torn-down session can still match it and
            // silently re-open this screen (with the OLD session's data)
            // moments after reset_session_state just force-closed it. Same
            // guard class as spawn_collection_revalidate's own, applied
            // here too since that one only covers the cache-hit revalidate
            // path, not this, the actual open-screen path.
            if !crate::session_current(&state_task, &client) { return; }

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

    // Cache-hit only: the screen above already showed instantly from cached
    // data. Real gap, live-reported: Jellyfin's WebSocket only delivers
    // LibraryChanged to the most-recently-connected client when multiple
    // clients share a session (JELLYFIN.md) — editing through Jellyfin's own
    // web UI while Fjord sits connected can silently starve it of the event,
    // leaving this cache stale indefinitely with no other fallback. This
    // revalidation is what closes that gap for whatever the user is actually
    // looking at right now. Started before spawn_missing_items (below) so
    // its JoinHandle is ready to hand over — see that function's own doc
    // comment for why it needs to know about this specific revalidate.
    let revalidate_handle = if is_cache_hit {
        spawn_collection_revalidate(id_revalidate, gen, state_revalidate, ww_revalidate, rt_revalidate)
    } else {
        None
    };
    spawn_missing_items(id, state_missing, ww, rt, revalidate_handle);
}

// Returns the spawned task's `JoinHandle` (`None` when nothing was actually
// spawned — cooldown-skipped or no client) so `spawn_missing_items` can
// await this exact revalidate's completion and retry against its
// freshly-written cache, instead of firing its own redundant fetch — see
// that function's own doc comment for the bug this fixes.
fn spawn_collection_revalidate(
    id:    String,
    gen:   i32,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) -> Option<tokio::task::JoinHandle<()>> {
    if !crate::should_revalidate(&state, &id) { return None; }
    let client = state.lock().unwrap().client.as_ref().map(Arc::clone)?;
    Some(rt.spawn(async move {
        let (items_res, detail_res) = tokio::join!(client.get_boxset_items(&id), client.get_item_detail(&id));
        let (Ok(items), Ok(detail)) = (items_res, detail_res) else { return };
        // Sign-out (or a different account signing in on a shared HTPC) mid-
        // fetch must not let this stale/wrong-session data land in the new
        // session's caches — same guard class as main.rs::session_current's
        // own doc comment (CR11-2), reapplied here since this is exactly the
        // "background fetch writes per-user data into shared FjordState"
        // pattern that guard exists for.
        if !crate::session_current(&state, &client) { return; }
        {
            let mut s = state.lock().unwrap();
            s.boxset_items_cache.insert(id.clone(), items.clone());
            s.item_detail_cache.insert(id.clone(), detail.clone());
        }
        let bufs = fetch_card_posters(&client, &items).await;
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_collection_open_gen() != gen { return; }
            g.set_collection_overview(detail.overview.clone().unwrap_or_default().trim().into());
            g.set_collection_is_favorite(detail.user_data.is_favorite);
            g.set_collection_has_played(detail.user_data.played);
            let cards = items_to_cards(&items, bufs);
            g.set_collection_items(crate::apply_cards_preserving_identity(&g.get_collection_items(), cards));
        });
    }))
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
///
/// `revalidate_handle`: real bug fixed 2026-08-02, live-reported ("why did
/// they not update when i was in it?") — a first-time resolution failure
/// often just means `boxset_items_cache`/`item_detail_cache` haven't been
/// revalidated yet (e.g. the cached BoxSet member items still show no
/// `ProviderIds`, before Jellyfin/the cache had a chance to catch up). The
/// SAME screen open, on a cache hit, already kicks off `spawn_collection_-
/// revalidate` in parallel to refresh exactly those two caches — but the two
/// tasks were entirely independent, so the row just silently stayed empty
/// until the user reopened the screen a second time and got a fresh
/// resolution attempt against by-then-fresher data. If a revalidate is
/// genuinely in flight for this exact open, awaiting its `JoinHandle` here
/// and retrying once reuses ITS fetch (already written into the caches this
/// function reads) instead of firing a redundant one of its own.
fn spawn_missing_items(
    id:    String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
    revalidate_handle: Option<tokio::task::JoinHandle<()>>,
) {
    let (client, seerr) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        let Some(sr) = s.seerr_client.clone() else { return };
        (c, sr)
    };
    rt.spawn(async move {
        let mut collection_id = resolve_missing_items_collection_id(&id, &client, &seerr, &state).await;
        if collection_id.is_none() {
            if let Some(handle) = revalidate_handle {
                debug!("spawn_missing_items({id}): first attempt failed, waiting for the parallel revalidate before retrying");
                let _ = handle.await;
                collection_id = resolve_missing_items_collection_id(&id, &client, &seerr, &state).await;
            }
        }
        let Some(collection_id) = collection_id else {
            debug!("spawn_missing_items({id}): no tmdb collection resolved — row will not show");
            return;
        };

        let collection = match seerr.get_collection(collection_id).await {
            Ok(c)  => c,
            Err(e) => { warn!("seerr: get_collection({collection_id}): {e:#}"); return; }
        };
        let metas = crate::discover::build_filtered_metas(&collection.parts);
        debug!("spawn_missing_items({id}): collection {collection_id} -> {} missing item(s) before owned-filter", metas.len());
        let ready = crate::discover::resolve_and_fetch_discovery_row(&state, metas, 20).await;
        debug!("spawn_missing_items({id}): collection {collection_id} -> {} missing item(s) after owned-filter", ready.len());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_collection_id().as_str() != id { return; }
            let cards = crate::discover::discover_cards_from(ready);
            g.set_collection_missing(crate::apply_cards_preserving_identity(&g.get_collection_missing(), cards));
        });
    });
}

/// One resolution attempt (free `ProviderIds` path, then the multi-member
/// fallback loop) — extracted from `spawn_missing_items` 2026-08-02 so it
/// can be called twice: once immediately against whatever's cached right
/// now, and once more after an in-flight revalidate finishes, without
/// duplicating the whole resolution logic inline at both call sites.
async fn resolve_missing_items_collection_id(
    id:     &str,
    client: &Arc<fjord_api::JellyfinClient>,
    seerr:  &Arc<fjord_seerr::SeerrClient>,
    state:  &Arc<Mutex<FjordState>>,
) -> Option<i64> {
    let cached_items = state.lock().unwrap().boxset_items_cache.get(id);
    let items = match cached_items {
        Some(v) => v,
        None => match client.get_boxset_items(id).await {
            Ok(v)  => v,
            Err(e) => { warn!("spawn_missing_items get_boxset_items({id}): {:#}", e); return None; }
        }
    };

    let cached_detail = state.lock().unwrap().item_detail_cache.get(id);
    let detail = match cached_detail {
        Some(d) => Some(d),
        None => client.get_item_detail(id).await.ok(),
    };
    let mut collection_id = detail
        .as_ref()
        .and_then(|d| d.provider_ids.get("TmdbCollection"))
        .and_then(|s| s.parse::<i64>().ok());
    if let Some(cid) = collection_id {
        debug!("spawn_missing_items({id}): tmdb collection id resolved via ProviderIds -> {cid}");
    }

    if collection_id.is_none() {
        // Try every Movie-type member in turn, not just the first — a real
        // gap found live (Avatar's BoxSet): the first member can have zero
        // Jellyfin ProviderIds at all (never matched to any external
        // metadata source), while a later member of the same franchise
        // (e.g. a more recently-scanned sequel) is far more likely to have
        // a real Tmdb tag. Giving up after just the first member meant a
        // single poorly-tagged Jellyfin item could sink the whole row for
        // an otherwise well-known, real TMDB franchise.
        let movie_members: Vec<_> = items.iter().filter(|m| m.item_type == "Movie").collect();
        if movie_members.is_empty() {
            debug!("spawn_missing_items({id}): no Movie-type member in this BoxSet ({} item(s) total)", items.len());
        } else {
            debug!("spawn_missing_items({id}): {} Movie-type member(s) to try", movie_members.len());
            for m in &movie_members {
                let Some(tmdb_id) = m.provider_ids.get("Tmdb").and_then(|s| s.parse::<i64>().ok()) else {
                    debug!("spawn_missing_items({id}): member {:?} ({}) has no Tmdb ProviderId, trying next", m.name, m.id);
                    continue;
                };
                match seerr.get_movie(tmdb_id).await {
                    Ok(mv) => match mv.collection {
                        Some(c) => {
                            debug!("spawn_missing_items({id}): tmdb collection id resolved via member movie {:?} ({tmdb_id}) -> {}", m.name, c.id);
                            collection_id = Some(c.id);
                            break;
                        }
                        None => debug!("spawn_missing_items({id}): member movie {:?} ({tmdb_id}) has no TMDB collection, trying next", m.name),
                    },
                    Err(e) => warn!("spawn_missing_items get_movie({tmdb_id}): {:#}", e),
                }
            }
        }
    }
    collection_id
}

/// Bulk-blocklists every part of this BoxSet's TMDB collection at once
/// (`POST /blocklist/collection/{id}`, resolved server-side from just the
/// id) — the confirm dialog's own Confirm action. Reuses
/// `resolve_missing_items_collection_id` for the same free-`ProviderIds`-
/// then-member-movie-fallback resolution the Missing Items row already
/// does; no separate revalidate-and-retry here (unlike `spawn_missing_items`)
/// since this is a deliberate, infrequent user action, not an ambient
/// screen-open fetch — a failed resolution just toasts and the user can
/// retry after the screen's own background revalidate catches up, same as
/// it would for Missing Items. On success, re-invokes `spawn_missing_items`
/// (not a hand-patch of individual member cards) so the row reflects
/// whatever Seerr's own bulk-exclusion semantics actually did server-side,
/// rather than Fjord guessing at them. 2026-08-06, Seerr Blocklist support.
pub(crate) fn resolve_and_blocklist_collection(
    id:    String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    let (client, seerr) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        let Some(sr) = s.seerr_client.clone() else {
            crate::show_toast(ww, "Not connected to Seerr".into());
            return;
        };
        (c, sr)
    };
    let is_session_auth = seerr.is_session_auth();
    let rt2 = rt.clone();
    rt.spawn(async move {
        let Some(collection_id) = resolve_missing_items_collection_id(&id, &client, &seerr, &state).await else {
            crate::show_toast(ww, "Couldn't resolve this collection's TMDB id".into());
            return;
        };
        match seerr.add_blocklist_collection(collection_id).await {
            Ok(()) => {
                debug!("seerr: blocklisted collection {collection_id} (BoxSet {id})");
                crate::show_toast(ww.clone(), "Collection blocklisted".into());
                spawn_missing_items(id, state, ww, rt2, None);
            }
            Err(e) => crate::discover::handle_seerr_error(&state, &ww, is_session_auth, "Couldn't blocklist collection", &e),
        }
    });
}

// ── handle_key ────────────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;

    // ── Blocklist-collection confirm dialog (2026-08-06, Seerr Blocklist
    // support) — checked first, same as every other overlay-on-top-of-a-
    // screen precedent in this app (skip-segment overlays, Up Next banner):
    // while it's open, nothing else on this screen should react to input.
    if g.get_collection_blocklist_confirm_open() {
        return match action {
            Action::Left => {
                g.set_collection_blocklist_confirm_focused(0);
                true
            }
            Action::Right => {
                g.set_collection_blocklist_confirm_focused(1);
                true
            }
            Action::Confirm => {
                if g.get_collection_blocklist_confirm_focused() == 1 {
                    g.invoke_collection_blocklist_confirm();
                } else {
                    g.set_collection_blocklist_confirm_open(false);
                }
                true
            }
            Action::Back => {
                g.set_collection_blocklist_confirm_open(false);
                true
            }
            _ => true,
        };
    }

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

    // ── ♥/✓/⛔ button row focused ───────────────────────────────────────────────
    let btn = g.get_collection_btn_focused();
    if btn >= 0 {
        // 3rd button (⛔ Blocklist Collection) only actually renders when
        // seerr-can-manage-blocklist is true (collection.slint's own `if`
        // gate) — capping at 1 instead of 2 when it's false keeps keyboard
        // focus from landing on a button that isn't there, the exact
        // focus/visibility mismatch this codebase has been bitten by
        // before elsewhere. 2026-08-06, Seerr Blocklist support.
        let max_btn = if g.get_seerr_can_manage_blocklist() { 2 } else { 1 };
        return match action {
            Action::Left  => { g.set_collection_btn_focused((btn - 1).max(0)); true }
            Action::Right => { g.set_collection_btn_focused((btn + 1).min(max_btn)); true }
            Action::Confirm => {
                match btn {
                    0 => g.invoke_toggle_collection_fav(),
                    1 => g.invoke_toggle_collection_played(),
                    // Opens the confirm dialog rather than blocklisting
                    // directly — this bulk-blocklists every part of the
                    // franchise, globally, at once, per the design decision
                    // to require confirmation for that (unlike the single-
                    // item toggle elsewhere, which is low-friction on
                    // purpose, matching Watchlist's own precedent).
                    _ => {
                        g.set_collection_blocklist_confirm_focused(0);
                        g.set_collection_blocklist_confirm_open(true);
                    }
                }
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
