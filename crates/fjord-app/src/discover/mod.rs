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
//   is_401 / handle_seerr_error 401 (session-auth only) resets the connection via
//                              seerr_auth::clear_connection + toasts "reconnect in
//                              Settings"; any other error just toasts
//   ── Keyboard-navigation fixes (2026-07-18, planned via /plan after 5 parallel
//      investigation agents traced every Seerr keyboard-dispatch path — see
//      CLAUDE.md's Seerr integration section) ──
//   KnownRequest/known_requests_from_row/patch_known_request_state  a request's
//                              (request_id, pending, mine), built from the Requested row's
//                              own already-fetched RequestEntry list (no new network call)
//                              and cached in FjordState.discover_known_requests, keyed
//                              (item_type, tmdb_id); consulted to patch search-grid and
//                              non-Requested-landing-row DiscoverCardMetas, which never
//                              carried real request state before this — their context menu
//                              offered "Request" instead of "Edit/Cancel/View Request" for
//                              an already-requested item (real bug)
//   patch_discover_card_request_state  request_id/pending/mine counterpart to
//                              patch_discover_card_availability, patches a live
//                              discover-results row in place — used by submit_request's
//                              success handler so a freshly-submitted card is correct
//                              immediately, not just after the next Requested-row refresh
//   refresh_seerr_admin_status  re-fetches just GET /auth/me's permission bit (not the
//                              heavier region/language/settings fetch spawn_seerr_settings_fetch
//                              also does) on Discover-tab arrival, rate-limited to once per
//                              SEERR_ADMIN_REFRESH_COOLDOWN (60s, FjordState.seerr_admin_last_refresh)
//                              rather than a fetched-once flag — catches a server-side permission
//                              change mid-session without a reconnect, while a real HTPC hitch
//                              (2026-07-31: rapid sidebar cycling fired this on every single
//                              pass through nav==6, piling up concurrent GET /auth/me calls)
//                              is now a no-op within the cooldown window
//   ── Watchlist + Release Calendar (2026-07-18, planned via /plan, 2 rounds of
//      AskUserQuestion + an independent Plan-agent review — see CLAUDE.md's
//      Seerr integration section) ──
//   patch_watchlist_state       CardItem.on-watchlist counterpart to
//                              patch_known_request_state — consults
//                              FjordState.discover_watchlist_ids, patched onto
//                              search/landing DiscoverCardMetas alongside the request-state patch
//   resolve_discover_region     GET-once-per-connection resolver for the (distinct from
//                              streamingRegion) discoverRegion user setting, mirrors
//                              resolve_streaming_region's exact shape, cached in
//                              FjordState.seerr_discover_region
//   CalendarEntry/CalendarEntryKind  date/tmdb_id/item_type/title/poster_path/kind/
//                              episode_label — poster_path added 2026-07-19 (user request,
//                              "it hust dosent have posters" — reverses the original
//                              deliberately-text-only design)
//   ── Deep Seerr integration into existing native screens (2026-07-29) ──────
//   TMDB_POSTER_BASE/fetch_tmdb_image  bumped pub(crate) — reused directly by
//                              series.rs's Missing Seasons poster fetch instead of duplicating it
//   build_person_credit_metas  CombinedCredits (cast+crew, deduped by id+media_type) ->
//                              (DiscoverCardMeta, poster_path) pairs — Person "Other Work" row
//   resolve_and_fetch_discovery_row  shared pipeline for all 4 new rows this pass: drops anything
//                              that resolves to a local item (find_local_item) — the literal
//                              "discovery = not owned anywhere" rule (user's own words, confirmed
//                              via AskUserQuestion) — caps the result, patches request/watchlist
//                              state from the existing caches, fetches posters (bounded
//                              concurrency). Returns plain Send-safe pairs; discover_cards_from
//                              (below) builds the actual CardItems inside invoke_from_event_loop
//   discover_cards_from        UI-thread-only: DiscoverCardMeta+poster pairs -> Vec<CardItem>
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
        // Blocklisted split into its own arm, 2026-08-06 (Seerr Blocklist
        // support) — previously silently mapped to "" alongside Unknown/
        // Deleted, so a blocklisted item's card showed no indicator at all
        // and (via tier_status_label, see its own doc comment) its Request
        // button incorrectly still showed. This is also the ONLY per-card
        // signal Blocklist needs — unlike Watchlist (a genuinely
        // independent boolean axis), Blocklisted is just another value of
        // this same mutually-exclusive status field, so no new CardItem
        // field/id-set was needed for this feature.
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
    // Requested row only (2026-07-18) — false/false/false on every other
    // card (search results, landing rows), matching `availability`'s own
    // "only meaningful for Requested" scoping. See CardItem's own doc
    // comment (theme.slint) for what these drive.
    requested_4k: bool,
    other_tier_available: bool,
    other_tier_requested: bool,
    // Requested row only (2026-07-18) — the Seerr MediaRequest's own id
    // (distinct from `id` above, which is the tmdb id); "" everywhere else.
    // Drives the Discover context menu's Edit/Cancel/Approve/Decline rows.
    request_id: String,
    request_pending: bool,
    request_mine: bool,
    // Discover filters (2026-07-18) — NOT surfaced on CardItem at all (never
    // displayed); used purely by apply_search_filters' client-side genre/
    // rating filtering of already-fetched search results, kept alongside
    // the full unfiltered fetch history in FjordState.discover_search_metas.
    // Empty/0.0 on landing-row/Requested-row cards, which never go through
    // this filtering path.
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
    // Snapshotted from FjordState.discover_watchlist_ids at candidate-
    // selection time (2026-07-19, real bug fix — see build_calendar_entries'
    // own doc comment) — needed so push_coming_up_row's CardItems don't
    // silently default on-watchlist to false for an item that's on the
    // Coming Up row PRECISELY because it was just watchlisted.
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

/// Patches `request_id`/`request_pending`/`request_mine` onto a freshly-built
/// `DiscoverCardMeta` (search result or non-Requested landing-row card) from
/// the known-requests cache, when a match exists — real bug fixed 2026-07-18,
/// see `FjordState.discover_known_requests`'s own doc comment for the full
/// story. A no-op (leaves the meta's zeroed defaults) when the item isn't in
/// the cache, same as before this fix existed.
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
        // Filtered out at the source, 2026-08-06 — real bug, live-reported:
        // blocklisting an item only ever patched its pill in place, it never
        // actually left Discover, which defeats the entire stated purpose of
        // the feature ("for items they dont want to show up"). Every landing
        // row, search, and filtered-browse fetch routes through this one
        // function (directly or via `build_filtered_metas`), so filtering
        // here is the single choke point rather than a special case repeated
        // at each of the ~9 call sites — mirrors Seerr's own web frontend,
        // which does the identical filter in `MediaSlider` for any account
        // without VIEW_BLOCKLIST/MANAGE_BLOCKLIST permission; Fjord has no
        // "show blocklisted with a badge" mode of its own, so it always
        // filters, regardless of the connected account's permissions.
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

/// GET /person/{id}/combined_credits → `(DiscoverCardMeta, Option<String>)`
/// pairs, same shape as `build_filtered_metas` — Person screen's "Other
/// Work" row (2026-07-29, Deep Seerr integration). Cast and crew are merged
/// and deduped by `(id, media_type)` since a person can be both cast and
/// crew on the same title (e.g. an actor-director). `media_info` is
/// deliberately not read here — confirmed this endpoint's relation join is
/// watchlist-only (see `PersonCreditCast`'s own doc comment), so
/// `availability` starts empty and is patched in afterward by
/// `resolve_and_fetch_discovery_row`, same as every other row built here.
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

/// Shared pipeline for the 4 new discovery-style rows added 2026-07-29 (Deep
/// Seerr integration: Person Other Work, Detail/Series Recommended,
/// Collection Missing Items). Takes `(meta, poster_path)` pairs already
/// built by `build_filtered_metas`/`build_person_credit_metas`), and:
/// (1) drops anything that resolves to a local Jellyfin item via
/// `find_local_item` — the literal implementation of "discovery = not owned
/// anywhere" (user's own words, confirmed via `AskUserQuestion`); (2) caps
/// the result (`cap`, matching this codebase's established `.take(20)`
/// precedent for similarly-sized supplementary rows); (3) patches
/// request/watchlist state from the existing caches, same as every other
/// Discover-sourced row; (4) fetches posters, bounded concurrency. Returns
/// plain Send-safe pairs — callers build the final `Vec<CardItem>` via
/// `discover_cards_from` themselves, inside their own
/// `invoke_from_event_loop` (this function never touches `AppState`/
/// `CardItem`, matching the two-phase discipline this codebase learned the
/// hard way from `push_coming_up_row`'s bug).
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

/// Session-auth 401 means the cookie expired server-side — reset the
/// connection so Settings shows "Not connected" and the user can reconnect,
/// rather than every subsequent call failing silently. API-key auth doesn't
/// expire, so a 401 there means a revoked/invalid key — surfaced as a plain
/// error instead (reconnecting wouldn't help without a new key anyway).
/// `pub(crate)` since 2026-08-06 (Seerr Blocklist support) — `blocklist.rs`/
/// `collection.rs`'s own blocklist error paths reuse it rather than
/// duplicating the 401-reconnect logic.
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

/// Resolves and caches (`FjordState.seerr_streaming_region`) which
/// `watch_providers` region entry to display — the CONNECTED user's own
/// `streamingRegion` preference (`GET /auth/me` then `GET /user/{id}/
/// settings/main` — corrected from an earlier version of this function that
/// read the server-wide admin default at `/settings/public` instead, which
/// doesn't reflect a per-user override and, per Seerr's own frontend source,
/// isn't even what Seerr's own UI falls back to), falling back to `"US"`
/// when unset (matching Seerr's own frontend's identical fallback, found
/// live in `src/components/Settings/SettingsMain/index.tsx`). Also the read
/// side of the Settings -> Integrations -> Streaming Region picker
/// (`main.rs`'s `on_streaming_region_selected`), which updates this same
/// cache on a successful write so "Currently Streaming On" picks up a
/// change immediately, no reconnect needed. A failed fetch also caches the
/// `"US"` fallback rather than retrying on every subsequent item open —
/// this call is cheap and reliable enough, relative to everything else
/// already required for Discover to work at all, that treating a failure
/// differently from "not configured" isn't worth the extra state.
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

/// Mirrors `resolve_streaming_region` exactly, but for the DIFFERENT
/// `discoverRegion` user setting Seerr's own frontend uses specifically for
/// release-date display (`src/components/MovieDetails/index.tsx`) — not
/// the same region as "Currently Streaming On", confirmed from Seerr's real
/// source (Watchlist + Release Calendar, 2026-07-18).
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

/// Re-fetches just the connected account's own id + `MANAGE_REQUESTS`/ADMIN
/// permission bit (`GET /auth/me`, the same call `spawn_seerr_settings_fetch`
/// makes at startup/connect, but not the heavier region/language/settings
/// fetch that goes with it there) — called on every Discover-tab arrival,
/// unguarded by a "fetched once" flag, unlike `ensure_discover_filter_options`.
/// Real bug fixed 2026-07-18: `seerr-is-admin` was previously only ever set
/// once per connection, so a server-side permission change mid-session never
/// reflected in the Discover context menu's Approve/Decline gating without a
/// reconnect. Non-blocking and best-effort — the menu opens instantly with
/// whatever's currently cached; a failed fetch here just leaves that value
/// unchanged rather than erroring.
// Rate-limited to at most once every 60s per connection — this used to fire
// an unconditional `GET /auth/me` on every single arrival at the Discover
// sidebar tab (deliberate at the time: no once-per-session guard, so a
// server-side permission change mid-session would be picked up on the very
// next visit). Live-reported HTPC hitch, 2026-07-31: a user rapidly cycling
// the sidebar with a held arrow key passes through nav==6 many times a
// minute — each pass fired its own real network round trip, and a burst of
// these completing out of order (worse under a lower-end machine's higher
// latency/thinner thread-pool headroom) queued up `invoke_from_event_loop`
// closures that visibly collided with the next keypress, the same mechanism
// already documented for the Browse All rebuild hitch (browse.rs). A 60s
// cooldown keeps the "catch a mid-session permission change" intent (still
// checked on the next genuine visit after the cooldown) while making a rapid
// pass-through a no-op instead of a fresh request every time.
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

/// Patches `request_id`/`request_pending`/`request_mine` onto whichever
/// search-grid card matches `(media_type, tmdb_id)`, if visible — the
/// `discover-results` counterpart to `patch_discover_card_availability`
/// above, for the 3 fields that one doesn't touch. Real bug fixed
/// 2026-07-18: submitting a request from the search grid left that same
/// card's context menu still offering "Request" until the next full
/// landing-row refresh, since only `availability` was ever patched here.
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

/// Every AppState model that can hold a Discover-sourced `CardItem` — the
/// flat search/filtered-browse grid, all 9 `landing_row_get`/`_set` rows,
/// AND 5 more that `landing_row_get` does NOT cover: the Movies/TV-specific
/// split of the Watchlist row (`discover-watchlist-movies`/`-tv`, separate
/// models from the "mixed" one landing row 8 already reaches) and the
/// Home/TV/Movies dashboard split of the Coming Up row
/// (`discover-coming-up-mixed`/`-movies`/`-tv`, separate from landing row
/// 7's own single Discover-screen instance). Real gap found 2026-08-06
/// tracing what happens when a watchlisted item gets blocklisted: both
/// `patch_watchlist_on_all_models` and (the then-new) `remove_card_from_
/// all_models` only ever walked `landing_row_get`'s 9, so a card patched/
/// removed on the Discover screen stayed fully visible (and, for a
/// blocklisted item, requestable) on the Movies/TV dashboard's own
/// Watchlist row until some unrelated refresh silently caught up. Callers
/// that only need to read+patch in place (not reassign) can ignore the
/// second tuple element.
type CardModelSlot = (ModelRc<CardItem>, Box<dyn Fn(&AppState, ModelRc<CardItem>)>);

fn all_card_model_slots(g: &AppState) -> Vec<CardModelSlot> {
    let mut v: Vec<CardModelSlot> = vec![(
        g.get_discover_results(),
        Box::new(|g: &AppState, m| g.set_discover_results(m)),
    )];
    // Derived from landing_row_lens's own array length rather than a bare
    // literal repeated here — this exact "hardcoded row count drifts out of
    // sync with the real row count" gap was caught by an independent plan
    // review when the Watchlist row (8) was added, 2026-07-20.
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

/// Patches `on-watchlist` in place on every Discover card model that might
/// be showing this item — a watchlisted item can legitimately appear in
/// Trending/Popular/Upcoming/etc, not just Requested — matching
/// `discover_request_action`'s own "patch every model, don't just pick one"
/// shape. Watchlist + Release Calendar, 2026-07-18; widened to the full
/// `all_card_model_slots` list (was missing 5 of them) 2026-08-06.
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

/// Removes the matching card from every Discover-visible model (the full
/// `all_card_model_slots` list) — used by `discover_toggle_blocklist`'s
/// adding path. Blocklisting means "don't show this in Discover" (see
/// `search_result_to_meta`'s own doc comment for the full story: a fresh
/// fetch already filters a blocklisted item out, but a card blocklisted
/// from an already-open screen — the flat grid, or RequestDetailScreen
/// opened from one of these rows — is still sitting in an already-built
/// model and needs to be pulled out immediately rather than left showing a
/// "Blocklisted" pill). Each model here is always constructed as a
/// `VecModel<CardItem>` (every setter in `all_card_model_slots` wraps one),
/// so downcasting back to it and calling `.remove()` fires a real per-row
/// removal notification rather than rebuilding the whole model — same
/// reasoning as `blocklist.rs`'s own remove-row idiom, with the identical
/// defensive rebuild-and-reassign fallback in case that assumption ever
/// stops holding. 2026-08-06, Seerr Blocklist support.
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
            // Defensive fallback — every model here is always constructed as
            // a VecModel elsewhere in this file, so this should never
            // actually trigger (same "should never trigger" idiom as
            // blocklist.rs's own remove-row fallback).
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
