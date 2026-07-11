// ── fjord-app · prewarm.rs ───────────────────────────────────────────────────
//   session_current         Arc::ptr_eq guard (mirrors ws.rs's CR11-2 pattern) — checked
//                           before every metadata-prewarm write that inserts a MediaItem
//                           (per-user UserData) into a shared cache, so a sign-out (or a
//                           different account signing in on a shared HTPC) mid-sweep can't
//                           land the old session's results in the new one's caches
//   spawn_metadata_prewarm  opt-in, re-runnable full-library sweep: item_detail_cache
//                           (batched, top-level items) + unique cast members' own
//                           detail+filmography + 4 relationship caches (boxset/artist/
//                           tracks/similar), each rate-limited (semaphore); progress via
//                           FjordState.prewarm_metadata_* fields, read by a 1s AppState
//                           timer (main.rs::wire_prewarm_progress_timer); cost-logging
//                           summary (request counts by category + elapsed) on completion
//   spawn_image_prewarm     backdrops (tag-revalidated) + cast portraits (disk-check-
//                           first, explicitly pre-checked here for accurate "already
//                           cached" cost accounting) for whatever's currently in
//                           item_detail_cache — independent of whether metadata prewarm
//                           has run; same progress/cost-logging pattern
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use fjord_api::JellyfinClient;
use tracing::{info, warn};

use crate::config::FjordState;
use crate::poster::{fetch_backdrop_cached_tagged, fetch_poster_cached};

const PREWARM_CONCURRENCY: usize = 6;

/// True if `client` is still the session's live client. `spawn_metadata_prewarm`
/// runs for minutes (11,731 requests observed in one real run) inserting
/// `MediaItem`s — which embed per-user `UserData` (played/favorite) — into
/// shared `FjordState` caches; without this guard, a sign-out (or a different
/// account signing back in on a shared HTPC) mid-sweep lets the old account's
/// results keep landing in the new session's caches for as long as the sweep
/// keeps running. Mirrors `ws.rs`'s identical guard (CR11-2) for the same class
/// of risk. Not needed in `spawn_image_prewarm` — that one only caches raw
/// image bytes keyed by item/person id, which aren't per-user; a stale client
/// there just fails the request (logged, harmless) rather than corrupting state.
fn session_current(state: &Mutex<FjordState>, client: &Arc<JellyfinClient>) -> bool {
    state.lock().unwrap().client.as_ref().is_some_and(|c| Arc::ptr_eq(c, client))
}

// ── metadata prewarm ────────────────────────────────────────────────────────

pub(crate) fn spawn_metadata_prewarm(
    client: Arc<JellyfinClient>,
    state:  Arc<Mutex<FjordState>>,
    rt:     tokio::runtime::Handle,
) {
    {
        let mut s = state.lock().unwrap();
        if s.prewarm_metadata_running { return; }
        s.prewarm_metadata_running = true;
        s.prewarm_metadata_total   = 0;
        s.prewarm_metadata_done    = 0;
        s.prewarm_metadata_summary = String::new();
    }

    rt.spawn(async move {
        let start = Instant::now();

        // ── Snapshot the flat lists (already populated at startup/first grid open) ──
        let (movie_ids, series_ids, coll_ids, artist_ids, album_ids, playlist_ids) = {
            let s = state.lock().unwrap();
            (
                s.all_movies.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
                s.all_series.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
                s.all_collections.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
                s.all_artists.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
                s.all_albums.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
                s.all_playlists.iter().map(|i| i.id.clone()).collect::<Vec<_>>(),
            )
        };
        let mut top_ids: Vec<String> = Vec::new();
        top_ids.extend(movie_ids.iter().cloned());
        top_ids.extend(series_ids.iter().cloned());
        top_ids.extend(coll_ids.iter().cloned());
        top_ids.extend(artist_ids.iter().cloned());
        top_ids.extend(album_ids.iter().cloned());
        top_ids.extend(playlist_ids.iter().cloned());

        // Rough total up front (movies+series+etc detail, plus each container's own
        // relationship-cache request); refined once person ids are known below.
        // Also raise each cache's cap to fit what's about to be inserted — the
        // default 40 (sized for "recently viewed items") would otherwise evict
        // almost everything mid-sweep (confirmed via a real run: ~11,700
        // requests made, 40 entries survived per cache).
        {
            let mut s = state.lock().unwrap();
            s.prewarm_metadata_total = top_ids.len()
                + coll_ids.len() + artist_ids.len() + album_ids.len() + playlist_ids.len()
                + movie_ids.len() + series_ids.len();
            s.item_detail_cache.set_cap(top_ids.len());
            s.boxset_items_cache.set_cap(coll_ids.len());
            s.artist_albums_cache.set_cap(artist_ids.len());
            s.container_tracks_cache.set_cap(album_ids.len() + playlist_ids.len());
            s.similar_items_cache.set_cap(movie_ids.len() + series_ids.len());
        }

        // ── Step 1: item_detail_cache for every top-level item, batched ──
        let mut req_top_detail = 0usize;
        let mut fetched_top: Vec<fjord_api::models::MediaItem> = Vec::new();
        for chunk in top_ids.chunks(200) {
            if !session_current(&state, &client) { return; }
            req_top_detail += 1;
            match client.get_items_by_ids_detailed(chunk).await {
                Ok(items) => {
                    if !session_current(&state, &client) { return; }
                    let n = items.len();
                    let mut s = state.lock().unwrap();
                    for item in &items { s.item_detail_cache.insert(item.id.clone(), item.clone()); }
                    s.prewarm_metadata_done += n;
                    fetched_top.extend(items);
                }
                Err(e) => warn!("metadata prewarm: get_items_by_ids_detailed(top-level): {e:#}"),
            }
        }

        // ── Step 2: unique cast members (movies + series only) ──
        let mut person_ids: HashSet<String> = Default::default();
        for item in &fetched_top {
            if matches!(item.item_type.as_str(), "Movie" | "Series") {
                for p in &item.people {
                    if !p.id.is_empty() { person_ids.insert(p.id.clone()); }
                }
            }
        }
        let person_ids: Vec<String> = person_ids.into_iter().collect();
        let person_count = person_ids.len();
        {
            let mut s = state.lock().unwrap();
            // Each person needs a detail fetch + a filmography fetch.
            s.prewarm_metadata_total += person_count * 2;
            s.item_detail_cache.set_cap(top_ids.len() + person_count);
            s.person_filmography_cache.set_cap(person_count);
        }
        let mut req_person_detail = 0usize;
        for chunk in person_ids.chunks(200) {
            if !session_current(&state, &client) { return; }
            req_person_detail += 1;
            match client.get_items_by_ids_detailed(chunk).await {
                Ok(items) => {
                    if !session_current(&state, &client) { return; }
                    let n = items.len();
                    let mut s = state.lock().unwrap();
                    for item in items { s.item_detail_cache.insert(item.id.clone(), item); }
                    s.prewarm_metadata_done += n;
                }
                Err(e) => warn!("metadata prewarm: get_items_by_ids_detailed(persons): {e:#}"),
            }
        }

        // ── Step 3: relationship caches — no batch endpoint, rate-limited ──
        let req_person_film = person_count;
        let req_boxset      = coll_ids.len();
        let req_artist      = artist_ids.len();
        let req_tracks      = album_ids.len() + playlist_ids.len();
        let req_similar     = movie_ids.len() + series_ids.len();

        let sem = Arc::new(tokio::sync::Semaphore::new(PREWARM_CONCURRENCY));
        let mut set = tokio::task::JoinSet::new();

        for pid in person_ids {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) { return; }
                match client.get_person_filmography(&pid).await {
                    Ok(v)  => { if session_current(&state, &client) { state.lock().unwrap().person_filmography_cache.insert(pid, v); } }
                    Err(e) => if crate::is_not_found(&e) { state.lock().unwrap().person_filmography_cache.remove(&pid); }
                }
                state.lock().unwrap().prewarm_metadata_done += 1;
            });
        }
        for id in coll_ids {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) { return; }
                match client.get_boxset_items(&id).await {
                    Ok(v)  => { if session_current(&state, &client) { state.lock().unwrap().boxset_items_cache.insert(id, v); } }
                    Err(e) => if crate::is_not_found(&e) { state.lock().unwrap().boxset_items_cache.remove(&id); }
                }
                state.lock().unwrap().prewarm_metadata_done += 1;
            });
        }
        for id in artist_ids {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) { return; }
                match client.get_artist_albums(&id).await {
                    Ok(v)  => { if session_current(&state, &client) { state.lock().unwrap().artist_albums_cache.insert(id, v); } }
                    Err(e) => if crate::is_not_found(&e) { state.lock().unwrap().artist_albums_cache.remove(&id); }
                }
                state.lock().unwrap().prewarm_metadata_done += 1;
            });
        }
        for id in album_ids {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) { return; }
                match client.get_album_tracks(&id).await {
                    Ok(v)  => { if session_current(&state, &client) { state.lock().unwrap().container_tracks_cache.insert(id, v); } }
                    Err(e) => if crate::is_not_found(&e) { state.lock().unwrap().container_tracks_cache.remove(&id); }
                }
                state.lock().unwrap().prewarm_metadata_done += 1;
            });
        }
        for id in playlist_ids {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) { return; }
                match client.get_playlist_items(&id).await {
                    Ok(v)  => { if session_current(&state, &client) { state.lock().unwrap().container_tracks_cache.insert(id, v); } }
                    Err(e) => if crate::is_not_found(&e) { state.lock().unwrap().container_tracks_cache.remove(&id); }
                }
                state.lock().unwrap().prewarm_metadata_done += 1;
            });
        }
        for id in movie_ids.into_iter().chain(series_ids) {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                if !session_current(&state, &client) { return; }
                match client.get_similar_items(&id).await {
                    Ok(v)  => { if session_current(&state, &client) { state.lock().unwrap().similar_items_cache.insert(id, v); } }
                    Err(e) => if crate::is_not_found(&e) { state.lock().unwrap().similar_items_cache.remove(&id); }
                }
                state.lock().unwrap().prewarm_metadata_done += 1;
            });
        }

        while set.join_next().await.is_some() {}

        let elapsed = start.elapsed();
        let total_requests = req_top_detail + req_person_detail + req_similar
            + req_boxset + req_artist + req_tracks + req_person_film;
        let summary = format!(
            "metadata prewarm complete in {:.0?}: {total_requests} requests total \
             ({req_top_detail} top-level detail batches, {req_person_detail} person-detail \
             batches, {req_similar} similar-items, {req_boxset} boxset, {req_artist} \
             artist-album, {req_tracks} track, {req_person_film} person-filmography)",
            elapsed,
        );
        info!("{summary}");
        let mut s = state.lock().unwrap();
        s.prewarm_metadata_running = false;
        s.prewarm_metadata_summary = summary;
    });
}

// ── image prewarm ────────────────────────────────────────────────────────────

pub(crate) fn spawn_image_prewarm(
    client: Arc<JellyfinClient>,
    state:  Arc<Mutex<FjordState>>,
    rt:     tokio::runtime::Handle,
) {
    {
        let mut s = state.lock().unwrap();
        if s.prewarm_image_running { return; }
        s.prewarm_image_running = true;
        s.prewarm_image_total   = 0;
        s.prewarm_image_done    = 0;
        s.prewarm_image_summary = String::new();
    }

    rt.spawn(async move {
        let start = Instant::now();

        // Operate over whatever item_detail_cache currently holds — populated by
        // spawn_metadata_prewarm, by ordinary lazy screen-open caching, or both.
        let items: Vec<fjord_api::models::MediaItem> = {
            let s = state.lock().unwrap();
            s.item_detail_cache.iter().map(|(_, v)| v.clone()).collect()
        };

        let backdrop_targets: Vec<(String, String)> = items.iter()
            .filter(|i| !i.backdrop_image_tags.is_empty())
            .map(|i| (i.id.clone(), i.backdrop_image_tags[0].clone()))
            .collect();
        let mut person_ids: HashSet<String> = Default::default();
        for item in &items {
            for p in &item.people {
                if !p.id.is_empty() { person_ids.insert(p.id.clone()); }
            }
        }
        let person_ids: Vec<String> = person_ids.into_iter().collect();

        let backdrop_count = backdrop_targets.len();
        let portrait_count = person_ids.len();
        {
            let mut s = state.lock().unwrap();
            s.prewarm_image_total = backdrop_count + portrait_count;
        }

        // fetch_backdrop_cached_tagged/fetch_poster_cached already check disk
        // first and only hit the network on a miss or tag mismatch (see
        // poster.rs::fetch_image_cached) — a re-run of this sweep is naturally
        // cheap for anything already cached, without needing to duplicate that
        // check here just for logging purposes.
        let sem = Arc::new(tokio::sync::Semaphore::new(PREWARM_CONCURRENCY));
        let mut set = tokio::task::JoinSet::new();

        for (id, tag) in backdrop_targets {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                fetch_backdrop_cached_tagged(&client, &id, Some(&tag)).await;
                state.lock().unwrap().prewarm_image_done += 1;
            });
        }
        for pid in person_ids {
            let (client, state, sem) = (client.clone(), Arc::clone(&state), Arc::clone(&sem));
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                fetch_poster_cached(&client, &pid).await;
                state.lock().unwrap().prewarm_image_done += 1;
            });
        }

        while set.join_next().await.is_some() {}

        let elapsed = start.elapsed();
        let summary = format!(
            "image prewarm complete in {elapsed:.0?}: {backdrop_count} backdrop(s) + {portrait_count} portrait(s) processed (already-cached ones were disk-only, no network)",
        );
        info!("{summary}");
        let mut s = state.lock().unwrap();
        s.prewarm_image_running = false;
        s.prewarm_image_summary = summary;
    });
}
