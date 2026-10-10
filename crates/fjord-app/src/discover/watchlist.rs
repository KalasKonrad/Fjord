// ── fjord-app · discover/watchlist.rs ────────────────────────────────────────
//   ensure_discover_watchlist / refresh_watchlist / fetch_and_store_watchlist  the Seerr watchlist
//                              as an (item_type, tmdb_id) set (once per session; refreshed after every
//                              toggle), then the calendar rebuild and the watchlist rows
//   watchlist_movie_to_meta / watchlist_tv_to_meta  meta for a watchlist item (no request needed;
//                              None for blocklisted items)
//   WATCHLIST_ROW_CAP          20 detail-fetched items for the rows
//   populate_watchlist_rows / push_watchlist_rows / fetch_watchlist_posters  detail-fetch (bounded),
//                              split mixed/movies/tv for the Discover row and the Home/Movies/TV rows
//                              (apply_cards_preserving_identity), posters patched by id + item_type
//   resync_jellyfin_watchlist_stars  watchlisted tmdb ids → local Jellyfin ids
//                              (FjordState.jellyfin_watchlist_ids, read at card construction) + live
//                              star patch on rendered Jellyfin cards; generation-guarded; rerun after
//                              the library lists load
//   discover_toggle_watchlist  POST/DELETE /watchlist, patch every card model (Discover + Jellyfin),
//                              update the id sets, rebuild the calendar; re-adding a watched library
//                              item marks it unplayed
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// Watchlist row meta builders. `movie_details_to_meta`/`tv_details_to_meta` need a
/// `&RequestEntry` (a real request), which a watchlist item may not have, so
/// `availability` comes straight from `d.media_info` (single tier, like
/// `search_result_to_meta`); `on_watchlist` is true by definition. Callers run
/// `patch_known_request_state` afterward so an item that's also requested gets its
/// Edit/Cancel rows. Both return `None` for a Blocklisted item: blocklisting doesn't
/// remove a title from the Seerr watchlist (independent entities), so it would keep
/// resurfacing here.
fn watchlist_movie_to_meta(
    tmdb_id: i64,
    d: &MovieDetails,
) -> Option<(DiscoverCardMeta, Option<String>)> {
    let availability = availability_tag(d.media_info.as_ref().and_then(|mi| mi.status()));
    if availability == "blocklisted" {
        return None;
    }
    let year = d
        .release_date
        .as_deref()
        .filter(|s| s.len() >= 4)
        .map(|s| &s[..4])
        .unwrap_or("");
    let meta = DiscoverCardMeta {
        id: tmdb_id.to_string(),
        item_type: "DiscoverMovie",
        title: d.title.clone(),
        subtitle: year.to_string(),
        year: year.parse().unwrap_or(0),
        availability,
        requested_4k: false,
        other_tier_available: false,
        other_tier_requested: false,
        request_id: String::new(),
        request_pending: false,
        request_mine: false,
        genre_ids: Vec::new(),
        vote_average: 0.0,
        popularity: 0.0,
        on_watchlist: true,
    };
    Some((meta, d.poster_path.clone()))
}

fn watchlist_tv_to_meta(tmdb_id: i64, d: &TvDetails) -> Option<(DiscoverCardMeta, Option<String>)> {
    let availability = availability_tag(d.media_info.as_ref().and_then(|mi| mi.status()));
    if availability == "blocklisted" {
        return None;
    }
    let year = d
        .first_air_date
        .as_deref()
        .filter(|s| s.len() >= 4)
        .map(|s| &s[..4])
        .unwrap_or("");
    let meta = DiscoverCardMeta {
        id: tmdb_id.to_string(),
        item_type: "DiscoverTv",
        title: d.name.clone(),
        subtitle: year.to_string(),
        year: year.parse().unwrap_or(0),
        availability,
        requested_4k: false,
        other_tier_available: false,
        other_tier_requested: false,
        request_id: String::new(),
        request_pending: false,
        request_mine: false,
        genre_ids: Vec::new(),
        vote_average: 0.0,
        popularity: 0.0,
        on_watchlist: true,
    };
    Some((meta, d.poster_path.clone()))
}

/// Fetches every page of the user's Watchlist (`GET /discover/watchlist`) into an
/// `(item_type, tmdb_id)` set — once per session (`FjordState.discover_watchlist_fetched`).
/// All pages, not a capped prefix: these are cheap list rows with no per-item call;
/// safety cap 10 pages (200 items). Best-effort: a failed page just ends pagination.
pub(crate) fn ensure_discover_watchlist(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let client = {
        let mut s = state.lock().unwrap();
        if s.discover_watchlist_fetched {
            return;
        }
        let Some(client) = s.seerr_client.clone() else {
            return;
        };
        s.discover_watchlist_fetched = true;
        client
    };
    rt.spawn(async move {
        fetch_and_store_watchlist(&client, &state, &ww).await;
    });
}

/// Shared by `ensure_discover_watchlist` and `refresh_watchlist` — fetches
/// every page, stores the resulting id set, and triggers a calendar rebuild
/// (the watchlist is one of the two sets `build_calendar_entries` unions).
async fn fetch_and_store_watchlist(
    client: &fjord_seerr::SeerrClient,
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
) {
    let mut ids: std::collections::HashSet<(&'static str, String)> =
        std::collections::HashSet::new();
    let mut page = 1;
    loop {
        let resp = match client.get_watchlist(page).await {
            Ok(r) => r,
            Err(e) => {
                debug!("seerr: get_watchlist page {page} failed: {e:#}");
                break;
            }
        };
        for item in &resp.results {
            let item_type = if item.media_type == "movie" {
                "DiscoverMovie"
            } else {
                "DiscoverTv"
            };
            ids.insert((item_type, item.tmdb_id.to_string()));
        }
        if page >= resp.total_pages || page >= 10 {
            break;
        }
        page += 1;
    }
    debug!("seerr: watchlist -> {} id(s)", ids.len());
    state.lock().unwrap().discover_watchlist_ids = ids.clone();
    build_calendar_entries(Arc::clone(state), ww.clone()).await;

    // Detail-fetch a preview (title + poster) for the Discover Watchlist row and the
    // Home/Movies/TV rows — one fetch for all 4 consumers, spawned so its ~20 detail
    // fetches don't delay build_calendar_entries's commit above.
    let client2 = client.clone();
    let state2 = Arc::clone(state);
    let ww2 = ww.clone();
    let ids2 = ids.clone();
    tokio::spawn(async move {
        populate_watchlist_rows(client2, state2, ww2, ids2).await;
    });

    // In-library watchlist star resync (2026-07-20) — same trigger points
    // as the row population above (session start + every toggle refresh),
    // spawned independently so it can't delay either of the other two.
    let state3 = Arc::clone(state);
    let ww3 = ww.clone();
    tokio::spawn(async move {
        resync_jellyfin_watchlist_stars(state3, ww3).await;
    });
}

/// Re-resolves every watchlisted tmdb id (`FjordState.discover_watchlist_ids`, local —
/// no Seerr call) to a local Jellyfin item via `find_local_item`, then (1) writes the
/// result to `FjordState.jellyfin_watchlist_ids` — what `item_to_card_item`/
/// `items_to_model` read when building cards — and (2) patches the star onto every
/// rendered Jellyfin `CardItem` model (`context_menu::patch_watchlist_on_jellyfin_models`)
/// for immediate feedback. Ids that dropped out are patched back to false, so a star
/// can't stick. Lookup per watchlisted id, not a library scan. Resolve off-thread, then
/// one `invoke_from_event_loop` for the Slint patching.
/// Also re-run (no Seerr fetch) right after `all_movies`/`all_series` are populated:
/// the first resync, triggered by the watchlist fetch early at startup, usually runs
/// before the library lists exist and finds nothing.
pub(crate) async fn resync_jellyfin_watchlist_stars(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
) {
    // Generation guard — see FjordState.jellyfin_watchlist_resync_seq; taken before
    // the scan, so any later call gets a higher number.
    let my_seq = {
        let mut s = state.lock().unwrap();
        s.jellyfin_watchlist_resync_seq += 1;
        s.jellyfin_watchlist_resync_seq
    };
    let ids = state.lock().unwrap().discover_watchlist_ids.clone();
    let resolved: std::collections::HashSet<String> = ids
        .iter()
        .filter_map(|(item_type, tmdb_id)| {
            let media_type = if *item_type == "DiscoverMovie" {
                "movie"
            } else {
                "tv"
            };
            find_local_item(&state, media_type, tmdb_id).map(|(jellyfin_id, _)| jellyfin_id)
        })
        .collect();
    let previous = {
        let mut s = state.lock().unwrap();
        if s.jellyfin_watchlist_resync_seq != my_seq {
            // A newer resync already started while we were scanning — it
            // will (or already did) write a fresher result, so writing ours
            // now would risk clobbering it with staler data. Skip outright
            // rather than racing: only the most-recently-started call ever
            // writes.
            debug!(
                "seerr: resync_jellyfin_watchlist_stars -> stale (newer resync started), discarding"
            );
            return;
        }
        std::mem::replace(&mut s.jellyfin_watchlist_ids, resolved.clone())
    };
    debug!(
        "seerr: resync_jellyfin_watchlist_stars -> {} local match(es)",
        resolved.len()
    );
    if resolved.is_empty() && previous.is_empty() {
        return;
    }
    let removed: Vec<String> = previous.difference(&resolved).cloned().collect();
    let added: Vec<String> = resolved.into_iter().collect();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = ww.upgrade() else { return };
        let g = AppState::get(&w);
        for jellyfin_id in added {
            crate::context_menu::patch_watchlist_on_jellyfin_models(&g, &jellyfin_id, true);
        }
        for jellyfin_id in removed {
            crate::context_menu::patch_watchlist_on_jellyfin_models(&g, &jellyfin_id, false);
        }
    });
}

/// 20, the same cap as `fetch_requested_row` and `build_calendar_entries`' candidates
/// (per-item detail fetches are the expensive part).
const WATCHLIST_ROW_CAP: usize = 20;

async fn populate_watchlist_rows(
    client: fjord_seerr::SeerrClient,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    ids: std::collections::HashSet<(&'static str, String)>,
) {
    let candidates: Vec<(&'static str, String)> = ids.into_iter().take(WATCHLIST_ROW_CAP).collect();
    if candidates.is_empty() {
        push_watchlist_rows(&ww, &state, Vec::new());
        return;
    }
    let n = candidates.len();
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set: tokio::task::JoinSet<(usize, Option<RequestedRowItem>)> =
        tokio::task::JoinSet::new();
    for (idx, (item_type, tmdb_id_str)) in candidates.into_iter().enumerate() {
        let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else {
            continue;
        };
        let client = client.clone();
        let sem = Arc::clone(&sem);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let item = if item_type == "DiscoverMovie" {
                client
                    .get_movie(tmdb_id)
                    .await
                    .ok()
                    .and_then(|d| watchlist_movie_to_meta(tmdb_id, &d))
            } else {
                client
                    .get_tv(tmdb_id)
                    .await
                    .ok()
                    .and_then(|d| watchlist_tv_to_meta(tmdb_id, &d))
            };
            (idx, item)
        });
    }
    let mut out: Vec<Option<RequestedRowItem>> = (0..n).map(|_| None).collect();
    while let Some(res) = set.join_next().await {
        let Ok((idx, item)) = res else { continue };
        out[idx] = item;
    }
    let mut items: Vec<RequestedRowItem> = out.into_iter().flatten().collect();
    // Populates request_id/pending/mine (Edit/Cancel context-menu rows) for
    // an item that's ALSO requested — see watchlist_movie_to_meta's own doc
    // comment for why this doesn't touch the visual availability pill,
    // which is already set from media_info directly.
    let known = state.lock().unwrap().discover_known_requests.clone();
    for (meta, _) in &mut items {
        patch_known_request_state(meta, &known);
    }
    debug!("seerr: watchlist row items -> {} card(s)", items.len());
    push_watchlist_rows(&ww, &state, items.clone());
    fetch_watchlist_posters(ww, &items).await;
}

/// Splits by item_type into mixed/movies/tv (like home.rs's Continue Watching split,
/// client-side) and commits all 3 models in ONE `invoke_from_event_loop`: `items` is
/// Send-safe meta data, and `CardItem` (`!Send` — it holds a `slint::Image`) is only
/// built in here. All 3 go through `apply_cards_preserving_identity`: this runs after
/// every watchlist toggle, and a fresh `ModelRc` would re-create every delegate (all
/// posters re-fade).
fn push_watchlist_rows(
    ww: &Weak<MainWindow>,
    state: &Arc<Mutex<FjordState>>,
    items: Vec<RequestedRowItem>,
) {
    let ww = ww.clone();
    // Session guard: `client` arrives as a value-cloned `SeerrClient`, so there's no Arc
    // identity to compare (seerr_session_current). Coarser check: if Seerr is no longer
    // connected at all (sign-out, or a switch to a profile without Seerr), drop the
    // result. A switch to another account that also has Seerr isn't caught — a known gap.
    let state = Arc::clone(state);
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = ww.upgrade() else { return };
        if state.lock().unwrap().seerr_client.is_none() {
            return;
        }
        let g = AppState::get(&w);
        let mixed: Vec<CardItem> = items
            .iter()
            .map(|(m, _)| m.clone().into_card_item())
            .collect();
        let movies: Vec<CardItem> = items
            .iter()
            .filter(|(m, _)| m.item_type == "DiscoverMovie")
            .map(|(m, _)| m.clone().into_card_item())
            .collect();
        let tv: Vec<CardItem> = items
            .iter()
            .filter(|(m, _)| m.item_type == "DiscoverTv")
            .map(|(m, _)| m.clone().into_card_item())
            .collect();
        debug!(
            "seerr: push_watchlist_rows -> mixed={} movies={} tv={}",
            mixed.len(),
            movies.len(),
            tv.len()
        );
        g.set_discover_watchlist_mixed(crate::apply_cards_preserving_identity(
            &g.get_discover_watchlist_mixed(),
            mixed,
        ));
        g.set_discover_watchlist_movies(crate::apply_cards_preserving_identity(
            &g.get_discover_watchlist_movies(),
            movies,
        ));
        g.set_discover_watchlist_tv(crate::apply_cards_preserving_identity(
            &g.get_discover_watchlist_tv(),
            tv,
        ));
    });
}

/// Patches posters onto all 3 watchlist models by id+item_type match (not
/// by row index — the same tmdb id can sit at a DIFFERENT row index in
/// `discover-watchlist-mixed` vs. its own type-specific list, unlike the
/// Coming Up row's single-model index-based patch). Same bounded-
/// concurrency fetch-then-patch shape as `fetch_coming_up_posters`.
async fn fetch_watchlist_posters(ww: Weak<MainWindow>, items: &[RequestedRowItem]) {
    let jobs: Vec<(String, String, String)> = items
        .iter()
        .filter_map(|(m, p)| {
            p.clone()
                .map(|p| (m.item_type.to_string(), m.id.clone(), p))
        })
        .collect();
    if jobs.is_empty() {
        return;
    }
    let Ok(http) = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
    else {
        return;
    };
    let sem = Arc::new(tokio::sync::Semaphore::new(8));
    let mut set = tokio::task::JoinSet::new();
    for (item_type, tmdb_id, poster_path) in jobs {
        let http = http.clone();
        let sem = Arc::clone(&sem);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let cache_key = format!(
                "{}-{}",
                if item_type == "DiscoverMovie" {
                    "movie"
                } else {
                    "tv"
                },
                tmdb_id
            );
            let bytes = fetch_tmdb_image(&http, TMDB_POSTER_BASE, &poster_path, &cache_key).await?;
            let buf = decode_poster_buffer(&bytes)?;
            Some((item_type, tmdb_id, buf))
        });
    }
    while let Some(res) = set.join_next().await {
        let Ok(Some((item_type, tmdb_id, buf))) = res else {
            continue;
        };
        let ww2 = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            let g = AppState::get(&w);
            for model in [
                g.get_discover_watchlist_mixed(),
                g.get_discover_watchlist_movies(),
                g.get_discover_watchlist_tv(),
            ] {
                for i in 0..model.row_count() {
                    if let Some(mut card) = model.row_data(i)
                        && card.id.as_str() == tmdb_id
                        && card.item_type.as_str() == item_type
                    {
                        card.poster = slint::Image::from_rgba8(buf.clone());
                        card.has_poster = true;
                        model.set_row_data(i, card);
                    }
                }
            }
        });
    }
}

/// Re-fetches the full watchlist id set and rebuilds the calendar —
/// called right after `discover_toggle_watchlist` succeeds, mirroring
/// `refresh_requested_row`'s "cheap enough for an infrequent action" shape.
pub(crate) fn refresh_watchlist(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        return;
    };
    rt.spawn(async move {
        fetch_and_store_watchlist(&client, &state, &ww).await;
    });
}

/// Add/remove Watchlist (Discover context menu row, RequestDetailScreen button):
/// POST/DELETE, then patch every visible card, update the id cache and rebuild the
/// calendar. `success_toast` replaces the usual toast — a successful request auto-adds
/// to the watchlist and shows one combined toast.
#[allow(clippy::too_many_arguments)]
pub(crate) fn discover_toggle_watchlist(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    tmdb_id: i64,
    media_type: String,
    title: String,
    adding: bool,
    success_toast: Option<&'static str>,
) {
    debug!(
        "seerr: discover_toggle_watchlist tmdb={tmdb_id} media_type={media_type} adding={adding}"
    );
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    let item_type: &'static str = if media_type == "movie" {
        "DiscoverMovie"
    } else {
        "DiscoverTv"
    };
    let rt2 = rt.clone();

    rt.spawn(async move {
        let result = if adding {
            client.add_watchlist(tmdb_id, &media_type, &title).await
        } else {
            client.remove_watchlist(tmdb_id, &media_type).await
        };
        match result {
            Ok(()) => {
                debug!("seerr: discover_toggle_watchlist succeeded tmdb={tmdb_id} adding={adding}");
                {
                    let mut s = state.lock().unwrap();
                    let key = (item_type, tmdb_id.to_string());
                    if adding {
                        s.discover_watchlist_ids.insert(key);
                    } else {
                        s.discover_watchlist_ids.remove(&key);
                    }
                }
                // Resolved once, reused below for both the in-library star
                // patch (add or remove) and, on add only, the played-state
                // reset — a plain (Jellyfin id, Jellyfin item_type) lookup,
                // no network call.
                let local_item = crate::discover::find_local_item(&state, &media_type, &tmdb_id.to_string());

                // Re-adding an already watched library item to the watchlist means "watch it again":
                // mark it unplayed in Jellyfin (mark_unplayed — echoed via UserDataChanged, so every
                // model converges through ws.rs); the two writes below are only instant feedback.
                // Discover-only items have no played state.
                let mut reset_played = false;
                if adding && let Some((jellyfin_id, _)) = &local_item {
                    let was_played = {
                        let s = state.lock().unwrap();
                        let list: &[fjord_api::models::MediaItem] =
                            if media_type == "movie" { &s.all_movies } else { &s.all_series };
                        list.iter().find(|m| &m.id == jellyfin_id).is_some_and(|m| m.user_data.played)
                    };
                    if was_played {
                        let jf_client = state.lock().unwrap().client.as_ref().map(Arc::clone);
                        if let Some(jf_client) = jf_client {
                            match jf_client.mark_unplayed(jellyfin_id).await {
                                Ok(()) => {
                                    state.lock().unwrap().update_item_user_state(jellyfin_id, Some(false), None);
                                    reset_played = true;
                                }
                                Err(e) => warn!("discover_toggle_watchlist: mark_unplayed({jellyfin_id}) failed: {e:#}"),
                            }
                        }
                    }
                }

                let ww2 = ww.clone();
                let state3 = Arc::clone(&state);
                let local_item2 = local_item.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        patch_watchlist_on_all_models(&g, item_type, tmdb_id, adding);
                        if g.get_show_request_detail()
                            && g.get_request_detail_media_type().as_str() == media_type
                            && g.get_request_detail_tmdb_id() == tmdb_id as i32
                        {
                            g.set_request_detail_on_watchlist(adding);
                        }
                        // The item may also be a native Jellyfin card (Continue Watching, library grid):
                        // patch it by Jellyfin id, and update jellyfin_watchlist_ids right away so a screen
                        // rebuilt before the next resync gets the right value.
                        if let Some((jellyfin_id, _)) = &local_item2 {
                            crate::context_menu::patch_watchlist_on_jellyfin_models(&g, jellyfin_id, adding);
                            let mut s = state3.lock().unwrap();
                            if adding { s.jellyfin_watchlist_ids.insert(jellyfin_id.clone()); }
                            else      { s.jellyfin_watchlist_ids.remove(jellyfin_id); }
                            drop(s);
                            if reset_played {
                                crate::context_menu::update_card_in_all_models(&w, jellyfin_id, Some(false), None);
                            }
                        }
                    }
                });
                show_toast(
                    ww.clone(),
                    success_toast.unwrap_or(if adding {
                        if reset_played { "Added to Watchlist — marked unwatched" } else { "Added to Watchlist" }
                    } else {
                        "Removed from Watchlist"
                    }).into(),
                );
                refresh_watchlist(Arc::clone(&state), ww, rt2);
            }
            Err(e) => handle_seerr_error(&state, &ww, is_session_auth, "Couldn't update watchlist", &e),
        }
    });
}
