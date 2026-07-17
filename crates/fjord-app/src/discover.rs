// ── fjord-app · discover.rs ──────────────────────────────────────────────────
//   wire_discover              registers all Discover/RequestDetail AppState callbacks
//                              (search append/backspace/clear, open-discover-item,
//                              request-detail-toggle-season/-tag, request-detail-request,
//                              open-request-options, request-detail-set-quality); on first
//                              nav arrival also proactively refreshes all_movies (metadata
//                              only, crate::spawn_movies_list_fetch(..., with_posters=false))
//                              so find_local_item's ProviderIds match works on the first
//                              Discover visit, not just after the Movies grid has been
//                              opened this session (all_series has no such gap — the
//                              startup auto-login path already refreshes it unconditionally)
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
//   find_local_item             matches a Seerr/TMDB result to the local library by
//                              ProviderIds["Tmdb"] — no server-side Jellyfin lookup exists,
//                              so this scans the already-cached all_movies/all_series
//   open_discover_item         find_local_item hit -> detail::open_detail (the real
//                              Jellyfin item) instead of the Seerr flow below; else
//                              fetch movie/tv detail + poster + backdrop + available tags/
//                              quality profiles for BOTH quality tiers (best-effort, silently
//                              empty on failure — see available_request_options_both_tiers) in
//                              parallel, then cast/crew portraits + season posters (TMDB,
//                              bounded concurrency, same JoinSet+Semaphore shape as detail.rs's
//                              Jellyfin cast fetch); gen-guarded, populates RequestDetailScreen.
//                              Profile row 0 is always a synthetic "Default" entry (id 0)
//                              prepended so the picker has an explicit "no explicit choice"
//                              option, not just whatever's focused first.
//   build_cast_list/format_rating  Seerr credits -> capped cast+crew rows (2 Director/
//                              3 Writer/12 top-billed cast, same shape as detail.rs's
//                              Jellyfin cast) / TMDB voteAverage -> "★ 7.9" badge text
//   build_tag_profile_items    one quality tier's raw Seerr tags/profiles -> Slint TagItem/
//                              ProfileItem models; shared by both tiers in open_discover_item
//   set_quality                swaps in the other tier's pre-fetched tags/profiles/selected-
//                              profile-id when Quality actually changes — instant, no
//                              re-fetch/race, since both tiers were fetched up front; shared
//                              by the keyboard handler and the request-detail-set-quality
//                              callback the 2K/4K buttons' mouse clicks go through
//   submit_request              POST /request (seasons + is4k + selected tag ids + selected
//                              profileId, 0/Default omitted); on success flips local status
//                              (both the detail screen and the originating Discover card) + toasts
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
//   existing_zones/handle_key_request_detail  back -> button row (Request) -> storyline
//                              (collapsible overview) -> cast row — Up/Down step to the
//                              nearest zone that exists for this item. Request button opens
//                              the Request Options modal rather than exposing 4K/tags/seasons
//                              inline (see below).
//   existing_option_zones/handle_key_request_options  Request Options modal: Quality (2K/4K)
//                              row -> profile row (radio-select) -> tags row -> seasons row ->
//                              confirm row (Cancel/Request), same skip-absent-zones idiom as
//                              existing_zones but its own numbering
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
use crate::{
    show_toast, spawn_movies_list_fetch, AppState, CardItem, CastMember, MainWindow, ProfileItem, SeasonItem, TagItem,
};

const TMDB_POSTER_BASE: &str = "https://image.tmdb.org/t/p/w500";
const TMDB_BACKDROP_BASE: &str = "https://image.tmdb.org/t/p/w1280";

fn availability_tag(status: Option<MediaStatus>) -> &'static str {
    match status {
        Some(MediaStatus::Pending) => "requested",
        Some(MediaStatus::Processing) => "processing",
        Some(MediaStatus::PartiallyAvailable) => "partial",
        Some(MediaStatus::Available) => "available",
        Some(MediaStatus::Unknown) | Some(MediaStatus::Blocklisted) | Some(MediaStatus::Deleted) | None => "",
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
        4 => g.get_discover_upcoming_tv(),
        _ => g.get_discover_requested(),
    }
}

fn landing_row_set(g: &AppState, idx: usize, model: ModelRc<CardItem>) {
    match idx {
        0 => g.set_discover_trending(model),
        1 => g.set_discover_popular_movies(model),
        2 => g.set_discover_popular_tv(model),
        3 => g.set_discover_upcoming_movies(model),
        4 => g.set_discover_upcoming_tv(model),
        _ => g.set_discover_requested(model),
    }
}

fn landing_row_lens(g: &AppState) -> [i32; 6] {
    std::array::from_fn(|i| landing_row_get(g, i).row_count() as i32)
}

fn movie_details_to_meta(tmdb_id: i64, d: &MovieDetails, availability: &'static str) -> (DiscoverCardMeta, Option<String>) {
    let year = d.release_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let meta = DiscoverCardMeta {
        id: tmdb_id.to_string(),
        item_type: "DiscoverMovie",
        title: d.title.clone(),
        subtitle: year.to_string(),
        year: year.parse().unwrap_or(0),
        availability,
    };
    (meta, d.poster_path.clone())
}

fn tv_details_to_meta(tmdb_id: i64, d: &TvDetails, availability: &'static str) -> (DiscoverCardMeta, Option<String>) {
    let year = d.first_air_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let meta = DiscoverCardMeta {
        id: tmdb_id.to_string(),
        item_type: "DiscoverTv",
        title: d.name.clone(),
        subtitle: year.to_string(),
        year: year.parse().unwrap_or(0),
        availability,
    };
    (meta, d.poster_path.clone())
}

type RequestedRowItem = (DiscoverCardMeta, Option<String>);

/// Builds the Discover "Requested" landing row (still-pending/processing
/// requests). `GET /request` only carries a tmdbId per item — no title or
/// poster (confirmed from Seerr's real `MediaInfo` schema) — so each kept
/// request needs its own detail fetch, bounded concurrency, same shape as
/// the cast-portrait/season-poster fetches elsewhere in this app. Returns
/// `(meta, poster_path)` pairs so the caller can feed both the row's text
/// content and its poster-fetch jobs, mirroring the other 5 rows exactly.
/// Best-effort throughout: any failure just yields an empty/shorter row.
async fn fetch_requested_row(client: &fjord_seerr::SeerrClient) -> Vec<RequestedRowItem> {
    let (movies, tv) = match client.requested_not_available(15).await {
        Ok(v) => v,
        Err(e) => {
            debug!("seerr: couldn't fetch requested-not-available list: {e:#}");
            return Vec::new();
        }
    };
    // (media_type, tmdb_id, availability, created_at) — tagged so each
    // detail fetch knows which endpoint to call without re-deriving it.
    let mut entries: Vec<(&'static str, i64, &'static str, String)> = Vec::new();
    for r in &movies {
        let Some(media) = &r.media else { continue };
        let Some(tmdb_id) = media.tmdb_id else { continue };
        entries.push(("movie", tmdb_id, availability_tag(media.status()), r.created_at.clone().unwrap_or_default()));
    }
    for r in &tv {
        let Some(media) = &r.media else { continue };
        let Some(tmdb_id) = media.tmdb_id else { continue };
        entries.push(("tv", tmdb_id, availability_tag(media.status()), r.created_at.clone().unwrap_or_default()));
    }
    entries.sort_by(|a, b| b.3.cmp(&a.3)); // newest requested first
    entries.truncate(20);

    let n = entries.len();
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set: tokio::task::JoinSet<(usize, Option<RequestedRowItem>)> = tokio::task::JoinSet::new();
    for (idx, (media_type, tmdb_id, availability, _)) in entries.into_iter().enumerate() {
        let client = client.clone();
        let sem = Arc::clone(&sem);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let item = if media_type == "movie" {
                client.get_movie(tmdb_id).await.ok().map(|d| movie_details_to_meta(tmdb_id, &d, availability))
            } else {
                client.get_tv(tmdb_id).await.ok().map(|d| tv_details_to_meta(tmdb_id, &d, availability))
            };
            (idx, item)
        });
    }
    let mut out: Vec<Option<(DiscoverCardMeta, Option<String>)>> = (0..n).map(|_| None).collect();
    while let Some(res) = set.join_next().await {
        let Ok((idx, item)) = res else { continue };
        out[idx] = item;
    }
    out.into_iter().flatten().collect()
}

/// Fetches all 6 landing rows in parallel, once per session (guarded by
/// `FjordState.discover_landing_fetched`, reset on disconnect/reconnect/
/// sign-out since a different server means a different catalog). Same
/// two-phase commit as `spawn_discover_search`: text-only cards land first,
/// posters patch in as they arrive. Row 5 (Requested) is built differently
/// from rows 0-4 — see `fetch_requested_row`'s doc comment — but folds into
/// the same `metas_per_row`/`poster_jobs` shape immediately after, so the
/// rest of this function (commit + poster fetch) doesn't need to know rows
/// exist in two different shapes.
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
        let (r_trending, r_movies, r_tv, r_movies_up, r_tv_up, requested) = tokio::join!(
            client.discover_trending(1),
            client.discover_movies(1),
            client.discover_tv(1),
            client.discover_movies_upcoming(1),
            client.discover_tv_upcoming(1),
            fetch_requested_row(&client),
        );
        let responses = [r_trending, r_movies, r_tv, r_movies_up, r_tv_up];
        const ROW_NAMES: [&str; 6] =
            ["trending", "popular movies", "popular tv", "upcoming movies", "upcoming tv", "requested"];

        let mut metas_per_row: Vec<Vec<DiscoverCardMeta>> = Vec::with_capacity(6);
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

        // Row 5 (Requested) — resolved title/poster_path per item already,
        // via its own detail fetch, unlike rows 0-4 which get both straight
        // from /discover/*'s SearchResult.
        {
            let row = 5;
            debug!("seerr: landing row {} ({}) -> {} card(s)", row, ROW_NAMES[row], requested.len());
            let jobs: Vec<(usize, usize, String, String, String)> = requested
                .iter()
                .enumerate()
                .filter_map(|(idx, (m, poster_path))| {
                    poster_path.clone().map(|p| (row, idx, m.item_type.to_string(), m.id.clone(), p))
                })
                .collect();
            poster_jobs.extend(jobs);
            metas_per_row.push(requested.into_iter().map(|(m, _)| m).collect());
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

const TMDB_PROFILE_BASE: &str = "https://image.tmdb.org/t/p/w185";

/// One cast/crew row: (tmdb person id, name, role label, profile photo path).
/// Carried as a plain tuple rather than `CastMember` for the same !Send-image
/// reason as `DiscoverCardMeta` — built off-thread, turned into `CastMember`
/// only inside `invoke_from_event_loop`.
type CreditRow = (String, String, String, Option<String>);

/// Same director/writer/actor cap and precedence as `detail.rs`'s Jellyfin
/// cast (2 Directors, 3 Writers, then 12 top-billed actors by `order`),
/// deduped by id in case someone appears in more than one bucket (e.g. an
/// actor-director).
fn build_cast_list(credits: &Option<fjord_seerr::Credits>) -> Vec<CreditRow> {
    let Some(credits) = credits else { return Vec::new() };
    let mut seen_ids: std::collections::HashSet<i64> = Default::default();
    let mut out: Vec<CreditRow> = Vec::new();
    for c in credits.crew.iter().filter(|c| c.job.as_deref() == Some("Director")).take(2) {
        if seen_ids.insert(c.id) {
            out.push((c.id.to_string(), c.name.clone(), "Director".to_string(), c.profile_path.clone()));
        }
    }
    for c in credits
        .crew
        .iter()
        .filter(|c| matches!(c.job.as_deref(), Some("Writer") | Some("Screenplay")))
        .take(3)
    {
        if seen_ids.insert(c.id) {
            out.push((c.id.to_string(), c.name.clone(), "Writer".to_string(), c.profile_path.clone()));
        }
    }
    let mut cast_sorted: Vec<&fjord_seerr::Cast> = credits.cast.iter().collect();
    cast_sorted.sort_by_key(|c| c.order.unwrap_or(i64::MAX));
    for c in cast_sorted.into_iter().take(12) {
        if seen_ids.insert(c.id) {
            let role = c.character.clone().unwrap_or_default();
            out.push((c.id.to_string(), c.name.clone(), role, c.profile_path.clone()));
        }
    }
    out
}

/// `"★ 7.9"` for a real TMDB voteAverage, `""` (no badge) when absent or
/// zero (an unreleased/unvoted item reports 0.0, not a missing field) —
/// mirrors how `detail.rs` skips the badge when `community_rating` is `None`.
fn format_rating(vote_average: Option<f64>) -> String {
    match vote_average {
        Some(v) if v > 0.0 => format!("★ {v:.1}"),
        _ => String::new(),
    }
}

type TagProfileItems = (Vec<TagItem>, Vec<ProfileItem>);

/// Converts one quality tier's raw tags/profiles into the Slint-facing
/// models — shared by both tiers so `open_discover_item` doesn't duplicate
/// the mapping. Row 0 of `profiles` is always the synthetic "Default" entry
/// (id 0 — real Radarr/Sonarr profile ids start at 1) so the picker has an
/// explicit way to mean "don't send profileId at all," not just whatever
/// happens to be focused first; if nothing real is configured, the whole
/// list is cleared rather than showing just a lone Default entry.
fn build_tag_profile_items(options: (Vec<fjord_seerr::Tag>, Vec<fjord_seerr::Profile>)) -> TagProfileItems {
    let (tags, profiles) = options;
    let tags = tags.into_iter().map(|t| TagItem { id: t.id as i32, label: t.label.as_str().into(), selected: false }).collect();
    let mut profiles: Vec<ProfileItem> = std::iter::once(ProfileItem { id: 0, name: "Default".into() })
        .chain(profiles.into_iter().map(|p| ProfileItem { id: p.id as i32, name: p.name.as_str().into() }))
        .collect();
    if profiles.len() == 1 {
        profiles.clear();
    }
    (tags, profiles)
}

/// (season_number, name, episode_count, selected) — plain Send-safe tuple,
/// same reason as `CreditRow`: `SeasonItem` itself now carries a
/// `slint::Image` field, so it (like `CardItem`/`CastMember` elsewhere in
/// this app) can never be held across an `.await` point in a spawned task —
/// only ever constructed fresh inside `invoke_from_event_loop`.
type SeasonRow = (i32, String, i32, bool);

struct DetailFields {
    title: String,
    meta: String,
    overview: String,
    rating: String,
    poster_path: Option<String>,
    backdrop_path: Option<String>,
    status: Option<MediaStatus>,
    seasons: Vec<SeasonRow>,
    /// (season index into `seasons`, TMDB poster path).
    season_poster_paths: Vec<(usize, String)>,
    cast: Vec<CreditRow>,
}

fn movie_fields(d: MovieDetails) -> DetailFields {
    let year = d.release_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let genres = d.genres.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", ");
    let cast = build_cast_list(&d.credits);
    DetailFields {
        title: d.title,
        meta: if genres.is_empty() { year.to_string() } else { format!("{year} · {genres}") },
        overview: d.overview.unwrap_or_default(),
        rating: format_rating(d.vote_average),
        poster_path: d.poster_path,
        backdrop_path: d.backdrop_path,
        status: d.media_info.and_then(|mi| mi.status()),
        seasons: Vec::new(),
        season_poster_paths: Vec::new(),
        cast,
    }
}

fn tv_fields(d: TvDetails) -> DetailFields {
    let year = d.first_air_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let genres = d.genres.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", ");
    let cast = build_cast_list(&d.credits);
    let mut season_poster_paths = Vec::new();
    let seasons: Vec<SeasonRow> = d
        .seasons
        .iter()
        .enumerate()
        .map(|(i, s)| {
            if let Some(p) = &s.poster_path {
                season_poster_paths.push((i, p.clone()));
            }
            let name = if s.name.is_empty() { format!("Season {}", s.season_number) } else { s.name.clone() };
            (s.season_number as i32, name, s.episode_count as i32, true) // default all-checked, per plan decision 2
        })
        .collect();
    DetailFields {
        title: d.name,
        meta: if genres.is_empty() { year.to_string() } else { format!("{year} · {genres}") },
        overview: d.overview.unwrap_or_default(),
        rating: format_rating(d.vote_average),
        poster_path: d.poster_path,
        backdrop_path: d.backdrop_path,
        status: d.media_info.and_then(|mi| mi.status()),
        seasons,
        season_poster_paths,
        cast,
    }
}

/// Matches a Seerr/TMDB search result back to the corresponding local
/// library item by provider id, so a card that's already in the library can
/// open the real item (playable, has watch progress/favorite state) instead
/// of the Seerr request-detail page (which has nothing left to offer once
/// something is already available — just a static "In Library" pill).
/// Client-side by necessity: Jellyfin has no server-side "find item by
/// provider id" query (confirmed — no `AnyProviderIdEquals`-style parameter
/// exists), so this scans the already-cached `all_movies`/`all_series`
/// (populated from disk cache on warm start, refreshed in the background —
/// see CLAUDE.md's Disk caches section) for a `ProviderIds["Tmdb"]` match.
/// A miss (library not yet fetched, or genuinely not in the library) just
/// falls through to the normal Seerr detail flow.
fn find_local_item(state: &Arc<Mutex<FjordState>>, media_type: &str, tmdb_id_str: &str) -> Option<(String, String)> {
    let s = state.lock().unwrap();
    let items = if media_type == "movie" { &s.all_movies } else { &s.all_series };
    items
        .iter()
        .find(|m| m.provider_ids.get("Tmdb").map(String::as_str) == Some(tmdb_id_str))
        .map(|m| (m.id.clone(), m.item_type.clone()))
}

pub(crate) fn open_discover_item(
    media_type: String,
    tmdb_id_str: String,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    if let Some((id, item_type)) = find_local_item(&state, &media_type, &tmdb_id_str) {
        crate::detail::open_detail(id, item_type, state, ww, rt);
        return;
    }
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
        g.set_request_detail_overview_expanded(false);
        g.set_request_detail_rating("".into());
        g.set_request_detail_meta("".into());
        g.set_request_detail_has_poster(false);
        g.set_request_detail_has_backdrop(false);
        g.set_request_detail_status("".into());
        g.set_request_detail_cast(ModelRc::new(VecModel::from(Vec::<CastMember>::new())));
        g.set_request_detail_focused_cast(-1);
        g.set_request_detail_seasons(ModelRc::new(VecModel::from(Vec::<SeasonItem>::new())));
        g.set_request_detail_tags(ModelRc::new(VecModel::from(Vec::<TagItem>::new())));
        g.set_request_detail_profiles(ModelRc::new(VecModel::from(Vec::<ProfileItem>::new())));
        g.set_request_detail_tags_alt(ModelRc::new(VecModel::from(Vec::<TagItem>::new())));
        g.set_request_detail_profiles_alt(ModelRc::new(VecModel::from(Vec::<ProfileItem>::new())));
        g.set_request_detail_focused_profile(0);
        g.set_request_detail_selected_profile_id(0);
        g.set_request_detail_selected_profile_id_alt(0);
        g.set_request_detail_back_focused(true);
        g.set_request_detail_zone(0);
        g.set_request_detail_focused_season(0);
        g.set_request_detail_focused_tag(0);
        g.set_request_detail_want_4k(false);
        g.set_show_request_options(false); // defensive — shouldn't still be open across items
        g.set_show_request_detail(true);
        next
    };

    let media_type2 = media_type.clone();
    rt.spawn(async move {
        // Both quality tiers are fetched up front so the modal's Quality
        // toggle can swap between them instantly (request_detail_set_quality
        // below) instead of re-fetching live — no loading state, no race on
        // rapid toggling. The common single-instance setup only costs one
        // extra list call inside available_request_options_both_tiers, not
        // a duplicate detail fetch (see its doc comment in fjord-seerr).
        let (detail_result, options_result) = tokio::join!(
            async {
                if media_type2 == "movie" {
                    client.get_movie(tmdb_id).await.map(movie_fields)
                } else {
                    client.get_tv(tmdb_id).await.map(tv_fields)
                }
            },
            client.available_request_options_both_tiers(&media_type2),
        );
        let fields = match detail_result {
            Ok(f) => f,
            Err(e) => {
                handle_seerr_error(&state, &ww, is_session_auth, "Couldn't load details", &e);
                return;
            }
        };
        // Best-effort: no tags/profiles configured, or no permission to read
        // /service/* on this account, are both "just don't show that
        // picker," not a reason to fail opening the item.
        let ((tags, profiles), (tags_4k, profiles_4k)): (TagProfileItems, TagProfileItems) = match options_result {
            Ok((regular, fourk)) => (build_tag_profile_items(regular), build_tag_profile_items(fourk)),
            Err(e) => {
                debug!("seerr: couldn't fetch tags/profiles for {media_type2}: {e:#}");
                ((Vec::new(), Vec::new()), (Vec::new(), Vec::new()))
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

        // Cast/crew portraits + season posters — same bounded-concurrency
        // JoinSet+Semaphore shape as detail.rs's Jellyfin cast portrait fetch,
        // pointed at TMDB instead. Fetched together so neither trickles in
        // after the page is already shown.
        let sem = Arc::new(tokio::sync::Semaphore::new(6));
        let mut portrait_tasks: tokio::task::JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> =
            tokio::task::JoinSet::new();
        for (idx, (_, _, _, profile_path)) in fields.cast.iter().enumerate() {
            let Some(path) = profile_path.clone() else { continue };
            let http = http.clone();
            let sem = Arc::clone(&sem);
            let person_id = fields.cast[idx].0.clone();
            portrait_tasks.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let bytes = fetch_tmdb_image(&http, TMDB_PROFILE_BASE, &path, &format!("person-{person_id}")).await;
                (idx, bytes.as_deref().and_then(decode_poster_buffer))
            });
        }
        let mut portrait_bufs: Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> =
            vec![None; fields.cast.len()];
        while let Some(res) = portrait_tasks.join_next().await {
            let Ok((idx, buf)) = res else { continue };
            portrait_bufs[idx] = buf;
        }

        // Season posters are collected as plain Send-safe buffers here, same
        // reason as `portrait_bufs` — `SeasonItem` carries a `slint::Image`
        // field, so `fields.seasons` (moved into `invoke_from_event_loop`
        // below) must stay untouched by any real `Image` until it's on the
        // UI thread, or the whole closure fails to compile as `!Send`.
        let mut season_poster_bufs: Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> =
            vec![None; fields.seasons.len()];
        let mut season_tasks: tokio::task::JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> =
            tokio::task::JoinSet::new();
        for (season_idx, path) in fields.season_poster_paths.clone() {
            let http = http.clone();
            let sem = Arc::clone(&sem);
            let cache_key = format!("season-{tmdb_id}-{season_idx}");
            season_tasks.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let bytes = fetch_tmdb_image(&http, TMDB_POSTER_BASE, &path, &cache_key).await;
                (season_idx, bytes.as_deref().and_then(decode_poster_buffer))
            });
        }
        while let Some(res) = season_tasks.join_next().await {
            let Ok((season_idx, buf)) = res else { continue };
            if let Some(slot) = season_poster_bufs.get_mut(season_idx) {
                *slot = buf;
            }
        }

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_request_detail_open_gen() != gen {
                return; // superseded by a rapid re-open of a different item
            }
            g.set_request_detail_title(fields.title.as_str().into());
            g.set_request_detail_meta(fields.meta.as_str().into());
            g.set_request_detail_overview(fields.overview.as_str().into());
            g.set_request_detail_rating(fields.rating.as_str().into());
            g.set_request_detail_status(availability_tag(fields.status).into());
            let cast: Vec<CastMember> = fields
                .cast
                .into_iter()
                .zip(portrait_bufs)
                .map(|((id, name, role, _), buf)| {
                    let (photo, has_photo) = match buf {
                        Some(b) => (slint::Image::from_rgba8(b), true),
                        None => (Default::default(), false),
                    };
                    CastMember { id: id.as_str().into(), name: name.as_str().into(), role: role.as_str().into(), photo, has_photo }
                })
                .collect();
            g.set_request_detail_cast(ModelRc::new(VecModel::from(cast)));
            let seasons: Vec<SeasonItem> = fields
                .seasons
                .into_iter()
                .zip(season_poster_bufs)
                .map(|((season_number, name, episode_count, selected), buf)| {
                    let (poster, has_poster) = match buf {
                        Some(b) => (slint::Image::from_rgba8(b), true),
                        None => (Default::default(), false),
                    };
                    SeasonItem { season_number, name: name.as_str().into(), episode_count, selected, poster, has_poster }
                })
                .collect();
            g.set_request_detail_seasons(ModelRc::new(VecModel::from(seasons)));
            g.set_request_detail_tags(ModelRc::new(VecModel::from(tags)));
            g.set_request_detail_profiles(ModelRc::new(VecModel::from(profiles)));
            g.set_request_detail_tags_alt(ModelRc::new(VecModel::from(tags_4k)));
            g.set_request_detail_profiles_alt(ModelRc::new(VecModel::from(profiles_4k)));
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
    let tag_ids: Vec<i64> = {
        let model = g.get_request_detail_tags();
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .filter(|t| t.selected)
            .map(|t| t.id as i64)
            .collect()
    };
    // 0 means the synthetic "Default" row — don't send profileId at all,
    // same as an unset choice.
    let profile_id = match g.get_request_detail_selected_profile_id() {
        0 => None,
        id => Some(id as i64),
    };

    g.set_request_detail_requesting(true);
    drop(g);

    rt.spawn(async move {
        let result = client.create_request(&media_type, tmdb_id, seasons_selector, is_4k, tag_ids, profile_id).await;
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
    //
    // Also proactively refreshes the movie list (metadata only, no poster
    // sweep — `with_posters: false`) here: unlike `all_series`, which the
    // startup auto-login path refreshes unconditionally on every login,
    // `all_movies` is lazy-fetched only when the Movies library grid is
    // opened, so on a session where the user goes straight to Discover
    // without ever opening Movies, `all_movies` (and its `ProviderIds`,
    // needed by `find_local_item`) can still be whatever a stale on-disk
    // cache holds — real bug, live-reported as "in-library redirect works
    // for TV but not movies."
    g.on_nav_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |nav| {
            if nav == 6 {
                ensure_discover_landing(Arc::clone(&state), ww.clone(), rt.clone());
                spawn_movies_list_fetch(Arc::clone(&state), ww.clone(), rt.clone(), false);
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

    g.on_request_detail_toggle_tag({
        let ww = window.as_weak();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_request_detail_tags();
            if let Some(mut t) = model.row_data(idx as usize) {
                t.selected = !t.selected;
                model.set_row_data(idx as usize, t);
            }
        }
    });

    g.on_request_detail_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || submit_request(Arc::clone(&state), ww.clone(), rt.clone())
    });

    g.on_open_request_options({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let zones = existing_option_zones(&g);
            g.set_request_options_zone(zones.first().copied().unwrap_or(0));
            g.set_request_options_confirm_focused(1);
            g.set_show_request_options(true);
        }
    });

    g.on_request_detail_set_quality({
        let ww = window.as_weak();
        move |want_4k| {
            let Some(w) = ww.upgrade() else { return };
            set_quality(&AppState::get(&w), want_4k);
        }
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
            if (fs as usize) + 1 < lens.len() {
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

/// Zones below the button row, in vertical order, that exist for the current
/// item — storyline/cast each drop out individually when there's nothing to
/// show. Up/Down between zones (and the button row itself, always zone 0)
/// only ever step to the nearest zone that actually exists. Tags/seasons/4K
/// used to be zones here too, but moved into the Request Options modal
/// (`existing_option_zones`/`handle_key_request_options` below) — the
/// Request button opens that modal instead of exposing every picker inline.
fn existing_zones(g: &AppState) -> Vec<i32> {
    let mut zones = vec![0]; // button row always exists
    if !g.get_request_detail_overview().as_str().is_empty() {
        zones.push(1);
    }
    if g.get_request_detail_cast().row_count() > 0 {
        zones.push(2);
    }
    zones
}

fn zone_focus_reset(g: &AppState, zone: i32) {
    if zone == 2 {
        g.set_request_detail_focused_cast(0);
    }
}

/// Vertical flow: Back -> button row (Request) -> storyline (collapsible
/// overview) -> cast row. Each zone's Up-at-top/Down-at-bottom hands off to
/// the nearest neighboring zone that exists for the current item.
pub(crate) fn handle_key_request_detail(action: &Action, g: &AppState) -> bool {
    if g.get_request_detail_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_request_detail(false);
                true
            }
            Action::Down => {
                g.set_request_detail_back_focused(false);
                g.set_request_detail_zone(0);
                true
            }
            Action::Up => false, // let focus_bar_on_up handle the mini-player bar
            _ => true,
        };
    }

    let zones = existing_zones(g);
    let zone = g.get_request_detail_zone();
    let zone_pos = zones.iter().position(|&z| z == zone).unwrap_or(0);
    let prev_zone = || zone_pos.checked_sub(1).and_then(|i| zones.get(i)).copied();
    let next_zone = || zones.get(zone_pos + 1).copied();

    match zone {
        // Storyline — Enter toggles expand/collapse, no L/R (single item).
        1 => {
            return match action {
                Action::Confirm => {
                    g.set_request_detail_overview_expanded(!g.get_request_detail_overview_expanded());
                    true
                }
                Action::Up => {
                    g.set_request_detail_back_focused(true);
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_detail_zone(next);
                        zone_focus_reset(g, next);
                    }
                    true
                }
                Action::Back => {
                    g.set_show_request_detail(false);
                    true
                }
                _ => true,
            };
        }
        // Cast & Crew row — L/R scroll, Enter no-ops (no TMDB-person detail
        // screen to open), row stays keyboard-reachable for consistency.
        2 => {
            let count = g.get_request_detail_cast().row_count() as i32;
            return match action {
                Action::Left => {
                    let f = g.get_request_detail_focused_cast();
                    if f > 0 { g.set_request_detail_focused_cast(f - 1); }
                    true
                }
                Action::Right => {
                    let f = g.get_request_detail_focused_cast();
                    if f + 1 < count { g.set_request_detail_focused_cast(f + 1); }
                    true
                }
                Action::Up => {
                    g.set_request_detail_focused_cast(-1);
                    match prev_zone() {
                        Some(prev) => g.set_request_detail_zone(prev),
                        None => g.set_request_detail_back_focused(true),
                    }
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_detail_focused_cast(-1);
                        g.set_request_detail_zone(next);
                        zone_focus_reset(g, next);
                    }
                    true
                }
                Action::Back => {
                    g.set_show_request_detail(false);
                    true
                }
                _ => true,
            };
        }
        _ => {} // zone 0 falls through to the button row below
    }

    // Zone 0: single Request button (or a static status pill once already
    // requested/available — nothing left to interact with in that state).
    // Confirm opens the Request Options modal rather than submitting
    // directly; 4K/tags/seasons are configured there, not on this page.
    let requestable = g.get_request_detail_status().as_str() == "";
    match action {
        Action::Up => {
            g.set_request_detail_back_focused(true);
            true
        }
        Action::Down => {
            if let Some(next) = next_zone() {
                g.set_request_detail_zone(next);
                zone_focus_reset(g, next);
            }
            true
        }
        Action::Confirm => {
            if requestable {
                g.invoke_open_request_options();
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

// ── Keyboard: Request Options modal ─────────────────────────────────────────

/// Zones inside the modal, in vertical order, that exist for the current
/// item: 0=Quality (2K/4K, always — every requestable item can ask for 4K),
/// 1=profile row (if any configured), 2=tags row (if any configured),
/// 3=seasons row (if any, TV only), 4=confirm row (always, Cancel/Request).
/// Same skip-absent-zones array-position navigation idiom as `existing_zones`
/// above — a distinct numbering scheme, not a continuation of it, since this
/// is an independent vertical flow inside its own modal.
fn existing_option_zones(g: &AppState) -> Vec<i32> {
    let mut zones = vec![0];
    if g.get_request_detail_profiles().row_count() > 0 {
        zones.push(1);
    }
    if g.get_request_detail_tags().row_count() > 0 {
        zones.push(2);
    }
    if g.get_request_detail_media_type().as_str() == "tv" && g.get_request_detail_seasons().row_count() > 0 {
        zones.push(3);
    }
    zones.push(4);
    zones
}

fn option_zone_focus_reset(g: &AppState, zone: i32) {
    match zone {
        1 => g.set_request_detail_focused_profile(0),
        2 => g.set_request_detail_focused_tag(0),
        3 => g.set_request_detail_focused_season(0),
        _ => {}
    }
}

/// Sets the Quality toggle, swapping in the other tier's pre-fetched
/// tags/profiles/selected-profile-id when the value actually changes —
/// both tiers were fetched up front by `open_discover_item`
/// (`available_request_options_both_tiers`), so this is instant with no
/// network call and no race on rapid toggling. Selections travel with
/// whichever set they belong to (swapped, not reset), so toggling away and
/// back preserves what was picked on each tier. Shared by both the keyboard
/// handler below and the Slint `request-detail-set-quality` callback the
/// 2K/4K buttons' mouse clicks go through.
pub(crate) fn set_quality(g: &AppState, want_4k: bool) {
    if g.get_request_detail_want_4k() == want_4k {
        return;
    }
    let tags = g.get_request_detail_tags();
    g.set_request_detail_tags(g.get_request_detail_tags_alt());
    g.set_request_detail_tags_alt(tags);

    let profiles = g.get_request_detail_profiles();
    g.set_request_detail_profiles(g.get_request_detail_profiles_alt());
    g.set_request_detail_profiles_alt(profiles);

    let selected_profile_id = g.get_request_detail_selected_profile_id();
    g.set_request_detail_selected_profile_id(g.get_request_detail_selected_profile_id_alt());
    g.set_request_detail_selected_profile_id_alt(selected_profile_id);

    g.set_request_detail_focused_tag(0);
    g.set_request_detail_focused_profile(0);
    g.set_request_detail_want_4k(want_4k);
}

/// Back/Escape closes the modal (Cancel) from any zone. Otherwise: Quality
/// row -> profile row -> tags row -> seasons row -> confirm row
/// (Cancel/Request), Up/Down stepping to the nearest zone that exists for
/// this item.
pub(crate) fn handle_key_request_options(action: &Action, g: &AppState) -> bool {
    if matches!(action, Action::Back) {
        g.set_show_request_options(false);
        return true;
    }

    let zones = existing_option_zones(g);
    let zone = g.get_request_options_zone();
    let zone_pos = zones.iter().position(|&z| z == zone).unwrap_or(0);
    let prev_zone = || zone_pos.checked_sub(1).and_then(|i| zones.get(i)).copied();
    let next_zone = || zones.get(zone_pos + 1).copied();

    match zone {
        // Quality (2K/4K) pair, not a single on/off toggle — Left/Right pick
        // the value directly rather than moving a separate cursor, since the
        // selected button already IS the keyboard position (no Confirm step
        // needed on top of it).
        0 => match action {
            Action::Left => {
                set_quality(g, false);
                true
            }
            Action::Right => {
                set_quality(g, true);
                true
            }
            Action::Down => {
                if let Some(next) = next_zone() {
                    g.set_request_options_zone(next);
                    option_zone_focus_reset(g, next);
                }
                true
            }
            _ => true, // Up absorbed — already the top zone
        },
        // Quality profile row (radio-select) — L/R scroll the cursor, Enter
        // selects the profile under it (replacing, not toggling like tags).
        1 => {
            let count = g.get_request_detail_profiles().row_count() as i32;
            match action {
                Action::Left => {
                    let f = g.get_request_detail_focused_profile();
                    if f > 0 { g.set_request_detail_focused_profile(f - 1); }
                    true
                }
                Action::Right => {
                    let f = g.get_request_detail_focused_profile();
                    if f + 1 < count { g.set_request_detail_focused_profile(f + 1); }
                    true
                }
                Action::Confirm => {
                    let idx = g.get_request_detail_focused_profile();
                    if let Some(p) = g.get_request_detail_profiles().row_data(idx as usize) {
                        g.set_request_detail_selected_profile_id(p.id);
                    }
                    true
                }
                Action::Up => {
                    if let Some(prev) = prev_zone() { g.set_request_options_zone(prev); }
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_options_zone(next);
                        option_zone_focus_reset(g, next);
                    }
                    true
                }
                _ => true,
            }
        }
        2 => {
            let count = g.get_request_detail_tags().row_count() as i32;
            match action {
                Action::Left => {
                    let f = g.get_request_detail_focused_tag();
                    if f > 0 { g.set_request_detail_focused_tag(f - 1); }
                    true
                }
                Action::Right => {
                    let f = g.get_request_detail_focused_tag();
                    if f + 1 < count { g.set_request_detail_focused_tag(f + 1); }
                    true
                }
                Action::Confirm => {
                    g.invoke_request_detail_toggle_tag(g.get_request_detail_focused_tag());
                    true
                }
                Action::Up => {
                    if let Some(prev) = prev_zone() { g.set_request_options_zone(prev); }
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_options_zone(next);
                        option_zone_focus_reset(g, next);
                    }
                    true
                }
                _ => true,
            }
        }
        3 => {
            let count = g.get_request_detail_seasons().row_count() as i32;
            match action {
                Action::Left => {
                    let f = g.get_request_detail_focused_season();
                    if f > 0 { g.set_request_detail_focused_season(f - 1); }
                    true
                }
                Action::Right => {
                    let f = g.get_request_detail_focused_season();
                    if f + 1 < count { g.set_request_detail_focused_season(f + 1); }
                    true
                }
                Action::Confirm => {
                    g.invoke_request_detail_toggle_season(g.get_request_detail_focused_season());
                    true
                }
                Action::Up => {
                    if let Some(prev) = prev_zone() { g.set_request_options_zone(prev); }
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() { g.set_request_options_zone(next); }
                    true
                }
                _ => true,
            }
        }
        _ => match action {
            // Confirm row: Left/Right toggle Cancel/Request, Enter activates
            // whichever is focused. Request submits and closes; Cancel just closes.
            Action::Left => {
                g.set_request_options_confirm_focused(0);
                true
            }
            Action::Right => {
                g.set_request_options_confirm_focused(1);
                true
            }
            Action::Up => {
                if let Some(prev) = prev_zone() { g.set_request_options_zone(prev); }
                true
            }
            Action::Confirm => {
                let submit = g.get_request_options_confirm_focused() == 1;
                g.set_show_request_options(false);
                if submit {
                    g.invoke_request_detail_request();
                }
                true
            }
            _ => true,
        },
    }
}
