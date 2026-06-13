// ── fjord-app · poster.rs ────────────────────────────────────────────────────
//   fetch_poster_cached    fetch/cache raw poster bytes for any item
//   fetch_backdrop_cached  fetch/cache raw backdrop bytes for any item
//   decode_poster_buffer   JPEG/PNG bytes → SharedPixelBuffer (CPU decode)
//   spawn_poster_loading   parallel poster fetch for dashboard section rows
//   spawn_series_poster_loading  same for series cards → AppState.all-series
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;

use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, ModelRc, SharedString, VecModel};

use crate::config::{poster_cache_path, backdrop_cache_path};
use crate::{AppState, CardItem, MainWindow};

pub(crate) async fn fetch_poster_cached(client: &JellyfinClient, item_id: &str) -> Option<Vec<u8>> {
    let path = poster_cache_path(item_id);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return tokio::fs::read(&path).await.ok();
    }
    let bytes = client.fetch_poster_bytes(item_id).await.ok()?;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&path, &bytes).await;
    Some(bytes)
}

pub(crate) async fn fetch_backdrop_cached(client: &JellyfinClient, item_id: &str) -> Option<Vec<u8>> {
    let path = backdrop_cache_path(item_id);
    if tokio::fs::try_exists(&path).await.unwrap_or(false) {
        return tokio::fs::read(&path).await.ok();
    }
    let bytes = client.fetch_backdrop_bytes(item_id).await.ok()?;
    if let Some(parent) = path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&path, &bytes).await;
    Some(bytes)
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

        // Per-section card metadata: (item_id, poster_id, title, year, played, resume_pct).
        // For episodes, poster_id = series_id so we show the series poster, not an episode thumb.
        let section_meta: Vec<Vec<(String, String, String, i32, bool, f32)>> = sections.iter()
            .map(|items| items.iter().map(|i| {
                let poster_id = if i.item_type == "Episode" {
                    i.series_id.clone().unwrap_or_else(|| i.id.clone())
                } else {
                    i.id.clone()
                };
                (i.id.clone(), poster_id, i.display_name(),
                 i.production_year.unwrap_or(0) as i32, i.user_data.played, i.resume_pct())
            }).collect())
            .collect();

        // Pending set per section — keyed by poster_id, removed as each poster arrives.
        let mut section_pending: Vec<HashSet<String>> = section_meta.iter()
            .map(|cards| cards.iter().map(|(_, poster_id, _, _, _, _)| poster_id.clone()).collect())
            .collect();

        // Deduplicate: each unique poster_id is fetched exactly once.
        let unique_ids: HashSet<String> = section_meta.iter().flatten()
            .map(|(_, poster_id, _, _, _, _)| poster_id.clone())
            .collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for poster_id in unique_ids {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            fetch_set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
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
                type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
                let decoded: Vec<(SharedString, SharedString, i32, bool, f32, Option<Buf>)> =
                    section_meta[sec_idx].iter().map(|(item_id, poster_id, title, year, played, rpct)| {
                        let buf = poster_map.get(poster_id).and_then(|b| decode_poster_buffer(b));
                        (SharedString::from(item_id.as_str()), SharedString::from(title.as_str()), *year, *played, *rpct, buf)
                    }).collect();
                let ww = window_weak.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let items: Vec<CardItem> = decoded.into_iter().map(|(id, title, year, played, rpct, buf)| {
                            let mut h = CardItem::default();
                            h.id         = id;
                            h.title      = title;
                            h.year       = year;
                            h.has_played = played;
                            h.resume_pct = rpct;
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

        let meta: Vec<(String, String, i32, bool, f32, i32)> = series.iter()
            .map(|i| (i.id.clone(), i.display_name(), i.production_year.unwrap_or(0) as i32, i.user_data.played, i.resume_pct(), i.user_data.unplayed_item_count))
            .collect();
        let mut pending: HashSet<String> = meta.iter().map(|(id, _, _, _, _, _)| id.clone()).collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for (id, _, _, _, _, _) in &meta {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            let id     = id.clone();
            fetch_set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
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
            let decoded: Vec<(SharedString, SharedString, i32, bool, f32, i32, Option<Buf>)> =
                meta.iter().map(|(cid, title, year, played, rpct, upc)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (SharedString::from(cid.as_str()), SharedString::from(title.as_str()), *year, *played, *rpct, *upc, buf)
                }).collect();
            let ww = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let items: Vec<CardItem> = decoded.into_iter().map(|(id, title, year, played, rpct, upc, buf)| {
                        let mut h = CardItem::default();
                        h.id             = id;
                        h.title          = title;
                        h.year           = year;
                        h.has_played     = played;
                        h.resume_pct     = rpct;
                        h.unplayed_count = upc;
                        if let Some(spb) = buf { h.poster = slint::Image::from_rgba8(spb); h.has_poster = true; }
                        h
                    }).collect();
                    AppState::get(&w).set_all_series(ModelRc::new(VecModel::from(items)));
                }
            });
        }
    });
}
