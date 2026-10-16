// ── fjord-app · hero.rs ──────────────────────────────────────────────────────
//   HeroText            what the dashboard hero says: title, meta line, rating label, overview
//   jellyfin_text       MediaItem → HeroText per type (episode: series title + S1:E3 • name;
//                       album: artist • year; collection/playlist: item count) (unit-tested)
//   discover_text       Seerr movie/TV details → HeroText (year, ★ vote, seasons) (unit-tested)
//   Art                 the hero's picture: a backdrop, or (Music) an album cover + its blur
//   load_jellyfin       text from item_detail_cache or get_item_detail (fills that cache, so the
//                       detail page opens instantly afterwards) + backdrop (an episode's: its
//                       series'; albums/playlists: the cover)
//   load_discover       Seerr get_movie/get_tv (hero_discover_cache) + the TMDB backdrop (the
//                       request page's disk cache key)
//   wire                hero-request: wait HERO_DELAY (fast scrolling only loads where it stops),
//                       newest request wins (GEN), fill the hidden layer (hero-a/-b), flip
//                       hero-front-a (crossfade); session-guarded
//   reset               both layers empty, in-flight loads dropped (reset_session_state)
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use fjord_api::{JellyfinClient, models::MediaItem};
use slint::{ComponentHandle, Global, SharedPixelBuffer};
use tracing::debug;

use crate::config::FjordState;
use crate::{AppState, HeroInfo, MainWindow};

/// Wait this long after the focus stops on a card before loading its hero.
const HERO_DELAY: Duration = Duration::from_millis(250);
/// Newest hero-request; a load whose number is no longer current is dropped.
static GEN: AtomicU64 = AtomicU64::new(0);

type Buf = SharedPixelBuffer<slint::Rgba8Pixel>;

#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct HeroText {
    pub title: String,
    pub meta: String,
    pub rating_label: String,
    pub overview: String,
}

enum Art {
    None,
    Backdrop(Buf),
    Cover { sharp: Buf, blurred: Buf },
}

fn rating_label(r: Option<f64>) -> String {
    r.filter(|r| *r > 0.0)
        .map(|r| format!("★ {r:.1}"))
        .unwrap_or_default()
}

fn overview(o: Option<&str>) -> String {
    crate::strip_html_to_text(o.unwrap_or_default().trim())
}

pub(crate) fn jellyfin_text(item: &MediaItem) -> HeroText {
    let mut meta: Vec<String> = Vec::new();
    let mut title = item.name.clone();
    match item.item_type.as_str() {
        "Episode" => {
            if let Some(series) = item.series_name.as_deref().filter(|s| !s.is_empty()) {
                title = series.to_string();
                if let (Some(s), Some(e)) = (item.parent_index_number, item.index_number) {
                    meta.push(format!("S{s}:E{e}"));
                }
                meta.push(item.name.clone());
            }
            meta.extend(item.runtime_string());
        }
        "MusicAlbum" => {
            meta.extend(item.album_artist.clone().filter(|a| !a.is_empty()));
            meta.extend(item.production_year.map(|y| y.to_string()));
        }
        "BoxSet" | "Playlist" => {
            meta.push(
                if item.item_type == "BoxSet" {
                    "Collection"
                } else {
                    "Playlist"
                }
                .to_string(),
            );
            if let Some(n) = item.child_count.filter(|n| *n > 0) {
                meta.push(crate::plural(n as u64, "item", "items"));
            }
        }
        _ => {
            meta.extend(item.production_year.map(|y| y.to_string()));
            meta.extend(item.official_rating.clone().filter(|r| !r.is_empty()));
            if item.item_type != "Series" {
                meta.extend(item.runtime_string());
            }
        }
    }
    HeroText {
        title,
        meta: meta.join(" • "),
        rating_label: rating_label(item.community_rating.map(f64::from)),
        overview: overview(item.overview.as_deref()),
    }
}

/// Seerr details → hero text. `seasons`: season numbers (specials = 0 aren't counted).
pub(crate) fn discover_text(
    title: &str,
    date: Option<&str>,
    vote: Option<f64>,
    seasons: Option<&[u32]>,
    ov: Option<&str>,
) -> HeroText {
    let mut meta: Vec<String> = Vec::new();
    meta.extend(date.and_then(|d| d.get(..4)).map(str::to_string));
    if let Some(n) = seasons
        .map(|s| s.iter().filter(|n| **n > 0).count())
        .filter(|n| *n > 0)
    {
        meta.push(crate::plural(n as u64, "season", "seasons"));
    }
    HeroText {
        title: title.to_string(),
        meta: meta.join(" • "),
        rating_label: rating_label(vote),
        overview: overview(ov),
    }
}

async fn load_jellyfin(
    client: &Arc<JellyfinClient>,
    state: &Arc<Mutex<FjordState>>,
    id: &str,
    series_id: &str,
) -> Option<(HeroText, Art)> {
    let cached = state.lock().unwrap().item_detail_cache.get(id);
    let item = match cached {
        Some(item) => item,
        None => match client.get_item_detail(id).await {
            Ok(item) => {
                if crate::session_current(state, client) {
                    state
                        .lock()
                        .unwrap()
                        .item_detail_cache
                        .insert(id.to_string(), item.clone());
                }
                item
            }
            Err(e) => {
                debug!("hero: couldn't load {id}: {e:#}");
                return None;
            }
        },
    };
    let text = jellyfin_text(&item);
    let art = match item.item_type.as_str() {
        "MusicAlbum" | "Playlist" | "Audio" => {
            // A track shows its album's cover.
            let cover_id = match item.item_type.as_str() {
                "Audio" => item.album_id.as_deref().unwrap_or(id),
                _ => id,
            };
            let bytes = crate::poster::fetch_poster_cached(client, cover_id).await;
            match bytes.as_deref().map(|b| {
                (
                    crate::poster::decode_poster_buffer(b),
                    crate::poster::decode_blur_buffer(b),
                )
            }) {
                Some((Some(sharp), Some(blurred))) => Art::Cover { sharp, blurred },
                _ => Art::None,
            }
        }
        _ => {
            let series = item.series_id.as_deref().unwrap_or(series_id);
            let bytes = if let Some(tag) = item.backdrop_image_tags.first() {
                crate::poster::fetch_backdrop_cached_tagged(client, id, Some(tag)).await
            } else if !series.is_empty() {
                crate::poster::fetch_backdrop_cached(client, series).await
            } else {
                None
            };
            match bytes
                .as_deref()
                .and_then(crate::poster::decode_backdrop_buffer)
            {
                Some(b) => Art::Backdrop(b),
                None => Art::None,
            }
        }
    };
    Some((text, art))
}

fn tmdb_http() -> &'static reqwest::Client {
    static HTTP: OnceLock<reqwest::Client> = OnceLock::new();
    HTTP.get_or_init(|| {
        reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_default()
    })
}

async fn load_discover(
    client: &Arc<fjord_seerr::SeerrClient>,
    state: &Arc<Mutex<FjordState>>,
    media: &str,
    id: &str,
) -> Option<(HeroText, Art)> {
    let tmdb_id: i64 = id.parse().ok()?;
    let key = format!("{media}-{tmdb_id}");
    let cached = state.lock().unwrap().hero_discover_cache.get(&key);
    let (text, backdrop_path) = match cached {
        Some(c) => c,
        None => {
            let fetched = if media == "movie" {
                client.get_movie(tmdb_id).await.map(|d| {
                    let t = discover_text(
                        &d.title,
                        d.release_date.as_deref(),
                        d.vote_average,
                        None,
                        d.overview.as_deref(),
                    );
                    (t, d.backdrop_path)
                })
            } else {
                client.get_tv(tmdb_id).await.map(|d| {
                    let seasons: Vec<u32> = d.seasons.iter().map(|s| s.season_number).collect();
                    let t = discover_text(
                        &d.name,
                        d.first_air_date.as_deref(),
                        d.vote_average,
                        Some(&seasons),
                        d.overview.as_deref(),
                    );
                    (t, d.backdrop_path)
                })
            };
            match fetched {
                Ok(v) => {
                    if crate::seerr_session_current(state, client) {
                        state
                            .lock()
                            .unwrap()
                            .hero_discover_cache
                            .insert(key.clone(), v.clone());
                    }
                    v
                }
                Err(e) => {
                    debug!("hero: couldn't load {key} from Seerr: {e:#}");
                    return None;
                }
            }
        }
    };
    let art = match &backdrop_path {
        Some(p) => crate::discover::fetch_tmdb_image(
            tmdb_http(),
            crate::discover::TMDB_BACKDROP_BASE,
            p,
            &format!("{key}-bg"),
        )
        .await
        .as_deref()
        .and_then(crate::poster::decode_backdrop_buffer)
        .map_or(Art::None, Art::Backdrop),
        None => Art::None,
    };
    Some((text, art))
}

fn to_info(id: &str, text: HeroText, art: Art) -> HeroInfo {
    let img = |b: Buf| slint::Image::from_rgba8(b);
    let (backdrop, cover) = match art {
        Art::None => (None, None),
        Art::Backdrop(b) => (Some(img(b)), None),
        Art::Cover { sharp, blurred } => (Some(img(blurred)), Some(img(sharp))),
    };
    HeroInfo {
        id: id.into(),
        title: text.title.into(),
        meta: text.meta.into(),
        rating_label: text.rating_label.into(),
        overview: text.overview.into(),
        has_backdrop: backdrop.is_some(),
        backdrop: backdrop.unwrap_or_default(),
        has_cover: cover.is_some(),
        cover: cover.unwrap_or_default(),
    }
}

pub(crate) fn reset(g: &AppState<'_>) {
    GEN.fetch_add(1, Ordering::SeqCst);
    g.set_hero_a(HeroInfo::default());
    g.set_hero_b(HeroInfo::default());
    g.set_hero_front_a(true);
}

pub(crate) fn wire(
    window: &MainWindow,
    state: &Arc<Mutex<FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    let state = Arc::clone(state);
    let ww = window.as_weak();
    let rt = rt.handle().clone();
    AppState::get(window).on_hero_request(move |id, item_type, series_id| {
        let generation = GEN.fetch_add(1, Ordering::SeqCst) + 1;
        if id.is_empty() {
            return;
        }
        let Some(w) = ww.upgrade() else { return };
        let g = AppState::get(&w);
        let front = if g.get_hero_front_a() {
            g.get_hero_a()
        } else {
            g.get_hero_b()
        };
        if front.id == id {
            return;
        }
        let (id, item_type, series_id) =
            (id.to_string(), item_type.to_string(), series_id.to_string());
        let (jellyfin, seerr) = {
            let s = state.lock().unwrap();
            (s.client.clone(), s.seerr_client.clone())
        };
        let state = Arc::clone(&state);
        let ww = ww.clone();
        rt.spawn(async move {
            tokio::time::sleep(HERO_DELAY).await;
            if GEN.load(Ordering::SeqCst) != generation {
                return;
            }
            let started = std::time::Instant::now();
            let discover_media = match item_type.as_str() {
                "DiscoverMovie" => Some("movie"),
                "DiscoverTv" => Some("tv"),
                _ => None,
            };
            let loaded = match (discover_media, &jellyfin, &seerr) {
                (Some(media), _, Some(c)) => load_discover(c, &state, media, &id).await,
                (None, Some(c), _) => load_jellyfin(c, &state, &id, &series_id).await,
                _ => None,
            };
            let Some((text, art)) = loaded else { return };
            debug!(
                "hero: {item_type} {id} loaded in {} ms (picture: {})",
                started.elapsed().as_millis(),
                match art {
                    Art::None => "none",
                    Art::Backdrop(_) => "backdrop",
                    Art::Cover { .. } => "cover",
                }
            );
            let _ = slint::invoke_from_event_loop(move || {
                // Newest request only, and never across a profile switch / sign-out.
                if GEN.load(Ordering::SeqCst) != generation {
                    return;
                }
                let still_current = match (discover_media, &jellyfin, &seerr) {
                    (Some(_), _, Some(c)) => crate::seerr_session_current(&state, c),
                    (None, Some(c), _) => crate::session_current(&state, c),
                    _ => false,
                };
                if !still_current {
                    return;
                }
                let Some(w) = ww.upgrade() else { return };
                let g = AppState::get(&w);
                let info = to_info(&id, text, art);
                if g.get_hero_front_a() {
                    g.set_hero_b(info);
                } else {
                    g.set_hero_a(info);
                }
                g.set_hero_front_a(!g.get_hero_front_a());
            });
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(json: &str) -> MediaItem {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn jellyfin_text_per_type() {
        let movie = item(
            r#"{"Id":"a","Name":"Movie A","Type":"Movie","ProductionYear":2024,
               "OfficialRating":"PG-13","RunTimeTicks":99600000000,"CommunityRating":8.62,
               "Overview":"<p>Story</p>"}"#,
        );
        assert_eq!(
            jellyfin_text(&movie),
            HeroText {
                title: "Movie A".into(),
                meta: "2024 • PG-13 • 2h 46m".into(),
                rating_label: "★ 8.6".into(),
                overview: "Story".into(),
            }
        );
        let episode = item(
            r#"{"Id":"b","Name":"Pilot","Type":"Episode","SeriesName":"Show B",
               "ParentIndexNumber":1,"IndexNumber":3,"RunTimeTicks":27000000000}"#,
        );
        let t = jellyfin_text(&episode);
        assert_eq!(
            (t.title.as_str(), t.meta.as_str()),
            ("Show B", "S1:E3 • Pilot • 45m")
        );
        assert_eq!(t.rating_label, "");
        let series = item(r#"{"Id":"c","Name":"Show C","Type":"Series","ProductionYear":2019}"#);
        assert_eq!(jellyfin_text(&series).meta, "2019");
        let album = item(
            r#"{"Id":"d","Name":"Album D","Type":"MusicAlbum","AlbumArtist":"Artist",
               "ProductionYear":1999}"#,
        );
        assert_eq!(jellyfin_text(&album).meta, "Artist • 1999");
        let boxset = item(r#"{"Id":"e","Name":"Set","Type":"BoxSet","ChildCount":1}"#);
        assert_eq!(jellyfin_text(&boxset).meta, "Collection • 1 item");
    }

    #[test]
    fn discover_text_counts_real_seasons() {
        let t = discover_text(
            "Show",
            Some("2021-03-04"),
            Some(7.44),
            Some(&[0, 1, 2]),
            None,
        );
        assert_eq!(
            (t.meta.as_str(), t.rating_label.as_str()),
            ("2021 • 2 seasons", "★ 7.4")
        );
        let t = discover_text("Movie", None, Some(0.0), None, Some("Plot"));
        assert_eq!((t.meta.as_str(), t.rating_label.as_str()), ("", ""));
        assert_eq!(t.overview, "Plot");
    }
}
