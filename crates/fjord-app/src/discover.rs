// ── fjord-app · discover.rs ──────────────────────────────────────────────────
//   wire_discover              registers all Discover/RequestDetail AppState callbacks
//                              (search append/backspace/clear, load-more, open-discover-item,
//                              request-detail-toggle-season/-tag, request-detail-request,
//                              open-request-options, request-detail-set-quality); on first
//                              nav arrival also proactively refreshes all_movies (metadata
//                              only, crate::spawn_movies_list_fetch(..., with_posters=false))
//                              so find_local_item's ProviderIds match works on the first
//                              Discover visit, not just after the Movies grid has been
//                              opened this session (all_series has no such gap — the
//                              startup auto-login path already refreshes it unconditionally)
//   spawn_discover_search      debounced (300ms) + generation-guarded search dispatch (page 1);
//                              text-only cards pushed immediately, posters patched in
//                              as they arrive (bounded concurrency, TMDB CDN, own disk cache);
//                              records page/total_pages in FjordState for spawn_discover_search_more
//   spawn_discover_search_more  fetches+appends the next results page — triggered by
//                              handle_key's Down-at-last-row via the discover-load-more
//                              callback (Seerr/TMDB search commonly has far more pages than
//                              the single page v1 ever fetched, capping results well below
//                              what Seerr's own web UI shows for the same query); no-ops
//                              quietly with no next page / a fetch already in flight
//   fetch_and_patch_posters    bounded-concurrency TMDB poster fetch + in-place model patch,
//                              shared by both search functions above (idx is pre-offset by
//                              the caller for the append case)
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
//   request_entry/RequestEntry  one kept request's raw fields for the "Requested" landing row —
//                              picks status vs status4k based on r.is4k (real bug fixed
//                              2026-07-18: fetch_requested_row's own availability badge had the
//                              identical tier-blindness bug as requested_not_available in
//                              fjord-seerr, just manifesting as a wrong badge instead of a wrong
//                              filter result); falls back to "requested" when the tier's own
//                              status is Unknown rather than leaving the main pill blank (real
//                              bug, live-reported 2026-07-18 — an active 4K request can sit at
//                              status4k==Unknown indefinitely); computes other_tier_available
//                              (OTHER tier already available, "Available in 2K/4K" pill) and
//                              other_tier_requested (OTHER tier ALSO actively requested but not
//                              yet available, "Also requested in 2K/4K" pill — via the sibling
//                              dual_tier_tmdb_ids set, since one MediaRequest has no visibility
//                              into whether a request for the other tier exists)
//   dual_tier_tmdb_ids          tmdb ids with an active, not-yet-available request in BOTH
//                              tiers within one requested_not_available result list
//   handle_key                  Discover screen: replicates dispatch_dashboard's
//                              focused-section sidebar/content contract itself (Discover has
//                              its own AppMode, not Dashboard, so doesn't get this for free) —
//                              fs<0 = sidebar (Up/Down cycle tabs, Right enters); fs>=0 = grid,
//                              2D nav mirrors LibraryGrid's math (AppState.library-cols),
//                              Left-at-col-0/Back return to the sidebar. Search-field typing is
//                              a separate raw-key pre-dispatch in keys.rs (handle_discover_search),
//                              mirroring browse.rs's handle_browse_search. Action::OpenContextMenu
//                              (C key) in both the grid and handle_key_landing invokes
//                              open-context-menu-discover(card) with the focused card (2026-07-18)
//   open_discover_item_ex/PostOpenAction/open_request_options_modal  open_discover_item is now a
//                              thin wrapper around this with PostOpenAction::None; ::OpenRequestOptions
//                              (Discover context menu's "Request" row) opens the modal the instant the
//                              fetch lands; ::EditRequest(id) additionally fetches GET /request/{id}
//                              fresh (SeerrClient::get_request) and pre-selects its profile/tags/seasons,
//                              setting request-options-editing so the modal hides Quality and Confirm
//                              PUTs via submit_edit_request instead of POSTing via submit_request
//                              (2026-07-18)
//   existing_zones/handle_key_request_detail  back -> button row (Request +
//                              Trailer, independent of each other — Left/Right toggle between
//                              them, clamped to whichever actually exists) -> storyline
//                              (collapsible overview) -> cast row — Up/Down step to the
//                              nearest zone that exists for this item. Request button opens
//                              the Request Options modal rather than exposing 4K/tags/seasons
//                              inline (see below); Trailer button fires play-trailer() (Watch
//                              Trailer — Discover only, see CLAUDE.md's Seerr integration section).
//   find_trailer_url            MovieDetails/TvDetails.relatedVideos -> best trailer URL
//                              (prefers Trailer, falls back to Teaser, else None)
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
    show_toast, spawn_movies_list_fetch, AppState, CardItem, CastMember, MainWindow, ProfileItem, SeasonItem,
    StreamingProvider, TagItem,
};

const TMDB_POSTER_BASE: &str = "https://image.tmdb.org/t/p/w500";
const TMDB_BACKDROP_BASE: &str = "https://image.tmdb.org/t/p/w1280";
const TMDB_LOGO_BASE: &str = "https://image.tmdb.org/t/p/w92"; // small, icon-sized — provider chips

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
        requested_4k: false,
        other_tier_available: false,
        other_tier_requested: false,
        request_id: String::new(),
        request_pending: false,
        request_mine: false,
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

/// Rough target row count for "the grid looks full without scrolling" —
/// deliberately a fixed estimate, not a pixel-exact viewport-height
/// computation (that would need a new geometry property pushed from
/// `MainWindow::sync_layout()`, mirroring `dash-cw`/`dash-ch`/`library-cols`,
/// for comparatively little payoff over a conservative constant). Real UX
/// gap, live-reported: "search should fill the screen so you don't need to
/// go to the end of a row to get new items" — a single TMDB search page
/// (~20 raw results, fewer once `person` is filtered out) often doesn't
/// fill even a modest window, so the user hit the Down-triggered load-more
/// on almost every search before this existed.
const DISCOVER_AUTOFILL_ROWS: i32 = 6;

/// Called from both search commit closures (page 1 and each appended page)
/// on the UI thread, right after `discover-results` is set. If the grid
/// still has fewer rows than `DISCOVER_AUTOFILL_ROWS` would need, reuses the
/// exact same `discover-load-more` callback the Down-at-last-row keyboard
/// path already fires — `spawn_discover_search_more`'s own guards (no next
/// page / already loading) make this safe to call unconditionally here;
/// each successful page's own commit re-checks and chains again, so this
/// naturally stops once the grid is full or the search runs out of pages.
fn maybe_autofill_grid(g: &AppState) {
    let cols = g.get_library_cols().max(1);
    let target = cols * DISCOVER_AUTOFILL_ROWS;
    if (g.get_discover_results().row_count() as i32) < target {
        g.invoke_discover_load_more();
    }
}

/// Bounded-concurrency TMDB poster fetch + in-place patch into
/// `discover-results`, shared by `spawn_discover_search` (page 1, `idx`
/// is the row's own position) and `spawn_discover_search_more` (page N,
/// `idx` is already offset by the row count at the time of the fetch —
/// see that function's own comment for why that offset has to be captured
/// synchronously before the fetch starts rather than recomputed here).
async fn fetch_and_patch_posters(
    ww: Weak<MainWindow>,
    gen: Arc<AtomicU64>,
    my_gen: u64,
    poster_jobs: Vec<(usize, String, String, String)>,
) {
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
}

pub(crate) fn spawn_discover_search(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    query: String,
    gen: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;

    if query.trim().is_empty() {
        {
            let mut s = state.lock().unwrap();
            s.discover_search_page = 0;
            s.discover_search_total_pages = 0;
            s.discover_search_loading_more = false;
        }
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

        // Search commonly has far more than one page's worth of results
        // (a common word can run into the hundreds) — page 1 alone is what
        // used to cap Fjord's result count well below what Seerr's own web
        // UI shows for the same query, real bug, live-reported. This state
        // is what `spawn_discover_search_more` (below) reads to fetch
        // subsequent pages, triggered as the user's keyboard nav reaches
        // the last row of the grid.
        {
            let mut s = state.lock().unwrap();
            s.discover_search_page = 1;
            s.discover_search_total_pages = response.total_pages;
            s.discover_search_loading_more = false;
        }

        let results = response.results;
        let metas: Vec<DiscoverCardMeta> = results.iter().filter_map(search_result_to_meta).collect();
        debug!(
            "seerr: search {query:?} page 1/{} -> {} raw result(s), {} movie/tv card(s)",
            response.total_pages,
            results.len(),
            metas.len()
        );

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
                maybe_autofill_grid(&g);
            }
        });

        fetch_and_patch_posters(ww, gen, my_gen, poster_jobs).await;
    });
}

/// Fetches the next page of the *current* search and appends it to
/// `discover-results` (Seerr/TMDB search commonly has far more pages than
/// Fjord originally ever fetched — see `spawn_discover_search`'s own
/// comment). Triggered by `discover::handle_key`'s Down-at-last-row branch
/// via the `discover-load-more` AppState callback: `keys.rs::handle_key`
/// doesn't hold `state`/`rt` in the per-mode match arms, so the fetch has
/// to be dispatched from wherever this callback is registered instead (see
/// `wire_discover`) — the same reason several other keyboard-triggered
/// async actions in this codebase (e.g. context menu's queue mutations) go
/// through an `AppState` callback rather than being threaded through
/// `keys.rs` directly. No-ops quietly (not an error, no toast) when
/// there's no next page, a fetch is already in flight, or no search has
/// landed yet — every one of those is an entirely normal state to be in on
/// any given Down press, not a failure.
pub(crate) fn spawn_discover_search_more(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    query: String,
    gen: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = gen.load(Ordering::SeqCst);
    let (client, next_page) = {
        let mut s = state.lock().unwrap();
        if s.discover_search_loading_more { return; }
        if s.discover_search_page == 0 || s.discover_search_page >= s.discover_search_total_pages { return; }
        let Some(client) = s.seerr_client.clone() else { return };
        s.discover_search_loading_more = true;
        (client, s.discover_search_page + 1)
    };
    let is_session_auth = client.is_session_auth();

    // Append offset: the row count *right now*, read synchronously on the
    // calling (UI event loop) thread — `discover-results` can only be
    // touched from there. Safe against a race with a fresh search landing
    // first: that path bumps `gen` synchronously before its own debounce
    // sleep even starts, so this fetch's `gen` check below (after the
    // network round trip) will already see the mismatch and bail before
    // ever using this offset.
    let offset = ww.upgrade().map(|w| AppState::get(&w).get_discover_results().row_count()).unwrap_or(0);

    let state2 = Arc::clone(&state);
    rt.spawn(async move {
        debug!("seerr: loading more results for {query:?}, page {next_page}");
        let response = match client.search(&query, next_page).await {
            Ok(r) => r,
            Err(e) => {
                state2.lock().unwrap().discover_search_loading_more = false;
                handle_seerr_error(&state2, &ww, is_session_auth, "Seerr search failed", &e);
                return;
            }
        };
        if gen.load(Ordering::SeqCst) != my_gen {
            state2.lock().unwrap().discover_search_loading_more = false;
            return; // a newer search superseded this one before it landed
        }
        {
            let mut s = state2.lock().unwrap();
            s.discover_search_page = next_page;
            s.discover_search_total_pages = response.total_pages;
            s.discover_search_loading_more = false;
        }

        let results = response.results;
        let metas: Vec<DiscoverCardMeta> = results.iter().filter_map(search_result_to_meta).collect();
        debug!(
            "seerr: search {query:?} page {next_page}/{} -> {} raw result(s), {} card(s)",
            response.total_pages,
            results.len(),
            metas.len()
        );

        let poster_jobs: Vec<(usize, String, String, String)> = metas
            .iter()
            .enumerate()
            .zip(results.iter().filter(|r| r.media_type == "movie" || r.media_type == "tv"))
            .filter_map(|((i, meta), r)| {
                r.poster_path.clone().map(|p| (offset + i, meta.item_type.to_string(), meta.id.clone(), p))
            })
            .collect();

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_commit.upgrade() {
                let g = AppState::get(&w);
                let existing = g.get_discover_results();
                let mut all: Vec<CardItem> = (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();
                all.extend(metas.into_iter().map(DiscoverCardMeta::into_card_item));
                g.set_discover_results(ModelRc::new(VecModel::from(all)));
                maybe_autofill_grid(&g);
            }
        });

        fetch_and_patch_posters(ww, gen, my_gen, poster_jobs).await;
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

// Both take `&RequestEntry` (not its ~7 fields unpacked as loose scalars —
// clippy's too-many-arguments, and every one of these values already lives
// on `entry` at both call sites in `fetch_requested_row`) rather than the
// per-field signature these had before `request_id`/`request_pending`/
// `request_mine` were added.
fn movie_details_to_meta(tmdb_id: i64, d: &MovieDetails, entry: &RequestEntry) -> (DiscoverCardMeta, Option<String>) {
    let year = d.release_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let meta = DiscoverCardMeta {
        id: tmdb_id.to_string(),
        item_type: "DiscoverMovie",
        title: d.title.clone(),
        subtitle: year.to_string(),
        year: year.parse().unwrap_or(0),
        availability: entry.availability,
        requested_4k: entry.is4k,
        other_tier_available: entry.other_tier_available,
        other_tier_requested: entry.other_tier_requested,
        request_id: entry.request_id.to_string(),
        request_pending: entry.pending,
        request_mine: entry.mine,
    };
    (meta, d.poster_path.clone())
}

fn tv_details_to_meta(tmdb_id: i64, d: &TvDetails, entry: &RequestEntry) -> (DiscoverCardMeta, Option<String>) {
    let year = d.first_air_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let meta = DiscoverCardMeta {
        id: tmdb_id.to_string(),
        item_type: "DiscoverTv",
        title: d.name.clone(),
        subtitle: year.to_string(),
        year: year.parse().unwrap_or(0),
        availability: entry.availability,
        requested_4k: entry.is4k,
        other_tier_available: entry.other_tier_available,
        other_tier_requested: entry.other_tier_requested,
        request_id: entry.request_id.to_string(),
        request_pending: entry.pending,
        request_mine: entry.mine,
    };
    (meta, d.poster_path.clone())
}

type RequestedRowItem = (DiscoverCardMeta, Option<String>);

/// One kept request's raw fields, tagged with which endpoint its detail
/// fetch needs — `is4k`/`other_tier_available` are what let the card show
/// "4K Requested" plus a separate "Available in 2K" badge instead of just a
/// flat, tier-blind "Requested" (see `requested_not_available`'s own doc
/// comment in fjord-seerr for why `status`/`status4k` must be picked based
/// on which tier the request is actually for, not `status` unconditionally
/// — the identical bug, fixed here too since this row builds its badge
/// text independently of that filter). `request_id`/`pending`/`mine` feed
/// the Discover context menu's Edit/Cancel/Approve/Decline row set
/// (2026-07-18) — `pending`/`mine` are the request's own approval-workflow
/// state (`MediaRequest.status`/`requestedBy.id`), a different thing from
/// `availability` (media fulfillment status).
struct RequestEntry {
    media_type: &'static str,
    tmdb_id: i64,
    availability: &'static str,
    is4k: bool,
    other_tier_available: bool,
    other_tier_requested: bool,
    created_at: String,
    request_id: i64,
    pending: bool,
    mine: bool,
}

/// `dual_tier_tmdb_ids`: tmdb ids that have an active (still-kept, i.e.
/// not-yet-available) request for BOTH tiers within the same
/// `requested_not_available` result list — computed once per media type
/// in `fetch_requested_row` and passed in here, since a single
/// `MediaRequest` only ever describes its own tier and has no visibility
/// into whether a sibling request exists for the other one.
fn request_entry(
    media_type: &'static str, r: &fjord_seerr::MediaRequest, dual_tier_tmdb_ids: &std::collections::HashSet<i64>,
    my_user_id: Option<i64>,
) -> Option<RequestEntry> {
    let media = r.media.as_ref()?;
    let tmdb_id = media.tmdb_id?;
    let (requested_status, other_status) =
        if r.is4k { (media.status4k(), media.status()) } else { (media.status(), media.status4k()) };
    // A row reaching this function is, by construction, an active request
    // for this exact tier (requested_not_available's own filter guarantees
    // it) — but Seerr can still report that tier's own media status as
    // Unknown well after the request was created (confirmed live,
    // 2026-07-18: 3 of 49 real 4K requests on a real account had
    // status4k==Unknown despite a genuine MediaRequest existing — most
    // likely a TV show whose top-level status hasn't been recomputed from
    // its season-level state), which must not read as "no request" here
    // the way availability_tag's blank result correctly does for its
    // other caller (a plain, unrequested search result). Fall back to
    // "requested" rather than leaving the main pill blank on a card
    // that's only ever shown in this row because a request exists.
    let availability = match availability_tag(requested_status) {
        "" => "requested",
        tag => tag,
    };
    let other_tier_available = matches!(other_status, Some(MediaStatus::Available));
    // Missing my_user_id (spawn_seerr_settings_fetch hasn't resolved yet,
    // or /auth/me failed) defaults to "mine" — the common single-user setup
    // this is built for is unaffected either way, and the permissive
    // default keeps Edit/Cancel visible rather than silently hiding them;
    // a genuine ownership mismatch just 403s server-side, same as any
    // other stale-permission action in this app.
    let mine = my_user_id.zip(r.requested_by.as_ref().map(|rb| rb.id)).map(|(mine, theirs)| mine == theirs).unwrap_or(true);
    debug!(
        "seerr: request_entry {media_type} tmdb={tmdb_id} request_id={} is4k={} status={} pending={} \
         requested_by={:?} my_user_id={my_user_id:?} mine={mine}",
        r.id, r.is4k, r.status, r.is_pending(), r.requested_by.as_ref().map(|rb| rb.id),
    );
    Some(RequestEntry {
        media_type,
        tmdb_id,
        availability,
        is4k: r.is4k,
        other_tier_available,
        // Available takes priority over merely-requested when somehow
        // both would be true (shouldn't happen — status transitions to
        // Available once, not back — but Available winning is the more
        // useful thing to show either way).
        other_tier_requested: !other_tier_available && dual_tier_tmdb_ids.contains(&tmdb_id),
        created_at: r.created_at.clone().unwrap_or_default(),
        request_id: r.id,
        pending: r.is_pending(),
        mine,
    })
}

/// tmdb ids present with BOTH `is4k=false` and `is4k=true` requests in the
/// same (already not-yet-available-filtered) list — i.e. both tiers were
/// requested and neither has been fulfilled yet. Drives the "Also
/// requested in 2K/4K" badge; see `request_entry`'s own doc comment.
fn dual_tier_tmdb_ids(requests: &[fjord_seerr::MediaRequest]) -> std::collections::HashSet<i64> {
    use std::collections::HashSet;
    let mut has_2k: HashSet<i64> = HashSet::new();
    let mut has_4k: HashSet<i64> = HashSet::new();
    for r in requests {
        let Some(tmdb_id) = r.media.as_ref().and_then(|m| m.tmdb_id) else { continue };
        if r.is4k {
            has_4k.insert(tmdb_id);
        } else {
            has_2k.insert(tmdb_id);
        }
    }
    has_2k.intersection(&has_4k).copied().collect()
}

/// Builds the Discover "Requested" landing row (still-pending/processing
/// requests). `GET /request` only carries a tmdbId per item — no title or
/// poster (confirmed from Seerr's real `MediaInfo` schema) — so each kept
/// request needs its own detail fetch, bounded concurrency, same shape as
/// the cast-portrait/season-poster fetches elsewhere in this app. Returns
/// `(meta, poster_path)` pairs so the caller can feed both the row's text
/// content and its poster-fetch jobs, mirroring the other 5 rows exactly.
/// Best-effort throughout: any failure just yields an empty/shorter row.
async fn fetch_requested_row(client: &fjord_seerr::SeerrClient, my_user_id: Option<i64>) -> Vec<RequestedRowItem> {
    let (movies, tv) = match client.requested_not_available(15).await {
        Ok(v) => v,
        Err(e) => {
            debug!("seerr: couldn't fetch requested-not-available list: {e:#}");
            return Vec::new();
        }
    };
    let dual_movie_ids = dual_tier_tmdb_ids(&movies);
    let dual_tv_ids = dual_tier_tmdb_ids(&tv);
    let mut entries: Vec<RequestEntry> = movies
        .iter()
        .filter_map(|r| request_entry("movie", r, &dual_movie_ids, my_user_id))
        .chain(tv.iter().filter_map(|r| request_entry("tv", r, &dual_tv_ids, my_user_id)))
        .collect();
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at)); // newest requested first
    entries.truncate(20);

    let n = entries.len();
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set: tokio::task::JoinSet<(usize, Option<RequestedRowItem>)> = tokio::task::JoinSet::new();
    for (idx, entry) in entries.into_iter().enumerate() {
        let client = client.clone();
        let sem = Arc::clone(&sem);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let item = if entry.media_type == "movie" {
                client.get_movie(entry.tmdb_id).await.ok().map(|d| movie_details_to_meta(entry.tmdb_id, &d, &entry))
            } else {
                client.get_tv(entry.tmdb_id).await.ok().map(|d| tv_details_to_meta(entry.tmdb_id, &d, &entry))
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

/// Re-fetches just the Requested landing row and replaces `discover-requested`
/// wholesale — called right after a new request is submitted (both the
/// ordinary Request button and the Discover context menu's Request/Edit
/// actions). Real bug, live-reported 2026-07-18: "if a request an item the
/// request row did not update even thou it was added to the requests in the
/// webinterface" — `submit_request`'s own success handler only patches the
/// availability badge on whichever card is ALREADY visible somewhere
/// (`patch_discover_card_availability`); a freshly-created request has never
/// been in `discover-requested` before that moment, so there was nothing
/// there for it to patch, and the row otherwise only refreshes once per
/// session (`ensure_discover_landing`'s own guard). A full re-fetch of this
/// one row (not all 6 — Trending/Popular/Upcoming didn't change) is cheap
/// enough for an infrequent action like submitting a request.
fn refresh_requested_row(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let (client, my_user_id) = {
        let s = state.lock().unwrap();
        let Some(client) = s.seerr_client.clone() else { return };
        (client, s.seerr_user_id)
    };
    rt.spawn(async move {
        let requested = fetch_requested_row(&client, my_user_id).await;
        let poster_jobs: Vec<(usize, String, String, String)> = requested
            .iter()
            .enumerate()
            .filter_map(|(idx, (m, poster_path))| poster_path.clone().map(|p| (idx, m.item_type.to_string(), m.id.clone(), p)))
            .collect();
        debug!("seerr: refresh_requested_row -> {} card(s)", requested.len());
        // metas (not CardItem) crosses the thread boundary — CardItem carries
        // a slint::Image field and is `!Send` regardless of whether it's
        // populated (same reason ensure_discover_landing's own commit closure
        // builds CardItem only inside invoke_from_event_loop, never before).
        let metas: Vec<DiscoverCardMeta> = requested.into_iter().map(|(m, _)| m).collect();
        let ww2 = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww2.upgrade() {
                let cards: Vec<CardItem> = metas.into_iter().map(DiscoverCardMeta::into_card_item).collect();
                AppState::get(&w).set_discover_requested(ModelRc::new(VecModel::from(cards)));
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
            let ww2 = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww2.upgrade() else { return };
                let g = AppState::get(&w);
                let model = g.get_discover_requested();
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
    let (client, my_user_id) = {
        let mut s = state.lock().unwrap();
        if s.discover_landing_fetched {
            return;
        }
        let Some(client) = s.seerr_client.clone() else { return };
        s.discover_landing_fetched = true;
        (client, s.seerr_user_id)
    };
    let is_session_auth = client.is_session_auth();

    rt.spawn(async move {
        let (r_trending, r_movies, r_tv, r_movies_up, r_tv_up, requested) = tokio::join!(
            client.discover_trending(1),
            client.discover_movies(1),
            client.discover_tv(1),
            client.discover_movies_upcoming(1),
            client.discover_tv_upcoming(1),
            fetch_requested_row(&client, my_user_id),
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

/// "2026-07-14" -> "July 14, 2026"; empty/unparseable input -> "". A hand-
/// rolled month-name table would duplicate what `chrono` (already a
/// workspace dependency, used elsewhere for wall-clock formatting) does
/// correctly for free.
fn format_date_pretty(iso: &str) -> String {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d").map(|d| d.format("%B %-d, %Y").to_string()).unwrap_or_default()
}

/// TMDB's `original_language` is an ISO 639-1 code ("en", "ja", ...) with no
/// display name in the response itself. Small hardcoded table, same idiom
/// (and same language set) as `playback.rs::sub_lang_code`'s reverse
/// mapping — a full ISO-639 name table would be a lot of data for a
/// cosmetic label; anything outside this common set just shows its raw code
/// uppercased rather than silently blank.
fn language_display_name(code: &str) -> String {
    match code {
        "en" => "English".into(), "de" => "German".into(), "fr" => "French".into(),
        "ja" => "Japanese".into(), "es" => "Spanish".into(), "it" => "Italian".into(),
        "pt" => "Portuguese".into(), "ru" => "Russian".into(), "ko" => "Korean".into(),
        "zh" => "Chinese".into(), "nl" => "Dutch".into(), "sv" => "Swedish".into(),
        "pl" => "Polish".into(), "cs" => "Czech".into(), "ar" => "Arabic".into(),
        "tr" => "Turkish".into(), "fi" => "Finnish".into(), "da" => "Danish".into(),
        "no" => "Norwegian".into(),
        "" => String::new(),
        other => other.to_uppercase(),
    }
}

/// ISO 3166-1 alpha-2 ("US", "GB") -> flag emoji, built from the two
/// Unicode Regional Indicator Symbols rather than a lookup table — every
/// valid 2-letter country code maps this way, no data to maintain. Falls
/// back to the bare code for anything that isn't exactly 2 ASCII letters
/// (shouldn't happen for real TMDB data, but this is display-only content,
/// not worth a hard failure over).
fn country_flag_emoji(iso: &str) -> String {
    let upper = iso.to_uppercase();
    let chars: Vec<char> = upper.chars().collect();
    if chars.len() == 2 && chars.iter().all(|c| c.is_ascii_uppercase()) {
        let regional = |c: char| char::from_u32(0x1F1E6 + (c as u32 - 'A' as u32));
        if let (Some(a), Some(b)) = (regional(chars[0]), regional(chars[1])) {
            return format!("{a}{b}");
        }
    }
    iso.to_string()
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

/// (provider id, name, TMDB logo path) — same Send-safe-tuple reasoning as
/// `SeasonRow`/`CreditRow`; `StreamingProvider` carries a `slint::Image`.
type ProviderRow = (i64, String, Option<String>);

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
async fn resolve_streaming_region(client: &fjord_seerr::SeerrClient, state: &Arc<Mutex<FjordState>>) -> String {
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

/// Picks the `flatrate` (subscription-included) providers for one region
/// out of `MovieDetails`/`TvDetails.watch_providers` — an empty result just
/// means nothing streams there (or the title has no watch-provider data at
/// all, common for less mainstream/older content), not an error.
fn resolve_providers(providers: &[fjord_seerr::WatchProviderEntry], region: &str) -> Vec<ProviderRow> {
    providers
        .iter()
        .find(|p| p.iso_3166_1 == region)
        .map(|p| p.flatrate.iter().map(|d| (d.id, d.name.clone(), d.logo_path.clone())).collect())
        .unwrap_or_default()
}

/// "🇺🇸 United States\n🇬🇧 United Kingdom" — see request-detail-production-
/// countries' own doc comment in app_state.slint for why this is a single
/// newline-joined string rather than a list model.
fn format_countries(countries: &[fjord_seerr::ProductionCountry]) -> String {
    countries.iter().map(|c| format!("{} {}", country_flag_emoji(&c.iso_3166_1), c.name)).collect::<Vec<_>>().join("\n")
}

/// Picks the one video to offer as "Watch Trailer" — prefers a real
/// `Trailer`, falls back to a `Teaser` (a shorter preview, still trailer-
/// like) when no trailer exists, otherwise `None` (a `Clip`/`Featurette`/
/// etc. isn't what "Watch Trailer" implies). `url` is already a fully-
/// formed YouTube watch-page link — see `Video`'s own doc comment in
/// fjord-seerr for why only `kind`/`url` are modeled at all.
fn find_trailer_url(videos: &[fjord_seerr::Video]) -> Option<String> {
    videos
        .iter()
        .find(|v| v.kind == "Trailer")
        .or_else(|| videos.iter().find(|v| v.kind == "Teaser"))
        .map(|v| v.url.clone())
}

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
    production_status: String,
    date_label: &'static str,
    date_value: String,
    next_air_date: String,
    original_language: String,
    production_countries: String,
    network: String,
    providers: Vec<ProviderRow>,
    trailer_url: Option<String>,
}

fn movie_fields(d: MovieDetails, region: &str) -> DetailFields {
    let year = d.release_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let genres = d.genres.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", ");
    let cast = build_cast_list(&d.credits);
    let providers = resolve_providers(&d.watch_providers, region);
    let trailer_url = find_trailer_url(&d.related_videos);
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
        production_status: d.status,
        date_label: "Release Date",
        date_value: d.release_date.as_deref().map(format_date_pretty).unwrap_or_default(),
        next_air_date: String::new(),
        original_language: language_display_name(&d.original_language),
        production_countries: format_countries(&d.production_countries),
        network: String::new(),
        providers,
        trailer_url,
    }
}

fn tv_fields(d: TvDetails, region: &str) -> DetailFields {
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
    let providers = resolve_providers(&d.watch_providers, region);
    let network = d.networks.iter().map(|n| n.name.clone()).collect::<Vec<_>>().join(", ");
    let next_air_date =
        d.next_episode_to_air.as_ref().and_then(|e| e.air_date.as_deref()).map(format_date_pretty).unwrap_or_default();
    let trailer_url = find_trailer_url(&d.related_videos);
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
        production_status: d.status,
        date_label: "First Air Date",
        date_value: d.first_air_date.as_deref().map(format_date_pretty).unwrap_or_default(),
        next_air_date,
        original_language: language_display_name(&d.original_language),
        production_countries: format_countries(&d.production_countries),
        network,
        providers,
        trailer_url,
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

/// What to do once `open_discover_item`'s fetch lands, beyond just showing
/// `RequestDetailScreen` — the Discover context menu's Request/Edit Request
/// rows both need everything that fetch already does (title/poster/tags/
/// profiles for both tiers) plus one extra step, so they reuse this
/// function rather than duplicating its ~270-line body.
enum PostOpenAction {
    None,
    /// Immediately opens the Request Options modal once ready — same as
    /// View Details followed by pressing the Request button, collapsed
    /// into one action for the context menu's "Request" row.
    OpenRequestOptions,
    /// Same, but also fetches the given (already-existing) request's own
    /// `is4k`/`profileId`/`tags`/`seasons` (a fresh `GET /request/{id}`,
    /// not a cached snapshot — see `SeerrClient::get_request`'s own doc
    /// comment) and pre-selects them, with `request-options-editing` set so
    /// the modal hides Quality and Confirm calls `update_request`/PUT
    /// instead of `create_request`/POST.
    EditRequest(i64),
}

/// Resets zone/focus and opens the Request Options modal — shared by the
/// Request button's own callback and `PostOpenAction`'s two variants above,
/// so both entry points stay in sync.
fn open_request_options_modal(g: &AppState) {
    let zones = existing_option_zones(g);
    g.set_request_options_zone(zones.first().copied().unwrap_or(0));
    g.set_request_options_confirm_focused(1);
    g.set_show_request_options(true);
}

pub(crate) fn open_discover_item(
    media_type: String,
    tmdb_id_str: String,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    open_discover_item_ex(media_type, tmdb_id_str, state, ww, rt, PostOpenAction::None);
}

fn open_discover_item_ex(
    media_type: String,
    tmdb_id_str: String,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    post_action: PostOpenAction,
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
        // Land on the button row (Request), not the Back button — real
        // issue, live-reported 2026-07-18: opening a Discover item always
        // required an extra Down press before Request was even reachable,
        // unlike every other detail-style screen's own entry focus.
        g.set_request_detail_back_focused(false);
        g.set_request_detail_zone(0);
        g.set_request_detail_focused_season(0);
        g.set_request_detail_focused_tag(0);
        g.set_request_detail_want_4k(false);
        g.set_request_detail_production_status("".into());
        g.set_request_detail_date_label("".into());
        g.set_request_detail_date_value("".into());
        g.set_request_detail_next_air_date("".into());
        g.set_request_detail_original_language("".into());
        g.set_request_detail_production_countries("".into());
        g.set_request_detail_network("".into());
        g.set_request_detail_providers(ModelRc::new(VecModel::from(Vec::<StreamingProvider>::new())));
        g.set_request_detail_trailer_url("".into());
        g.set_request_detail_btn_focused(0);
        g.set_show_request_options(false); // defensive — shouldn't still be open across items
        g.set_request_options_editing(false);
        g.set_request_options_editing_request_id("".into());
        g.set_show_request_detail(true);
        next
    };

    let media_type2 = media_type.clone();
    rt.spawn(async move {
        // Cached after the first item opened this connection (see
        // resolve_streaming_region's own doc comment) — cheap enough not to
        // bother joining in parallel with the detail/options fetch below.
        let region = resolve_streaming_region(&client, &state).await;

        // Both quality tiers are fetched up front so the modal's Quality
        // toggle can swap between them instantly (request_detail_set_quality
        // below) instead of re-fetching live — no loading state, no race on
        // rapid toggling. The common single-instance setup only costs one
        // extra list call inside available_request_options_both_tiers, not
        // a duplicate detail fetch (see its doc comment in fjord-seerr).
        let editing_request_id = match &post_action {
            PostOpenAction::EditRequest(id) => Some(*id),
            _ => None,
        };
        let (detail_result, options_result, editing_request_result) = tokio::join!(
            async {
                if media_type2 == "movie" {
                    client.get_movie(tmdb_id).await.map(|d| movie_fields(d, &region))
                } else {
                    client.get_tv(tmdb_id).await.map(|d| tv_fields(d, &region))
                }
            },
            client.available_request_options_both_tiers(&media_type2),
            async {
                match editing_request_id {
                    Some(id) => Some(client.get_request(id).await),
                    None => None,
                }
            },
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

        // Streaming-provider logos — same bounded-concurrency shape as cast
        // portraits/season posters above, small TMDB CDN icons.
        let mut provider_bufs: Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> =
            vec![None; fields.providers.len()];
        let mut provider_tasks: tokio::task::JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> =
            tokio::task::JoinSet::new();
        for (idx, (provider_id, _, logo_path)) in fields.providers.iter().enumerate() {
            let Some(path) = logo_path.clone() else { continue };
            let http = http.clone();
            let sem = Arc::clone(&sem);
            let cache_key = format!("provider-{provider_id}");
            provider_tasks.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let bytes = fetch_tmdb_image(&http, TMDB_LOGO_BASE, &path, &cache_key).await;
                (idx, bytes.as_deref().and_then(decode_poster_buffer))
            });
        }
        while let Some(res) = provider_tasks.join_next().await {
            let Ok((idx, buf)) = res else { continue };
            provider_bufs[idx] = buf;
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
            g.set_request_detail_production_status(fields.production_status.as_str().into());
            g.set_request_detail_date_label(fields.date_label.into());
            g.set_request_detail_date_value(fields.date_value.as_str().into());
            g.set_request_detail_next_air_date(fields.next_air_date.as_str().into());
            g.set_request_detail_original_language(fields.original_language.as_str().into());
            g.set_request_detail_production_countries(fields.production_countries.as_str().into());
            g.set_request_detail_network(fields.network.as_str().into());
            let providers: Vec<StreamingProvider> = fields
                .providers
                .into_iter()
                .zip(provider_bufs)
                .map(|((id, name, _), buf)| {
                    let (logo, has_logo) = match buf {
                        Some(b) => (slint::Image::from_rgba8(b), true),
                        None => (Default::default(), false),
                    };
                    StreamingProvider { id: id as i32, name: name.as_str().into(), logo, has_logo }
                })
                .collect();
            g.set_request_detail_providers(ModelRc::new(VecModel::from(providers)));
            g.set_request_detail_trailer_url(fields.trailer_url.unwrap_or_default().as_str().into());
            if let Some(buf) = poster_buf {
                g.set_request_detail_poster(slint::Image::from_rgba8(buf));
                g.set_request_detail_has_poster(true);
            }
            if let Some(buf) = backdrop_buf {
                g.set_request_detail_backdrop(slint::Image::from_rgba8(buf));
                g.set_request_detail_has_backdrop(true);
            }
            match post_action {
                PostOpenAction::None => {}
                PostOpenAction::OpenRequestOptions => open_request_options_modal(&g),
                PostOpenAction::EditRequest(id) => {
                    match editing_request_result {
                        Some(Ok(r)) => {
                            if r.is4k {
                                set_quality(&g, true);
                            }
                            g.set_request_detail_selected_profile_id(r.profile_id.unwrap_or(0) as i32);
                            if let Some(tag_ids) = &r.tags {
                                let model = g.get_request_detail_tags();
                                for i in 0..model.row_count() {
                                    if let Some(mut t) = model.row_data(i) {
                                        t.selected = tag_ids.contains(&(t.id as i64));
                                        model.set_row_data(i, t);
                                    }
                                }
                            }
                            if !r.seasons.is_empty() {
                                let wanted: std::collections::HashSet<u32> =
                                    r.seasons.iter().map(|s| s.season_number).collect();
                                let model = g.get_request_detail_seasons();
                                for i in 0..model.row_count() {
                                    if let Some(mut s) = model.row_data(i) {
                                        s.selected = wanted.contains(&(s.season_number as u32));
                                        model.set_row_data(i, s);
                                    }
                                }
                            }
                            g.set_request_options_editing(true);
                            g.set_request_options_editing_request_id(id.to_string().as_str().into());
                        }
                        Some(Err(e)) => {
                            warn!("seerr: couldn't fetch request {id} for editing: {e:#}");
                            show_toast(ww.clone(), "Couldn't load request for editing".into());
                            return;
                        }
                        None => return, // shouldn't happen — editing_request_id was Some
                    }
                    open_request_options_modal(&g);
                }
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

    let rt2 = rt.clone();
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
                show_toast(ww.clone(), "Requested".into());
                // The freshly-created request has never been in
                // discover-requested before now — patch_discover_card_availability
                // above only updates a card ALREADY visible elsewhere (search
                // grid/other landing rows), so the Requested row itself stays
                // stale without this (real bug, live-reported 2026-07-18).
                refresh_requested_row(Arc::clone(&state), ww, rt2);
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

/// Confirm inside the Request Options modal while `request-options-editing`
/// is set (Discover context menu's Edit Request, 2026-07-18) — same shape
/// as `submit_request` but PUTs the existing request instead of POSTing a
/// new one, and deliberately does NOT read `request-detail-want-4k`: the
/// tier can't be changed by editing (confirmed from Seerr's real route
/// source — see `SeerrClient::update_request`'s own doc comment), so it's
/// never sent. No `status != ""` guard, unlike `submit_request` — an
/// existing request obviously already has one.
fn submit_edit_request(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    if g.get_request_detail_requesting() {
        return;
    }
    let Ok(request_id) = g.get_request_options_editing_request_id().parse::<i64>() else { return };
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    let media_type = g.get_request_detail_media_type().to_string();

    let seasons_selector = if media_type == "tv" {
        let model = g.get_request_detail_seasons();
        let total = model.row_count();
        let selected: Vec<u32> = (0..total)
            .filter_map(|i| model.row_data(i))
            .filter(|s| s.selected)
            .map(|s| s.season_number as u32)
            .collect();
        if selected.is_empty() {
            show_toast(ww.clone(), "Select at least one season".into());
            return;
        }
        Some(if selected.len() == total { SeasonsSelector::all() } else { SeasonsSelector::Numbers(selected) })
    } else {
        None
    };
    let tag_ids: Vec<i64> = {
        let model = g.get_request_detail_tags();
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .filter(|t| t.selected)
            .map(|t| t.id as i64)
            .collect()
    };
    let profile_id = match g.get_request_detail_selected_profile_id() {
        0 => None,
        id => Some(id as i64),
    };

    g.set_request_detail_requesting(true);
    drop(g);

    rt.spawn(async move {
        let result = client.update_request(request_id, &media_type, seasons_selector, tag_ids, profile_id).await;
        match result {
            Ok(()) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        g.set_request_detail_requesting(false);
                        g.set_show_request_options(false);
                        g.set_show_request_detail(false);
                    }
                });
                show_toast(ww, "Request updated".into());
            }
            Err(e) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() { AppState::get(&w).set_request_detail_requesting(false); }
                });
                handle_seerr_error(&state, &ww, is_session_auth, "Edit request failed", &e);
            }
        }
    });
}

/// Discover context menu's Cancel/Approve/Decline (2026-07-18) — all three
/// share this exact shape: one Seerr call by request id, then either remove
/// the card from `discover-requested` (Cancel) or leave it in place (its
/// status/badge will simply be stale until the next landing-row fetch —
/// Approve/Decline don't change what's ALREADY on screen enough to be worth
/// a full row rebuild for an infrequent admin action).
fn discover_request_action(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    request_id: i64,
    action: &'static str, // "cancel" | "approve" | "decline"
    remove_on_success: bool,
) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    rt.spawn(async move {
        let result = match action {
            "cancel" => client.delete_request(request_id).await,
            "approve" => client.approve_request(request_id).await,
            _ => client.decline_request(request_id).await,
        };
        match result {
            Ok(()) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww2.upgrade() else { return };
                    let g = AppState::get(&w);
                    if remove_on_success {
                        let model = g.get_discover_requested();
                        let kept: Vec<CardItem> = (0..model.row_count())
                            .filter_map(|i| model.row_data(i))
                            .filter(|c| c.request_id.as_str() != request_id.to_string())
                            .collect();
                        g.set_discover_requested(ModelRc::new(VecModel::from(kept)));
                    }
                });
                let verb = match action {
                    "cancel" => "cancelled",
                    "approve" => "approved",
                    _ => "declined",
                };
                show_toast(ww, format!("Request {verb}"));
            }
            Err(e) => handle_seerr_error(&state, &ww, is_session_auth, "Couldn't update request", &e),
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
    g.on_discover_load_more({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let query = AppState::get(&w).get_discover_query().to_string();
            if query.is_empty() { return; }
            spawn_discover_search_more(ww.clone(), Arc::clone(&state), query, Arc::clone(&gen), &rt);
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
        move || {
            let Some(w) = ww.upgrade() else { return };
            if AppState::get(&w).get_request_options_editing() {
                submit_edit_request(Arc::clone(&state), ww.clone(), rt.clone());
            } else {
                submit_request(Arc::clone(&state), ww.clone(), rt.clone());
            }
        }
    });

    g.on_open_request_options({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            open_request_options_modal(&AppState::get(&w));
        }
    });

    g.on_request_detail_set_quality({
        let ww = window.as_weak();
        move |want_4k| {
            let Some(w) = ww.upgrade() else { return };
            set_quality(&AppState::get(&w), want_4k);
        }
    });

    // ── Discover context menu (2026-07-18) ────────────────────────────────
    g.on_open_context_menu_discover({
        let ww = window.as_weak();
        move |item| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_context_menu_item_id(item.id.clone());
            g.set_context_menu_item_type(item.item_type.clone());
            g.set_context_menu_title(item.title.clone());
            g.set_context_menu_request_id(item.request_id.clone());
            g.set_context_menu_availability(item.availability.clone());
            g.set_context_menu_request_pending(item.request_pending);
            g.set_context_menu_request_mine(item.request_mine);
            debug!(
                "seerr: discover context menu opened for {} ({}) request_id={:?} pending={} mine={} seerr-is-admin={}",
                item.id, item.item_type, item.request_id, item.request_pending, item.request_mine, g.get_seerr_is_admin(),
            );
            g.set_context_menu_focused(0);
            g.set_show_context_menu(true);
        }
    });

    g.on_context_discover_view_details({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" { "movie" } else { "tv" };
            let tmdb_id = g.get_context_menu_item_id().to_string();
            g.set_show_context_menu(false);
            open_discover_item(media_type.into(), tmdb_id, Arc::clone(&state), ww.clone(), rt.clone());
        }
    });

    g.on_context_discover_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" { "movie" } else { "tv" };
            let tmdb_id = g.get_context_menu_item_id().to_string();
            g.set_show_context_menu(false);
            open_discover_item_ex(
                media_type.into(),
                tmdb_id,
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                PostOpenAction::OpenRequestOptions,
            );
        }
    });

    g.on_context_discover_edit_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_context_menu_request_id().parse::<i64>() else { return };
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" { "movie" } else { "tv" };
            let tmdb_id = g.get_context_menu_item_id().to_string();
            g.set_show_context_menu(false);
            open_discover_item_ex(
                media_type.into(),
                tmdb_id,
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                PostOpenAction::EditRequest(request_id),
            );
        }
    });

    g.on_context_discover_cancel_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_context_menu_request_id().parse::<i64>() else { return };
            g.set_show_context_menu(false);
            discover_request_action(Arc::clone(&state), ww.clone(), rt.clone(), request_id, "cancel", true);
        }
    });

    g.on_context_discover_approve_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_context_menu_request_id().parse::<i64>() else { return };
            g.set_show_context_menu(false);
            discover_request_action(Arc::clone(&state), ww.clone(), rt.clone(), request_id, "approve", false);
        }
    });

    g.on_context_discover_decline_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Ok(request_id) = g.get_context_menu_request_id().parse::<i64>() else { return };
            g.set_show_context_menu(false);
            discover_request_action(Arc::clone(&state), ww.clone(), rt.clone(), request_id, "decline", true);
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
                // At the last row of the currently-loaded results: kick off
                // a fetch of the next search-results page, if one exists
                // (see spawn_discover_search_more — quietly no-ops when
                // there isn't one, one's already in flight, or this isn't
                // a search grid at all). Still returns false either way —
                // this doesn't move focus, so focus_bar_on_down should
                // still get a chance to run for an active player bar.
                g.invoke_discover_load_more();
                false
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
        Action::OpenContextMenu => {
            let f = g.get_discover_focused();
            if f < count {
                if let Some(card) = g.get_discover_results().row_data(f as usize) {
                    g.invoke_open_context_menu_discover(card);
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
        Action::OpenContextMenu => {
            let c = g.get_discover_landing_card().max(0);
            if c < count {
                if let Some(card) = landing_row_get(g, fs as usize).row_data(c as usize) {
                    g.invoke_open_context_menu_discover(card);
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

    // Zone 0: Request button (or a static status pill once already
    // requested/available) plus an independent Trailer button (Watch
    // Trailer is unrelated to request status — see request-detail-trailer-
    // url's own doc comment). Confirm on Request opens the Request Options
    // modal rather than submitting directly; 4K/tags/seasons are configured
    // there, not on this page.
    let requestable = g.get_request_detail_status().as_str() == "";
    let has_trailer = !g.get_request_detail_trailer_url().as_str().is_empty() && g.get_yt_dlp_available();
    // Clamp request-detail-btn-focused to whatever's actually present: when
    // the status pill has replaced the Request button, 1 (Trailer) is the
    // only valid target, not 0 (nothing interactive there); when there's no
    // trailer at all, 0 (Request) is the only target. Recomputed on every
    // zone-0 key press rather than only at zone-entry, so it self-corrects
    // regardless of which transition landed here.
    let btn_focused = if has_trailer && !requestable {
        1
    } else if !has_trailer {
        0
    } else {
        g.get_request_detail_btn_focused()
    };
    if btn_focused != g.get_request_detail_btn_focused() {
        g.set_request_detail_btn_focused(btn_focused);
    }
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
        Action::Left => {
            if requestable && has_trailer && btn_focused == 1 {
                g.set_request_detail_btn_focused(0);
                true
            } else {
                false
            }
        }
        Action::Right => {
            if requestable && has_trailer && btn_focused == 0 {
                g.set_request_detail_btn_focused(1);
                true
            } else {
                false
            }
        }
        Action::Confirm => {
            if has_trailer && btn_focused == 1 {
                g.invoke_play_trailer();
            } else if requestable {
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
    // Zone 0 (Quality) is hidden entirely while editing — see
    // request_detail.slint's own comment on the Quality section for why.
    let mut zones = if g.get_request_options_editing() { Vec::new() } else { vec![0] };
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
