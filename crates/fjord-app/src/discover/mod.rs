// ── fjord-app · discover/mod.rs ──────────────────────────────────────────────
//   Submodules (re-exported here, so callers keep using crate::discover::*):
//     wire           wire_discover — every Discover/RequestDetail AppState callback
//     search         search field: debounced search + paging + poster patching
//     landing        no-query landing rows (Trending/Popular/Upcoming/Requested/New in theaters)
//     filters        filter pills, filtered browse, client-side search filters
//     watchlist      Seerr watchlist rows + toggle + Jellyfin star resync
//     calendar       Release Calendar + Coming Up row
//     request_detail RequestDetailScreen: open/fill (open_discover_item_ex), tiers, cast, local match
//     requests       request submit/edit/actions, blocklist toggle
//     trailers       trailer URL allow-list + yt-dlp check
//     keys           keyboard: Discover grid/landing/filter bar/popups, request detail + options
//   TMDB_*_BASE / fetch_tmdb_image  TMDB image URLs + cached fetch (also used by series.rs)
//   availability_tag    MediaStatus → CardItem.availability ("blocklisted" included)
//   DiscoverCardMeta    Send-safe card data built off-thread; KnownRequest; CalendarEntry/-Kind
//   known_requests_from_row / patch_known_request_state / patch_watchlist_state  request and
//                       watchlist state for cards outside the Requested row, from FjordState caches
//   search_result_to_meta  SearchResult → meta (blocklisted items are dropped here, for every row)
//   build_person_credit_metas  CombinedCredits → (meta, poster_path) pairs (Person "Other Work")
//   resolve_and_fetch_discovery_row  shared pipeline for the discovery rows: drop locally owned
//                       items, cap, patch request/watchlist state, fetch posters (Send-safe output)
//   discover_cards_from UI-thread-only: (meta, poster) pairs → Vec<CardItem>
//   is_401 / handle_seerr_error  a session-auth 401 resets the connection
//                       (seerr_auth::clear_connection + "reconnect in Settings" toast); else a toast
//   resolve_streaming_region / resolve_discover_region  the user's streamingRegion /
//                       discoverRegion (cached per connection, "US" fallback)
//   refresh_seerr_admin_status  re-reads the permission bits on Discover arrival (60 s cooldown)
//   patch_discover_card_availability / patch_discover_card_request_state  patch a live grid card
//   all_card_model_slots / patch_watchlist_on_all_models / remove_card_from_all_models  every
//                       model that can hold a Discover card (grid, landing rows, dashboard splits)
// ─────────────────────────────────────────────────────────────────────────────

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use fjord_seerr::{MediaStatus, MovieDetails, SearchResult, SeasonsSelector, TvDetails};
use slint::{ComponentHandle, Global, Model, ModelRc, VecModel, Weak};

use tracing::{debug, info, warn};

use crate::config::{
    FjordState, ProfileSettings, RequestPreference, discover_poster_cache_path, save_config,
};
use crate::keys::Action;
use crate::poster::decode_poster_buffer;
use crate::{
    AppState, CardItem, CastMember, GenreItem, MainWindow, ProfileItem, ProviderItem, SeasonItem,
    StreamingProvider, TagItem, show_toast, spawn_movies_list_fetch,
};

mod calendar;
mod filters;
mod keys;
mod landing;
mod request_detail;
mod requests;
mod search;
mod trailers;
mod watchlist;
mod wire;

pub(crate) use calendar::*;
pub(crate) use filters::*;
pub(crate) use keys::*;
pub(crate) use landing::*;
pub(crate) use request_detail::*;
pub(crate) use requests::*;
pub(crate) use search::*;
pub(crate) use trailers::*;
pub(crate) use watchlist::*;
pub(crate) use wire::*;

pub(crate) const TMDB_POSTER_BASE: &str = "https://image.tmdb.org/t/p/w500";

const TMDB_BACKDROP_BASE: &str = "https://image.tmdb.org/t/p/w1280";

const TMDB_LOGO_BASE: &str = "https://image.tmdb.org/t/p/w92"; // small, icon-sized — provider chips

// Shared by push_coming_up_row and fetch_coming_up_posters — both must
// truncate the same source list identically, since the poster fetch
// patches discover-coming-up by the row index the text-only commit used.
const COMING_UP_PREVIEW_CAP: usize = 20;

fn availability_tag(status: Option<MediaStatus>) -> &'static str {
    match status {
        Some(MediaStatus::Pending) => "requested",
        Some(MediaStatus::Processing) => "processing",
        Some(MediaStatus::PartiallyAvailable) => "partial",
        Some(MediaStatus::Available) => "available",
        // Blocklisted gets its own value: it's just another value of this exclusive status
        // field (unlike Watchlist, an independent boolean), and it hides the Request button
        // (tier_status_label). It's the only per-card signal Blocklist needs.
        Some(MediaStatus::Blocklisted) => "blocklisted",
        Some(MediaStatus::Unknown) | Some(MediaStatus::Deleted) | None => "",
    }
}

/// Plain Send-able card metadata, built off-thread. `CardItem` itself always
/// carries a `slint::Image` field (even when it's the default/empty value —
/// `Send` is a type-level property, not a runtime one), so it can never cross
/// a thread boundary; every `CardItem` here is constructed fresh on the UI
/// thread, inside an `invoke_from_event_loop` closure, from one of these.
#[derive(Clone)]
pub(crate) struct DiscoverCardMeta {
    id: String,
    item_type: &'static str,
    title: String,
    subtitle: String,
    year: i32,
    availability: &'static str,
    // Requested row only — false on every other card (like `availability`); see
    // CardItem in theme.slint for what these drive.
    requested_4k: bool,
    other_tier_available: bool,
    other_tier_requested: bool,
    // Requested row only (2026-07-18) — the Seerr MediaRequest's own id
    // (distinct from `id` above, which is the tmdb id); "" everywhere else.
    // Drives the Discover context menu's Edit/Cancel/Approve/Decline rows.
    request_id: String,
    request_pending: bool,
    request_mine: bool,
    // Never displayed (not on CardItem): only for apply_search_filters' client-side
    // genre/rating filtering of fetched search results (kept in
    // FjordState.discover_search_metas). Empty/0.0 on landing-row cards.
    genre_ids: Vec<i64>,
    vote_average: f64,
    // Type=All filtered-browse merge only (2026-07-18) — see
    // fjord_seerr::SearchResult.popularity's own doc comment. 0.0 on every
    // other card, same scoping as genre_ids/vote_average above.
    popularity: f64,
    // Watchlist + Release Calendar (2026-07-18) — see CardItem's own doc
    // comment (theme.slint).
    on_watchlist: bool,
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
            requested_4k: self.requested_4k,
            other_tier_available: self.other_tier_available,
            other_tier_requested: self.other_tier_requested,
            request_id: self.request_id.as_str().into(),
            request_pending: self.request_pending,
            request_mine: self.request_mine,
            on_watchlist: self.on_watchlist,
            ..Default::default()
        }
    }
}

/// One cached entry in `FjordState.discover_known_requests` — the minimal
/// request-state fields a search/landing-row `DiscoverCardMeta` needs
/// patched onto it so its context menu offers Edit/Cancel/View Request
/// instead of Request for an item that's already been requested. See that
/// field's own doc comment (config.rs) for the cache's scope/limits.
#[derive(Clone)]
pub(crate) struct KnownRequest {
    pub(crate) request_id: String,
    pub(crate) pending: bool,
    pub(crate) mine: bool,
}

/// One "Coming Up" calendar entry — a movie's theatrical/digital/physical
/// release date (region-resolved via `resolve_discover_region`) or a TV
/// show's next episode air date. Watchlist + Release Calendar, 2026-07-18.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CalendarEntryKind {
    Theatrical,
    Digital,
    Physical,
    Episode,
}

// poster_path added 2026-07-19, user request ("it hust dosent have
// posters") — the original "deliberately text-only" design was reversed;
// see push_coming_up_row/fetch_coming_up_posters' own doc comments.
#[derive(Clone)]
pub(crate) struct CalendarEntry {
    pub(crate) date: chrono::NaiveDate,
    pub(crate) tmdb_id: String,
    pub(crate) item_type: &'static str,
    pub(crate) title: String,
    pub(crate) poster_path: Option<String>,
    // Snapshot of FjordState.discover_watchlist_ids at candidate selection, so
    // push_coming_up_row's cards show the watchlist star — an item is often on Coming Up
    // precisely because it was just watchlisted.
    pub(crate) on_watchlist: bool,
    pub(crate) kind: CalendarEntryKind,
    // "S2E4 — Episode Name" for a TV entry; None for movies.
    pub(crate) episode_label: Option<String>,
}

/// Builds the known-requests lookup from a freshly-fetched Requested row —
/// called by both `ensure_discover_landing` and `refresh_requested_row`
/// right after `fetch_requested_row` returns, no extra network call. Keyed
/// identically to `ensure_discover_landing`'s own `requested_keys` dedup set.
fn known_requests_from_row(
    requested: &[RequestedRowItem],
) -> std::collections::HashMap<(&'static str, String), KnownRequest> {
    requested
        .iter()
        .map(|(m, _)| {
            (
                (m.item_type, m.id.clone()),
                KnownRequest {
                    request_id: m.request_id.clone(),
                    pending: m.request_pending,
                    mine: m.request_mine,
                },
            )
        })
        .collect()
}

/// Patches `request_id`/`request_pending`/`request_mine` onto a freshly built meta
/// (search result or non-Requested landing card) from the known-requests cache
/// (FjordState.discover_known_requests); a no-op when the item isn't there.
fn patch_known_request_state(
    meta: &mut DiscoverCardMeta,
    known: &std::collections::HashMap<(&'static str, String), KnownRequest>,
) {
    if let Some(k) = known.get(&(meta.item_type, meta.id.clone())) {
        meta.request_id = k.request_id.clone();
        meta.request_pending = k.pending;
        meta.request_mine = k.mine;
    }
}

/// `on_watchlist` counterpart to `patch_known_request_state` above — same
/// shape, consulting `FjordState.discover_watchlist_ids` instead. Watchlist
/// + Release Calendar, 2026-07-18.
fn patch_watchlist_state(
    meta: &mut DiscoverCardMeta,
    watchlist_ids: &std::collections::HashSet<(&'static str, String)>,
) {
    meta.on_watchlist = watchlist_ids.contains(&(meta.item_type, meta.id.clone()));
}

fn search_result_to_meta(r: &SearchResult) -> Option<DiscoverCardMeta> {
    if r.media_type != "movie" && r.media_type != "tv" {
        return None; // person results filtered out — v1 shows movies/TV only
    }
    let availability = availability_tag(r.media_info.as_ref().and_then(|mi| mi.status()));
    if availability == "blocklisted" {
        // Blocklisted items never show in Discover: every landing row, search and
        // filtered-browse fetch goes through this function (directly or via
        // `build_filtered_metas`), so this is the one filter point — like Seerr's own web
        // UI for accounts without blocklist permissions. Fjord has no "show with a badge"
        // mode, so it always filters.
        return None;
    }
    Some(DiscoverCardMeta {
        id: r.id.to_string(),
        item_type: if r.media_type == "movie" {
            "DiscoverMovie"
        } else {
            "DiscoverTv"
        },
        title: r.display_title().to_string(),
        subtitle: r.year().unwrap_or("").to_string(),
        year: r.year().and_then(|y| y.parse().ok()).unwrap_or(0),
        availability,
        requested_4k: false,
        other_tier_available: false,
        other_tier_requested: false,
        request_id: String::new(),
        request_pending: false,
        request_mine: false,
        genre_ids: r.genre_ids.clone(),
        vote_average: r.vote_average.unwrap_or(0.0),
        popularity: r.popularity.unwrap_or(0.0),
        on_watchlist: false,
    })
}

/// GET /person/{id}/combined_credits → `(DiscoverCardMeta, Option<String>)` pairs
/// (like `build_filtered_metas`) for the Person screen's "Other Work" row. Cast and
/// crew are merged and deduped by `(id, media_type)` (an actor-director). This
/// endpoint carries no `media_info` (see `PersonCreditCast`), so `availability` is
/// patched in later by `resolve_and_fetch_discovery_row`.
pub(crate) fn build_person_credit_metas(
    credits: &fjord_seerr::CombinedCredits,
) -> Vec<(DiscoverCardMeta, Option<String>)> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for c in &credits.cast {
        let Some(mt) = c.media_type.as_deref() else {
            continue;
        };
        if mt != "movie" && mt != "tv" {
            continue;
        }
        if !seen.insert((mt, c.id)) {
            continue;
        }
        let item_type = if mt == "movie" {
            "DiscoverMovie"
        } else {
            "DiscoverTv"
        };
        let year_str = c.year().unwrap_or("");
        out.push((
            DiscoverCardMeta {
                id: c.id.to_string(),
                item_type,
                title: c.display_title().to_string(),
                subtitle: year_str.to_string(),
                year: year_str.parse().unwrap_or(0),
                availability: "",
                requested_4k: false,
                other_tier_available: false,
                other_tier_requested: false,
                request_id: String::new(),
                request_pending: false,
                request_mine: false,
                genre_ids: Vec::new(),
                vote_average: 0.0,
                popularity: 0.0,
                on_watchlist: false,
            },
            c.poster_path.clone(),
        ));
    }
    for c in &credits.crew {
        let Some(mt) = c.media_type.as_deref() else {
            continue;
        };
        if mt != "movie" && mt != "tv" {
            continue;
        }
        if !seen.insert((mt, c.id)) {
            continue;
        }
        let item_type = if mt == "movie" {
            "DiscoverMovie"
        } else {
            "DiscoverTv"
        };
        let year_str = c.year().unwrap_or("");
        out.push((
            DiscoverCardMeta {
                id: c.id.to_string(),
                item_type,
                title: c.display_title().to_string(),
                subtitle: year_str.to_string(),
                year: year_str.parse().unwrap_or(0),
                availability: "",
                requested_4k: false,
                other_tier_available: false,
                other_tier_requested: false,
                request_id: String::new(),
                request_pending: false,
                request_mine: false,
                genre_ids: Vec::new(),
                vote_average: 0.0,
                popularity: 0.0,
                on_watchlist: false,
            },
            c.poster_path.clone(),
        ));
    }
    out
}

/// Shared pipeline for the discovery rows (Person Other Work, Detail/Series
/// Recommended, Collection Missing Items), from `(meta, poster_path)` pairs:
/// (1) drop anything that matches a local Jellyfin item (`find_local_item`) —
/// discovery = not owned; (2) cap to `cap`; (3) patch request/watchlist state from
/// the caches; (4) fetch posters with bounded concurrency. Returns Send-safe pairs:
/// callers build their `CardItem`s with `discover_cards_from` inside their own
/// `invoke_from_event_loop` (this function never touches `AppState`/`CardItem`).
pub(crate) async fn resolve_and_fetch_discovery_row(
    state: &Arc<Mutex<FjordState>>,
    items: Vec<(DiscoverCardMeta, Option<String>)>,
    cap: usize,
) -> Vec<(
    DiscoverCardMeta,
    Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
)> {
    let mut filtered: Vec<(DiscoverCardMeta, Option<String>)> = items
        .into_iter()
        .filter(|(m, _)| {
            let media_type = if m.item_type == "DiscoverMovie" {
                "movie"
            } else {
                "tv"
            };
            find_local_item(state, media_type, &m.id).is_none()
        })
        .collect();
    filtered.truncate(cap);

    let (known, watchlist) = {
        let s = state.lock().unwrap();
        (
            s.discover_known_requests.clone(),
            s.discover_watchlist_ids.clone(),
        )
    };
    for (meta, _) in &mut filtered {
        patch_known_request_state(meta, &known);
        patch_watchlist_state(meta, &watchlist);
    }

    if filtered.is_empty() {
        return Vec::new();
    }
    let Ok(http) = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
    else {
        return filtered.into_iter().map(|(m, _)| (m, None)).collect();
    };
    let sem = Arc::new(tokio::sync::Semaphore::new(8));
    let mut set = tokio::task::JoinSet::new();
    for (idx, (_, poster_path)) in filtered.iter().enumerate() {
        let Some(path) = poster_path.clone() else {
            continue;
        };
        let http = http.clone();
        let sem = Arc::clone(&sem);
        let item_type = filtered[idx].0.item_type;
        let id = filtered[idx].0.id.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let cache_key = format!(
                "{}-{}",
                if item_type == "DiscoverMovie" {
                    "movie"
                } else {
                    "tv"
                },
                id
            );
            let bytes = fetch_tmdb_image(&http, TMDB_POSTER_BASE, &path, &cache_key).await?;
            let buf = decode_poster_buffer(&bytes)?;
            Some((idx, buf))
        });
    }
    let mut bufs: Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> =
        vec![None; filtered.len()];
    while let Some(res) = set.join_next().await {
        if let Ok(Some((idx, buf))) = res {
            bufs[idx] = Some(buf);
        }
    }
    filtered
        .into_iter()
        .zip(bufs)
        .map(|((m, _), buf)| (m, buf))
        .collect()
}

/// UI-thread-only: builds the final `Vec<CardItem>` from
/// `resolve_and_fetch_discovery_row`'s output — call inside
/// `invoke_from_event_loop`, never off-thread (`CardItem` carries a
/// `slint::Image` field, `!Send` regardless of whether it's populated).
pub(crate) fn discover_cards_from(
    items: Vec<(
        DiscoverCardMeta,
        Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
    )>,
) -> Vec<CardItem> {
    items
        .into_iter()
        .map(|(meta, buf)| {
            let mut card = meta.into_card_item();
            if let Some(b) = buf {
                card.poster = slint::Image::from_rgba8(b);
                card.has_poster = true;
            }
            card
        })
        .collect()
}

fn is_401(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|re| re.status())
        .map(|s| s == reqwest::StatusCode::UNAUTHORIZED)
        .unwrap_or(false)
}

/// A session-auth 401 means the cookie expired server-side: reset the connection so
/// Settings shows "Not connected" and the user can reconnect. An API key doesn't
/// expire, so a 401 there (revoked/invalid key) is shown as a plain error.
/// Also used by blocklist.rs / collection.rs.
pub(crate) fn handle_seerr_error(
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    is_session_auth: bool,
    context: &str,
    e: &anyhow::Error,
) {
    if is_session_auth && is_401(e) {
        warn!("seerr: {context}: session expired (401) — resetting connection: {e:#}");
        crate::seerr_auth::clear_connection(state, ww);
        show_toast(
            ww.clone(),
            "Seerr session expired — reconnect in Settings".into(),
        );
    } else {
        warn!("seerr: {context}: {e:#}");
        show_toast(ww.clone(), format!("{context}: {e}"));
    }
}

pub(crate) async fn fetch_tmdb_image(
    http: &reqwest::Client,
    base: &str,
    path: &str,
    cache_key: &str,
) -> Option<Vec<u8>> {
    // None for a key that isn't a safe file name: fetched, not cached.
    let cache_path = discover_poster_cache_path(cache_key);
    if let Some(p) = &cache_path
        && let Ok(bytes) = tokio::fs::read(p).await
    {
        return Some(bytes);
    }
    let url = format!("{base}{path}");
    let bytes = http
        .get(&url)
        .send()
        .await
        .ok()?
        .error_for_status()
        .ok()?
        .bytes()
        .await
        .ok()?
        .to_vec();
    // Only real images reach the disk (poster::is_image).
    if !crate::poster::is_image(&bytes) {
        return None;
    }
    if let Some(cache_path) = cache_path {
        if let Some(parent) = cache_path.parent() {
            let _ = tokio::fs::create_dir_all(parent).await;
        }
        let _ = tokio::fs::write(&cache_path, &bytes).await;
    }
    Some(bytes)
}

/// Which `watch_providers` region to show ("Currently Streaming On"): the CONNECTED
/// user's own `streamingRegion` (`GET /auth/me` → `GET /user/{id}/settings/main`),
/// "US" when unset — Seerr's own frontend fallback (SettingsMain/index.tsx). Cached
/// in `FjordState.seerr_streaming_region`; the Settings Streaming Region picker updates
/// the same cache on a successful write. A failed fetch also caches "US" instead of
/// retrying on every open.
async fn resolve_streaming_region(
    client: &fjord_seerr::SeerrClient,
    state: &Arc<Mutex<FjordState>>,
) -> String {
    if let Some(region) = state.lock().unwrap().seerr_streaming_region.clone() {
        return region;
    }
    let region = async {
        let user = client.get_current_user().await.ok()?;
        let settings = client.get_user_settings(user.id).await.ok()?;
        settings.streaming_region.filter(|s| !s.is_empty())
    }
    .await
    .unwrap_or_else(|| "US".to_string());
    state.lock().unwrap().seerr_streaming_region = Some(region.clone());
    region
}

/// Like `resolve_streaming_region`, but for Seerr's separate `discoverRegion`
/// setting, which its frontend uses for release dates (MovieDetails/index.tsx).
async fn resolve_discover_region(
    client: &fjord_seerr::SeerrClient,
    state: &Arc<Mutex<FjordState>>,
) -> String {
    if let Some(region) = state.lock().unwrap().seerr_discover_region.clone() {
        return region;
    }
    let region = async {
        let user = client.get_current_user().await.ok()?;
        let settings = client.get_user_settings(user.id).await.ok()?;
        settings.discover_region.filter(|s| !s.is_empty())
    }
    .await
    .unwrap_or_else(|| "US".to_string());
    state.lock().unwrap().seerr_discover_region = Some(region.clone());
    region
}

/// Re-fetches just the account id + MANAGE_REQUESTS/ADMIN bit (`GET /auth/me`, not
/// the heavier settings fetch) when Discover is opened, so a permission change
/// mid-session reaches the context menu's Approve/Decline gating. Best-effort and
/// non-blocking: the menu uses the cached value; a failure leaves it unchanged.
/// At most once per 60 s: holding an arrow key through the sidebar passed Discover
/// many times a minute, and the burst of late replies hitched the UI.
const SEERR_ADMIN_REFRESH_COOLDOWN: Duration = Duration::from_secs(60);

fn refresh_seerr_admin_status(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let client = {
        let mut s = state.lock().unwrap();
        if s.seerr_admin_last_refresh
            .is_some_and(|t| t.elapsed() < SEERR_ADMIN_REFRESH_COOLDOWN)
        {
            debug!("seerr: refresh_seerr_admin_status skipped, within cooldown");
            return;
        }
        let Some(client) = s.seerr_client.clone() else {
            return;
        };
        s.seerr_admin_last_refresh = Some(Instant::now());
        client
    };
    debug!("seerr: refresh_seerr_admin_status firing");
    rt.spawn(async move {
        let Ok(user) = client.get_current_user().await else {
            return;
        };
        let (user_id, is_admin) = (Some(user.id), user.can_manage_requests());
        let can_manage_blocklist = user.can_manage_blocklist();
        {
            let mut s = state.lock().unwrap();
            s.seerr_user_id = user_id;
            s.seerr_is_admin = is_admin;
            s.seerr_can_manage_blocklist = can_manage_blocklist;
        }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_seerr_is_admin(is_admin);
                g.set_seerr_can_manage_blocklist(can_manage_blocklist);
            }
        });
    });
}

fn patch_discover_card_availability(
    g: &AppState,
    media_type: &str,
    tmdb_id: i64,
    availability: &str,
) {
    let item_type = if media_type == "movie" {
        "DiscoverMovie"
    } else {
        "DiscoverTv"
    };
    let id_str = tmdb_id.to_string();
    let model = g.get_discover_results();
    for i in 0..model.row_count() {
        if let Some(mut card) = model.row_data(i)
            && card.id.as_str() == id_str
            && card.item_type.as_str() == item_type
        {
            card.availability = availability.into();
            model.set_row_data(i, card);
            break;
        }
    }
}

/// Patches `request_id`/`request_pending`/`request_mine` onto the matching
/// search-grid card, if visible — the counterpart of
/// `patch_discover_card_availability` for the fields that one doesn't touch, so a card
/// requested from the grid offers Edit/Cancel right away.
fn patch_discover_card_request_state(
    g: &AppState,
    media_type: &str,
    tmdb_id: i64,
    request_id: &str,
    pending: bool,
    mine: bool,
) {
    let item_type = if media_type == "movie" {
        "DiscoverMovie"
    } else {
        "DiscoverTv"
    };
    let id_str = tmdb_id.to_string();
    let model = g.get_discover_results();
    for i in 0..model.row_count() {
        if let Some(mut card) = model.row_data(i)
            && card.id.as_str() == id_str
            && card.item_type.as_str() == item_type
        {
            card.request_id = request_id.into();
            card.request_pending = pending;
            card.request_mine = mine;
            model.set_row_data(i, card);
            break;
        }
    }
}

/// Every AppState model that can hold a Discover-sourced `CardItem`: the flat
/// search/filtered grid, the 9 landing rows (`landing_row_get`/`_set`), and 5 more —
/// the Watchlist row's Movies/TV split (`discover-watchlist-movies`/`-tv`) and the
/// Coming Up row's dashboard split (`discover-coming-up-mixed`/`-movies`/`-tv`). Patch
/// and remove helpers walk all of them, so a change shows on the dashboards too.
/// Callers that only patch in place can ignore the setter.
type CardModelSlot = (ModelRc<CardItem>, Box<dyn Fn(&AppState, ModelRc<CardItem>)>);

fn all_card_model_slots(g: &AppState) -> Vec<CardModelSlot> {
    let mut v: Vec<CardModelSlot> = vec![(
        g.get_discover_results(),
        Box::new(|g: &AppState, m| g.set_discover_results(m)),
    )];
    // Row count from landing_row_lens, not a literal, so it can't drift when a row is
    // added.
    for row in 0..landing_row_lens(g).len() {
        v.push((
            landing_row_get(g, row),
            Box::new(move |g: &AppState, m| landing_row_set(g, row, m)),
        ));
    }
    v.push((
        g.get_discover_watchlist_movies(),
        Box::new(|g: &AppState, m| g.set_discover_watchlist_movies(m)),
    ));
    v.push((
        g.get_discover_watchlist_tv(),
        Box::new(|g: &AppState, m| g.set_discover_watchlist_tv(m)),
    ));
    v.push((
        g.get_discover_coming_up_mixed(),
        Box::new(|g: &AppState, m| g.set_discover_coming_up_mixed(m)),
    ));
    v.push((
        g.get_discover_coming_up_movies(),
        Box::new(|g: &AppState, m| g.set_discover_coming_up_movies(m)),
    ));
    v.push((
        g.get_discover_coming_up_tv(),
        Box::new(|g: &AppState, m| g.set_discover_coming_up_tv(m)),
    ));
    v
}

/// Patches `on-watchlist` on every Discover card model that may show this item (an
/// item can sit in Trending/Popular/… too), via `all_card_model_slots`.
fn patch_watchlist_on_all_models(g: &AppState, item_type: &str, tmdb_id: i64, on_watchlist: bool) {
    let id_str = tmdb_id.to_string();
    let mut patched = 0;
    for (model, _) in all_card_model_slots(g) {
        for i in 0..model.row_count() {
            if let Some(mut card) = model.row_data(i)
                && card.id.as_str() == id_str
                && card.item_type.as_str() == item_type
            {
                card.on_watchlist = on_watchlist;
                model.set_row_data(i, card);
                patched += 1;
            }
        }
    }
    debug!(
        "seerr: patch_watchlist_on_all_models tmdb={tmdb_id} item_type={item_type} on_watchlist={on_watchlist} -> patched {patched} card(s)"
    );
}

/// Removes the matching card from every Discover-visible model
/// (`all_card_model_slots`) — for `discover_toggle_blocklist`'s add path: a fresh
/// fetch already filters blocklisted items (`search_result_to_meta`), but cards in
/// models already on screen must go now. Each model is a `VecModel<CardItem>`, so a
/// downcast + `.remove()` notifies per row instead of rebuilding, with a
/// rebuild-and-reassign fallback (like blocklist.rs).
fn remove_card_from_all_models(g: &AppState, item_type: &str, tmdb_id: i64) {
    let id_str = tmdb_id.to_string();
    let mut removed = 0;
    for (model, set) in all_card_model_slots(g) {
        let hit: Vec<usize> = (0..model.row_count())
            .filter(|&i| {
                model
                    .row_data(i)
                    .is_some_and(|c| c.id.as_str() == id_str && c.item_type.as_str() == item_type)
            })
            .collect();
        if hit.is_empty() {
            continue;
        }
        if let Some(vm) = model.as_any().downcast_ref::<VecModel<CardItem>>() {
            for &i in hit.iter().rev() {
                vm.remove(i);
            }
            removed += hit.len();
        } else {
            // Fallback only — every model here is a VecModel, so this shouldn't run.
            let kept: Vec<CardItem> = (0..model.row_count())
                .filter(|i| !hit.contains(i))
                .filter_map(|i| model.row_data(i))
                .collect();
            removed += hit.len();
            set(g, ModelRc::new(VecModel::from(kept)));
        }
    }
    debug!(
        "seerr: remove_card_from_all_models tmdb={tmdb_id} item_type={item_type} -> removed {removed} card(s)"
    );
}
