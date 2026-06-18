// ── fjord-app · poster.rs ────────────────────────────────────────────────────
//   ImageKind              Poster | Backdrop — selects cache path and API method
//   fetch_image_cached     shared fetch-or-cache implementation for both kinds
//   fetch_poster_cached    thin wrapper: fetch_image_cached(…, Poster)
//   fetch_backdrop_cached  thin wrapper: fetch_image_cached(…, Backdrop)
//   decode_poster_buffer   JPEG/PNG bytes → SharedPixelBuffer (CPU decode)
//   spawn_poster_loading   parallel poster fetch for dashboard section rows; sets series-id on Episode cards
//   spawn_series_poster_loading  same for series cards → AppState.all-series
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;

use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, ModelRc, SharedString, VecModel};

use crate::config::{poster_cache_path, backdrop_cache_path};
use crate::{AppState, CardItem, MainWindow};

enum ImageKind { Poster, Backdrop }

async fn fetch_image_cached(client: &JellyfinClient, item_id: &str, kind: ImageKind) -> Option<Vec<u8>> {
    let path = match kind {
        ImageKind::Poster   => poster_cache_path(item_id),
        ImageKind::Backdrop => backdrop_cache_path(item_id),
    };
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return tokio::fs::read(&path).await.ok();
    }
    let bytes = match kind {
        ImageKind::Poster   => client.fetch_poster_bytes(item_id).await.ok()?,
        ImageKind::Backdrop => client.fetch_backdrop_bytes(item_id).await.ok()?,
    };
    if let Some(parent) = path.parent() { let _ = tokio::fs::create_dir_all(parent).await; }
    let _ = tokio::fs::write(&path, &bytes).await;
    Some(bytes)
}

pub(crate) async fn fetch_poster_cached(client: &JellyfinClient, item_id: &str) -> Option<Vec<u8>> {
    fetch_image_cached(client, item_id, ImageKind::Poster).await
}

pub(crate) async fn fetch_backdrop_cached(client: &JellyfinClient, item_id: &str) -> Option<Vec<u8>> {
    fetch_image_cached(client, item_id, ImageKind::Backdrop).await
}

// Returns a Send-able pixel buffer rather than slint::Image (which is !Send).
// Callers must call Image::from_rgba8 on the UI thread.
pub(crate) fn decode_poster_buffer(bytes: &[u8]) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
    let img = image::load_from_memory(bytes).ok()?.into_rgba8();
    let (w, h) = img.dimensions();
    Some(slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        img.as_raw(), w, h,
    ))
}

pub(crate) fn spawn_poster_loading(
    client:      Arc<JellyfinClient>,
    sections:    [Vec<MediaItem>; 9],
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    rt_handle.spawn(async move {
        use std::collections::{HashMap, HashSet};
        use std::sync::Arc as SArc;

        // Per-section card metadata: (item_id, poster_id, item_type, title, year, played, is_fav, resume_pct, unplayed_count).
        // For episodes, poster_id = series_id so we show the series poster, not an episode thumb.
        let section_meta: Vec<Vec<(String, String, String, String, i32, bool, bool, f32, i32)>> = sections.iter()
            .map(|items| items.iter().map(|i| {
                let poster_id = if i.item_type == "Episode" {
                    i.series_id.clone().unwrap_or_else(|| i.id.clone())
                } else {
                    i.id.clone()
                };
                (i.id.clone(), poster_id, i.item_type.clone(), i.display_name(),
                 i.production_year.unwrap_or(0) as i32, i.user_data.played,
                 i.user_data.is_favorite, i.resume_pct(), i.user_data.unplayed_item_count)
            }).collect())
            .collect();

        // Pending set per section — keyed by poster_id, removed as each poster arrives.
        let mut section_pending: Vec<HashSet<String>> = section_meta.iter()
            .map(|cards| cards.iter().map(|(_, poster_id, _, _, _, _, _, _, _)| poster_id.clone()).collect())
            .collect();

        // Deduplicate: each unique poster_id is fetched exactly once.
        let unique_ids: HashSet<String> = section_meta.iter().flatten()
            .map(|(_, poster_id, _, _, _, _, _, _, _)| poster_id.clone())
            .collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for poster_id in unique_ids {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            fetch_set.spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else { return (poster_id, None) };
                let bytes   = fetch_poster_cached(&*client, &poster_id).await.map(SArc::new);
                (poster_id, bytes)
            });
        }

        let mut poster_map: HashMap<String, SArc<Vec<u8>>> = HashMap::new();

        while let Some(res) = fetch_set.join_next().await {
            let Ok((poster_id, bytes)) = res else { continue };
            if let Some(b) = bytes { poster_map.insert(poster_id.clone(), b); }

            // Mark this poster_id done in every section that references it.
            // Push a section the moment its last pending poster is resolved.
            for sec_idx in 0..9usize {
                if !section_pending[sec_idx].remove(&poster_id) { continue; }
                if !section_pending[sec_idx].is_empty()         { continue; }
                // Decode JPEG/PNG here (async worker thread) — produces Send-able
                // SharedPixelBuffer.  Image::from_rgba8 runs on the UI thread below.
                type Buf     = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
                type Decoded = (SharedString, SharedString, SharedString, SharedString, i32, bool, bool, f32, i32, Option<Buf>); // id, series_id, item_type, title, year, played, is_fav, rpct, upc, buf
                let decoded: Vec<Decoded> =
                    section_meta[sec_idx].iter().map(|(item_id, poster_id, item_type, title, year, played, is_fav, rpct, upc)| {
                        let buf = poster_map.get(poster_id).and_then(|b| decode_poster_buffer(b));
                        let series_id = if item_type == "Episode" { SharedString::from(poster_id.as_str()) } else { SharedString::default() };
                        (SharedString::from(item_id.as_str()), series_id, SharedString::from(item_type.as_str()), SharedString::from(title.as_str()), *year, *played, *is_fav, *rpct, *upc, buf)
                    }).collect();
                let ww = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let items: Vec<CardItem> = decoded.into_iter().map(|(id, series_id, item_type, title, year, played, is_fav, rpct, upc, buf)| {
                            let mut h = CardItem::default();
                            h.id             = id;
                            h.series_id      = series_id;
                            h.item_type      = item_type;
                            h.title          = title;
                            h.year           = year;
                            h.has_played     = played;
                            h.is_favorite    = is_fav;
                            h.resume_pct     = rpct;
                            h.unplayed_count = upc;
                            if let Some(spb) = buf {
                                h.poster     = slint::Image::from_rgba8(spb);
                                h.has_poster = true;
                            }
                            h
                        }).collect();
                        crate::push_section_model(&w, sec_idx, ModelRc::new(VecModel::from(items)));
                    }
                });
            }
        }
    });
}

pub(crate) fn spawn_series_poster_loading(
    client:      Arc<JellyfinClient>,
    series:      Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    rt_handle.spawn(async move {
        use std::collections::HashSet;
        use std::sync::Arc as SArc;

        let meta: Vec<(String, String, i32, bool, bool, f32, i32)> = series.iter()
            .map(|i| (i.id.clone(), i.display_name(), i.production_year.unwrap_or(0) as i32, i.user_data.played, i.user_data.is_favorite, i.resume_pct(), i.user_data.unplayed_item_count))
            .collect();
        let mut pending: HashSet<String> = meta.iter().map(|(id, _, _, _, _, _, _)| id.clone()).collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for (id, _, _, _, _, _, _) in &meta {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            let id     = id.clone();
            fetch_set.spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else { return (id, None) };
                let bytes   = fetch_poster_cached(&*client, &id).await.map(SArc::new);
                (id, bytes)
            });
        }

        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();

        while let Some(res) = fetch_set.join_next().await {
            let Ok((id, bytes)) = res else { continue };
            if let Some(b) = bytes { poster_map.insert(id.clone(), b); }
            pending.remove(&id);
            if !pending.is_empty() { continue; }

            type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
            let decoded: Vec<(SharedString, SharedString, i32, bool, bool, f32, i32, Option<Buf>)> =
                meta.iter().map(|(cid, title, year, played, is_fav, rpct, upc)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (SharedString::from(cid.as_str()), SharedString::from(title.as_str()), *year, *played, *is_fav, *rpct, *upc, buf)
                }).collect();
            let ww = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let items: Vec<CardItem> = decoded.into_iter().map(|(id, title, year, played, is_fav, rpct, upc, buf)| {
                        let mut h = CardItem::default();
                        h.id             = id;
                        h.item_type      = "Series".into();
                        h.title          = title;
                        h.year           = year;
                        h.has_played     = played;
                        h.is_favorite    = is_fav;
                        h.resume_pct     = rpct;
                        h.unplayed_count = upc;
                        if let Some(spb) = buf { h.poster = slint::Image::from_rgba8(spb); h.has_poster = true; }
                        h
                    }).collect();
                    let model = ModelRc::new(VecModel::from(items));
                    let g = AppState::get(&w);
                    g.set_all_series(model.clone());
                    if g.get_show_library() && g.get_active_nav() == 2 && g.get_library_query().is_empty() {
                        g.set_library_display(model);
                    }
                }
            });
        }
    });
}
