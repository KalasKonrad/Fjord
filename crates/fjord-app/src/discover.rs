// ── fjord-app · discover.rs ──────────────────────────────────────────────────
//   wire_discover              registers all Discover/RequestDetail AppState callbacks
//                              (search append/backspace/clear, open-discover-item,
//                              request-detail-toggle-season, request-detail-request)
//   spawn_discover_search      debounced (300ms) + generation-guarded search dispatch;
//                              text-only cards pushed immediately, posters patched in
//                              as they arrive (bounded concurrency, TMDB CDN, own disk cache)
//   ensure_discover_landing    fetches the 5 no-query landing rows (Trending/Popular
//                              Movies/Popular TV/Upcoming Movies/Upcoming TV) once per
//                              session (FjordState.discover_landing_fetched guard), on
//                              first nav arrival at Discover; same text-first-then-posters
//                              two-phase commit as search
//   landing_row_get/_set/_lens  AppState accessors for the 5 fixed landing-row lists,
//                              indexed 0-4, shared by the fetch and by handle_key's landing branch
//   open_discover_item         fetch movie/tv detail + poster + backdrop, gen-guarded
//                              (rapid re-open), populates RequestDetailScreen
//   submit_request              POST /request; on success flips local status (both the
//                              detail screen and the originating Discover card) + toasts
//   is_401 / handle_seerr_error 401 (session-auth only) resets the connection via
//                              seerr_auth::clear_connection + toasts "reconnect in
//                              Settings"; any other error just toasts
//   handle_key                  Discover screen: replicates dispatch_dashboard's
//                              focused-section sidebar/content contract itself (Discover has
//                              its own AppMode, not Dashboard, so doesn't get this for free) —
//                              fs<0 = sidebar (Up/Down cycle tabs, Right enters); fs>=0 = grid,
//                              2D nav mirrors LibraryGrid's math (AppState.library-cols),
//                              Left-at-col-0/Back return to the sidebar. Search-field typing is
//                              a separate raw-key pre-dispatch in keys.rs (handle_discover_search),
//                              mirroring browse.rs's handle_browse_search
//   handle_key_request_detail  back/button row -> season checklist -> Request button
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fjord_seerr::{MediaStatus, MovieDetails, SearchResult, SeasonsSelector, TvDetails};
use slint::{ComponentHandle, Global, Model, ModelRc, VecModel, Weak};

use tracing::{debug, warn};

use crate::config::{discover_poster_cache_path, FjordState};
use crate::keys::Action;
use crate::poster::decode_poster_buffer;
use crate::{show_toast, AppState, CardItem, MainWindow, SeasonItem};

const TMDB_POSTER_BASE: &str = "https://image.tmdb.org/t/p/w500";
const TMDB_BACKDROP_BASE: &str = "https://image.tmdb.org/t/p/w1280";

fn availability_tag(status: Option<MediaStatus>) -> &'static str {
    match status {
        Some(MediaStatus::Pending) => "requested",
        Some(MediaStatus::Processing) => "processing",
        Some(MediaStatus::PartiallyAvailable) => "partial",
        Some(MediaStatus::Available) => "available",
        Some(MediaStatus::Unknown) | Some(MediaStatus::Deleted) | None => "",
    }
}

/// Plain Send-able card metadata, built off-thread. `CardItem` itself always
/// carries a `slint::Image` field (even when it's the default/empty value —
/// `Send` is a type-level property, not a runtime one), so it can never cross
/// a thread boundary; every `CardItem` here is constructed fresh on the UI
/// thread, inside an `invoke_from_event_loop` closure, from one of these.
struct DiscoverCardMeta {
    id: String,
    item_type: &'static str,
    title: String,
    subtitle: String,
    year: i32,
    availability: &'static str,
}

impl DiscoverCardMeta {
    fn into_card_item(self) -> CardItem {
        CardItem {
            id: self.id.as_str().into(),
            item_type: self.item_type.into(),
            title: self.title.as_str().into(),
            subtitle: self.subtitle.as_str().into(),
            year: self.year,
            availability: self.availability.into(),
            ..Default::default()
        }
    }
}

fn search_result_to_meta(r: &SearchResult) -> Option<DiscoverCardMeta> {
    if r.media_type != "movie" && r.media_type != "tv" {
        return None; // person results filtered out — v1 shows movies/TV only
    }
    Some(DiscoverCardMeta {
        id: r.id.to_string(),
        item_type: if r.media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" },
        title: r.display_title().to_string(),
        subtitle: r.year().unwrap_or("").to_string(),
        year: r.year().and_then(|y| y.parse().ok()).unwrap_or(0),
        availability: availability_tag(r.media_info.as_ref().and_then(|mi| mi.status())),
    })
}

fn is_401(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|re| re.status())
        .map(|s| s == reqwest::StatusCode::UNAUTHORIZED)
        .unwrap_or(false)
}

/// Session-auth 401 means the cookie expired server-side — reset the
/// connection so Settings shows "Not connected" and the user can reconnect,
/// rather than every subsequent call failing silently. API-key auth doesn't
/// expire, so a 401 there means a revoked/invalid key — surfaced as a plain
/// error instead (reconnecting wouldn't help without a new key anyway).
fn handle_seerr_error(
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    is_session_auth: bool,
    context: &str,
    e: &anyhow::Error,
) {
    if is_session_auth && is_401(e) {
        warn!("seerr: {context}: session expired (401) — resetting connection: {e:#}");
        crate::seerr_auth::clear_connection(state, ww);
        show_toast(ww.clone(), "Seerr session expired — reconnect in Settings".into());
    } else {
        warn!("seerr: {context}: {e:#}");
        show_toast(ww.clone(), format!("{context}: {e}"));
    }
}

async fn fetch_tmdb_image(http: &reqwest::Client, base: &str, path: &str, cache_key: &str) -> Option<Vec<u8>> {
    let cache_path = discover_poster_cache_path(cache_key);
    if let Ok(bytes) = tokio::fs::read(&cache_path).await {
        return Some(bytes);
    }
    let url = format!("{base}{path}");
    let bytes = http.get(&url).send().await.ok()?.error_for_status().ok()?.bytes().await.ok()?.to_vec();
    if let Some(parent) = cache_path.parent() {
        let _ = tokio::fs::create_dir_all(parent).await;
    }
    let _ = tokio::fs::write(&cache_path, &bytes).await;
    Some(bytes)
}

// ── Search ───────────────────────────────────────────────────────────────────

pub(crate) fn spawn_discover_search(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    query: String,
    gen: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;

    if query.trim().is_empty() {
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_discover_results(ModelRc::new(VecModel::from(Vec::<CardItem>::new())));
                g.set_discover_searching(false);
            }
        });
        return;
    }

    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        // Silent no-op here used to look identical to "found nothing" from
        // the user's side — no error, no spinner, no log line. Real bug,
        // found live: a search typed while (for whatever reason)
        // `seerr_client` was `None` produced literally no feedback at all.
        warn!("seerr: search dispatched with no seerr_client set — not connected?");
        show_toast(ww, "Not connected to Seerr — check Settings → Integrations".into());
        return;
    };
    let is_session_auth = client.is_session_auth();

    rt.spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        if gen.load(Ordering::SeqCst) != my_gen {
            return; // superseded by a newer keystroke before the debounce elapsed
        }
        {
            let ww2 = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww2.upgrade() { AppState::get(&w).set_discover_searching(true); }
            });
        }

        debug!("seerr: searching for {query:?}");
        let response = match client.search(&query, 1).await {
            Ok(r) => r,
            Err(e) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() { AppState::get(&w).set_discover_searching(false); }
                });
                handle_seerr_error(&state, &ww, is_session_auth, "Seerr search failed", &e);
                return;
            }
        };
        if gen.load(Ordering::SeqCst) != my_gen {
            return; // a newer search already superseded this response
        }

        let results = response.results;
        let metas: Vec<DiscoverCardMeta> = results.iter().filter_map(search_result_to_meta).collect();
        debug!("seerr: search {query:?} -> {} raw result(s), {} movie/tv card(s)", results.len(), metas.len());

        // Poster pass targets, derived before `metas` is consumed below: both
        // iterators apply the identical movie/tv filter in the same order, so
        // zipping them keeps (row index, poster_path) in sync with `metas`.
        let poster_jobs: Vec<(usize, String, String, String)> = metas
            .iter()
            .enumerate()
            .zip(results.iter().filter(|r| r.media_type == "movie" || r.media_type == "tv"))
            .filter_map(|((i, meta), r)| r.poster_path.clone().map(|p| (i, meta.item_type.to_string(), meta.id.clone(), p)))
            .collect();

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_commit.upgrade() {
                let g = AppState::get(&w);
                let cards: Vec<CardItem> = metas.into_iter().map(DiscoverCardMeta::into_card_item).collect();
                g.set_discover_results(ModelRc::new(VecModel::from(cards)));
                g.set_discover_searching(false);
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
            }
        });

        if poster_jobs.is_empty() {
            return;
        }

        let Ok(http) = reqwest::Client::builder().timeout(Duration::from_secs(30)).build() else { return };
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut set = tokio::task::JoinSet::new();
        for (idx, item_type, tmdb_id, poster_path) in poster_jobs {
            let http = http.clone();
            let sem = Arc::clone(&sem);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let cache_key = format!("{}-{}", if item_type == "DiscoverMovie" { "movie" } else { "tv" }, tmdb_id);
                let bytes = fetch_tmdb_image(&http, TMDB_POSTER_BASE, &poster_path, &cache_key).await?;
                let buf = decode_poster_buffer(&bytes)?;
                Some((idx, item_type, tmdb_id, buf))
            });
        }
        while let Some(res) = set.join_next().await {
            let Ok(Some((idx, item_type, tmdb_id, buf))) = res else { continue };
            if gen.load(Ordering::SeqCst) != my_gen {
                break; // a newer search superseded this one — stop patching stale rows
            }
            let ww2 = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww2.upgrade() else { return };
                let g = AppState::get(&w);
                let model = g.get_discover_results();
                let Some(mut card) = model.row_data(idx) else { return };
                // Defensive: confirm the row at this index is still the same
                // item before patching (belt-and-braces alongside the gen
                // check above, matching the id-match guard used elsewhere in
                // this codebase for in-place model patches).
                if card.id.as_str() != tmdb_id || card.item_type.as_str() != item_type {
                    return;
                }
                card.poster = slint::Image::from_rgba8(buf);
                card.has_poster = true;
                model.set_row_data(idx, card);
            });
        }
    });
}

// ── Landing rows (Trending / Popular / Upcoming, shown when query == "") ───

fn landing_row_get(g: &AppState, idx: usize) -> ModelRc<CardItem> {
    match idx {
        0 => g.get_discover_trending(),
        1 => g.get_discover_popular_movies(),
        2 => g.get_discover_popular_tv(),
        3 => g.get_discover_upcoming_movies(),
        _ => g.get_discover_upcoming_tv(),
    }
}

fn landing_row_set(g: &AppState, idx: usize, model: ModelRc<CardItem>) {
    match idx {
        0 => g.set_discover_trending(model),
        1 => g.set_discover_popular_movies(model),
        2 => g.set_discover_popular_tv(model),
        3 => g.set_discover_upcoming_movies(model),
        _ => g.set_discover_upcoming_tv(model),
    }
}

fn landing_row_lens(g: &AppState) -> [i32; 5] {
    std::array::from_fn(|i| landing_row_get(g, i).row_count() as i32)
}

/// Fetches all 5 landing rows in parallel, once per session (guarded by
/// `FjordState.discover_landing_fetched`, reset on disconnect/reconnect/
/// sign-out since a different server means a different catalog). Same
/// two-phase commit as `spawn_discover_search`: text-only cards land first,
/// posters patch in as they arrive.
pub(crate) fn ensure_discover_landing(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let client = {
        let mut s = state.lock().unwrap();
        if s.discover_landing_fetched {
            return;
        }
        let Some(client) = s.seerr_client.clone() else { return };
        s.discover_landing_fetched = true;
        client
    };
    let is_session_auth = client.is_session_auth();

    rt.spawn(async move {
        let (r_trending, r_movies, r_tv, r_movies_up, r_tv_up) = tokio::join!(
            client.discover_trending(1),
            client.discover_movies(1),
            client.discover_tv(1),
            client.discover_movies_upcoming(1),
            client.discover_tv_upcoming(1),
        );
        let responses = [r_trending, r_movies, r_tv, r_movies_up, r_tv_up];
        const ROW_NAMES: [&str; 5] =
            ["trending", "popular movies", "popular tv", "upcoming movies", "upcoming tv"];

        let mut metas_per_row: Vec<Vec<DiscoverCardMeta>> = Vec::with_capacity(5);
        // (row, idx-within-row, item_type, tmdb_id, poster_path)
        let mut poster_jobs: Vec<(usize, usize, String, String, String)> = Vec::new();
        let mut first_error: Option<anyhow::Error> = None;
        for (row, r) in responses.into_iter().enumerate() {
            match r {
                Ok(resp) => {
                    let metas: Vec<DiscoverCardMeta> = resp.results.iter().filter_map(search_result_to_meta).collect();
                    debug!("seerr: landing row {} ({}) -> {} card(s)", row, ROW_NAMES[row], metas.len());
                    let jobs: Vec<(usize, usize, String, String, String)> = metas
                        .iter()
                        .enumerate()
                        .zip(resp.results.iter().filter(|r| r.media_type == "movie" || r.media_type == "tv"))
                        .filter_map(|((idx, m), r)| {
                            r.poster_path.clone().map(|p| (row, idx, m.item_type.to_string(), m.id.clone(), p))
                        })
                        .collect();
                    poster_jobs.extend(jobs);
                    metas_per_row.push(metas);
                }
                Err(e) => {
                    warn!("seerr: landing row {} ({}) fetch failed: {e:#}", row, ROW_NAMES[row]);
                    first_error.get_or_insert(e);
                    metas_per_row.push(Vec::new());
                }
            }
        }

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_commit.upgrade() else { return };
            let g = AppState::get(&w);
            for (row, metas) in metas_per_row.into_iter().enumerate() {
                let cards: Vec<CardItem> = metas.into_iter().map(DiscoverCardMeta::into_card_item).collect();
                landing_row_set(&g, row, ModelRc::new(VecModel::from(cards)));
            }
        });

        if let Some(e) = first_error {
            // Best-effort: rows that succeeded still show. Only surface an
            // error/reset-on-401 if at least one row actually failed.
            handle_seerr_error(&state, &ww, is_session_auth, "Couldn't load Discover", &e);
        }

        if poster_jobs.is_empty() {
            return;
        }
        let Ok(http) = reqwest::Client::builder().timeout(Duration::from_secs(30)).build() else { return };
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut set = tokio::task::JoinSet::new();
        for (row, idx, item_type, tmdb_id, poster_path) in poster_jobs {
            let http = http.clone();
            let sem = Arc::clone(&sem);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let cache_key = format!("{}-{}", if item_type == "DiscoverMovie" { "movie" } else { "tv" }, tmdb_id);
                let bytes = fetch_tmdb_image(&http, TMDB_POSTER_BASE, &poster_path, &cache_key).await?;
                let buf = decode_poster_buffer(&bytes)?;
                Some((row, idx, item_type, tmdb_id, buf))
            });
        }
        while let Some(res) = set.join_next().await {
            let Ok(Some((row, idx, item_type, tmdb_id, buf))) = res else { continue };
            let ww2 = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww2.upgrade() else { return };
                let g = AppState::get(&w);
                let model = landing_row_get(&g, row);
                let Some(mut card) = model.row_data(idx) else { return };
                if card.id.as_str() != tmdb_id || card.item_type.as_str() != item_type {
                    return; // row reshuffled since the fetch started — skip rather than mispatch
                }
                card.poster = slint::Image::from_rgba8(buf);
                card.has_poster = true;
                model.set_row_data(idx, card);
            });
        }
    });
}

// ── Request detail ──────────────────────────────────────────────────────────

struct DetailFields {
    title: String,
    meta: String,
    overview: String,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    status: Option<MediaStatus>,
    seasons: Vec<SeasonItem>,
}

fn movie_fields(d: MovieDetails) -> DetailFields {
    let year = d.release_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let genres = d.genres.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", ");
    DetailFields {
        title: d.title,
        meta: if genres.is_empty() { year.to_string() } else { format!("{year} · {genres}") },
        overview: d.overview.unwrap_or_default(),
        poster_path: d.poster_path,
        backdrop_path: d.backdrop_path,
        status: d.media_info.and_then(|mi| mi.status()),
        seasons: Vec::new(),
    }
}

fn tv_fields(d: TvDetails) -> DetailFields {
    let year = d.first_air_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let genres = d.genres.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", ");
    let seasons = d
        .seasons
        .iter()
        .map(|s| SeasonItem {
            season_number: s.season_number as i32,
            name: (if s.name.is_empty() { format!("Season {}", s.season_number) } else { s.name.clone() }).into(),
            episode_count: s.episode_count as i32,
            selected: true, // default all-checked, per plan decision 2
        })
        .collect();
    DetailFields {
        title: d.name,
        meta: if genres.is_empty() { year.to_string() } else { format!("{year} · {genres}") },
        overview: d.overview.unwrap_or_default(),
        poster_path: d.poster_path,
        backdrop_path: d.backdrop_path,
        status: d.media_info.and_then(|mi| mi.status()),
        seasons,
    }
}

pub(crate) fn open_discover_item(
    media_type: String,
    tmdb_id_str: String,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else { return };
    let Some(client) = state.lock().unwrap().seerr_client.clone() else { return };
    let is_session_auth = client.is_session_auth();

    let gen = {
        let Some(w) = ww.upgrade() else { return };
        let g = AppState::get(&w);
        let next = g.get_request_detail_open_gen() + 1;
        g.set_request_detail_open_gen(next);
        // Reset immediately so a stale previous item's data doesn't flash
        // before the new fetch completes (same idiom as open_collection_screen).
        g.set_request_detail_media_type(media_type.as_str().into());
        g.set_request_detail_tmdb_id(tmdb_id as i32);
        g.set_request_detail_title("".into());
        g.set_request_detail_overview("".into());
        g.set_request_detail_meta("".into());
        g.set_request_detail_has_poster(false);
        g.set_request_detail_has_backdrop(false);
        g.set_request_detail_status("".into());
        g.set_request_detail_seasons(ModelRc::new(VecModel::from(Vec::<SeasonItem>::new())));
        g.set_request_detail_back_focused(true);
        g.set_request_detail_in_seasons(false);
        g.set_request_detail_focused_season(0);
        g.set_request_detail_btn_focused(0);
        g.set_request_detail_want_4k(false);
        g.set_show_request_detail(true);
        next
    };

    let media_type2 = media_type.clone();
    rt.spawn(async move {
        let fields = if media_type2 == "movie" {
            client.get_movie(tmdb_id).await.map(movie_fields)
        } else {
            client.get_tv(tmdb_id).await.map(tv_fields)
        };
        let fields = match fields {
            Ok(f) => f,
            Err(e) => {
                handle_seerr_error(&state, &ww, is_session_auth, "Couldn't load details", &e);
                return;
            }
        };

        let Ok(http) = reqwest::Client::builder().timeout(Duration::from_secs(30)).build() else { return };
        let cache_prefix = if media_type2 == "movie" { "movie" } else { "tv" };
        let poster_buf = if let Some(p) = &fields.poster_path {
            fetch_tmdb_image(&http, TMDB_POSTER_BASE, p, &format!("{cache_prefix}-{tmdb_id}"))
                .await
                .and_then(|b| decode_poster_buffer(&b))
        } else {
            None
        };
        let backdrop_buf = if let Some(p) = &fields.backdrop_path {
            fetch_tmdb_image(&http, TMDB_BACKDROP_BASE, p, &format!("{cache_prefix}-{tmdb_id}-bg"))
                .await
                .and_then(|b| crate::poster::decode_backdrop_buffer(&b))
        } else {
            None
        };

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_request_detail_open_gen() != gen {
                return; // superseded by a rapid re-open of a different item
            }
            g.set_request_detail_title(fields.title.as_str().into());
            g.set_request_detail_meta(fields.meta.as_str().into());
            g.set_request_detail_overview(fields.overview.as_str().into());
            g.set_request_detail_status(availability_tag(fields.status).into());
            g.set_request_detail_seasons(ModelRc::new(VecModel::from(fields.seasons)));
            if let Some(buf) = poster_buf {
                g.set_request_detail_poster(slint::Image::from_rgba8(buf));
                g.set_request_detail_has_poster(true);
            }
            if let Some(buf) = backdrop_buf {
                g.set_request_detail_backdrop(slint::Image::from_rgba8(buf));
                g.set_request_detail_has_backdrop(true);
            }
        });
    });
}

fn patch_discover_card_availability(g: &AppState, media_type: &str, tmdb_id: i64, availability: &str) {
    let item_type = if media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" };
    let id_str = tmdb_id.to_string();
    let model = g.get_discover_results();
    for i in 0..model.row_count() {
        if let Some(mut card) = model.row_data(i) {
            if card.id.as_str() == id_str && card.item_type.as_str() == item_type {
                card.availability = availability.into();
                model.set_row_data(i, card);
                break;
            }
        }
    }
}

pub(crate) fn submit_request(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    if g.get_request_detail_requesting() || g.get_request_detail_status().as_str() != "" {
        return;
    }
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    let media_type = g.get_request_detail_media_type().to_string();
    let tmdb_id = g.get_request_detail_tmdb_id() as i64;

    let seasons_selector = if media_type == "tv" {
        let model = g.get_request_detail_seasons();
        let total = model.row_count();
        let selected: Vec<u32> = (0..total)
            .filter_map(|i| model.row_data(i))
            .filter(|s| s.selected)
            .map(|s| s.season_number as u32)
            .collect();
        if selected.is_empty() {
            show_toast(ww.clone(), "Select at least one season to request".into());
            return;
        }
        Some(if selected.len() == total { SeasonsSelector::all() } else { SeasonsSelector::Numbers(selected) })
    } else {
        None
    };
    let is_4k = g.get_request_detail_want_4k();

    g.set_request_detail_requesting(true);
    drop(g);

    rt.spawn(async move {
        let result = client.create_request(&media_type, tmdb_id, seasons_selector, is_4k).await;
        match result {
            Ok(_) => {
                let ww2 = ww.clone();
                let mt = media_type.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        g.set_request_detail_requesting(false);
                        g.set_request_detail_status("requested".into());
                        patch_discover_card_availability(&g, &mt, tmdb_id, "requested");
                    }
                });
                show_toast(ww, "Requested".into());
            }
            Err(e) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() { AppState::get(&w).set_request_detail_requesting(false); }
                });
                handle_seerr_error(&state, &ww, is_session_auth, "Request failed", &e);
            }
        }
    });
}

// ── Wiring ───────────────────────────────────────────────────────────────────

pub(crate) fn wire_discover(window: &MainWindow, state: Arc<Mutex<FjordState>>, rt: tokio::runtime::Handle) {
    let g = AppState::get(window);
    let discover_gen = Arc::new(AtomicU64::new(0));

    // Landing rows: fetched once per session on first arrival at the
    // Discover tab. nav-selected fires from both the sidebar's mouse click
    // handler and browse::sidebar_nav's keyboard-cycle path, so this one
    // registration covers both entry points — previously unused/unwired
    // (Slint declared it, nothing listened), so this doesn't change
    // behavior for any other nav value.
    g.on_nav_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |nav| {
            if nav == 6 {
                ensure_discover_landing(Arc::clone(&state), ww.clone(), rt.clone());
            }
        }
    });

    g.on_discover_search_append({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |ch| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let mut q = g.get_discover_query().to_string();
            let was_landing = q.is_empty();
            q.push_str(ch.as_str());
            g.set_discover_query(q.as_str().into());
            if was_landing {
                // First character typed: the view switches from the 5
                // landing SectionRows to the flat results grid, which only
                // ever means "grid has focus" at focused-section == 0 —
                // reset it so a query typed while parked on a non-zero
                // landing row (reachable by clicking the search field
                // directly, bypassing the keyboard path that always funnels
                // through row 0 first) doesn't leave focused-section stuck
                // on a row index the grid view doesn't understand.
                g.set_focused_section(0);
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
            }
            spawn_discover_search(ww.clone(), Arc::clone(&state), q, Arc::clone(&gen), &rt);
        }
    });
    g.on_discover_search_backspace({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let mut q = g.get_discover_query().to_string();
            q.pop();
            g.set_discover_query(q.as_str().into());
            spawn_discover_search(ww.clone(), Arc::clone(&state), q, Arc::clone(&gen), &rt);
        }
    });
    g.on_discover_search_clear({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_discover_query("".into());
            g.set_discover_focused(0);
            g.set_discover_focused_row(0);
            spawn_discover_search(ww.clone(), Arc::clone(&state), String::new(), Arc::clone(&gen), &rt);
        }
    });

    g.on_open_discover_item({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |media_type, tmdb_id| {
            open_discover_item(media_type.to_string(), tmdb_id.to_string(), Arc::clone(&state), ww.clone(), rt.clone());
        }
    });

    g.on_request_detail_toggle_season({
        let ww = window.as_weak();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_request_detail_seasons();
            if let Some(mut s) = model.row_data(idx as usize) {
                s.selected = !s.selected;
                model.set_row_data(idx as usize, s);
            }
        }
    });

    g.on_request_detail_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || submit_request(Arc::clone(&state), ww.clone(), rt.clone())
    });
}

// ── Keyboard: Discover grid (search-field typing is a raw pre-dispatch in keys.rs) ──
//
// `focused_section` (`fs`, shared across every dashboard-tier tab) is the
// sidebar/content toggle: `< 0` = sidebar has focus, `>= 0` = the screen's own
// content does. Every other dashboard-tier tab gets this for free from
// `dispatch_dashboard`; Discover has its own `AppMode` (not `Dashboard`, since
// its content is a flat poster grid, not `dispatch_dashboard`'s SectionRow
// model), so it has to replicate the same contract itself — real bug, found
// live: without this, arriving at Discover with zero results (the state on
// every first visit, before a query is typed) had no keyboard path out at
// all, since the old code only ever handled `Action::Up` there.
pub(crate) fn handle_key(action: &Action, g: &AppState) -> bool {
    let fs = g.get_focused_section();
    let landing = g.get_discover_query().is_empty();

    if fs < 0 {
        // Sidebar-focused: Up/Down cycle sidebar tabs (matches
        // dispatch_dashboard's fs<0 branch exactly — Down does NOT enter the
        // grid, it moves to the next tab); Right enters Discover's own
        // content — the first non-empty landing row when there's no query,
        // the results grid when there is one, the search field if neither
        // has anything to focus yet.
        return match action {
            Action::Up => { crate::browse::sidebar_nav(g, -1); true }
            Action::Down => { crate::browse::sidebar_nav(g, 1); true }
            Action::Right => {
                if landing {
                    if let Some(first) = landing_row_lens(g).iter().position(|&n| n > 0) {
                        g.set_focused_section(first as i32);
                        g.set_discover_landing_card(0);
                    } else {
                        g.set_discover_header_focused(true);
                    }
                } else if g.get_discover_results().row_count() > 0 {
                    g.set_focused_section(0);
                    g.set_discover_focused(0);
                    g.set_discover_focused_row(0);
                } else {
                    g.set_discover_header_focused(true);
                }
                true
            }
            _ => false,
        };
    }

    if landing {
        return handle_key_landing(action, g, fs);
    }

    let count = g.get_discover_results().row_count() as i32;
    if count == 0 {
        return match action {
            Action::Up => { g.set_discover_header_focused(true); true }
            Action::Back | Action::Left => { g.set_focused_section(-1); true }
            _ => false,
        };
    }
    let cols = g.get_library_cols().max(1);
    match action {
        Action::Left => {
            let f = g.get_discover_focused();
            if f % cols > 0 {
                g.set_discover_focused(f - 1);
            } else if f > 0 {
                let nf = f - 1;
                g.set_discover_focused(nf);
                g.set_discover_focused_row(nf / cols);
            } else {
                g.set_focused_section(-1); // leftmost card, top row — back to sidebar
            }
            true
        }
        Action::Right => {
            let f = g.get_discover_focused();
            if f % cols < cols - 1 && f + 1 < count {
                g.set_discover_focused(f + 1);
            } else if f + 1 < count {
                let nf = f + 1;
                g.set_discover_focused(nf);
                g.set_discover_focused_row(nf / cols);
            }
            true
        }
        Action::Up => {
            let f = g.get_discover_focused();
            if f >= cols {
                let nf = f - cols;
                g.set_discover_focused(nf);
                g.set_discover_focused_row(nf / cols);
            } else {
                g.set_discover_header_focused(true);
            }
            true
        }
        Action::Down => {
            let f = g.get_discover_focused();
            if f + cols < count {
                let nf = f + cols;
                g.set_discover_focused(nf);
                g.set_discover_focused_row(nf / cols);
                true
            } else {
                false // at last row — let focus_bar_on_down handle it
            }
        }
        Action::Confirm => {
            let f = g.get_discover_focused();
            if f < count {
                if let Some(card) = g.get_discover_results().row_data(f as usize) {
                    let media_type = if card.item_type.as_str() == "DiscoverMovie" { "movie" } else { "tv" };
                    g.invoke_open_discover_item(media_type.into(), card.id);
                }
            }
            true
        }
        Action::Back => {
            g.set_focused_section(-1);
            true
        }
        _ => false,
    }
}

/// Landing-row nav (fs 0-4 selects the row; discover-landing-card is the
/// column within it). No column-tracking-per-row-for-scroll needed the way
/// the flat grid needs discover-focused-row — each row is its own
/// independently-scrolling SectionRow (kb-x, same as Home's), so only the
/// row index (fs) and the column within whichever row is focused matter.
fn handle_key_landing(action: &Action, g: &AppState, fs: i32) -> bool {
    let lens = landing_row_lens(g);
    let count = lens[fs as usize];
    match action {
        Action::Left => {
            let c = g.get_discover_landing_card();
            if c > 0 {
                g.set_discover_landing_card(c - 1);
            } else {
                g.set_focused_section(-1);
            }
            true
        }
        Action::Right => {
            let c = g.get_discover_landing_card();
            if c + 1 < count {
                g.set_discover_landing_card(c + 1);
            }
            true
        }
        Action::Up => {
            if fs > 0 {
                let nf = fs - 1;
                g.set_focused_section(nf);
                g.set_discover_landing_card(g.get_discover_landing_card().min((lens[nf as usize] - 1).max(0)));
            } else {
                g.set_discover_header_focused(true);
            }
            true
        }
        Action::Down => {
            if fs + 1 < 5 {
                let nf = fs + 1;
                g.set_focused_section(nf);
                g.set_discover_landing_card(g.get_discover_landing_card().min((lens[nf as usize] - 1).max(0)));
                true
            } else {
                false // last row — let focus_bar_on_down handle it
            }
        }
        Action::Confirm => {
            let c = g.get_discover_landing_card().max(0);
            if c < count {
                if let Some(card) = landing_row_get(g, fs as usize).row_data(c as usize) {
                    let media_type = if card.item_type.as_str() == "DiscoverMovie" { "movie" } else { "tv" };
                    g.invoke_open_discover_item(media_type.into(), card.id);
                }
            }
            true
        }
        Action::Back => {
            g.set_focused_section(-1);
            true
        }
        _ => false,
    }
}

// ── Keyboard: Request detail screen ─────────────────────────────────────────

pub(crate) fn handle_key_request_detail(action: &Action, g: &AppState) -> bool {
    if g.get_request_detail_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_request_detail(false);
                true
            }
            Action::Down => {
                g.set_request_detail_back_focused(false);
                true
            }
            Action::Up => false, // let focus_bar_on_up handle the mini-player bar
            _ => true,
        };
    }

    let has_seasons =
        g.get_request_detail_media_type().as_str() == "tv" && g.get_request_detail_seasons().row_count() > 0;

    if g.get_request_detail_in_seasons() {
        let count = g.get_request_detail_seasons().row_count() as i32;
        return match action {
            Action::Up => {
                let f = g.get_request_detail_focused_season();
                if f > 0 { g.set_request_detail_focused_season(f - 1); }
                else { g.set_request_detail_in_seasons(false); }
                true
            }
            Action::Down => {
                let f = g.get_request_detail_focused_season();
                if f + 1 < count { g.set_request_detail_focused_season(f + 1); }
                true
            }
            Action::Confirm => {
                g.invoke_request_detail_toggle_season(g.get_request_detail_focused_season());
                true
            }
            Action::Back => {
                g.set_request_detail_in_seasons(false);
                true
            }
            _ => true,
        };
    }

    // Button row: 0=Request, 1=4K toggle (mirrors Album/Artist's multi-button
    // row idiom — request-detail-btn-focused, Left/Right between the two).
    // The 4K toggle only exists while the item is still requestable; once
    // requested/available there's nothing left to toggle.
    let requestable = g.get_request_detail_status().as_str() == "";
    match action {
        Action::Up => {
            g.set_request_detail_back_focused(true);
            true
        }
        Action::Down if has_seasons => {
            g.set_request_detail_in_seasons(true);
            g.set_request_detail_focused_season(0);
            true
        }
        Action::Left => {
            if requestable && g.get_request_detail_btn_focused() > 0 {
                g.set_request_detail_btn_focused(0);
            }
            true
        }
        Action::Right => {
            if requestable && g.get_request_detail_btn_focused() < 1 {
                g.set_request_detail_btn_focused(1);
            }
            true
        }
        Action::Confirm => {
            if requestable {
                if g.get_request_detail_btn_focused() == 1 {
                    g.set_request_detail_want_4k(!g.get_request_detail_want_4k());
                } else {
                    g.invoke_request_detail_request();
                }
            }
            true
        }
        Action::Back => {
            g.set_show_request_detail(false);
            true
        }
        _ => false,
    }
}
