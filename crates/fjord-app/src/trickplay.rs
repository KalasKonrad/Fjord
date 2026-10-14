// ── fjord-app · trickplay.rs ─────────────────────────────────────────────────
//   Trickplay thumbnails over the seek bar (Jellyfin's trickplay images, verified on 10.11.11).
//   pick_rendition   the media source (the item's own id first) and the width closest to
//                    TARGET_WIDTH (pure, unit-tested)
//   Cell / locate    playback position → thumbnail index → tile index + pixel rect in that tile
//                    (pure, unit-tested)
//   Session          one playing item: rendition, client, tile cache (every fetched tile kept
//                    encoded, only the last DECODED_TILES decoded — one decoded 10×10 tile of
//                    320 px thumbnails is ~23 MB), newest-request counter; lives in
//                    VideoState.trickplay, so teardown drops it
//   spawn_fetch      after a video starts: get_trickplay_info → VideoState.trickplay (only while
//                    the same playback still runs) + AppState.trickplay-aspect
//   request_thumb    a position to show (seek-bar hover, D-pad seek target) → crop off-thread →
//                    AppState.trickplay-thumb/-has-thumb; only the newest request lands. Nothing
//                    happens for an item without trickplay images (the UI stays as it was)
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use fjord_api::{JellyfinClient, models::TrickplayInfo};
use slint::Global;
use tracing::{debug, info, warn};

use crate::playback::VideoState;
use crate::{AppState, MainWindow};

/// Preferred thumbnail width in px (Jellyfin's default rendition is 320).
const TARGET_WIDTH: u32 = 320;
/// Decoded tiles kept per item.
const DECODED_TILES: usize = 2;

/// The rendition to use: the media source whose id is the item's own (the default version),
/// else the first by id; then the usable width closest to TARGET_WIDTH (the smaller on a tie).
pub(crate) fn pick_rendition(
    item_id: &str,
    map: &HashMap<String, HashMap<String, TrickplayInfo>>,
) -> Option<(String, u32, TrickplayInfo)> {
    let msid = if map.contains_key(item_id) {
        item_id.to_string()
    } else {
        let mut keys: Vec<&String> = map.keys().collect();
        keys.sort();
        keys.first()?.to_string()
    };
    map.get(&msid)?
        .iter()
        .filter_map(|(w, info)| w.parse::<u32>().ok().map(|w| (w, *info)))
        .filter(|(_, info)| locate(info, 0).is_some())
        .min_by_key(|(w, _)| (w.abs_diff(TARGET_WIDTH), *w))
        .map(|(w, info)| (msid, w, info))
}

/// Where one thumbnail is: its index, the tile it's in, and its rect in that tile (px).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Cell {
    pub index: u32,
    pub tile: u32,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Thumbnail for `pos_ms`: index = pos / interval (capped at the last), tile = index / cells per
/// tile, cells row-major. None for unusable info (any zero size/count/interval).
pub(crate) fn locate(info: &TrickplayInfo, pos_ms: u64) -> Option<Cell> {
    if info.interval == 0
        || info.thumbnail_count == 0
        || info.tile_width == 0
        || info.tile_height == 0
        || info.width == 0
        || info.height == 0
    {
        return None;
    }
    let index = (pos_ms / info.interval as u64).min(info.thumbnail_count as u64 - 1) as u32;
    let per_tile = info.tile_width * info.tile_height;
    let cell = index % per_tile;
    Some(Cell {
        index,
        tile: index / per_tile,
        x: (cell % info.tile_width) * info.width,
        y: (cell / info.tile_width) * info.height,
        w: info.width,
        h: info.height,
    })
}

/// Trickplay for the playing item (see the header).
pub(crate) struct Session {
    item_id: String,
    media_source_id: String,
    width: u32,
    info: TrickplayInfo,
    client: Arc<JellyfinClient>,
    tiles: tokio::sync::Mutex<TileCache>,
    /// Bumped by every request; a result for an older one is dropped.
    latest: AtomicU64,
    /// The thumbnail index last requested (a repeat of it does nothing).
    requested: Mutex<Option<u32>>,
}

#[derive(Default)]
struct TileCache {
    encoded: HashMap<u32, Arc<Vec<u8>>>,
    decoded: VecDeque<(u32, Arc<image::RgbaImage>)>,
}

impl Session {
    /// The thumbnail for `cell`: from a decoded tile, else decoding the cached JPEG, else
    /// fetching it. None when superseded before any work, or on a fetch/decode failure.
    async fn thumb(
        &self,
        cell: Cell,
        generation: u64,
    ) -> Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>> {
        let mut cache = self.tiles.lock().await;
        if self.latest.load(Ordering::Relaxed) != generation {
            return None; // the pointer moved on while this waited
        }
        let img = match cache.decoded.iter().find(|(t, _)| *t == cell.tile) {
            Some((_, img)) => Arc::clone(img),
            None => {
                let bytes = match cache.encoded.get(&cell.tile) {
                    Some(b) => Arc::clone(b),
                    None => {
                        let b = match self
                            .client
                            .get_trickplay_tile(
                                &self.item_id,
                                self.width,
                                cell.tile,
                                &self.media_source_id,
                            )
                            .await
                        {
                            Ok(b) => Arc::new(b),
                            Err(e) => {
                                warn!("trickplay tile {} of {}: {e:#}", cell.tile, self.item_id);
                                return None;
                            }
                        };
                        cache.encoded.insert(cell.tile, Arc::clone(&b));
                        b
                    }
                };
                let img = match image::load_from_memory(&bytes) {
                    Ok(i) => Arc::new(i.to_rgba8()),
                    Err(e) => {
                        warn!(
                            "trickplay tile {} of {} didn't decode: {e}",
                            cell.tile, self.item_id
                        );
                        return None;
                    }
                };
                debug!(
                    "trickplay: tile {} of {} decoded ({}x{})",
                    cell.tile,
                    self.item_id,
                    img.width(),
                    img.height()
                );
                cache.decoded.push_back((cell.tile, Arc::clone(&img)));
                while cache.decoded.len() > DECODED_TILES {
                    cache.decoded.pop_front();
                }
                img
            }
        };
        drop(cache);
        if cell.x + cell.w > img.width() || cell.y + cell.h > img.height() {
            debug!(
                "trickplay: cell {:?} outside tile {}x{}",
                cell,
                img.width(),
                img.height()
            );
            return None;
        }
        let sub =
            image::imageops::crop_imm(img.as_ref(), cell.x, cell.y, cell.w, cell.h).to_image();
        Some(slint::SharedPixelBuffer::clone_from_slice(
            sub.as_raw(),
            cell.w,
            cell.h,
        ))
    }
}

/// After a video starts: fetch its trickplay info and, if it has any, attach a Session to
/// VideoState (only while `generation` still plays `item_id`).
pub(crate) fn spawn_fetch(
    client: Arc<JellyfinClient>,
    item_id: String,
    generation: u64,
    video: &Arc<Mutex<VideoState>>,
    ww: &slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
) {
    let video = Arc::clone(video);
    let ww = ww.clone();
    rt.spawn(async move {
        let map = match client.get_trickplay_info(&item_id).await {
            Ok(m) => m,
            Err(e) => {
                debug!("trickplay info for {item_id}: {e:#}");
                return;
            }
        };
        let Some((media_source_id, width, info)) = pick_rendition(&item_id, &map) else {
            debug!("trickplay: none for {item_id}");
            return;
        };
        info!(
            "trickplay: {item_id} has {} thumbnail(s) of {}x{}, one every {} ms, {}x{} per tile",
            info.thumbnail_count,
            info.width,
            info.height,
            info.interval,
            info.tile_width,
            info.tile_height
        );
        let session = Arc::new(Session {
            item_id: item_id.clone(),
            media_source_id,
            width,
            info,
            client,
            tiles: tokio::sync::Mutex::new(TileCache::default()),
            latest: AtomicU64::new(0),
            requested: Mutex::new(None),
        });
        {
            let mut vs = video.lock().unwrap();
            if vs.playback_generation != generation
                || vs.item_id.as_deref() != Some(item_id.as_str())
            {
                return;
            }
            vs.trickplay = Some(session);
        }
        let aspect = info.height as f32 / info.width as f32;
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_trickplay_aspect(aspect);
            }
        });
    });
}

/// Show the thumbnail for `pos_secs` (seek-bar hover, D-pad seek target). UI thread.
pub(crate) fn request_thumb(
    video: &Arc<Mutex<VideoState>>,
    ww: &slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    pos_secs: f64,
) {
    let Some(session) = video.lock().unwrap().trickplay.clone() else {
        return;
    };
    let Some(cell) = locate(&session.info, (pos_secs.max(0.0) * 1000.0) as u64) else {
        return;
    };
    {
        let mut requested = session.requested.lock().unwrap();
        if *requested == Some(cell.index) {
            return;
        }
        *requested = Some(cell.index);
    }
    let generation = session.latest.fetch_add(1, Ordering::Relaxed) + 1;
    let ww = ww.clone();
    rt.spawn(async move {
        let thumb = session.thumb(cell, generation).await;
        if session.latest.load(Ordering::Relaxed) != generation {
            return;
        }
        let Some(buf) = thumb else {
            // Failed (not superseded): let the same position be asked again.
            *session.requested.lock().unwrap() = None;
            return;
        };
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_trickplay_thumb(slint::Image::from_rgba8(buf));
                g.set_trickplay_has_thumb(true);
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(w: u32, h: u32, tw: u32, th: u32, count: u32, interval: u32) -> TrickplayInfo {
        TrickplayInfo {
            width: w,
            height: h,
            tile_width: tw,
            tile_height: th,
            thumbnail_count: count,
            interval,
            bandwidth: 0,
        }
    }

    #[test]
    fn locate_cells_and_tiles() {
        // 320x180 thumbnails, 10x10 per tile, one every 10 s, 250 in all (3 tiles).
        let i = info(320, 180, 10, 10, 250, 10_000);
        let c = |ms| locate(&i, ms).unwrap();
        assert_eq!(
            c(0),
            Cell {
                index: 0,
                tile: 0,
                x: 0,
                y: 0,
                w: 320,
                h: 180
            }
        );
        assert_eq!(c(9_999).index, 0);
        assert_eq!(
            c(10_000),
            Cell {
                index: 1,
                tile: 0,
                x: 320,
                y: 0,
                w: 320,
                h: 180
            }
        );
        // index 23 → row 2, column 3
        assert_eq!(
            c(230_000),
            Cell {
                index: 23,
                tile: 0,
                x: 960,
                y: 360,
                w: 320,
                h: 180
            }
        );
        // index 99 → the last cell of tile 0
        assert_eq!(
            c(990_000),
            Cell {
                index: 99,
                tile: 0,
                x: 2880,
                y: 1620,
                w: 320,
                h: 180
            }
        );
        // index 100 → first cell of tile 1
        assert_eq!(
            c(1_000_000),
            Cell {
                index: 100,
                tile: 1,
                x: 0,
                y: 0,
                w: 320,
                h: 180
            }
        );
        // past the end → the last thumbnail (249: tile 2, cell 49 → row 4, column 9)
        assert_eq!(
            c(99_999_999),
            Cell {
                index: 249,
                tile: 2,
                x: 2880,
                y: 720,
                w: 320,
                h: 180
            }
        );
    }

    #[test]
    fn locate_rejects_unusable_info() {
        assert!(locate(&info(320, 180, 10, 10, 0, 10_000), 0).is_none());
        assert!(locate(&info(320, 180, 10, 10, 5, 0), 0).is_none());
        assert!(locate(&info(320, 180, 0, 10, 5, 10_000), 0).is_none());
        assert!(locate(&info(0, 180, 10, 10, 5, 10_000), 0).is_none());
    }

    #[test]
    fn pick_rendition_prefers_own_source_and_closest_width() {
        let mut widths = HashMap::new();
        widths.insert("240".to_string(), info(240, 135, 10, 10, 5, 10_000));
        widths.insert("480".to_string(), info(480, 270, 10, 10, 5, 10_000));
        widths.insert("320".to_string(), info(320, 180, 10, 10, 5, 10_000));
        let mut other = HashMap::new();
        other.insert("320".to_string(), info(320, 180, 10, 10, 9, 10_000));
        let mut map = HashMap::new();
        map.insert("item".to_string(), widths);
        map.insert("aaa-other-version".to_string(), other);
        let (ms, w, i) = pick_rendition("item", &map).unwrap();
        assert_eq!((ms.as_str(), w, i.thumbnail_count), ("item", 320, 5));

        // Without 320: the closer one (240 vs 480 → 240 is 80 away, 480 is 160).
        map.get_mut("item").unwrap().remove("320");
        assert_eq!(pick_rendition("item", &map).unwrap().1, 240);

        // The item's own source missing: the first other one.
        map.remove("item");
        assert_eq!(pick_rendition("item", &map).unwrap().0, "aaa-other-version");

        // Unusable renditions are skipped; nothing usable → None.
        let mut bad = HashMap::new();
        bad.insert("320".to_string(), info(320, 180, 10, 10, 0, 10_000));
        let mut map = HashMap::new();
        map.insert("item".to_string(), bad);
        assert!(pick_rendition("item", &map).is_none());
        assert!(pick_rendition("item", &HashMap::new()).is_none());
    }
}
