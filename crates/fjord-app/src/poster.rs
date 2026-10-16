// ── fjord-app · poster.rs ────────────────────────────────────────────────────
//   ImageKind              Poster | Backdrop — selects cache path and API method
//   fetch_image_cached     shared fetch-or-cache implementation for both kinds; no disk cache for
//                          an id that isn't a safe cache name; only bytes passing is_image are
//                          written
//   is_image               JPEG/PNG/WebP header with dimensions (unit-tested)
//   fetch_poster_cached    thin wrapper: fetch_image_cached(…, Poster)
//   fetch_backdrop_cached  thin wrapper: fetch_image_cached(…, Backdrop)
//   fetch_posters_for_delta  generic concurrent poster fetch for a WS delta batch (any item
//                          type mix); doesn't replace a model — caller patches rows itself
//   decode_scaled / decode_poster_buffer (≤600px) / decode_backdrop_buffer (≤3840px) /
//                          decode_blur_buffer (24px — the Music hero's blurred cover)
//                          decode-to-size: originals are 1000-3000px, cards render ≤400px
//   push_decoded_section   decode poster bytes for one section and push it on the UI thread;
//                          session-guarded (per-profile content — the push_ functions and their
//                          spawn_ callers take `state` for the check)
//   spawn_poster_loading   parallel poster fetch for [(HomeSection, Vec<MediaItem>); 17]; sets
//                          series-id on Episode cards
//   load_grid_posters / PosterBufs  a library grid's posters: 8 parallel fetch+decode tasks, the
//                          decoded buffers pushed in batches (POSTER_BATCH_INTERVAL 150 ms, then a
//                          final push) so the grid fills as they arrive; `timing:` first batch/total
//   spawn_series_poster_loading / push_decoded_series  the TV grid through load_grid_posters →
//                          AppState.all-series (a row keeps the poster it already shows)
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_api::{JellyfinClient, models::MediaItem};
use slint::{Global, Model, SharedString};

use crate::config::{FjordState, backdrop_cache_path, poster_cache_path};
use crate::home::HomeSection;
use crate::{AppState, CardItem, MainWindow};

enum ImageKind {
    Poster,
    Backdrop,
}

// Tag-revalidating image cache. `expected_tag` is the server's current image
// hash (ImageTags.Primary / BackdropImageTags[0]): when it matches the `.tag`
// sidecar the disk file is served; when it differs the image is re-downloaded
// (artwork was replaced server-side — before this, a cached poster was stale
// forever). `None` keeps the old serve-whatever-is-on-disk behaviour for
// callers that don't know the tag. A failed re-download falls back to the
// stale disk copy rather than showing nothing.
async fn fetch_image_cached(
    client: &JellyfinClient,
    item_id: &str,
    kind: ImageKind,
    expected_tag: Option<&str>,
) -> Option<Vec<u8>> {
    let path = match kind {
        ImageKind::Poster => poster_cache_path(item_id),
        ImageKind::Backdrop => backdrop_cache_path(item_id),
    };
    // Not a valid cache name (config::safe_cache_name): fetch, but don't cache.
    let Some(path) = path else {
        let fetched = match kind {
            ImageKind::Poster => client.fetch_poster_bytes(item_id).await,
            ImageKind::Backdrop => client.fetch_backdrop_bytes(item_id).await,
        };
        return fetched.ok().filter(|b| is_image(b));
    };
    let tag_path = path.with_extension("tag");
    let cached = tokio::fs::try_exists(&path).await.unwrap_or(false);

    if cached {
        let fresh = match expected_tag {
            None => true,
            Some(tag) => tokio::fs::read_to_string(&tag_path)
                .await
                .is_ok_and(|t| t.trim() == tag),
        };
        if fresh {
            return tokio::fs::read(&path).await.ok();
        }
    }

    let fetched = match kind {
        ImageKind::Poster => client.fetch_poster_bytes(item_id).await,
        ImageKind::Backdrop => client.fetch_backdrop_bytes(item_id).await,
    };
    let bytes = match fetched {
        Ok(b) if is_image(&b) => b,
        // Network failure, or the server sent something that isn't an
        // image (never written to disk): a stale image beats no image.
        _ if cached => return tokio::fs::read(&path).await.ok(),
        _ => return None,
    };

    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    // Write to a tmp file then rename atomically so concurrent fetchers for the
    // same id never produce a partial/interleaved cache entry.
    let tmp = path.with_extension("tmp");
    if tokio::fs::write(&tmp, &bytes).await.is_ok() {
        let _ = tokio::fs::rename(&tmp, &path).await;
    }
    match expected_tag {
        Some(tag) => {
            let tag_tmp = path.with_extension("tag.tmp");
            if tokio::fs::write(&tag_tmp, tag).await.is_ok() {
                let _ = tokio::fs::rename(&tag_tmp, &tag_path).await;
            }
        }
        // No tag known for this download — drop any stale sidecar so a later
        // tagged fetch can't mistake this image for a specific version.
        None => {
            let _ = tokio::fs::remove_file(&tag_path).await;
        }
    }
    Some(bytes)
}

pub(crate) async fn fetch_poster_cached(client: &JellyfinClient, item_id: &str) -> Option<Vec<u8>> {
    fetch_image_cached(client, item_id, ImageKind::Poster, None).await
}

pub(crate) async fn fetch_poster_cached_tagged(
    client: &JellyfinClient,
    item_id: &str,
    tag: Option<&str>,
) -> Option<Vec<u8>> {
    fetch_image_cached(client, item_id, ImageKind::Poster, tag).await
}

pub(crate) async fn fetch_backdrop_cached(
    client: &JellyfinClient,
    item_id: &str,
) -> Option<Vec<u8>> {
    fetch_image_cached(client, item_id, ImageKind::Backdrop, None).await
}

pub(crate) async fn fetch_backdrop_cached_tagged(
    client: &JellyfinClient,
    item_id: &str,
    tag: Option<&str>,
) -> Option<Vec<u8>> {
    fetch_image_cached(client, item_id, ImageKind::Backdrop, tag).await
}

/// True when `bytes` start like an image Fjord can decode (JPEG/PNG/WebP
/// header with dimensions) — checked before anything is written to the
/// cache, so a server can't use it to store other content.
pub(crate) fn is_image(bytes: &[u8]) -> bool {
    image::ImageReader::new(std::io::Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|r| r.format().is_some().then_some(r))
        .is_some_and(|r| r.into_dimensions().is_ok())
}

/// Fetch + decode posters for a WS delta batch (any mix of item types — library-list
/// upserts, Recently Added inserts, etc. all share one call). Unlike the six per-type
/// spawn_*_poster_loading loaders, this never replaces a whole model — callers patch
/// results into rows themselves (see context_menu::upsert_cards_in_model). Every item in
/// the batch is re-resolved (not just new ones): fetch_poster_cached_tagged already no-ops
/// to the cached file when the tag is unchanged, so this stays cheap for updates too.
pub(crate) async fn fetch_posters_for_delta(
    client: &Arc<JellyfinClient>,
    items: &[MediaItem],
) -> std::collections::HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let sem = Arc::new(tokio::sync::Semaphore::new(8));
    let mut set: tokio::task::JoinSet<(
        String,
        Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    )> = tokio::task::JoinSet::new();
    for item in items {
        let client = Arc::clone(client);
        let sem = Arc::clone(&sem);
        let id = item.id.clone();
        let tag = item.primary_image_tag().map(|t| t.to_string());
        set.spawn(async move {
            let Ok(_permit) = sem.acquire_owned().await else {
                return (id, None);
            };
            let bytes = fetch_poster_cached_tagged(&client, &id, tag.as_deref()).await;
            (id, bytes.and_then(|b| decode_poster_buffer(&b)))
        });
    }
    let mut map = std::collections::HashMap::new();
    while let Some(res) = set.join_next().await {
        match res {
            Ok((id, Some(buf))) => {
                map.insert(id, buf);
            }
            Ok((_, None)) => {}
            Err(e) => tracing::warn!("delta poster task panicked: {e}"),
        }
    }
    map
}

// Returns a Send-able pixel buffer rather than slint::Image (which is !Send).
// Callers must call Image::from_rgba8 on the UI thread.
// Decode and downscale so the longest side is ≤ max_dim. Servers deliver
// 1000–3000 px originals; a full decode is ~6 MB of RGBA that then lives in
// every CardItem model row. Cards render at ~300–400 px even on 4K, so
// keeping originals decoded multiplied memory use ~6-10× for zero visible
// gain. `thumbnail` preserves aspect and uses the fast path.
fn decode_scaled(
    bytes: &[u8],
    max_dim: u32,
) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = if img.width().max(img.height()) > max_dim {
        img.thumbnail(max_dim, max_dim)
    } else {
        img
    };
    let img = img.into_rgba8();
    let (w, h) = img.dimensions();
    Some(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(img.as_raw(), w, h))
}

/// Posters, portraits, episode thumbs, album art: longest side capped at 600 px
/// (a 2:3 poster decodes to 400×600 — crisp on ~300-400 px cards, ~1 MB RGBA).
pub(crate) fn decode_poster_buffer(
    bytes: &[u8],
) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    decode_scaled(bytes, 600)
}

/// The Music hero's background: the album cover decoded to 24 px — Slint stretches it smooth,
/// which reads as a blur.
pub(crate) fn decode_blur_buffer(
    bytes: &[u8],
) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    decode_scaled(bytes, 24)
}

/// Backdrops render full-window — cap at 3840 so 4K sources display 1:1 on a
/// 4K screen (most server backdrops are 1920-wide anyway). Bounded cost: only
/// a handful of backdrop properties are alive at once, ≤33 MB each worst case.
/// 8K screens upscale 2× — same as they do to all 4K content.
pub(crate) fn decode_backdrop_buffer(
    bytes: &[u8],
) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    decode_scaled(bytes, 3840)
}

type SectionMeta = Vec<(
    String,
    String,
    String,
    String,
    String,
    i32,
    bool,
    bool,
    f32,
    i32,
)>;
type SeriesMeta = Vec<(String, String, String, i32, bool, bool, f32, i32)>;
// SeriesMeta's fields, Slint-ready (SharedString) plus the decoded poster buffer
// (None when that item has no poster or decode failed).
type DecodedSeriesCard = (
    SharedString,
    SharedString,
    SharedString,
    i32,
    bool,
    bool,
    f32,
    i32,
    Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
);

/// Decode poster bytes for every item in one section and push the completed
/// CardItem model to AppState on the UI thread via invoke_from_event_loop.
/// Called from both the normal completion path (last poster in a section
/// arrives) and the post-loop flush (task panics left some posters unresolved).
fn push_decoded_section(
    sec: HomeSection,
    meta: &SectionMeta,
    poster_map: &std::collections::HashMap<String, std::sync::Arc<Vec<u8>>>,
    ww: &slint::Weak<MainWindow>,
    state: &Arc<Mutex<FjordState>>,
    client: &Arc<JellyfinClient>,
) {
    type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
    type Decoded = (
        SharedString,
        SharedString,
        SharedString,
        SharedString,
        SharedString,
        i32,
        bool,
        bool,
        f32,
        i32,
        Option<Buf>,
    );
    let decoded: Vec<Decoded> = meta
        .iter()
        .map(
            |(item_id, poster_id, item_type, title, subtitle, year, played, is_fav, rpct, upc)| {
                let buf = poster_map
                    .get(poster_id)
                    .and_then(|b| decode_poster_buffer(b));
                let series_id = if item_type == "Episode" {
                    SharedString::from(poster_id.as_str())
                } else {
                    SharedString::default()
                };
                (
                    SharedString::from(item_id.as_str()),
                    series_id,
                    SharedString::from(item_type.as_str()),
                    SharedString::from(title.as_str()),
                    SharedString::from(subtitle.as_str()),
                    *year,
                    *played,
                    *is_fav,
                    *rpct,
                    *upc,
                    buf,
                )
            },
        )
        .collect();
    let ww = ww.clone();
    let state = Arc::clone(state);
    let client = Arc::clone(client);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            // Session guard: these are per-profile Home rows (Continue Watching, Favorites…) — a
            // mid-fetch sign-out or switch must not show the old profile's in the new session.
            if !crate::session_current(&state, &client) {
                return;
            }
            let old = crate::get_section_model(&w, sec);
            // Prefer whatever poster the row already has over a freshly-decoded one
            // (same reasoning as movies.rs::push_library_cards): a different-but-
            // identical Image handle still means every card's texture gets swapped
            // even when apply_cards_preserving_identity avoids recreating elements.
            let old_by_id: std::collections::HashMap<String, CardItem> = (0..old.row_count())
                .filter_map(|i| old.row_data(i))
                .map(|c| (c.id.to_string(), c))
                .collect();
            let items: Vec<CardItem> = decoded
                .into_iter()
                .map(
                    |(
                        id,
                        series_id,
                        item_type,
                        title,
                        subtitle,
                        year,
                        played,
                        is_fav,
                        rpct,
                        upc,
                        buf,
                    )| {
                        let mut h = CardItem::default();
                        let existing_poster = old_by_id
                            .get(id.as_str())
                            .filter(|c| c.has_poster)
                            .map(|c| c.poster.clone());
                        h.id = id;
                        h.series_id = series_id;
                        h.item_type = item_type;
                        h.title = title;
                        h.subtitle = subtitle;
                        h.year = year;
                        h.has_played = played;
                        h.is_favorite = is_fav;
                        h.resume_pct = rpct;
                        h.unplayed_count = upc;
                        if let Some(poster) = existing_poster {
                            h.poster = poster;
                            h.has_poster = true;
                        } else if let Some(spb) = buf {
                            h.poster = slint::Image::from_rgba8(spb);
                            h.has_poster = true;
                        }
                        h
                    },
                )
                .collect();
            crate::push_section_model(&w, sec, crate::apply_cards_preserving_identity(&old, items));
        }
    });
}

/// One load_grid_posters batch → AppState.all-series (and library-display when the TV grid is
/// open): every card from `meta`, the newly decoded posters for rows that have none yet (a row
/// keeps the poster it already shows).
fn push_decoded_series(
    meta: &SeriesMeta,
    mut bufs: PosterBufs,
    ww: &slint::Weak<MainWindow>,
    state: &Arc<Mutex<FjordState>>,
    client: &Arc<JellyfinClient>,
) {
    let decoded: Vec<DecodedSeriesCard> = meta
        .iter()
        .map(|(cid, title, subtitle, year, played, is_fav, rpct, upc)| {
            let buf = bufs.remove(cid);
            (
                SharedString::from(cid.as_str()),
                SharedString::from(title.as_str()),
                SharedString::from(subtitle.as_str()),
                *year,
                *played,
                *is_fav,
                *rpct,
                *upc,
                buf,
            )
        })
        .collect();
    let ww = ww.clone();
    let state = Arc::clone(state);
    let client = Arc::clone(client);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            // Session guard: all-series is per-profile content too (see push_decoded_section).
            if !crate::session_current(&state, &client) {
                return;
            }
            let old = AppState::get(&w).get_all_series();
            let old_by_id: std::collections::HashMap<String, CardItem> = (0..old.row_count())
                .filter_map(|i| old.row_data(i))
                .map(|c| (c.id.to_string(), c))
                .collect();
            let items: Vec<CardItem> = decoded
                .into_iter()
                .map(
                    |(id, title, subtitle, year, played, is_fav, rpct, upc, buf)| {
                        let mut h = CardItem::default();
                        let existing_poster = old_by_id
                            .get(id.as_str())
                            .filter(|c| c.has_poster)
                            .map(|c| c.poster.clone());
                        h.id = id;
                        h.item_type = "Series".into();
                        h.title = title;
                        h.subtitle = subtitle;
                        h.year = year;
                        h.has_played = played;
                        h.is_favorite = is_fav;
                        h.resume_pct = rpct;
                        h.unplayed_count = upc;
                        if let Some(poster) = existing_poster {
                            h.poster = poster;
                            h.has_poster = true;
                        } else if let Some(spb) = buf {
                            h.poster = slint::Image::from_rgba8(spb);
                            h.has_poster = true;
                        }
                        h
                    },
                )
                .collect();
            AppState::get(&w).set_all_series(crate::apply_cards_preserving_identity(&old, items));
            // TV grid is nav 1 (nav 2 is Movies — the old ==2 guard meant the TV
            // grid never refreshed after posters loaded, and the Movies grid got
            // a spurious refresh that re-shuffled sort=Random). CR10-9.
            if AppState::get(&w).get_show_library() && AppState::get(&w).get_active_nav() == 1 {
                crate::browse::refresh_library_display(&w);
            }
        }
    });
}

pub(crate) fn spawn_poster_loading(
    client: Arc<JellyfinClient>,
    sections: [(HomeSection, Vec<MediaItem>); 17],
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
    state: Arc<Mutex<FjordState>>,
) {
    let total_items: usize = sections.iter().map(|(_, items)| items.len()).sum();
    tracing::debug!("spawn_poster_loading: starting, {total_items} item(s) across 17 sections");
    let call_start = std::time::Instant::now();
    rt_handle.spawn(async move {
        use std::collections::{HashMap, HashSet};
        use std::sync::Arc as SArc;

        let section_kinds: [HomeSection; 17] = std::array::from_fn(|i| sections[i].0);

        // Per-section card metadata: (item_id, poster_id, item_type, title, year, played, is_fav, resume_pct, unplayed_count).
        // For episodes, poster_id = series_id so we show the series poster, not an episode thumb.
        let section_meta: Vec<SectionMeta> = sections.iter()
            .map(|(_, items)| items.iter().map(|i| {
                let poster_id = if i.item_type == "Episode" {
                    i.series_id.clone().unwrap_or_else(|| i.id.clone())
                } else {
                    i.id.clone()
                };
                (i.id.clone(), poster_id, i.item_type.clone(), i.card_title(), i.card_subtitle(),
                 i.production_year.unwrap_or(0) as i32, i.user_data.played,
                 i.user_data.is_favorite, i.resume_pct(), i.user_data.unplayed_item_count)
            }).collect())
            .collect();

        // Pending set per section — keyed by poster_id, removed as each poster arrives.
        let mut section_pending: Vec<HashSet<String>> = section_meta.iter()
            .map(|cards| cards.iter().map(|(_, poster_id, _, _, _, _, _, _, _, _)| poster_id.clone()).collect())
            .collect();

        // Deduplicate: each unique poster_id is fetched exactly once.
        let unique_ids: HashSet<String> = section_meta.iter().flatten()
            .map(|(_, poster_id, _, _, _, _, _, _, _, _)| poster_id.clone())
            .collect();

        // poster_id → primary image tag, for artwork revalidation. Episode cards
        // use the SERIES poster: the series' own tag lands here when a Series
        // item appears in any section; otherwise that fetch stays untagged.
        let tag_map: HashMap<String, String> = sections.iter()
            .flat_map(|(_, items)| items.iter())
            .filter(|i| i.item_type != "Episode")
            .filter_map(|i| i.primary_image_tag().map(|t| (i.id.clone(), t.to_string())))
            .collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for poster_id in unique_ids {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            let tag    = tag_map.get(&poster_id).cloned();
            fetch_set.spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else { return (poster_id, None) };
                let bytes = fetch_poster_cached_tagged(&client, &poster_id, tag.as_deref()).await.map(SArc::new);
                (poster_id, bytes)
            });
        }

        let mut poster_map: HashMap<String, SArc<Vec<u8>>> = HashMap::new();

        while let Some(res) = fetch_set.join_next().await {
            let (poster_id, bytes) = match res {
                Ok(pair) => pair,
                Err(e) => { tracing::warn!("home poster task panicked: {e}"); continue; }
            };
            if let Some(b) = bytes { poster_map.insert(poster_id.clone(), b); }

            // Mark this poster_id done in every section that references it.
            // Push a section the moment its last pending poster is resolved.
            for sec_idx in 0..16usize {
                if !section_pending[sec_idx].remove(&poster_id) { continue; }
                if !section_pending[sec_idx].is_empty()         { continue; }
                // Decode JPEG/PNG here (async worker thread) — produces Send-able
                // SharedPixelBuffer.  Image::from_rgba8 runs on the UI thread inside the helper.
                tracing::debug!("spawn_poster_loading: section {sec_idx} ({} items) resolved at {:.2}s", section_meta[sec_idx].len(), call_start.elapsed().as_secs_f64());
                push_decoded_section(section_kinds[sec_idx], &section_meta[sec_idx], &poster_map, &window_weak, &state, &client);
            }
        }

        // Post-loop flush: push sections whose last poster coincided with a task panic
        // and were never flushed inside the while loop above.
        for sec_idx in 0..16usize {
            if section_pending[sec_idx].is_empty() { continue; }
            tracing::warn!("home poster section {sec_idx}: {} item(s) never resolved — pushing partial section", section_pending[sec_idx].len());
            push_decoded_section(section_kinds[sec_idx], &section_meta[sec_idx], &poster_map, &window_weak, &state, &client);
        }
        tracing::debug!("spawn_poster_loading: all sections done at {:.2}s", call_start.elapsed().as_secs_f64());
    });
}

/// Decoded poster buffers by item id — one batch for a library grid.
pub(crate) type PosterBufs =
    std::collections::HashMap<String, slint::SharedPixelBuffer<slint::Rgba8Pixel>>;

/// How often a library grid gets the posters decoded so far.
const POSTER_BATCH_INTERVAL: std::time::Duration = std::time::Duration::from_millis(150);

/// Fetches and decodes a library grid's posters, 8 at a time — each task decodes its own
/// poster, off the UI thread. `push` gets the buffers decoded since its last call: at most
/// every POSTER_BATCH_INTERVAL while fetches are still running, then once at the end (always,
/// even with nothing new), so the grid fills as posters arrive instead of all at once. Fetches
/// start in `items` order (the semaphore is FIFO), so the first rows fill first. `items` =
/// (id, primary image tag). Logs `timing:` for the first batch and the whole run.
pub(crate) async fn load_grid_posters(
    client: &Arc<JellyfinClient>,
    items: &[(String, Option<String>)],
    label: &str,
    mut push: impl FnMut(PosterBufs),
) {
    let started = std::time::Instant::now();
    let sem = Arc::new(tokio::sync::Semaphore::new(8));
    let mut set: tokio::task::JoinSet<(
        String,
        Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    )> = tokio::task::JoinSet::new();
    for (id, tag) in items {
        let client = Arc::clone(client);
        let sem = Arc::clone(&sem);
        let (id, tag) = (id.clone(), tag.clone());
        set.spawn(async move {
            let Ok(_permit) = sem.acquire_owned().await else {
                return (id, None);
            };
            let buf = fetch_poster_cached_tagged(&client, &id, tag.as_deref())
                .await
                .and_then(|b| decode_poster_buffer(&b));
            (id, buf)
        });
    }
    let mut fresh = PosterBufs::new();
    let (mut decoded, mut pushes) = (0usize, 0usize);
    let mut last_push = std::time::Instant::now();
    while let Some(res) = set.join_next().await {
        match res {
            Ok((id, Some(buf))) => {
                decoded += 1;
                fresh.insert(id, buf);
            }
            Ok((_, None)) => {}
            Err(e) => tracing::warn!("{label} poster task panicked: {e}"),
        }
        if !set.is_empty() && !fresh.is_empty() && last_push.elapsed() >= POSTER_BATCH_INTERVAL {
            if pushes == 0 {
                tracing::debug!(
                    "timing: {label} posters — first batch ({} of {}) after {:.3}s",
                    fresh.len(),
                    items.len(),
                    started.elapsed().as_secs_f64()
                );
            }
            push(std::mem::take(&mut fresh));
            pushes += 1;
            last_push = std::time::Instant::now();
        }
    }
    push(fresh);
    tracing::debug!(
        "timing: {label} posters — {decoded} of {} decoded in {:.3}s, {} push(es)",
        items.len(),
        started.elapsed().as_secs_f64(),
        pushes + 1
    );
}

pub(crate) fn spawn_series_poster_loading(
    client: Arc<JellyfinClient>,
    series: Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
    state: Arc<Mutex<FjordState>>,
) {
    rt_handle.spawn(async move {
        use std::collections::HashSet;

        // Deduplicate by ID: every card is pushed once per batch by id.
        let mut seen: HashSet<String> = HashSet::new();
        let unique: Vec<&MediaItem> = series
            .iter()
            .filter(|i| seen.insert(i.id.clone()))
            .collect();
        let meta: SeriesMeta = unique
            .iter()
            .map(|i| {
                (
                    i.id.clone(),
                    i.card_title(),
                    i.card_subtitle(),
                    i.production_year.unwrap_or(0) as i32,
                    i.user_data.played,
                    i.user_data.is_favorite,
                    i.resume_pct(),
                    i.user_data.unplayed_item_count,
                )
            })
            .collect();
        // (id, primary image tag) — the tag revalidates cached artwork.
        let ids: Vec<(String, Option<String>)> = unique
            .iter()
            .map(|i| (i.id.clone(), i.primary_image_tag().map(str::to_string)))
            .collect();
        load_grid_posters(&client, &ids, "Series", |bufs| {
            push_decoded_series(&meta, bufs, &window_weak, &state, &client);
        })
        .await;
    });
}

#[cfg(test)]
mod tests {
    use super::is_image;

    #[test]
    fn only_real_images_count() {
        let mut png = Vec::new();
        image::RgbImage::new(2, 2)
            .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert!(is_image(&png));
        assert!(!is_image(
            b"[Desktop Entry]\nType=Application\nExec=sh -c x\n"
        ));
        assert!(!is_image(&[
            0xFF, 0xD8, 0xFF, 0xE0, 0, 0, b'n', b'o', b'p', b'e'
        ]));
        assert!(!is_image(b""));
    }
}
