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
//                              profileId, 0/Default omitted); on success flips ONLY the just-
//                              requested tier's own request-detail-status/-4k (real bug fixed
//                              2026-07-18: used to blank out BOTH tiers regardless of is_4k,
//                              hiding the still-open other tier's own Request option entirely —
//                              "if you reqest 4k then the requestbutton changes to requested...
//                              so you cant also request 2k") + patches the originating Discover
//                              card + toasts
//   tier_status_label            one tier's FINAL display text ("Needs Approval"/"Approved"/
//                              "Processing"/"Partially Available"/"Available"/"Declined"/
//                              "Failed"/"") combining MediaStatus (fulfillment) with the
//                              request's own MediaRequestStatus (approval workflow) — real gap
//                              fixed 2026-07-18, "it shuld reflect the status, like if its
//                              aproved or needs aprovment etc"; feeds request-detail-status/-4k
//                              AND (movie_fields/tv_fields) drives RequestDetailScreen's poster
//                              badge, both tier pills, and the Request button's visibility
//   tier_request/pick_primary_request  tier_request finds the one MediaRequest for a given is4k
//                              tier out of MediaInfo.requests (only populated on the single-item
//                              detail endpoints — see MediaInfo's own doc comment in fjord-seerr);
//                              pick_primary_request resolves the (request_id, pending, mine)
//                              triple the ⋮ More button's context menu acts on, preferring the
//                              4K request when both tiers have one (documented tiebreak, not a
//                              full per-tier action UI)
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
//   existing_zones/handle_key_request_detail  back -> button row -> storyline
//                              (collapsible overview) -> cast row — Up/Down step to the
//                              nearest zone that exists for this item.
//   existing_detail_btn_slots   button row's own "gaps are fine" slot list (2026-07-18,
//                              generalized from the original hardcoded Request/Trailer binary
//                              once a 3rd slot joined it): 0=Request (at least one tier still
//                              requestable), 1=Trailer (found + yt-dlp available), 2=⋮ More
//                              (opens the Discover context menu — View Request/Edit/Cancel/
//                              Approve/Decline — sourced from request-detail-request-id/
//                              -pending/-mine rather than a CardItem; only shown once a request
//                              exists). Left/Right move within existing slots; Confirm dispatches
//                              open-request-options/play-trailer/open-discover-menu-from-detail.
//                              Request opens the Request Options modal rather than exposing
//                              4K/tags/seasons inline (see below); Trailer button fires
//                              play-trailer() (Watch Trailer — Discover only, see CLAUDE.md's
//                              Seerr integration section).
//   find_trailer_url            MovieDetails/TvDetails.relatedVideos -> best trailer URL
//                              (prefers Trailer, falls back to Teaser, else None)
//   existing_option_zones/handle_key_request_options  Request Options modal: Quality (2K/4K)
//                              row -> profile row (radio-select) -> tags row -> seasons row ->
//                              confirm row (Cancel/Request), same skip-absent-zones idiom as
//                              existing_zones but its own numbering
//   ── Discover filters (2026-07-18, planned via /plan, 3 rounds of
//      AskUserQuestion — see CLAUDE.md's Seerr integration section) ──
//   ensure_discover_filter_options  fetches genre + watch-provider lists (both media
//                              types) once per session; also restores discover-filters-active
//                              + the 4 *-desc properties from persisted Config and, if filters
//                              were already active, kicks off spawn_discover_filtered_browse
//                              immediately rather than waiting for a pill touch
//   build_discover_filters      current filter selections -> a real fjord_seerr::DiscoverFilters
//                              for one media type's /discover/* call; genre NAMES re-resolved
//                              to that type's own raw id (movie/TV genre id spaces don't match)
//   build_genre_items/build_provider_items/push_or_merge_genre/refresh_discover_filter_models
//                              raw Seerr genre/provider lists -> Slint GenreItem/ProviderItem
//                              chip models; push_or_merge_genre merges a same-named genre's
//                              movie-side and TV-side ids into one chip (GenreItem carries
//                              both, since the id spaces don't overlap); providers dedupe by
//                              id directly (shared across media types, unlike genre)
//   discover_filters_active/search_filters_active  discover_filters_active: is ANY of the
//                              6 dimensions non-default (landing-rows vs filtered-browse
//                              switch); search_filters_active: the narrower subset that
//                              actually applies to search results (excludes Type — /search
//                              always mixes both types — and Provider, which /search's
//                              response carries no data for at all)
//   apply_search_filters        client-side genre/rating/year/sort pass over the full raw
//                              fetch history (FjordState.discover_search_metas) — the only
//                              way filters can apply to search results, since /search takes
//                              no filter params; preserves posters by id lookup (not index —
//                              filtering reorders/removes rows); must run strictly AFTER
//                              fetch_and_patch_posters finishes, never before
//   build_filtered_metas/merge_filtered_metas  SearchResult list -> (meta, poster_path)
//                              pairs; merge_filtered_metas interleaves movie+TV results into
//                              one grid for Type=All, sorted by the active sort key's real
//                              value (concatenate-then-sort, not a two-pointer merge — the
//                              inputs are small enough that this is simpler for the same result)
//   spawn_discover_filtered_browse/_more  the new filtered-browse view (query empty, >=1
//                              filter active) — mirrors spawn_discover_search/_more's two-
//                              phase commit shape but sources from discover_movies_filtered/
//                              discover_tv_filtered (real server-side filtering); fetches both
//                              types in parallel when Type=All; shares spawn_discover_search's
//                              OWN discover_gen counter (required — a race between the two
//                              view types would otherwise clobber discover-results); load-more
//                              advances both underlying TMDB pages in lockstep, stopping on
//                              max() of the two total_pages so Type=All doesn't cut off early
//   on_discover_filter_changed  shared tail of every filter-pill-changed callback: saves
//                              Config, recomputes discover-filters-active, then either
//                              triggers spawn_discover_filtered_browse (query empty + active),
//                              clears discover-results (query empty + inactive), or calls
//                              apply_search_filters (query non-empty)
//   discover_popup_options/open_discover_popup/handle_key_discover_popup  the Type/Sort/
//                              Rating/Year single-select popups + Genre/Provider multi-select
//                              chip popups opened from the filter bar — its own dropdown/
//                              zone state machine (modeled on, not reused from,
//                              settings.rs::dispatch_settings/handle_key_request_options,
//                              since Discover's keyboard model has no existing "capture all
//                              input while a popup is open" mechanism of its own); mouse
//                              clicks on PopupOption/FilterChip (discover.slint) set the same
//                              cursor state then invoke Action::Confirm through this same
//                              function via the discover-popup-confirm callback, so mouse and
//                              keyboard can never diverge (the exact bug class Phase 142's
//                              zone-numbering note documents)
//   handle_key_discover_filter_bar  Left/Right move the cursor across the 7 pills (Type/
//                              Genre/Sort/Rating/Year/Provider/Clear), Up returns to the
//                              search field, Down enters the grid/landing rows, Confirm opens
//                              the focused pill's popup (or fires Clear); mouse clicks on
//                              FilterPill mirror the popup pattern above (set state, invoke
//                              Action::Confirm through this function via discover-filter-bar-
//                              confirm)
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
//                              also does) on every Discover-tab arrival, unguarded by a
//                              fetched-once flag — real bug fixed: seerr-is-admin was
//                              previously only ever set once per connection, so a
//                              server-side permission change mid-session never reflected
//                              in Approve/Decline gating without a reconnect
//   on_nav_selected            (in wire_discover) also now resets discover-popup-open/
//                              discover-filter-bar-active when leaving Discover (real bug:
//                              a filter popup left open silently reappeared on return) and
//                              calls refresh_seerr_admin_status on every arrival
//   ── Watchlist + Release Calendar (2026-07-18, planned via /plan, 2 rounds of
//      AskUserQuestion + an independent Plan-agent review — see CLAUDE.md's
//      Seerr integration section) ──
//   patch_watchlist_state       CardItem.on-watchlist counterpart to
//                              patch_known_request_state — consults
//                              FjordState.discover_watchlist_ids, patched onto
//                              search/landing DiscoverCardMetas alongside the request-state patch
//   discover_toggle_watchlist   POST/DELETE /watchlist, updates discover_watchlist_ids,
//                              patches every model the card might be visible in
//                              (patch_watchlist_on_all_models) + request-detail-on-watchlist
//                              if that item's detail page is open, toasts, calls refresh_watchlist
//                              (which rebuilds the calendar too); debug!-logged at entry/success
//                              (2026-07-19, live report of "no confirmation" with no evidence in
//                              the log of the call ever happening — added to get direct proof of
//                              where it breaks on the next attempt instead of guessing again);
//                              the context-menu callsite also warn!s if context-menu-item-id
//                              fails to parse as a tmdb id (its one silent-early-return path)
//   ensure_discover_watchlist/refresh_watchlist/fetch_and_store_watchlist  fetch-once-per-
//                              session (paginated, 200-item safety cap) + refresh-after-toggle
//                              pair mirroring ensure_discover_landing/refresh_requested_row;
//                              both funnel through the shared fetch_and_store_watchlist, which
//                              also triggers build_calendar_entries on every fetch
//   resolve_discover_region     GET-once-per-connection resolver for the (distinct from
//                              streamingRegion) discoverRegion user setting, mirrors
//                              resolve_streaming_region's exact shape, cached in
//                              FjordState.seerr_discover_region
//   release_dates_for_region/calendar_kind_for_release_type  ReleaseDatesResult + region ->
//                              deduped-by-type (3/4/5) (type, date) pairs, mirrors Seerr's own
//                              frontend filter; type -> CalendarEntryKind (Theatrical/Digital/Physical)
//   CalendarEntry/CalendarEntryKind  date/tmdb_id/item_type/title/poster_path/kind/
//                              episode_label — poster_path added 2026-07-19 (user request,
//                              "it hust dosent have posters" — reverses the original
//                              deliberately-text-only design)
//   build_calendar_entries      unions discover_watchlist_ids ∪ discover_known_requests keys,
//                              capped at 20 (mirrors fetch_requested_row's own cap), detail-
//                              fetches (bounded Semaphore+JoinSet) each and extracts movie
//                              release dates or TV next_episode_to_air; sorted soonest-first;
//                              called after every watchlist/request mutation (toggle, submit,
//                              cancel/approve/decline), not just on session fetch — ALSO now
//                              spawned from ensure_discover_landing itself right after it
//                              populates discover_known_requests (real bug, live-reported
//                              2026-07-19: ensure_discover_watchlist's own post-fetch call
//                              races ensure_discover_landing's tokio::join! and nearly always
//                              wins — the watchlist fetch is comparatively instant, the
//                              landing join is a real network round trip — so on a session
//                              with zero watchlist items, candidates was empty at the ONE
//                              call that ever ran, and nothing re-triggered it afterward; the
//                              Coming Up row stayed sentinel-only for the whole session)
//   push_coming_up_row          discover_calendar_entries -> discover-coming-up CardItem list
//                              (capped PREVIEW_CAP=20) + a trailing sentinel card (id="",
//                              title="📅", subtitle="Full Calendar") whose Enter/click opens
//                              CalendarScreen instead of an item. Real bug, live-reported
//                              2026-07-19 ("highlight disappears, nothing shows anywhere"):
//                              this function's only caller (build_calendar_entries) runs on a
//                              Tokio worker thread, never invoke_from_event_loop-wrapped, but
//                              this function called ww.upgrade()/AppState setters directly —
//                              slint::Weak::upgrade() silently returns None off the UI thread
//                              (confirmed from i-slint-core's real source), so
//                              discover-coming-up was never actually set, on any run, since
//                              this feature shipped; build_calendar_entries's own success log
//                              made the Rust-side computation look like it worked, masking
//                              that the UI-side commit was silently failing every time. Fixed
//                              to match every other UI mutation in this file: clone entries
//                              (plain Send-safe data) before the closure, build CardItems and
//                              call the AppState setter only inside invoke_from_event_loop
//   fetch_coming_up_posters     patches posters onto the already-committed Coming Up row
//                              (2026-07-19, user request), bounded-concurrency fetch-then-
//                              patch-by-index, same shape as refresh_requested_row's own
//                              poster pass; must truncate with the same COMING_UP_PREVIEW_CAP
//                              and source order push_coming_up_row used (patches by index)
//   fetch_new_in_theaters        canned DiscoverFilters preset (primaryReleaseDateGte=today-45d,
//                              primaryReleaseDateLte=today, sort=popularity.desc) over the
//                              existing discover_movies_filtered — an honest approximation,
//                              Seerr's /discover/movies has no verified "still showing" signal
//   handle_key_landing          Confirm/OpenContextMenu on the Coming Up row's sentinel card
//                              (last card, row index LANDING_ROW_COMING_UP) special-cased to
//                              open the calendar / no-op instead of falling through to the
//                              generic open_discover_item/open_context_menu_discover
//   calendar_grid_dims/push_calendar_view/calendar_day_entries  month-grid data: leading-
//                              blank-count + day-count for a year/month (Sunday-first);
//                              calendar-days CardItem list (day number as title, entry count
//                              via unplayed-count, first entry's own title via subtitle —
//                              2026-07-19, user request, CalendarDayCell shows it instead of
//                              just a count pill); one day's matching CalendarEntry rows -> popup CardItems
//   handle_key_calendar/handle_key_calendar_day_popup  CalendarScreen's own AppMode dispatch —
//                              header zone (calendar-cursor-row<0) vs. 7-col day grid; Left/Right
//                              at the header directly invoke calendar-prev-month()/-next-month()
//                              (2026-07-19, user request — previously just cycled a cursor among
//                              Back/Prev/Next, needing a separate Confirm; Back is still reachable
//                              via Escape/Backspace, the universal close-key convention, or Enter
//                              at the initial Back-focused position); Confirm on a real day
//                              invokes calendar-day-selected(day) (the SAME callback the mouse
//                              path calls, so keyboard/mouse can't diverge); day popup: Up/Down
//                              cursor, Confirm -> calendar-day-popup-entry-selected(idx), Back
//                              closes the popup only
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fjord_seerr::{MediaStatus, MovieDetails, SearchResult, SeasonsSelector, TvDetails};
use slint::{ComponentHandle, Global, Model, ModelRc, VecModel, Weak};

use tracing::{debug, warn};

use crate::config::{discover_poster_cache_path, save_config, Config, FjordState};
use crate::keys::Action;
use crate::poster::decode_poster_buffer;
use crate::{
    show_toast, spawn_movies_list_fetch, AppState, CardItem, CastMember, GenreItem, MainWindow, ProfileItem,
    ProviderItem, SeasonItem, StreamingProvider, TagItem,
};

const TMDB_POSTER_BASE: &str = "https://image.tmdb.org/t/p/w500";
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
        Some(MediaStatus::Unknown) | Some(MediaStatus::Blocklisted) | Some(MediaStatus::Deleted) | None => "",
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
                KnownRequest { request_id: m.request_id.clone(), pending: m.request_pending, mine: m.request_mine },
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
fn patch_known_request_state(meta: &mut DiscoverCardMeta, known: &std::collections::HashMap<(&'static str, String), KnownRequest>) {
    if let Some(k) = known.get(&(meta.item_type, meta.id.clone())) {
        meta.request_id = k.request_id.clone();
        meta.request_pending = k.pending;
        meta.request_mine = k.mine;
    }
}

/// `on_watchlist` counterpart to `patch_known_request_state` above — same
/// shape, consulting `FjordState.discover_watchlist_ids` instead. Watchlist
/// + Release Calendar, 2026-07-18.
fn patch_watchlist_state(meta: &mut DiscoverCardMeta, watchlist_ids: &std::collections::HashSet<(&'static str, String)>) {
    meta.on_watchlist = watchlist_ids.contains(&(meta.item_type, meta.id.clone()));
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
        genre_ids: r.genre_ids.clone(),
        vote_average: r.vote_average.unwrap_or(0.0),
        popularity: r.popularity.unwrap_or(0.0),
        on_watchlist: false,
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
        let results = response.results;
        let mut metas: Vec<DiscoverCardMeta> = results.iter().filter_map(search_result_to_meta).collect();
        debug!(
            "seerr: search {query:?} page 1/{} -> {} raw result(s), {} movie/tv card(s)",
            response.total_pages,
            results.len(),
            metas.len()
        );
        {
            let mut s = state.lock().unwrap();
            s.discover_search_page = 1;
            s.discover_search_total_pages = response.total_pages;
            s.discover_search_loading_more = false;
            // Real bug fixed 2026-07-18 — see FjordState.discover_known_requests'
            // own doc comment: search results never carried real request
            // state at all, so an already-requested item's context menu
            // offered "Request" instead of "Edit/Cancel/View Request".
            for m in &mut metas {
                patch_known_request_state(m, &s.discover_known_requests);
                patch_watchlist_state(m, &s.discover_watchlist_ids);
            }
            // Full raw fetch history for this query — apply_search_filters'
            // only source of genre_ids/vote_average, which never make it
            // onto CardItem (never displayed). Overwritten (not extended)
            // here since this is page 1 of a fresh query.
            s.discover_search_metas = metas.clone();
        }

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

        fetch_and_patch_posters(ww.clone(), Arc::clone(&gen), my_gen, poster_jobs).await;
        // Re-narrow to whatever filters are already set — a no-op when
        // none are (search_filters_active's own early return), so this is
        // safe to call unconditionally after every commit. Must run AFTER
        // the poster patch above, not before — see apply_search_filters'
        // own doc comment for why.
        if gen.load(Ordering::SeqCst) == my_gen {
            apply_search_filters(&state, &ww);
        }
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
        let results = response.results;
        let mut metas: Vec<DiscoverCardMeta> = results.iter().filter_map(search_result_to_meta).collect();
        debug!(
            "seerr: search {query:?} page {next_page}/{} -> {} raw result(s), {} card(s)",
            response.total_pages,
            results.len(),
            metas.len()
        );
        {
            let mut s = state2.lock().unwrap();
            s.discover_search_page = next_page;
            s.discover_search_total_pages = response.total_pages;
            s.discover_search_loading_more = false;
            // See spawn_discover_search's identical patch — real bug fixed
            // 2026-07-18.
            for m in &mut metas {
                patch_known_request_state(m, &s.discover_known_requests);
                patch_watchlist_state(m, &s.discover_watchlist_ids);
            }
            s.discover_search_metas.extend(metas.clone());
        }

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

        fetch_and_patch_posters(ww.clone(), Arc::clone(&gen), my_gen, poster_jobs).await;
        if gen.load(Ordering::SeqCst) == my_gen {
            apply_search_filters(&state2, &ww);
        }
    });
}

// ── Landing rows (Trending / Popular / Upcoming, shown when query == "") ───

// Row indices, named — used by handle_key_landing's sentinel special-case
// (Watchlist + Release Calendar, 2026-07-18) so that check doesn't depend
// on a bare literal matching this match's own row ordering.
pub(crate) const LANDING_ROW_NEW_IN_THEATERS: usize = 6;
pub(crate) const LANDING_ROW_COMING_UP: usize = 7;

fn landing_row_get(g: &AppState, idx: usize) -> ModelRc<CardItem> {
    match idx {
        0 => g.get_discover_trending(),
        1 => g.get_discover_popular_movies(),
        2 => g.get_discover_popular_tv(),
        3 => g.get_discover_upcoming_movies(),
        4 => g.get_discover_upcoming_tv(),
        5 => g.get_discover_requested(),
        6 => g.get_discover_new_in_theaters(),
        _ => g.get_discover_coming_up(),
    }
}

fn landing_row_set(g: &AppState, idx: usize, model: ModelRc<CardItem>) {
    match idx {
        0 => g.set_discover_trending(model),
        1 => g.set_discover_popular_movies(model),
        2 => g.set_discover_popular_tv(model),
        3 => g.set_discover_upcoming_movies(model),
        4 => g.set_discover_upcoming_tv(model),
        5 => g.set_discover_requested(model),
        6 => g.set_discover_new_in_theaters(model),
        _ => g.set_discover_coming_up(model),
    }
}

fn landing_row_lens(g: &AppState) -> [i32; 8] {
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
        genre_ids: Vec::new(),
        vote_average: 0.0,
        popularity: 0.0,
        on_watchlist: d.on_user_watchlist,
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
        genre_ids: Vec::new(),
        vote_average: 0.0,
        popularity: 0.0,
        on_watchlist: d.on_user_watchlist,
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
        // Real bug fixed 2026-07-18 — see FjordState.discover_known_requests'
        // own doc comment. Refreshed here too, not just in
        // ensure_discover_landing, so a request submitted THIS session is
        // immediately known everywhere, not just after the next full landing
        // refresh.
        state.lock().unwrap().discover_known_requests = known_requests_from_row(&requested);
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
/// "New in Theaters" — an honest APPROXIMATION, not a verified "still
/// showing" signal: Seerr's `/discover/movies` has no `with_release_type`
/// passthrough (confirmed by reading its real query schema, only a fixed
/// allowlist), so this can't filter by release TYPE directly. Instead uses
/// `primaryReleaseDateGte`/`Lte` (already supported, built for Discover
/// Filters) over roughly the last 6 weeks — most wide releases' `primary`
/// TMDB release date IS the theatrical date, but this isn't guaranteed for
/// every title. Reuses `discover_movies_filtered` (Discover Filters'
/// existing machinery) with a canned preset rather than a new fetch shape.
/// Watchlist + Release Calendar, 2026-07-18.
async fn fetch_new_in_theaters(client: &fjord_seerr::SeerrClient) -> anyhow::Result<fjord_seerr::SearchResponse> {
    let today = chrono::Local::now().date_naive();
    let six_weeks_ago = today - chrono::Duration::days(45);
    let filters = fjord_seerr::DiscoverFilters {
        sort: Some("popularity.desc"),
        date_gte: Some(("primaryReleaseDateGte", six_weeks_ago.format("%Y-%m-%d").to_string())),
        date_lte: Some(("primaryReleaseDateLte", today.format("%Y-%m-%d").to_string())),
        ..Default::default()
    };
    client.discover_movies_filtered(1, &filters).await
}

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
    let state2 = Arc::clone(&state);

    rt.spawn(async move {
        let (r_trending, r_movies, r_tv, r_movies_up, r_tv_up, requested, r_new_in_theaters) = tokio::join!(
            client.discover_trending(1),
            client.discover_movies(1),
            client.discover_tv(1),
            client.discover_movies_upcoming(1),
            client.discover_tv_upcoming(1),
            fetch_requested_row(&client, my_user_id),
            fetch_new_in_theaters(&client),
        );
        let responses = [r_trending, r_movies, r_tv, r_movies_up, r_tv_up];
        const ROW_NAMES: [&str; 7] = [
            "trending", "popular movies", "popular tv", "upcoming movies", "upcoming tv", "requested",
            "new in theaters",
        ];

        // Anything already in the Requested row shouldn't also show up in
        // Trending/Popular/Upcoming — real gap, live-reported 2026-07-18
        // ("If the series is in the request row it shuld not show up in any
        // other row in descovery, but shuld still show up when you search").
        // Deliberately only dedups against the Requested row, not the other
        // 5 rows against each other (confirmed via AskUserQuestion) — the
        // same title appearing in both Trending and Popular is normal for a
        // discovery page and left alone; search is untouched, per the user's
        // own explicit ask, since it isn't built from these landing-row
        // fetches at all. Keyed on (item_type, tmdb id) since a movie and a
        // tv show can share a raw tmdb id.
        let requested_keys: std::collections::HashSet<(&'static str, String)> =
            requested.iter().map(|(m, _)| (m.item_type, m.id.clone())).collect();

        // Real bug fixed 2026-07-18 — see FjordState.discover_known_requests'
        // own doc comment: without this, an already-requested item that
        // still shows in Trending/Popular/Upcoming (not deduped out above,
        // since dedup only excludes items requested_not_available itself
        // returned) had its context menu offer "Request" instead of
        // "Edit/Cancel/View Request". Built from `requested` before it's
        // consumed by row 5's own metas_per_row entry below.
        let known = known_requests_from_row(&requested);
        // Watchlist ids are fetched independently (ensure_discover_watchlist,
        // its own guard/trigger) — read whatever's already cached rather than
        // fetching again here; a not-yet-completed first-ever fetch just
        // means this pass shows no watchlist badges, self-healing on the
        // next landing/search refresh once it lands.
        let watchlist_ids = {
            let mut s = state2.lock().unwrap();
            s.discover_known_requests = known.clone();
            s.discover_watchlist_ids.clone()
        };
        // Real bug, live-reported 2026-07-19: `ensure_discover_watchlist`'s own
        // `build_calendar_entries` call races this task and near-always loses —
        // it reads `discover_known_requests` before this line above has had a
        // chance to populate it (this whole tokio::join! above is a network
        // round trip; the watchlist fetch is comparatively instant), so the
        // "Coming Up" row's candidate set (discover_watchlist_ids ∪
        // discover_known_requests) was empty at the one and only time
        // build_calendar_entries ever ran for a session with no watchlist
        // items, and nothing re-triggers it afterward — the row silently
        // stayed sentinel-only forever. Spawned (not awaited) so the calendar
        // rebuild's own per-item detail fetches don't delay committing the
        // rest of this landing-row screen.
        tokio::spawn(build_calendar_entries(Arc::clone(&state2), ww.clone()));

        let mut metas_per_row: Vec<Vec<DiscoverCardMeta>> = Vec::with_capacity(6);
        // (row, idx-within-row, item_type, tmdb_id, poster_path)
        let mut poster_jobs: Vec<(usize, usize, String, String, String)> = Vec::new();
        let mut first_error: Option<anyhow::Error> = None;
        for (row, r) in responses.into_iter().enumerate() {
            match r {
                Ok(resp) => {
                    // Filter the raw results (not the derived metas) so the
                    // poster-job zip below stays index-aligned with metas —
                    // filtering metas and the zip's own result sequence
                    // independently would let them drift out of sync.
                    let results: Vec<_> = resp.results.into_iter()
                        .filter(|r| {
                            let item_type = if r.media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" };
                            !requested_keys.contains(&(item_type, r.id.to_string()))
                        })
                        .collect();
                    let mut metas: Vec<DiscoverCardMeta> = results.iter().filter_map(search_result_to_meta).collect();
                    for m in &mut metas {
                        patch_known_request_state(m, &known);
                        patch_watchlist_state(m, &watchlist_ids);
                    }
                    debug!("seerr: landing row {} ({}) -> {} card(s)", row, ROW_NAMES[row], metas.len());
                    let jobs: Vec<(usize, usize, String, String, String)> = metas
                        .iter()
                        .enumerate()
                        .zip(results.iter().filter(|r| r.media_type == "movie" || r.media_type == "tv"))
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

        // Row 6 (New in Theaters) — same SearchResponse shape as rows 0-4,
        // handled in its own block since it isn't fetched via the uniform
        // `responses` array above (a separate canned-filter call, not a
        // plain unfiltered discover_* one). Same dedup-against-Requested
        // filter as rows 0-4, for the same reason.
        {
            let row = LANDING_ROW_NEW_IN_THEATERS;
            match r_new_in_theaters {
                Ok(resp) => {
                    let results: Vec<_> = resp.results.into_iter()
                        .filter(|r| !requested_keys.contains(&("DiscoverMovie", r.id.to_string())))
                        .collect();
                    let mut metas: Vec<DiscoverCardMeta> = results.iter().filter_map(search_result_to_meta).collect();
                    for m in &mut metas {
                        patch_known_request_state(m, &known);
                        patch_watchlist_state(m, &watchlist_ids);
                    }
                    debug!("seerr: landing row {} ({}) -> {} card(s)", row, ROW_NAMES[row], metas.len());
                    let jobs: Vec<(usize, usize, String, String, String)> = metas
                        .iter()
                        .enumerate()
                        .zip(results.iter())
                        .filter_map(|((idx, m), r)| r.poster_path.clone().map(|p| (row, idx, m.item_type.to_string(), m.id.clone(), p)))
                        .collect();
                    poster_jobs.extend(jobs);
                    metas_per_row.push(metas);
                }
                Err(e) => {
                    warn!("seerr: landing row {} ({}) fetch failed: {e:#}", row, ROW_NAMES[row]);
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

/// Mirrors `resolve_streaming_region` exactly, but for the DIFFERENT
/// `discoverRegion` user setting Seerr's own frontend uses specifically for
/// release-date display (`src/components/MovieDetails/index.tsx`) — not
/// the same region as "Currently Streaming On", confirmed from Seerr's real
/// source (Watchlist + Release Calendar, 2026-07-18).
async fn resolve_discover_region(client: &fjord_seerr::SeerrClient, state: &Arc<Mutex<FjordState>>) -> String {
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

/// `MovieDetails.releases` -> deduped-by-type `(type, date)` pairs for
/// `type` in {3=Theatrical, 4=Digital, 5=Physical} — mirrors Seerr's own
/// frontend filter exactly (`src/components/MovieDetails/index.tsx`:
/// `releases?.filter((r) => r.type > 2 && r.type < 6)`, `uniqBy(..., 'type')`).
/// TV has no equivalent (Watchlist + Release Calendar, 2026-07-18).
fn release_dates_for_region(releases: &fjord_seerr::ReleaseDatesResult, region: &str) -> Vec<(i32, String)> {
    let Some(entries) = releases.results.iter().find(|r| r.iso_3166_1 == region).map(|r| &r.release_dates) else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    entries
        .iter()
        .filter(|e| (3..6).contains(&e.release_type))
        .filter(|e| seen.insert(e.release_type))
        .map(|e| (e.release_type, e.release_date.clone()))
        .collect()
}

fn calendar_kind_for_release_type(t: i32) -> CalendarEntryKind {
    match t {
        3 => CalendarEntryKind::Theatrical,
        4 => CalendarEntryKind::Digital,
        _ => CalendarEntryKind::Physical,
    }
}

/// Builds the "Coming Up" row's data — every id in `discover_watchlist_ids`
/// union `discover_known_requests`' own keys (the latter already IS the
/// `requested_not_available` result set, populated from that exact call by
/// `ensure_discover_landing`/`refresh_requested_row` — reusing it here
/// avoids a second, duplicate `GET /request` round trip), capped at 20
/// CANDIDATES (not 20 RESULTING entries — a date isn't known until after
/// the detail fetch below, so the cap bounds the number of detail fetches,
/// not a pre-sorted "soonest 20"; the final list is what gets sorted by
/// date, not the candidate selection). Same bounded-concurrency JoinSet
/// shape as `fetch_requested_row`. Movies contribute up to 3 entries each
/// (Theatrical/Digital/Physical, whichever have a real future date); TV
/// contributes at most 1 (`next_episode_to_air`). Past dates are excluded —
/// a "Coming Up" calendar has nothing to say about something already out.
/// Watchlist + Release Calendar, 2026-07-18.
pub(crate) async fn build_calendar_entries(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else { return };
    let candidates: Vec<(&'static str, String)> = {
        let s = state.lock().unwrap();
        let mut ids: std::collections::HashSet<(&'static str, String)> = s.discover_watchlist_ids.clone();
        ids.extend(s.discover_known_requests.keys().cloned());
        ids.into_iter().take(20).collect()
    };
    if candidates.is_empty() {
        state.lock().unwrap().discover_calendar_entries.clear();
        push_coming_up_row(&ww, &[]);
        return;
    }
    let today = chrono::Local::now().date_naive();
    let region = resolve_discover_region(&client, &state).await;

    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set: tokio::task::JoinSet<Vec<CalendarEntry>> = tokio::task::JoinSet::new();
    for (item_type, tmdb_id_str) in candidates {
        let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else { continue };
        let client = client.clone();
        let sem = Arc::clone(&sem);
        let region = region.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let mut entries = Vec::new();
            if item_type == "DiscoverMovie" {
                let Ok(d) = client.get_movie(tmdb_id).await else { return entries };
                let Some(releases) = &d.releases else { return entries };
                for (release_type, date_str) in release_dates_for_region(releases, &region) {
                    let Ok(date) = chrono::NaiveDate::parse_from_str(&date_str[..date_str.len().min(10)], "%Y-%m-%d") else { continue };
                    entries.push(CalendarEntry {
                        date,
                        tmdb_id: tmdb_id.to_string(),
                        item_type: "DiscoverMovie",
                        title: d.title.clone(),
                        poster_path: d.poster_path.clone(),
                        kind: calendar_kind_for_release_type(release_type),
                        episode_label: None,
                    });
                }
            } else {
                let Ok(d) = client.get_tv(tmdb_id).await else { return entries };
                let Some(next) = &d.next_episode_to_air else { return entries };
                let Some(date_str) = &next.air_date else { return entries };
                let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else { return entries };
                let episode_label = match (next.season_number, next.episode_number, &next.name) {
                    (Some(s), Some(e), Some(name)) => Some(format!("S{s}E{e} — {name}")),
                    (Some(s), Some(e), None) => Some(format!("S{s}E{e}")),
                    _ => None,
                };
                entries.push(CalendarEntry {
                    date,
                    tmdb_id: tmdb_id.to_string(),
                    item_type: "DiscoverTv",
                    title: d.name.clone(),
                    poster_path: d.poster_path.clone(),
                    kind: CalendarEntryKind::Episode,
                    episode_label,
                });
            }
            entries
        });
    }
    let mut all: Vec<CalendarEntry> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(entries) = res {
            all.extend(entries.into_iter().filter(|e| e.date >= today));
        }
    }
    all.sort_by_key(|e| e.date);
    debug!("seerr: calendar -> {} entr{}", all.len(), if all.len() == 1 { "y" } else { "ies" });

    state.lock().unwrap().discover_calendar_entries = all.clone();
    push_coming_up_row(&ww, &all);
    fetch_coming_up_posters(ww, &all).await;
}

/// Pushes the "Coming Up" landing row from `entries` (soonest-first,
/// already sorted by `build_calendar_entries`) — capped to a preview count,
/// plus the trailing sentinel card `handle_key_landing` special-cases.
/// Text-only commit first, same two-phase pattern as every other landing
/// row — `fetch_coming_up_posters` (below) patches posters in afterward;
/// `ensure_discover_landing`'s own poster pass doesn't cover this row
/// since it's rebuilt independently on its own schedule, not as part of
/// the 8-way landing join.
///
/// Real bug, live-reported 2026-07-19 ("highlight disappears, nothing
/// shows anywhere"): this function is only ever called from
/// `build_calendar_entries`, an `async fn` that runs entirely on a Tokio
/// worker thread (spawned via `tokio::spawn`/`rt.spawn`, never routed
/// through `invoke_from_event_loop`) — but it called `ww.upgrade()` and
/// `AppState::get(&w).set_discover_coming_up(...)` directly, off the UI
/// thread. `slint::Weak::upgrade()` silently returns `None` when called
/// from any thread other than the one that owns the window (confirmed
/// from `i-slint-core`'s real source, not assumed: `if
/// std::thread::current().id() != self.thread { return None; }`, no
/// panic) — so `discover-coming-up` was never actually set, on any run,
/// since this feature first shipped; the `debug!("seerr: calendar -> N
/// entries")` log line in `build_calendar_entries` (which runs BEFORE
/// this function) made the Rust-side computation look like it succeeded,
/// masking that the UI-side commit was silently failing every single
/// time. Every other UI mutation in this file follows the two-phase
/// pattern (build plain Send-safe data off-thread, construct `CardItem`
/// only inside `invoke_from_event_loop`) for exactly this reason — this
/// one function was written without it. Fixed by clamping/cloning
/// `entries` (plain `Vec<CalendarEntry>`, genuinely `Send`) before the
/// closure, and moving the `CardItem`/`AppState` mutation inside.
fn push_coming_up_row(ww: &Weak<MainWindow>, entries: &[CalendarEntry]) {
    let entries: Vec<CalendarEntry> = entries.iter().take(COMING_UP_PREVIEW_CAP).cloned().collect();
    let ww = ww.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = ww.upgrade() else { return };
        let g = AppState::get(&w);
        let mut cards: Vec<CardItem> = entries
            .iter()
            .map(|e| {
                let kind_label = match e.kind {
                    CalendarEntryKind::Theatrical => "In Theaters",
                    CalendarEntryKind::Digital => "Streaming",
                    CalendarEntryKind::Physical => "Physical Release",
                    CalendarEntryKind::Episode => "New Episode",
                };
                let subtitle = e.episode_label.clone().unwrap_or_else(|| kind_label.to_string());
                CardItem {
                    id: e.tmdb_id.as_str().into(),
                    item_type: e.item_type.into(),
                    title: e.title.as_str().into(),
                    subtitle: format!("{} · {}", e.date.format("%b %-d"), subtitle).into(),
                    ..Default::default()
                }
            })
            .collect();
        cards.push(CardItem {
            id: "".into(),
            item_type: "".into(),
            title: "📅".into(),
            subtitle: "Full Calendar".into(),
            ..Default::default()
        });
        debug!("seerr: push_coming_up_row -> {} card(s)", cards.len());
        g.set_discover_coming_up(ModelRc::new(VecModel::from(cards)));
    });
}

/// Patches posters onto the already-committed Coming Up row (2026-07-19,
/// user request — "it hust dosent have posters"), same bounded-concurrency
/// fetch-then-patch-by-index shape as `refresh_requested_row`'s own poster
/// pass. Must truncate `entries` with the SAME `COMING_UP_PREVIEW_CAP` and
/// source order `push_coming_up_row` used, since patching is by row index
/// — the id/type check on each patch is the belt-and-braces guard against
/// the two ever drifting out of sync (same pattern used everywhere else in
/// this file a poster fetch patches a model by index).
async fn fetch_coming_up_posters(ww: Weak<MainWindow>, entries: &[CalendarEntry]) {
    let poster_jobs: Vec<(usize, String, String, String)> = entries
        .iter()
        .take(COMING_UP_PREVIEW_CAP)
        .enumerate()
        .filter_map(|(idx, e)| e.poster_path.clone().map(|p| (idx, e.item_type.to_string(), e.tmdb_id.clone(), p)))
        .collect();
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
            let model = g.get_discover_coming_up();
            let Some(mut card) = model.row_data(idx) else { return };
            if card.id.as_str() != tmdb_id || card.item_type.as_str() != item_type {
                return; // row reshuffled since the fetch started — skip rather than mispatch
            }
            card.poster = slint::Image::from_rgba8(buf);
            card.has_poster = true;
            model.set_row_data(idx, card);
        });
    }
}

// ── Calendar screen (Watchlist + Release Calendar, 2026-07-18) ─────────────

/// (leading blank cells before day 1, Sunday-first; total real days in the
/// month) — pure chrono math, computed here rather than replicated in
/// Slint. Sunday-first is an arbitrary but consistent choice (this app has
/// no established locale precedent to follow either way).
fn calendar_grid_dims(year: i32, month: u32) -> (i32, i32) {
    use chrono::Datelike;
    let Some(first) = chrono::NaiveDate::from_ymd_opt(year, month, 1) else { return (0, 30) };
    let leading = first.weekday().num_days_from_sunday() as i32;
    let (next_year, next_month) = if month == 12 { (year + 1, 1) } else { (year, month + 1) };
    let total = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .map(|next| (next - first).num_days() as i32)
        .unwrap_or(30);
    (leading, total)
}

/// Rebuilds `calendar-days`/`calendar-leading-blanks`/`calendar-total-days`
/// for whatever `calendar-year`/`calendar-month` currently are — called on
/// open and after every month-nav. One `CardItem` per REAL day (no blank
/// placeholders in the model itself, see `calendar-leading-blanks`' own doc
/// comment): `title` is the day number, `unplayed-count` repurposed as the
/// day's entry count, `subtitle` is the first entry's own title (2026-07-19,
/// user request — "write you the relese in the calander instead of just a
/// small marker": `CalendarDayCell` now shows this text directly rather
/// than only a numeric pill; a day with 2+ entries still gets the count
/// pill too, alongside the title, so a second/third release isn't silently
/// dropped from the cell — the day-popup remains the place to see all of
/// them by name), `id` is unused (day index is positional).
fn push_calendar_view(g: &AppState, s: &FjordState) {
    use chrono::Datelike;
    let year = g.get_calendar_year();
    let month = g.get_calendar_month().clamp(1, 12) as u32;
    let (leading, total) = calendar_grid_dims(year, month);
    let days: Vec<CardItem> = (1..=total)
        .map(|day| {
            let day_entries: Vec<&CalendarEntry> = s
                .discover_calendar_entries
                .iter()
                .filter(|e| e.date.year() == year && e.date.month() == month && e.date.day() as i32 == day)
                .collect();
            let subtitle = day_entries.first().map(|e| e.title.clone()).unwrap_or_default();
            CardItem {
                title: day.to_string().into(),
                subtitle: subtitle.into(),
                unplayed_count: day_entries.len() as i32,
                ..Default::default()
            }
        })
        .collect();
    g.set_calendar_days(ModelRc::new(VecModel::from(days)));
    g.set_calendar_leading_blanks(leading);
    g.set_calendar_total_days(total);
}

/// Entries for a specific day, in `calendar-day-popup-entries`' own CardItem
/// shape (id/item-type = real tmdb id/type, so a popup selection can open
/// the item directly; subtitle = date-independent label, the day itself is
/// already implied by which cell was opened).
fn calendar_day_entries(s: &FjordState, year: i32, month: u32, day: i32) -> Vec<CardItem> {
    use chrono::Datelike;
    s.discover_calendar_entries
        .iter()
        .filter(|e| e.date.year() == year && e.date.month() == month && e.date.day() as i32 == day)
        .map(|e| {
            let kind_label = match e.kind {
                CalendarEntryKind::Theatrical => "In Theaters",
                CalendarEntryKind::Digital => "Streaming",
                CalendarEntryKind::Physical => "Physical Release",
                CalendarEntryKind::Episode => "New Episode",
            };
            let subtitle = e.episode_label.clone().unwrap_or_else(|| kind_label.to_string());
            CardItem {
                id: e.tmdb_id.as_str().into(),
                item_type: e.item_type.into(),
                title: e.title.as_str().into(),
                subtitle: subtitle.into(),
                ..Default::default()
            }
        })
        .collect()
}

/// Zone -1 = header row (Back=col 0, Prev month=col 1, Next month=col 2,
/// Left/Right cycle among these 3, Confirm activates whichever is
/// focused); zone >= 0 = the day grid itself (7 columns,
/// `calendar-cursor-row`/`-col` are raw grid coordinates including blank
/// cells — landing on a blank is harmless, Enter there is just inert, same
/// "gaps are fine" tolerance the Coming Up row's own sentinel already
/// established, rather than clamping arrow keys around blanks). Confirm on
/// a real day routes through the SAME `calendar-day-selected` callback the
/// mouse path uses (`on_calendar_day_selected` in `wire_discover`) rather
/// than calling `open_calendar_day_popup` directly — `keys.rs`'s per-mode
/// match arms don't hold `state`/`ww`, and funneling both input paths
/// through one callback is also what guarantees they can't diverge (the
/// mouse/keyboard focus-desync bug class documented throughout this file).
///
/// Left/Right month-changing — corrected design, same day, after a live
/// report ("the left right to change the month works when you are on the
/// back button but not when you are on the end ow a row on the
/// monthgrid... it shuld not change when you press left or right on the
/// back buttun then you shuld just navigate the buttons"). The FIRST
/// attempt made header-zone Left/Right always fire the month change
/// immediately — wrong on two counts: it fired from the Back position too
/// (the user explicitly didn't want that — Left/Right on Back should just
/// navigate, not act), and it did nothing useful in the day grid at all.
/// Reverted the header zone back to its original cursor-cycling behavior
/// (Left/Right just move among Back/Prev/Next, Confirm activates); added
/// the actual requested behavior to the DAY GRID instead — Left at the
/// leftmost column (Sunday) or Right at the rightmost column (Saturday)
/// now continues past the edge into the adjacent month, mirroring the
/// common date-picker convention of browsing days seamlessly across a
/// month boundary. Reuses `invoke_calendar_prev_month`/`_next_month`
/// directly (both already reset the cursor into the new month's grid as
/// part of changing it, so no extra cursor bookkeeping needed here either).
pub(crate) fn handle_key_calendar(action: &Action, g: &AppState) -> bool {
    let row = g.get_calendar_cursor_row();
    let col = g.get_calendar_cursor_col();
    let total_days = g.get_calendar_total_days();
    let leading = g.get_calendar_leading_blanks();
    let total_rows = ((leading + total_days) as f32 / 7.0).ceil() as i32;
    match action {
        Action::Left => {
            if row < 0 {
                g.set_calendar_cursor_col((col - 1).max(0));
            } else if col > 0 {
                g.set_calendar_cursor_col(col - 1);
            } else {
                g.invoke_calendar_prev_month();
            }
            true
        }
        Action::Right => {
            if row < 0 {
                g.set_calendar_cursor_col((col + 1).min(2));
            } else if col < 6 {
                g.set_calendar_cursor_col(col + 1);
            } else {
                g.invoke_calendar_next_month();
            }
            true
        }
        Action::Up => {
            if row < 0 {
                // already at the header — nothing above it
            } else if row == 0 {
                g.set_calendar_cursor_row(-1);
                g.set_calendar_cursor_col(0);
            } else {
                g.set_calendar_cursor_row(row - 1);
            }
            true
        }
        Action::Down => {
            if row < 0 {
                g.set_calendar_cursor_row(0);
                g.set_calendar_cursor_col(0);
            } else if row + 1 < total_rows {
                g.set_calendar_cursor_row(row + 1);
            }
            true
        }
        Action::Confirm => {
            if row < 0 {
                match col {
                    0 => g.set_show_calendar(false),
                    1 => g.invoke_calendar_prev_month(),
                    _ => g.invoke_calendar_next_month(),
                }
            } else {
                let day = row * 7 + col - leading + 1;
                if day >= 1 && day <= total_days {
                    g.invoke_calendar_day_selected(day);
                }
            }
            true
        }
        Action::Back => {
            g.set_show_calendar(false);
            true
        }
        _ => false,
    }
}

fn open_calendar_day_popup(g: &AppState, state: &Arc<Mutex<FjordState>>, day: i32) {
    let year = g.get_calendar_year();
    let month = g.get_calendar_month().clamp(1, 12) as u32;
    let entries = calendar_day_entries(&state.lock().unwrap(), year, month, day);
    if entries.is_empty() {
        return;
    }
    g.set_calendar_day_popup_entries(ModelRc::new(VecModel::from(entries)));
    g.set_calendar_day_popup_cursor(0);
    g.set_show_calendar_day_popup(true);
}

pub(crate) fn handle_key_calendar_day_popup(action: &Action, g: &AppState) -> bool {
    let n = g.get_calendar_day_popup_entries().row_count() as i32;
    let cursor = g.get_calendar_day_popup_cursor();
    match action {
        Action::Up => {
            if cursor > 0 {
                g.set_calendar_day_popup_cursor(cursor - 1);
            }
            true
        }
        Action::Down => {
            if cursor + 1 < n {
                g.set_calendar_day_popup_cursor(cursor + 1);
            }
            true
        }
        Action::Confirm => {
            g.invoke_calendar_day_popup_entry_selected(cursor);
            true
        }
        Action::Back => {
            g.set_show_calendar_day_popup(false);
            true
        }
        _ => false,
    }
}

// ── Discover filters (2026-07-18) ───────────────────────────────────────────
//
// Six pills: Type/Sort/Rating/Year are single-value (desc string shown in
// the pill, internal key/value persisted in Config); Genre/Provider are
// multi-select chip pickers (GenreItem/ProviderItem models, each row's own
// `selected` toggled independently — TMDB's with_genres/with_watch_providers
// both take pipe-separated OR, see DiscoverFilters' own doc comment in
// fjord-seerr for why). Config stores the INTERNAL representation (""/
// "movie"/"tv", ""/"rating"/"newest"/"oldest", a raw f32/u32 bucket floor,
// genre NAMES (stable across the movie/TV id-space mismatch — see
// GenreItem's own doc comment in theme.slint), provider ids (stable across
// media types, unlike genre) — never the display string, which is derived
// fresh by the *_desc functions below every time it's needed.

const SORT_KEYS: &[(&str, &str)] = &[("", "Popularity"), ("rating", "Rating"), ("newest", "Newest"), ("oldest", "Oldest")];
const RATING_BUCKETS: &[(&str, f32)] = &[("Any", 0.0), ("6+", 6.0), ("7+", 7.0), ("8+", 8.0)];
const YEAR_BUCKETS: &[(&str, u32)] = &[("Any", 0), ("2000+", 2000), ("2010+", 2010), ("2015+", 2015), ("2020+", 2020)];

fn discover_type_desc(key: &str) -> &'static str {
    match key { "movie" => "Movies", "tv" => "TV", _ => "All" }
}
fn discover_type_key(desc: &str) -> &'static str {
    match desc { "Movies" => "movie", "TV" => "tv", _ => "" }
}
fn discover_sort_desc(key: &str) -> &'static str {
    SORT_KEYS.iter().find(|(k, _)| *k == key).map(|(_, d)| *d).unwrap_or("Popularity")
}
fn discover_sort_key(desc: &str) -> &'static str {
    SORT_KEYS.iter().find(|(_, d)| *d == desc).map(|(k, _)| *k).unwrap_or("")
}
fn discover_rating_desc(v: f32) -> &'static str {
    RATING_BUCKETS.iter().find(|(_, r)| *r == v).map(|(d, _)| *d).unwrap_or("Any")
}
fn discover_rating_value(desc: &str) -> f32 {
    RATING_BUCKETS.iter().find(|(d, _)| *d == desc).map(|(_, r)| *r).unwrap_or(0.0)
}
fn discover_year_desc(v: u32) -> &'static str {
    YEAR_BUCKETS.iter().find(|(_, y)| *y == v).map(|(d, _)| *d).unwrap_or("Any")
}
fn discover_year_value(desc: &str) -> u32 {
    YEAR_BUCKETS.iter().find(|(d, _)| *d == desc).map(|(_, y)| *y).unwrap_or(0)
}

/// Whether ANY of the 6 filter dimensions is set away from its default —
/// the landing-rows / filtered-browse view switch (query empty + this ==
/// false shows the original 6 landing rows unchanged; true replaces them
/// with the filtered-browse grid).
fn discover_filters_active(cfg: &Config) -> bool {
    !cfg.discover_filter_type.is_empty()
        || !cfg.discover_filter_genre_names.is_empty()
        || !cfg.discover_filter_sort.is_empty()
        || cfg.discover_filter_min_rating > 0.0
        || cfg.discover_filter_min_year > 0
        || !cfg.discover_filter_provider_ids.is_empty()
}

/// Narrower check for `apply_search_filters` below — Type and Provider
/// deliberately don't apply to search results (Type: `/search` always
/// returns both movies and TV mixed, matching the approved plan's own
/// scope, which lists genre/sort/rating/year for search but not Type;
/// Provider: TMDB's multi-search response carries no per-item provider
/// data at all, so there's nothing to filter by — the Provider pill is
/// shown disabled while a query is active).
fn search_filters_active(cfg: &Config) -> bool {
    !cfg.discover_filter_genre_names.is_empty()
        || !cfg.discover_filter_sort.is_empty()
        || cfg.discover_filter_min_rating > 0.0
        || cfg.discover_filter_min_year > 0
}

/// The TMDB sortBy VALUE for an internal sort key, resolved per media type
/// since movies/TV genuinely use different date-sort key names (confirmed
/// from Seerr's real route source — see `DiscoverFilters`' own doc comment
/// in fjord-seerr). `None` (Popularity) means omit `sortBy` entirely —
/// Seerr/TMDB's own default is already popularity-ranked.
fn tmdb_sort_value(key: &str, media_type: &str) -> Option<&'static str> {
    match key {
        "rating" => Some("vote_average.desc"),
        "newest" => Some(if media_type == "movie" { "primary_release_date.desc" } else { "first_air_date.desc" }),
        "oldest" => Some(if media_type == "movie" { "primary_release_date.asc" } else { "first_air_date.asc" }),
        _ => None,
    }
}
fn tmdb_date_gte_key(media_type: &str) -> &'static str {
    if media_type == "movie" { "primaryReleaseDateGte" } else { "firstAirDateGte" }
}

/// Resolves the current filter selections into a real `DiscoverFilters`
/// for one specific media type's `/discover/*` call — genre NAMES are
/// re-resolved to whichever id that name has in THIS type's own raw genre
/// list (a name with no match for this type, e.g. a TV-only genre while
/// querying movies, is silently skipped rather than erroring — the same
/// "gaps are fine" tolerance this codebase uses throughout for optional
/// per-item data).
fn build_discover_filters(s: &FjordState, media_type: &str, region: &str) -> fjord_seerr::DiscoverFilters {
    let cfg = &s.config;
    let raw_genres: &[fjord_seerr::Genre] = if media_type == "movie" { &s.seerr_genres_movie } else { &s.seerr_genres_tv };
    let genre_ids: Vec<i64> = cfg
        .discover_filter_genre_names
        .iter()
        .filter_map(|name| raw_genres.iter().find(|g| &g.name == name).map(|g| g.id))
        .collect();
    fjord_seerr::DiscoverFilters {
        genre_ids: if genre_ids.is_empty() { None } else { Some(genre_ids) },
        provider_ids: if cfg.discover_filter_provider_ids.is_empty() {
            None
        } else {
            Some(cfg.discover_filter_provider_ids.clone())
        },
        watch_region: Some(region.to_string()),
        sort: tmdb_sort_value(&cfg.discover_filter_sort, media_type),
        vote_average_gte: if cfg.discover_filter_min_rating > 0.0 { Some(cfg.discover_filter_min_rating) } else { None },
        date_gte: if cfg.discover_filter_min_year > 0 {
            Some((tmdb_date_gte_key(media_type), format!("{}-01-01", cfg.discover_filter_min_year)))
        } else {
            None
        },
        date_lte: None,
    }
}

/// Merges a same-name genre from the movie and TV lists into one chip —
/// see `GenreItem`'s own doc comment (theme.slint) for why both ids are
/// tracked separately rather than assuming they match.
fn push_or_merge_genre(items: &mut Vec<GenreItem>, name: &str, movie_id: Option<i64>, tv_id: Option<i64>, selected_names: &[String]) {
    if let Some(existing) = items.iter_mut().find(|g| g.name.as_str() == name) {
        if let Some(id) = movie_id {
            existing.movie_id = id as i32;
        }
        if let Some(id) = tv_id {
            existing.tv_id = id as i32;
        }
    } else {
        items.push(GenreItem {
            movie_id: movie_id.unwrap_or(0) as i32,
            tv_id: tv_id.unwrap_or(0) as i32,
            name: name.into(),
            selected: selected_names.iter().any(|n| n == name),
        });
    }
}

fn build_genre_items(
    movie: &[fjord_seerr::Genre],
    tv: &[fjord_seerr::Genre],
    type_key: &str,
    selected_names: &[String],
) -> Vec<GenreItem> {
    let mut items: Vec<GenreItem> = Vec::new();
    match type_key {
        "movie" => {
            for g in movie {
                push_or_merge_genre(&mut items, &g.name, Some(g.id), None, selected_names);
            }
        }
        "tv" => {
            for g in tv {
                push_or_merge_genre(&mut items, &g.name, None, Some(g.id), selected_names);
            }
        }
        _ => {
            for g in movie {
                push_or_merge_genre(&mut items, &g.name, Some(g.id), None, selected_names);
            }
            for g in tv {
                push_or_merge_genre(&mut items, &g.name, None, Some(g.id), selected_names);
            }
        }
    }
    items.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    items
}

/// Provider ids ARE shared across movie/TV in TMDB's real system (unlike
/// genre ids) — this just dedupes by id across whichever list(s) apply to
/// the current Type filter, no dual-id tracking needed.
fn build_provider_items(
    movie: &[fjord_seerr::WatchProviderDetail],
    tv: &[fjord_seerr::WatchProviderDetail],
    type_key: &str,
    selected_ids: &[i64],
) -> Vec<ProviderItem> {
    let mut items: Vec<ProviderItem> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let empty: &[fjord_seerr::WatchProviderDetail] = &[];
    let sources: [&[fjord_seerr::WatchProviderDetail]; 2] = match type_key {
        "movie" => [movie, empty],
        "tv" => [tv, empty],
        _ => [movie, tv],
    };
    for list in sources {
        for p in list {
            if seen.insert(p.id) {
                items.push(ProviderItem { id: p.id as i32, name: p.name.as_str().into(), selected: selected_ids.contains(&p.id) });
            }
        }
    }
    items.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    items
}

/// Rebuilds the Genre/Provider chip models from FjordState's cached raw
/// lists — called once after `ensure_discover_filter_options`'s fetch
/// lands, and again every time the Type pill changes (no re-fetch needed,
/// the raw lists don't depend on Type).
fn refresh_discover_filter_models(g: &AppState, s: &FjordState) {
    let type_key = s.config.discover_filter_type.as_str();
    g.set_discover_filter_genres(ModelRc::new(VecModel::from(build_genre_items(
        &s.seerr_genres_movie,
        &s.seerr_genres_tv,
        type_key,
        &s.config.discover_filter_genre_names,
    ))));
    g.set_discover_filter_providers(ModelRc::new(VecModel::from(build_provider_items(
        &s.seerr_providers_movie,
        &s.seerr_providers_tv,
        type_key,
        &s.config.discover_filter_provider_ids,
    ))));
    g.set_discover_filter_genre_count(s.config.discover_filter_genre_names.len() as i32);
    g.set_discover_filter_provider_count(s.config.discover_filter_provider_ids.len() as i32);
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
fn refresh_seerr_admin_status(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else { return };
    rt.spawn(async move {
        let Ok(user) = client.get_current_user().await else { return };
        let (user_id, is_admin) = (Some(user.id), user.can_manage_requests());
        {
            let mut s = state.lock().unwrap();
            s.seerr_user_id = user_id;
            s.seerr_is_admin = is_admin;
        }
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_seerr_is_admin(is_admin);
            }
        });
    });
}

/// Fetches every page of the connected user's Watchlist (`GET
/// /discover/watchlist`) into a plain `(item_type, tmdb_id)` id set — once
/// per session, guarded by `FjordState.discover_watchlist_fetched`, same
/// shape as `discover_landing_fetched`. Deliberately fetches ALL pages, not
/// just a capped prefix like `fetch_requested_row`'s 20-item cap: unlike
/// that cap (which bounds a much more expensive per-item DETAIL fetch),
/// this is plain id/title rows with no per-item network call, so even a
/// few hundred watchlist entries is a handful of cheap list fetches — safety-
/// capped at 10 pages (200 items) so a pathological watchlist can't loop
/// forever. Best-effort: a failed page just stops pagination early rather
/// than erroring the whole fetch. Watchlist + Release Calendar, 2026-07-18.
pub(crate) fn ensure_discover_watchlist(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let client = {
        let mut s = state.lock().unwrap();
        if s.discover_watchlist_fetched {
            return;
        }
        let Some(client) = s.seerr_client.clone() else { return };
        s.discover_watchlist_fetched = true;
        client
    };
    rt.spawn(async move {
        fetch_and_store_watchlist(&client, &state, &ww).await;
    });
}

/// Shared by `ensure_discover_watchlist` and `refresh_watchlist` — fetches
/// every page, stores the resulting id set, and triggers a calendar rebuild
/// (the watchlist is one of the two sets `build_calendar_entries` unions).
async fn fetch_and_store_watchlist(client: &fjord_seerr::SeerrClient, state: &Arc<Mutex<FjordState>>, ww: &Weak<MainWindow>) {
    let mut ids: std::collections::HashSet<(&'static str, String)> = std::collections::HashSet::new();
    let mut page = 1;
    loop {
        let resp = match client.get_watchlist(page).await {
            Ok(r) => r,
            Err(e) => {
                debug!("seerr: get_watchlist page {page} failed: {e:#}");
                break;
            }
        };
        for item in &resp.results {
            let item_type = if item.media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" };
            ids.insert((item_type, item.tmdb_id.to_string()));
        }
        if page >= resp.total_pages || page >= 10 {
            break;
        }
        page += 1;
    }
    debug!("seerr: watchlist -> {} id(s)", ids.len());
    state.lock().unwrap().discover_watchlist_ids = ids;
    build_calendar_entries(Arc::clone(state), ww.clone()).await;
}

/// Re-fetches the full watchlist id set and rebuilds the calendar —
/// called right after `discover_toggle_watchlist` succeeds, mirroring
/// `refresh_requested_row`'s "cheap enough for an infrequent action" shape.
pub(crate) fn refresh_watchlist(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else { return };
    rt.spawn(async move {
        fetch_and_store_watchlist(&client, &state, &ww).await;
    });
}

/// Fetches genre + watch-provider lists (both media types) once per
/// session — same guard shape as `ensure_discover_landing`. A fetch
/// failure for any one list just leaves that picker empty (best-effort,
/// matching this codebase's existing tolerance for optional Discover
/// metadata like tags/profiles), not a hard error. Also restores
/// `discover-filters-active`/the 4 `*-desc` properties from whatever was
/// persisted last session, and — if that means filters are already active
/// — kicks off the filtered-browse fetch right here rather than leaving
/// the screen showing landing rows until the user touches a filter pill
/// (hence needing `gen`, unlike a pure fetch-and-cache function).
pub(crate) fn ensure_discover_filter_options(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    gen: Arc<AtomicU64>,
    rt: tokio::runtime::Handle,
) {
    let client = {
        let mut s = state.lock().unwrap();
        if s.discover_filter_options_fetched {
            return;
        }
        let Some(client) = s.seerr_client.clone() else { return };
        s.discover_filter_options_fetched = true;
        client
    };
    let rt2 = rt.clone();
    rt.spawn(async move {
        let region = resolve_streaming_region(&client, &state).await;
        let (movie_genres, tv_genres, movie_providers, tv_providers) = tokio::join!(
            client.get_movie_genres(),
            client.get_tv_genres(),
            client.get_movie_watch_providers(&region),
            client.get_tv_watch_providers(&region),
        );
        let movie_genres = movie_genres.unwrap_or_else(|e| {
            warn!("seerr: get_movie_genres: {e:#}");
            Vec::new()
        });
        let tv_genres = tv_genres.unwrap_or_else(|e| {
            warn!("seerr: get_tv_genres: {e:#}");
            Vec::new()
        });
        let movie_providers = movie_providers.unwrap_or_else(|e| {
            warn!("seerr: get_movie_watch_providers: {e:#}");
            Vec::new()
        });
        let tv_providers = tv_providers.unwrap_or_else(|e| {
            warn!("seerr: get_tv_watch_providers: {e:#}");
            Vec::new()
        });
        {
            let mut s = state.lock().unwrap();
            s.seerr_genres_movie = movie_genres;
            s.seerr_genres_tv = tv_genres;
            s.seerr_providers_movie = movie_providers;
            s.seerr_providers_tv = tv_providers;
        }
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let active = {
                let s = state.lock().unwrap();
                g.set_discover_filter_type_desc(discover_type_desc(&s.config.discover_filter_type).into());
                g.set_discover_filter_sort_desc(discover_sort_desc(&s.config.discover_filter_sort).into());
                g.set_discover_filter_rating_desc(discover_rating_desc(s.config.discover_filter_min_rating).into());
                g.set_discover_filter_year_desc(discover_year_desc(s.config.discover_filter_min_year).into());
                refresh_discover_filter_models(&g, &s);
                discover_filters_active(&s.config)
            };
            g.set_discover_filters_active(active);
            // Filters were already active last session — show the filtered-
            // browse view immediately rather than landing rows until the
            // user touches a pill. query must still be empty (a saved
            // in-progress search query isn't persisted, but this fires
            // before the user could have typed anything new yet either way).
            if active && g.get_discover_query().as_str().is_empty() {
                spawn_discover_filtered_browse(ww.clone(), Arc::clone(&state), Arc::clone(&gen), &rt2);
            }
        });
    });
}

/// Client-side genre/rating/year/sort pass over the full raw fetch history
/// for the CURRENT search (`FjordState.discover_search_metas`, accumulated
/// across every page `spawn_discover_search`/`_more` have committed) —
/// rebuilds `discover-results` from it. `/search` accepts no filter params
/// at all (see `DiscoverFilters`' own doc comment in fjord-seerr), so this
/// is the only way genre/rating/year/sort can apply to search results at
/// all; `search_filters_active` deliberately excludes Type and Provider —
/// see that function's own doc comment. No-op (leaves `discover-results`
/// alone) when no relevant filter is set, so callers can call this
/// unconditionally after every search commit without a wasted rebuild.
///
/// Preserves already-decoded poster Images for kept rows by looking them
/// up BY ID from the model's CURRENT content, not by index — filtering can
/// reorder/remove rows, so an index-based carry-forward would silently
/// mismatch. This is why this is only ever called strictly AFTER
/// `fetch_and_patch_posters` has finished patching the full unfiltered
/// list (posters are correctly placed by then); calling it any earlier
/// would race that patch's own index-based lookups.
pub(crate) fn apply_search_filters(state: &Arc<Mutex<FjordState>>, ww: &Weak<MainWindow>) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    let s = state.lock().unwrap();
    if !search_filters_active(&s.config) {
        return;
    }
    let cfg = &s.config;
    let genre_names: std::collections::HashSet<&str> = cfg.discover_filter_genre_names.iter().map(String::as_str).collect();
    // A search result's genre_ids come back in whichever id-space matches
    // its OWN media_type — resolve every selected NAME to every id it
    // could appear as (movie side or TV side) so matching works regardless
    // of which type the name was originally selected under.
    let genre_ids: std::collections::HashSet<i64> = if genre_names.is_empty() {
        std::collections::HashSet::new()
    } else {
        s.seerr_genres_movie
            .iter()
            .chain(s.seerr_genres_tv.iter())
            .filter(|g| genre_names.contains(g.name.as_str()))
            .map(|g| g.id)
            .collect()
    };
    let min_rating = cfg.discover_filter_min_rating;
    let min_year = cfg.discover_filter_min_year;
    let sort_key = cfg.discover_filter_sort.clone();

    let mut kept: Vec<DiscoverCardMeta> = s
        .discover_search_metas
        .iter()
        .filter(|m| genre_ids.is_empty() || m.genre_ids.iter().any(|id| genre_ids.contains(id)))
        .filter(|m| min_rating <= 0.0 || m.vote_average >= min_rating as f64)
        .filter(|m| min_year == 0 || m.year >= min_year as i32)
        .cloned()
        .collect();
    match sort_key.as_str() {
        "rating" => kept.sort_by(|a, b| b.vote_average.partial_cmp(&a.vote_average).unwrap_or(std::cmp::Ordering::Equal)),
        "newest" => kept.sort_by_key(|m| std::cmp::Reverse(m.year)),
        "oldest" => kept.sort_by_key(|m| m.year),
        _ => {} // Popularity — keep TMDB's own original relevance order
    }
    drop(s);

    let old = g.get_discover_results();
    let old_posters: std::collections::HashMap<(String, String), (slint::Image, bool)> = (0..old.row_count())
        .filter_map(|i| old.row_data(i))
        .map(|c| ((c.id.to_string(), c.item_type.to_string()), (c.poster.clone(), c.has_poster)))
        .collect();
    let cards: Vec<CardItem> = kept
        .into_iter()
        .map(|m| {
            let key = (m.id.clone(), m.item_type.to_string());
            let mut card = m.into_card_item();
            if let Some((poster, has_poster)) = old_posters.get(&key) {
                card.poster = poster.clone();
                card.has_poster = *has_poster;
            }
            card
        })
        .collect();
    let len = cards.len() as i32;
    g.set_discover_results(ModelRc::new(VecModel::from(cards)));
    g.set_discover_focused(g.get_discover_focused().clamp(0, (len - 1).max(0)));
    maybe_autofill_grid(&g);
}

/// One filtered-browse page's raw results, tagged with poster path — mirrors
/// `RequestedRowItem`'s own (meta, poster_path) shape for the identical
/// reason: the poster path has to travel alongside its meta through
/// `merge_filtered_metas`' re-sort, since an index-based zip (the pattern
/// `spawn_discover_search` itself uses) would break the moment merging
/// reorders rows.
type FilteredRowItem = (DiscoverCardMeta, Option<String>);

fn build_filtered_metas(results: &[SearchResult]) -> Vec<FilteredRowItem> {
    results.iter().filter_map(|r| search_result_to_meta(r).map(|m| (m, r.poster_path.clone()))).collect()
}

/// Merges movie + TV filtered-browse results into one grid for Type=All —
/// confirmed via `AskUserQuestion`: interleaved by the ACTUAL value of
/// whichever sort key is active (both types' `popularity`/`vote_average`
/// are directly comparable; Newest/Oldest compare `year`, already
/// normalized to a plain int regardless of which date field it came from),
/// not the simpler movies-then-TV split. Implemented as concatenate-then-
/// sort rather than a true two-pointer merge — the two inputs are already
/// server-sorted, but re-sorting the small (≤2 pages') combined list
/// outright is simpler code for the identical final order.
fn merge_filtered_metas(movie: Vec<FilteredRowItem>, tv: Vec<FilteredRowItem>, sort_key: &str) -> Vec<FilteredRowItem> {
    let mut merged: Vec<FilteredRowItem> = movie.into_iter().chain(tv).collect();
    match sort_key {
        "rating" => merged.sort_by(|a, b| b.0.vote_average.partial_cmp(&a.0.vote_average).unwrap_or(std::cmp::Ordering::Equal)),
        "newest" => merged.sort_by_key(|m| std::cmp::Reverse(m.0.year)),
        "oldest" => merged.sort_by_key(|m| m.0.year),
        _ => merged.sort_by(|a, b| b.0.popularity.partial_cmp(&a.0.popularity).unwrap_or(std::cmp::Ordering::Equal)),
    }
    merged
}

/// Discover filters' filtered-browse view (query empty, ≥1 filter active) —
/// page 1. Mirrors `spawn_discover_search`'s two-phase (text-then-posters)
/// commit shape closely, but sources from `discover_movies_filtered`/
/// `discover_tv_filtered` (real server-side filtering) instead of
/// `client.search`, and fetches both media types in parallel when Type is
/// "All" (`tokio::join!`, same shape `ensure_discover_landing` already
/// uses for its own 6-way parallel fetch), merging via
/// `merge_filtered_metas`. Shares the exact same `discover_gen` counter
/// `spawn_discover_search` uses — required, not optional: without it, a
/// slow debounced search response landing after this commits (or vice
/// versa) would clobber `discover-results` with a stale patch, since both
/// write into the same model.
pub(crate) fn spawn_discover_filtered_browse(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    gen: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = gen.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        warn!("seerr: filtered-browse dispatched with no seerr_client set — not connected?");
        return;
    };
    {
        let mut s = state.lock().unwrap();
        s.discover_filtered_page = 0;
        s.discover_filtered_total_pages_movie = 0;
        s.discover_filtered_total_pages_tv = 0;
        s.discover_filtered_loading_more = false;
    }
    let is_session_auth = client.is_session_auth();

    rt.spawn(async move {
        let (type_key, sort_key) = {
            let s = state.lock().unwrap();
            (s.config.discover_filter_type.clone(), s.config.discover_filter_sort.clone())
        };
        let region = resolve_streaming_region(&client, &state).await;
        if gen.load(Ordering::SeqCst) != my_gen {
            return; // superseded before the region lookup even finished
        }
        let want_movie = type_key != "tv";
        let want_tv = type_key != "movie";
        let (movie_filters, tv_filters) = {
            let s = state.lock().unwrap();
            (build_discover_filters(&s, "movie", &region), build_discover_filters(&s, "tv", &region))
        };
        let (movie_res, tv_res) = tokio::join!(
            async { if want_movie { Some(client.discover_movies_filtered(1, &movie_filters).await) } else { None } },
            async { if want_tv { Some(client.discover_tv_filtered(1, &tv_filters).await) } else { None } },
        );
        if gen.load(Ordering::SeqCst) != my_gen {
            return; // a newer filter change / search already superseded this
        }
        let movie_resp = match movie_res {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                handle_seerr_error(&state, &ww, is_session_auth, "Discover filter (movies) failed", &e);
                None
            }
            None => None,
        };
        let tv_resp = match tv_res {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                handle_seerr_error(&state, &ww, is_session_auth, "Discover filter (TV) failed", &e);
                None
            }
            None => None,
        };
        if movie_resp.is_none() && tv_resp.is_none() {
            return; // both wanted sides failed (error already surfaced above)
        }

        let movie_metas = movie_resp.as_ref().map(|r| build_filtered_metas(&r.results)).unwrap_or_default();
        let tv_metas = tv_resp.as_ref().map(|r| build_filtered_metas(&r.results)).unwrap_or_default();
        {
            let mut s = state.lock().unwrap();
            s.discover_filtered_page = 1;
            s.discover_filtered_total_pages_movie = movie_resp.as_ref().map(|r| r.total_pages).unwrap_or(0);
            s.discover_filtered_total_pages_tv = tv_resp.as_ref().map(|r| r.total_pages).unwrap_or(0);
            s.discover_filtered_loading_more = false;
        }
        let merged = merge_filtered_metas(movie_metas, tv_metas, &sort_key);
        debug!("seerr: filtered-browse page 1 (type={type_key:?}) -> {} card(s)", merged.len());

        let poster_jobs: Vec<(usize, String, String, String)> = merged
            .iter()
            .enumerate()
            .filter_map(|(i, (m, p))| p.clone().map(|p| (i, m.item_type.to_string(), m.id.clone(), p)))
            .collect();

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_commit.upgrade() {
                let g = AppState::get(&w);
                let cards: Vec<CardItem> = merged.into_iter().map(|(m, _)| m.into_card_item()).collect();
                g.set_discover_results(ModelRc::new(VecModel::from(cards)));
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
                maybe_autofill_grid(&g);
            }
        });

        fetch_and_patch_posters(ww, gen, my_gen, poster_jobs).await;
    });
}

/// Filtered-browse's own load-more — mirrors `spawn_discover_search_more`
/// exactly, including the same synchronous-offset-read-before-network-call
/// safety (see that function's own comment for why it's race-safe against
/// a fresh fetch landing first). Both underlying TMDB pages (movie + TV)
/// advance together in lockstep rather than tracking two independent
/// cursors (confirmed via `AskUserQuestion` — simpler, and requesting one
/// page past a side that's already exhausted just returns an empty result
/// for that side, which merges in as a no-op); stops once BOTH sides are
/// exhausted (`max` of the two `total_pages`, not `min` — so a Type=All
/// browse doesn't stop early just because the shorter-tailed type ran out
/// first).
pub(crate) fn spawn_discover_filtered_browse_more(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    gen: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = gen.load(Ordering::SeqCst);
    let (client, next_page, type_key, sort_key) = {
        let mut s = state.lock().unwrap();
        if s.discover_filtered_loading_more {
            return;
        }
        let max_total = s.discover_filtered_total_pages_movie.max(s.discover_filtered_total_pages_tv);
        if s.discover_filtered_page == 0 || s.discover_filtered_page >= max_total {
            return;
        }
        let Some(client) = s.seerr_client.clone() else { return };
        s.discover_filtered_loading_more = true;
        (client, s.discover_filtered_page + 1, s.config.discover_filter_type.clone(), s.config.discover_filter_sort.clone())
    };
    let is_session_auth = client.is_session_auth();
    let offset = ww.upgrade().map(|w| AppState::get(&w).get_discover_results().row_count()).unwrap_or(0);

    let state2 = Arc::clone(&state);
    rt.spawn(async move {
        let region = resolve_streaming_region(&client, &state2).await;
        if gen.load(Ordering::SeqCst) != my_gen {
            state2.lock().unwrap().discover_filtered_loading_more = false;
            return;
        }
        let want_movie = type_key != "tv";
        let want_tv = type_key != "movie";
        let (movie_filters, tv_filters) = {
            let s = state2.lock().unwrap();
            (build_discover_filters(&s, "movie", &region), build_discover_filters(&s, "tv", &region))
        };
        let (movie_res, tv_res) = tokio::join!(
            async { if want_movie { Some(client.discover_movies_filtered(next_page, &movie_filters).await) } else { None } },
            async { if want_tv { Some(client.discover_tv_filtered(next_page, &tv_filters).await) } else { None } },
        );
        if gen.load(Ordering::SeqCst) != my_gen {
            state2.lock().unwrap().discover_filtered_loading_more = false;
            return;
        }
        let movie_resp = match movie_res {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                state2.lock().unwrap().discover_filtered_loading_more = false;
                handle_seerr_error(&state2, &ww, is_session_auth, "Discover filter (movies) failed", &e);
                None
            }
            None => None,
        };
        let tv_resp = match tv_res {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                state2.lock().unwrap().discover_filtered_loading_more = false;
                handle_seerr_error(&state2, &ww, is_session_auth, "Discover filter (TV) failed", &e);
                None
            }
            None => None,
        };
        if movie_resp.is_none() && tv_resp.is_none() {
            state2.lock().unwrap().discover_filtered_loading_more = false;
            return;
        }
        let movie_metas = movie_resp.as_ref().map(|r| build_filtered_metas(&r.results)).unwrap_or_default();
        let tv_metas = tv_resp.as_ref().map(|r| build_filtered_metas(&r.results)).unwrap_or_default();
        {
            let mut s = state2.lock().unwrap();
            s.discover_filtered_page = next_page;
            if let Some(r) = &movie_resp {
                s.discover_filtered_total_pages_movie = r.total_pages;
            }
            if let Some(r) = &tv_resp {
                s.discover_filtered_total_pages_tv = r.total_pages;
            }
            s.discover_filtered_loading_more = false;
        }
        let merged = merge_filtered_metas(movie_metas, tv_metas, &sort_key);
        debug!("seerr: filtered-browse page {next_page} (type={type_key:?}) -> {} more card(s)", merged.len());

        let poster_jobs: Vec<(usize, String, String, String)> = merged
            .iter()
            .enumerate()
            .filter_map(|(i, (m, p))| p.clone().map(|p| (offset + i, m.item_type.to_string(), m.id.clone(), p)))
            .collect();

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_commit.upgrade() {
                let g = AppState::get(&w);
                let existing = g.get_discover_results();
                let mut all: Vec<CardItem> = (0..existing.row_count()).filter_map(|i| existing.row_data(i)).collect();
                all.extend(merged.into_iter().map(|(m, _)| m.into_card_item()));
                g.set_discover_results(ModelRc::new(VecModel::from(all)));
                maybe_autofill_grid(&g);
            }
        });

        fetch_and_patch_posters(ww, gen, my_gen, poster_jobs).await;
    });
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
    /// 2K/4K user-facing status labels ("Requested"/"Needs Approval"/
    /// "Processing"/"Partially Available"/"Available"/"Declined"/"") — see
    /// `tier_status_label`'s own doc comment. Blank means that tier is
    /// still requestable.
    status_label: String,
    status4k_label: String,
    /// The request the Discover context menu's Edit/Cancel/Approve/Decline
    /// rows should act on when opened from this page's ⋮ More button — see
    /// `pick_primary_request`'s own doc comment for the tiebreak when both
    /// tiers have an active request. "" when neither tier has one.
    request_id: String,
    request_pending: bool,
    request_mine: bool,
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
    // Watchlist + Release Calendar (2026-07-18) — MovieDetails/TvDetails.
    // onUserWatchlist verbatim.
    on_watchlist: bool,
}

/// One tier's user-facing status label, combining Seerr's two independent
/// status signals — `MediaStatus` (fulfillment: is the file available yet)
/// and the request's own `MediaRequestStatus` (workflow: has an admin
/// approved it yet) — into one string. `availability_tag` alone
/// (fulfillment only) can't distinguish "needs an admin to approve it" from
/// "approved, waiting on Radarr/Sonarr" — both read as blank/Requested
/// without the request's own status. Real gap, live-reported 2026-07-18:
/// "it shuld reflect the status, like if its aproved or needs aprovment
/// etc." `request.status == 3` is `MediaRequestStatus::Declined` (see
/// `MediaRequestStatus`'s own doc comment in fjord-seerr — no local const,
/// matching the same raw-int style `requested_not_available` already uses
/// for the identical check).
fn tier_status_label(status: Option<MediaStatus>, request: Option<&fjord_seerr::MediaRequest>) -> String {
    if status == Some(MediaStatus::Available) {
        return "Available".to_string();
    }
    // request.status is MediaRequestStatus (1=Pending 2=Approved 3=Declined
    // 4=Failed 5=Completed — see fjord-seerr's own doc comment). Pending/
    // Declined/Failed are unambiguous regardless of media fulfillment
    // status; Approved(2)/Completed(5) fall through to the fulfillment-
    // driven labels below, defaulting to "Approved" if fulfillment hasn't
    // progressed yet (waiting on Radarr/Sonarr to pick it up).
    if let Some(r) = request {
        match r.status {
            1 => return "Needs Approval".to_string(),
            3 => return "Declined".to_string(),
            4 => return "Failed".to_string(),
            _ => {}
        }
    }
    match status {
        Some(MediaStatus::Processing) => "Processing".to_string(),
        Some(MediaStatus::PartiallyAvailable) => "Partially Available".to_string(),
        _ if request.is_some() => "Approved".to_string(),
        _ => String::new(),
    }
}

/// The one `MediaRequest` for a given tier, from `MediaInfo.requests`
/// (only populated on the single-item detail endpoints — see its own doc
/// comment in fjord-seerr).
fn tier_request(mi: Option<&fjord_seerr::MediaInfo>, is4k: bool) -> Option<&fjord_seerr::MediaRequest> {
    mi?.requests.iter().find(|r| r.is4k == is4k)
}

/// Resolves the `(request_id, pending, mine)` triple the Discover context
/// menu's Edit/Cancel/Approve/Decline rows need, for whichever ONE request
/// this page's ⋮ More button should act on. When both tiers have an active
/// request (a real, if rarer, case — see the Discover grid's own "Also
/// requested in 2K/4K" badge), prefers the 4K one — an arbitrary but
/// documented tiebreak, not a full per-tier action UI; easy to revisit if
/// it turns out to matter in practice.
fn pick_primary_request(
    req_2k: Option<&fjord_seerr::MediaRequest>,
    req_4k: Option<&fjord_seerr::MediaRequest>,
    my_user_id: Option<i64>,
) -> (String, bool, bool) {
    match req_4k.or(req_2k) {
        Some(r) => {
            let mine =
                my_user_id.zip(r.requested_by.as_ref().map(|rb| rb.id)).map(|(mine, theirs)| mine == theirs).unwrap_or(true);
            (r.id.to_string(), r.is_pending(), mine)
        }
        None => (String::new(), false, false),
    }
}

fn movie_fields(d: MovieDetails, region: &str, my_user_id: Option<i64>) -> DetailFields {
    let year = d.release_date.as_deref().filter(|s| s.len() >= 4).map(|s| &s[..4]).unwrap_or("");
    let genres = d.genres.iter().map(|g| g.name.clone()).collect::<Vec<_>>().join(", ");
    let cast = build_cast_list(&d.credits);
    let providers = resolve_providers(&d.watch_providers, region);
    let trailer_url = find_trailer_url(&d.related_videos);
    let req_2k = tier_request(d.media_info.as_ref(), false);
    let req_4k = tier_request(d.media_info.as_ref(), true);
    let status_label = tier_status_label(d.media_info.as_ref().and_then(|mi| mi.status()), req_2k);
    let status4k_label = tier_status_label(d.media_info.as_ref().and_then(|mi| mi.status4k()), req_4k);
    let (request_id, request_pending, request_mine) = pick_primary_request(req_2k, req_4k, my_user_id);
    DetailFields {
        title: d.title,
        meta: if genres.is_empty() { year.to_string() } else { format!("{year} · {genres}") },
        overview: d.overview.unwrap_or_default(),
        rating: format_rating(d.vote_average),
        poster_path: d.poster_path,
        backdrop_path: d.backdrop_path,
        status_label,
        status4k_label,
        request_id,
        request_pending,
        request_mine,
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
        on_watchlist: d.on_user_watchlist,
    }
}

fn tv_fields(d: TvDetails, region: &str, my_user_id: Option<i64>) -> DetailFields {
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
    let req_2k = tier_request(d.media_info.as_ref(), false);
    let req_4k = tier_request(d.media_info.as_ref(), true);
    let status_label = tier_status_label(d.media_info.as_ref().and_then(|mi| mi.status()), req_2k);
    let status4k_label = tier_status_label(d.media_info.as_ref().and_then(|mi| mi.status4k()), req_4k);
    let (request_id, request_pending, request_mine) = pick_primary_request(req_2k, req_4k, my_user_id);
    DetailFields {
        title: d.name,
        meta: if genres.is_empty() { year.to_string() } else { format!("{year} · {genres}") },
        overview: d.overview.unwrap_or_default(),
        rating: format_rating(d.vote_average),
        poster_path: d.poster_path,
        backdrop_path: d.backdrop_path,
        status_label,
        status4k_label,
        request_id,
        request_pending,
        request_mine,
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
        on_watchlist: d.on_user_watchlist,
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
    open_discover_item_ex(media_type, tmdb_id_str, state, ww, rt, PostOpenAction::None, true);
}

/// `check_local_library`: `true` for every existing call site (View
/// Details/Request/Edit Request) — unchanged behavior. `false` only for the
/// Discover context menu's "View Request" row (2026-07-18): a card with a
/// known Seerr request (`context-menu-request-id != ""`) can ALSO be
/// partially present in the local Jellyfin library (e.g. a series missing
/// some seasons) — real bug, live-reported: "if like for a series you have
/// partial you cant get to request detail only to the series detail even
/// trouhu the context menu." Per the user's own suggested fix (asked, not
/// assumed — offered "always skip the redirect" and "only when partial" as
/// the two obvious options, and the user proposed a third: add a dedicated
/// row instead), View Details/Request/Edit Request keep redirecting to the
/// real Jellyfin item exactly as before; only this new row bypasses it.
fn open_discover_item_ex(
    media_type: String,
    tmdb_id_str: String,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    post_action: PostOpenAction,
    check_local_library: bool,
) {
    if check_local_library {
        if let Some((id, item_type)) = find_local_item(&state, &media_type, &tmdb_id_str) {
            crate::detail::open_detail(id, item_type, state, ww, rt);
            return;
        }
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
        g.set_request_detail_status_4k("".into());
        g.set_request_detail_request_id("".into());
        g.set_request_detail_request_pending(false);
        g.set_request_detail_request_mine(false);
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
        // Needed to resolve "mine" for the ⋮ More button's request (below) —
        // cheap synchronous read, same value ensure_discover_landing/
        // fetch_requested_row already use for the identical purpose.
        let my_user_id = state.lock().unwrap().seerr_user_id;

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
                    client.get_movie(tmdb_id).await.map(|d| movie_fields(d, &region, my_user_id))
                } else {
                    client.get_tv(tmdb_id).await.map(|d| tv_fields(d, &region, my_user_id))
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
            g.set_request_detail_status(fields.status_label.as_str().into());
            g.set_request_detail_status_4k(fields.status4k_label.as_str().into());
            g.set_request_detail_request_id(fields.request_id.as_str().into());
            g.set_request_detail_request_pending(fields.request_pending);
            g.set_request_detail_request_mine(fields.request_mine);
            g.set_request_detail_on_watchlist(fields.on_watchlist);
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

/// Patches `request_id`/`request_pending`/`request_mine` onto whichever
/// search-grid card matches `(media_type, tmdb_id)`, if visible — the
/// `discover-results` counterpart to `patch_discover_card_availability`
/// above, for the 3 fields that one doesn't touch. Real bug fixed
/// 2026-07-18: submitting a request from the search grid left that same
/// card's context menu still offering "Request" until the next full
/// landing-row refresh, since only `availability` was ever patched here.
fn patch_discover_card_request_state(g: &AppState, media_type: &str, tmdb_id: i64, request_id: &str, pending: bool, mine: bool) {
    let item_type = if media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" };
    let id_str = tmdb_id.to_string();
    let model = g.get_discover_results();
    for i in 0..model.row_count() {
        if let Some(mut card) = model.row_data(i) {
            if card.id.as_str() == id_str && card.item_type.as_str() == item_type {
                card.request_id = request_id.into();
                card.request_pending = pending;
                card.request_mine = mine;
                model.set_row_data(i, card);
                break;
            }
        }
    }
}

/// Patches `on-watchlist` in place on every Discover card model that might
/// be showing this item — the search grid AND all 8 landing rows (a
/// watchlisted item can legitimately appear in Trending/Popular/Upcoming/
/// etc, not just Requested), matching `discover_request_action`'s own
/// "patch every model, don't just pick one" shape. Watchlist + Release
/// Calendar, 2026-07-18.
fn patch_watchlist_on_all_models(g: &AppState, item_type: &str, tmdb_id: i64, on_watchlist: bool) {
    let id_str = tmdb_id.to_string();
    let mut models = vec![g.get_discover_results()];
    for row in 0..8 {
        models.push(landing_row_get(g, row));
    }
    for model in models {
        for i in 0..model.row_count() {
            if let Some(mut card) = model.row_data(i) {
                if card.id.as_str() == id_str && card.item_type.as_str() == item_type {
                    card.on_watchlist = on_watchlist;
                    model.set_row_data(i, card);
                }
            }
        }
    }
}

/// Add/remove Watchlist — wired from the Discover context menu's Watchlist
/// row and RequestDetailScreen's Watchlist button. POST/DELETE, then
/// patches every visible card + updates the id cache + rebuilds the
/// calendar (a watchlist change is one of the two things that can change
/// what's on it). Watchlist + Release Calendar, 2026-07-18.
pub(crate) fn discover_toggle_watchlist(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    tmdb_id: i64,
    media_type: String,
    title: String,
    adding: bool,
) {
    debug!("seerr: discover_toggle_watchlist tmdb={tmdb_id} media_type={media_type} adding={adding}");
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    let item_type: &'static str = if media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" };
    let rt2 = rt.clone();

    rt.spawn(async move {
        let result = if adding {
            client.add_watchlist(tmdb_id, &media_type, &title).await
        } else {
            client.remove_watchlist(tmdb_id, &media_type).await
        };
        match result {
            Ok(()) => {
                debug!("seerr: discover_toggle_watchlist succeeded tmdb={tmdb_id} adding={adding}");
                {
                    let mut s = state.lock().unwrap();
                    let key = (item_type, tmdb_id.to_string());
                    if adding {
                        s.discover_watchlist_ids.insert(key);
                    } else {
                        s.discover_watchlist_ids.remove(&key);
                    }
                }
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        patch_watchlist_on_all_models(&g, item_type, tmdb_id, adding);
                        if g.get_show_request_detail()
                            && g.get_request_detail_media_type().as_str() == media_type
                            && g.get_request_detail_tmdb_id() == tmdb_id as i32
                        {
                            g.set_request_detail_on_watchlist(adding);
                        }
                    }
                });
                show_toast(ww.clone(), if adding { "Added to Watchlist" } else { "Removed from Watchlist" }.into());
                refresh_watchlist(Arc::clone(&state), ww, rt2);
            }
            Err(e) => handle_seerr_error(&state, &ww, is_session_auth, "Couldn't update watchlist", &e),
        }
    });
}

pub(crate) fn submit_request(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    // Guard against double-submitting the SAME tier that's currently
    // selected in the modal — not "any status exists at all." 2K and 4K are
    // independently requestable (real bug fixed 2026-07-18: requesting 4K
    // used to blank out the whole Request flow, hiding 2K too — see
    // tier_status_label's own doc comment for the full story).
    let is_4k = g.get_request_detail_want_4k();
    let tier_already_requested =
        if is_4k { g.get_request_detail_status_4k().as_str() != "" } else { g.get_request_detail_status().as_str() != "" };
    if g.get_request_detail_requesting() || tier_already_requested {
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
            Ok(req) => {
                let ww2 = ww.clone();
                let mt = media_type.clone();
                let request_id = req.id.to_string();
                let pending = req.is_pending();
                {
                    let mut s = state.lock().unwrap();
                    s.discover_known_requests.insert(
                        (if media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" }, tmdb_id.to_string()),
                        KnownRequest { request_id: request_id.clone(), pending, mine: true },
                    );
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        g.set_request_detail_requesting(false);
                        // Only the tier that was actually just requested —
                        // the other tier's own status is untouched, so it
                        // stays requestable (real bug fixed 2026-07-18: this
                        // used to blank out BOTH tiers regardless of is_4k).
                        if is_4k {
                            g.set_request_detail_status_4k("Requested".into());
                        } else {
                            g.set_request_detail_status("Requested".into());
                        }
                        patch_discover_card_availability(&g, &mt, tmdb_id, "requested");
                        // Real bug fixed 2026-07-18 — see
                        // patch_discover_card_request_state's own doc comment.
                        patch_discover_card_request_state(&g, &mt, tmdb_id, &request_id, pending, true);
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
/// a full row rebuild for an infrequent admin action). Also reloads
/// `RequestDetailScreen` (via `open_discover_item`) if it's open for the
/// exact item this request belongs to (added when the ⋮ More button gave
/// this page its own path to these three actions) — simpler than hand-
/// patching request-detail-status/-4k/-request-id per tier.
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
    let rt2 = rt.clone();
    rt.spawn(async move {
        let result = match action {
            "cancel" => client.delete_request(request_id).await,
            "approve" => client.approve_request(request_id).await,
            _ => client.decline_request(request_id).await,
        };
        // Real bug fixed 2026-07-18: Approve never patched request_pending
        // anywhere, so a non-admin could still see and attempt "Cancel
        // Request" on an already-approved request (which then fails
        // server-side, since DELETE requires status==PENDING). Also keeps
        // discover_known_requests in sync for both outcomes — approve
        // updates it to pending=false, cancel/decline remove the entry
        // entirely (the request no longer exists).
        let req_key = request_id.to_string();
        {
            let mut s = state.lock().unwrap();
            if remove_on_success {
                // cancel/decline: the request no longer exists.
                s.discover_known_requests.retain(|_, k| k.request_id != req_key);
            } else {
                // approve: still exists, just no longer Pending.
                if let Some(k) = s.discover_known_requests.values_mut().find(|k| k.request_id == req_key) {
                    k.pending = false;
                }
            }
        }
        match result {
            Ok(()) => {
                let ww2 = ww.clone();
                let state2 = Arc::clone(&state);
                let rt3 = rt2.clone();
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
                        // The item itself may also be visible in the search
                        // grid (unaffected by the Requested-row removal
                        // above) — clear its now-stale request state there
                        // too rather than leaving it pointed at a
                        // cancelled/declined request id.
                        let results = g.get_discover_results();
                        for i in 0..results.row_count() {
                            if let Some(mut card) = results.row_data(i) {
                                if card.request_id.as_str() == request_id.to_string() {
                                    card.request_id = "".into();
                                    card.request_pending = false;
                                    card.request_mine = false;
                                    results.set_row_data(i, card);
                                    break;
                                }
                            }
                        }
                    } else {
                        // Approve: patch request_pending=false in place on
                        // every model the card might currently be visible
                        // in, rather than removing it — an approved request
                        // stays in "Requested" until it's actually fulfilled.
                        for model in [g.get_discover_requested(), g.get_discover_results()] {
                            for i in 0..model.row_count() {
                                if let Some(mut card) = model.row_data(i) {
                                    if card.request_id.as_str() == request_id.to_string() {
                                        card.request_pending = false;
                                        model.set_row_data(i, card);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    // If the Discover detail page is open for the exact item
                    // this request belongs to, reload it fresh rather than
                    // hand-patching request-detail-status/-4k/-request-id —
                    // simpler than tracking which tier this request_id was
                    // for, and matches refresh_requested_row's own
                    // "just refetch" approach. Otherwise the page would keep
                    // showing a request that no longer exists (Cancel) or a
                    // stale Pending/"Needs Approval" label (Approve/Decline).
                    if g.get_show_request_detail() && g.get_request_detail_request_id().as_str() == request_id.to_string() {
                        let media_type = g.get_request_detail_media_type().to_string();
                        let tmdb_id = g.get_request_detail_tmdb_id().to_string();
                        open_discover_item(media_type, tmdb_id, state2, ww2.clone(), rt3);
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

/// Shared tail of every filter-change callback below (2026-07-18):
/// persists the change, then re-triggers whichever view is actually
/// relevant right now — a fresh filtered-browse fetch (query empty,
/// filters now active), a client-side re-filter (query non-empty), or
/// nothing extra (query empty, filters now all default — the landing rows
/// are already loaded and untouched; the Slint side's own view switch just
/// shows them again once `discover-results` is cleared).
fn on_discover_filter_changed(state: &Arc<Mutex<FjordState>>, ww: &Weak<MainWindow>, gen: &Arc<AtomicU64>, rt: &tokio::runtime::Handle) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    let active = {
        let s = state.lock().unwrap();
        save_config(&s.config);
        discover_filters_active(&s.config)
    };
    g.set_discover_filters_active(active);
    if g.get_discover_query().as_str().is_empty() {
        if active {
            spawn_discover_filtered_browse(ww.clone(), Arc::clone(state), Arc::clone(gen), rt);
        } else {
            g.set_discover_results(ModelRc::new(VecModel::from(Vec::<CardItem>::new())));
        }
    } else {
        apply_search_filters(state, ww);
    }
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
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |nav| {
            if nav == 6 {
                ensure_discover_landing(Arc::clone(&state), ww.clone(), rt.clone());
                spawn_movies_list_fetch(Arc::clone(&state), ww.clone(), rt.clone(), false);
                ensure_discover_filter_options(Arc::clone(&state), ww.clone(), Arc::clone(&gen), rt.clone());
                // Watchlist + Release Calendar, 2026-07-18 — same
                // once-per-session guard shape as ensure_discover_landing.
                ensure_discover_watchlist(Arc::clone(&state), ww.clone(), rt.clone());
                // Real bug, 2026-07-18: seerr-is-admin was only ever fetched
                // once per connection (spawn_seerr_settings_fetch at startup/
                // connect) and never refreshed, so a server-side permission
                // change mid-session never showed up in Approve/Decline
                // visibility without a reconnect. Non-blocking — the menu
                // still opens instantly with whatever's cached; this just
                // makes the NEXT open correct.
                refresh_seerr_admin_status(Arc::clone(&state), ww.clone(), rt.clone());
            } else if let Some(w) = ww.upgrade() {
                // Leaving Discover: a filter popup left open, or the filter
                // bar left active, otherwise silently reappears (backdrop
                // and all) the next time the user returns — real bug,
                // 2026-07-18. This is the single hook every sidebar tab
                // switch already funnels through (mouse NavItem.clicked AND
                // browse::sidebar_nav's keyboard cycle both call
                // nav-selected), so it's a more robust reset point than
                // touching every NavItem handler in layout.slint by hand.
                let g = AppState::get(&w);
                g.set_discover_popup_open("".into());
                g.set_discover_filter_bar_active(false);
            }
        }
    });

    // ── Discover filters (2026-07-18) ──────────────────────────────────────
    g.on_discover_filter_type_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |desc| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let key = discover_type_key(desc.as_str());
            {
                let mut s = state.lock().unwrap();
                s.config.discover_filter_type = key.to_string();
            }
            g.set_discover_filter_type_desc(discover_type_desc(key).into());
            // Genre/Provider's own selectable list depends on Type (a
            // movie-only or TV-only genre shouldn't be pickable while the
            // other type is excluded) — rebuild both from the already-
            // cached raw lists, no re-fetch needed.
            {
                let s = state.lock().unwrap();
                refresh_discover_filter_models(&g, &s);
            }
            on_discover_filter_changed(&state, &ww, &gen, &rt);
        }
    });

    g.on_discover_filter_sort_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |desc| {
            let key = discover_sort_key(desc.as_str());
            {
                let mut s = state.lock().unwrap();
                s.config.discover_filter_sort = key.to_string();
            }
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_discover_filter_sort_desc(discover_sort_desc(key).into());
            }
            on_discover_filter_changed(&state, &ww, &gen, &rt);
        }
    });

    g.on_discover_filter_rating_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |desc| {
            let value = discover_rating_value(desc.as_str());
            {
                let mut s = state.lock().unwrap();
                s.config.discover_filter_min_rating = value;
            }
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_discover_filter_rating_desc(discover_rating_desc(value).into());
            }
            on_discover_filter_changed(&state, &ww, &gen, &rt);
        }
    });

    g.on_discover_filter_year_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |desc| {
            let value = discover_year_value(desc.as_str());
            {
                let mut s = state.lock().unwrap();
                s.config.discover_filter_min_year = value;
            }
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_discover_filter_year_desc(discover_year_desc(value).into());
            }
            on_discover_filter_changed(&state, &ww, &gen, &rt);
        }
    });

    // Genre/Provider: multi-select, toggled by row index — the row's own
    // `selected` flips in the already-mounted model in place (cheap,
    // matches TagItem's own toggle pattern in RequestOptionsOverlay), then
    // Config's persisted name/id list is rebuilt from whichever rows ended
    // up selected.
    g.on_discover_filter_genre_toggle({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_discover_filter_genres();
            let Some(mut item) = model.row_data(idx as usize) else { return };
            item.selected = !item.selected;
            model.set_row_data(idx as usize, item);
            let names: Vec<String> = (0..model.row_count())
                .filter_map(|i| model.row_data(i))
                .filter(|g| g.selected)
                .map(|g| g.name.to_string())
                .collect();
            g.set_discover_filter_genre_count(names.len() as i32);
            state.lock().unwrap().config.discover_filter_genre_names = names;
            on_discover_filter_changed(&state, &ww, &gen, &rt);
        }
    });

    g.on_discover_filter_provider_toggle({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let model = g.get_discover_filter_providers();
            let Some(mut item) = model.row_data(idx as usize) else { return };
            item.selected = !item.selected;
            model.set_row_data(idx as usize, item);
            let ids: Vec<i64> =
                (0..model.row_count()).filter_map(|i| model.row_data(i)).filter(|p| p.selected).map(|p| p.id as i64).collect();
            g.set_discover_filter_provider_count(ids.len() as i32);
            state.lock().unwrap().config.discover_filter_provider_ids = ids;
            on_discover_filter_changed(&state, &ww, &gen, &rt);
        }
    });

    g.on_discover_filter_clear({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let gen = Arc::clone(&discover_gen);
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            {
                let mut s = state.lock().unwrap();
                s.config.discover_filter_type = String::new();
                s.config.discover_filter_genre_names.clear();
                s.config.discover_filter_sort = String::new();
                s.config.discover_filter_min_rating = 0.0;
                s.config.discover_filter_min_year = 0;
                s.config.discover_filter_provider_ids.clear();
            }
            g.set_discover_filter_type_desc(discover_type_desc("").into());
            g.set_discover_filter_sort_desc(discover_sort_desc("").into());
            g.set_discover_filter_rating_desc(discover_rating_desc(0.0).into());
            g.set_discover_filter_year_desc(discover_year_desc(0).into());
            {
                let s = state.lock().unwrap();
                refresh_discover_filter_models(&g, &s);
            }
            on_discover_filter_changed(&state, &ww, &gen, &rt);
        }
    });

    // Mouse-click equivalents of keyboard Confirm — reuse the exact same
    // dispatch functions as the keyboard path (see their own doc comments
    // above) rather than a second, independently-written click handler, so
    // mouse and keyboard can never disagree about what a pill/option/chip
    // does. discover.slint's click handlers set -bar-focused/-popup-cursor
    // to the clicked index first, then invoke these.
    g.on_discover_filter_bar_confirm({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            handle_key_discover_filter_bar(&Action::Confirm, &AppState::get(&w));
        }
    });
    g.on_discover_popup_confirm({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            handle_key_discover_popup(&Action::Confirm, &AppState::get(&w));
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
            if query.is_empty() {
                // Filtered-browse's own pagination (2026-07-18) — landing
                // rows (no filters active) have nothing to load more of.
                if discover_filters_active(&state.lock().unwrap().config) {
                    spawn_discover_filtered_browse_more(ww.clone(), Arc::clone(&state), Arc::clone(&gen), &rt);
                }
                return;
            }
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
            g.set_context_menu_on_watchlist(item.on_watchlist);
            debug!(
                "seerr: discover context menu opened for {} ({}) request_id={:?} pending={} mine={} seerr-is-admin={}",
                item.id, item.item_type, item.request_id, item.request_pending, item.request_mine, g.get_seerr_is_admin(),
            );
            g.set_context_menu_focused(0);
            g.set_show_context_menu(true);
        }
    });

    // RequestDetailScreen's own ⋮ More button (2026-07-18) — same
    // context-menu-* population as on_open_context_menu_discover above, but
    // sourced from request-detail-* state (no CardItem exists for this
    // page). request-detail-request-id/-pending/-mine are resolved by
    // discover.rs::pick_primary_request when the item's own detail loads.
    g.on_open_discover_menu_from_detail({
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let item_type = if g.get_request_detail_media_type().as_str() == "movie" { "DiscoverMovie" } else { "DiscoverTv" };
            g.set_context_menu_item_id(g.get_request_detail_tmdb_id().to_string().as_str().into());
            g.set_context_menu_item_type(item_type.into());
            g.set_context_menu_title(g.get_request_detail_title());
            g.set_context_menu_request_id(g.get_request_detail_request_id());
            g.set_context_menu_request_pending(g.get_request_detail_request_pending());
            g.set_context_menu_request_mine(g.get_request_detail_request_mine());
            g.set_context_menu_on_watchlist(g.get_request_detail_on_watchlist());
            debug!(
                "seerr: discover menu opened from detail page for {} ({}) request_id={:?} pending={} mine={}",
                g.get_request_detail_tmdb_id(), item_type, g.get_request_detail_request_id(),
                g.get_request_detail_request_pending(), g.get_request_detail_request_mine(),
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

    // "View Request" (2026-07-18) — only shown when context-menu-request-id
    // is non-empty (context_menu.slint); unlike View Details, deliberately
    // skips the find_local_item redirect so a partially-available item's
    // Seerr request stays reachable even though it's also (partly) in the
    // Jellyfin library. See open_discover_item_ex's own doc comment.
    g.on_context_discover_view_request({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" { "movie" } else { "tv" };
            let tmdb_id = g.get_context_menu_item_id().to_string();
            g.set_show_context_menu(false);
            open_discover_item_ex(media_type.into(), tmdb_id, Arc::clone(&state), ww.clone(), rt.clone(), PostOpenAction::None, false);
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
                true,
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
                true,
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

    // Watchlist + Release Calendar (2026-07-18) — always visible in the
    // Discover context menu, unlike Request's own availability gating.
    g.on_context_discover_toggle_watchlist({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = if g.get_context_menu_item_type().as_str() == "DiscoverMovie" { "movie" } else { "tv" };
            let raw_id = g.get_context_menu_item_id();
            let Ok(tmdb_id) = raw_id.parse::<i64>() else {
                warn!("seerr: on_context_discover_toggle_watchlist: bad tmdb id {raw_id:?}, item_type={:?}", g.get_context_menu_item_type());
                return;
            };
            let adding = !g.get_context_menu_on_watchlist();
            let title = g.get_context_menu_title().to_string();
            g.set_show_context_menu(false);
            discover_toggle_watchlist(Arc::clone(&state), ww.clone(), rt.clone(), tmdb_id, media_type.into(), title, adding);
        }
    });

    // RequestDetailScreen's own Watchlist button (2026-07-18) — same toggle,
    // sourced from request-detail-* state since this page has no CardItem.
    g.on_request_detail_toggle_watchlist({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let media_type = g.get_request_detail_media_type().to_string();
            let tmdb_id = g.get_request_detail_tmdb_id() as i64;
            let adding = !g.get_request_detail_on_watchlist();
            let title = g.get_request_detail_title().to_string();
            discover_toggle_watchlist(Arc::clone(&state), ww.clone(), rt.clone(), tmdb_id, media_type, title, adding);
        }
    });

    // ── Calendar screen (2026-07-18, Watchlist + Release Calendar) ──────────
    g.on_open_calendar({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let today = chrono::Local::now().date_naive();
            g.set_calendar_year(chrono::Datelike::year(&today));
            g.set_calendar_month(chrono::Datelike::month(&today) as i32);
            g.set_calendar_cursor_row(-1);
            g.set_calendar_cursor_col(0);
            g.set_show_calendar_day_popup(false);
            {
                let s = state.lock().unwrap();
                push_calendar_view(&g, &s);
            }
            g.set_show_calendar(true);
        }
    });

    g.on_calendar_prev_month({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let (mut y, mut m) = (g.get_calendar_year(), g.get_calendar_month());
            m -= 1;
            if m < 1 { m = 12; y -= 1; }
            g.set_calendar_year(y);
            g.set_calendar_month(m);
            g.set_calendar_cursor_row(0);
            g.set_calendar_cursor_col(0);
            push_calendar_view(&g, &state.lock().unwrap());
        }
    });

    g.on_calendar_next_month({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let (mut y, mut m) = (g.get_calendar_year(), g.get_calendar_month());
            m += 1;
            if m > 12 { m = 1; y += 1; }
            g.set_calendar_year(y);
            g.set_calendar_month(m);
            g.set_calendar_cursor_row(0);
            g.set_calendar_cursor_col(0);
            push_calendar_view(&g, &state.lock().unwrap());
        }
    });

    // Mouse click on a day cell — mirrors handle_key_calendar's Confirm arm
    // for the day-grid case, so mouse and keyboard can't diverge.
    g.on_calendar_day_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        move |day| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_calendar_cursor_row((day - 1 + g.get_calendar_leading_blanks()) / 7);
            g.set_calendar_cursor_col((day - 1 + g.get_calendar_leading_blanks()) % 7);
            open_calendar_day_popup(&g, &state, day);
        }
    });

    // Mouse click on a day-popup entry — mirrors
    // handle_key_calendar_day_popup's Confirm arm.
    g.on_calendar_day_popup_entry_selected({
        let state = Arc::clone(&state);
        let ww = window.as_weak();
        let rt = rt.clone();
        move |idx| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let Some(entry) = g.get_calendar_day_popup_entries().row_data(idx as usize) else { return };
            g.set_calendar_day_popup_cursor(idx);
            let media_type = if entry.item_type.as_str() == "DiscoverMovie" { "movie" } else { "tv" };
            g.set_show_calendar_day_popup(false);
            g.set_show_calendar(false);
            open_discover_item(media_type.into(), entry.id.to_string(), Arc::clone(&state), ww.clone(), rt.clone());
        }
    });
}

// ── Keyboard: Discover filter bar + popups (2026-07-18) ─────────────────────
//
// A 4th Discover keyboard mechanism, alongside the search field's raw
// pre-dispatch, `fs<0` sidebar, and `fs>=0` grid/landing — real, non-trivial
// wiring, not a drop-in "nearest zone" hand-off (see this module's own
// notes elsewhere on why). Two states: `discover-filter-bar-active` (no
// popup open — Left/Right move the pill cursor, matching Library grid's
// own sort-bar contract of "cursor moves freely, Enter applies"), and
// `discover-popup-open` (a popup is open — captures ALL input, same as
// Settings' own `settings-dropdown-open` capture, but built fresh here
// since `SettingsDropdown` itself has no keyboard path of its own to
// reuse — see `ensure_discover_filter_options`'s own doc comment).

fn discover_popup_options(kind: &str) -> Vec<&'static str> {
    match kind {
        "type" => vec!["All", "Movies", "TV"],
        "sort" => SORT_KEYS.iter().map(|(_, d)| *d).collect(),
        "rating" => RATING_BUCKETS.iter().map(|(d, _)| *d).collect(),
        "year" => YEAR_BUCKETS.iter().map(|(d, _)| *d).collect(),
        _ => Vec::new(),
    }
}

/// Opens a popup with the cursor pre-set to the currently active value's
/// index — same "cursor starts on the current selection" convention
/// `SettingsDropdown`'s own popup already uses. Genre/Provider (multi-
/// select chip strips, not a single current value) just start at 0.
fn open_discover_popup(g: &AppState, kind: &'static str) {
    let cursor = match kind {
        "genre" | "provider" => 0,
        _ => {
            let current = match kind {
                "type" => g.get_discover_filter_type_desc(),
                "sort" => g.get_discover_filter_sort_desc(),
                "rating" => g.get_discover_filter_rating_desc(),
                "year" => g.get_discover_filter_year_desc(),
                _ => Default::default(),
            };
            discover_popup_options(kind).iter().position(|&d| d == current.as_str()).unwrap_or(0)
        }
    };
    g.set_discover_popup_cursor(cursor as i32);
    g.set_discover_popup_open(kind.into());
}

/// `discover-popup-open != ""` — captures all input. Type/Sort/Rating/Year
/// are single-value lists (Up/Down move the cursor, Confirm applies +
/// closes); Genre/Provider are multi-select chip strips (Left/Right move
/// the cursor, Confirm toggles the chip at the cursor WITHOUT closing —
/// same "stays open for further toggling" shape `RequestOptionsOverlay`'s
/// own tag chips already use).
fn handle_key_discover_popup(action: &Action, g: &AppState) -> bool {
    let kind = g.get_discover_popup_open().to_string();
    match kind.as_str() {
        "type" | "sort" | "rating" | "year" => {
            let options = discover_popup_options(&kind);
            match action {
                Action::Up => {
                    let c = g.get_discover_popup_cursor();
                    if c > 0 {
                        g.set_discover_popup_cursor(c - 1);
                    }
                    true
                }
                Action::Down => {
                    let c = g.get_discover_popup_cursor();
                    if (c as usize) + 1 < options.len() {
                        g.set_discover_popup_cursor(c + 1);
                    }
                    true
                }
                Action::Confirm => {
                    let desc: slint::SharedString =
                        options.get(g.get_discover_popup_cursor() as usize).copied().unwrap_or("").into();
                    match kind.as_str() {
                        "type" => g.invoke_discover_filter_type_selected(desc),
                        "sort" => g.invoke_discover_filter_sort_selected(desc),
                        "rating" => g.invoke_discover_filter_rating_selected(desc),
                        "year" => g.invoke_discover_filter_year_selected(desc),
                        _ => {}
                    }
                    g.set_discover_popup_open("".into());
                    true
                }
                Action::Back => {
                    g.set_discover_popup_open("".into());
                    true
                }
                _ => true,
            }
        }
        "genre" | "provider" => {
            let count =
                if kind == "genre" { g.get_discover_filter_genres().row_count() } else { g.get_discover_filter_providers().row_count() }
                    as i32;
            match action {
                Action::Left => {
                    let c = g.get_discover_popup_cursor();
                    if c > 0 {
                        g.set_discover_popup_cursor(c - 1);
                    }
                    true
                }
                Action::Right => {
                    let c = g.get_discover_popup_cursor();
                    if c + 1 < count {
                        g.set_discover_popup_cursor(c + 1);
                    }
                    true
                }
                Action::Confirm => {
                    let c = g.get_discover_popup_cursor();
                    if kind == "genre" {
                        g.invoke_discover_filter_genre_toggle(c);
                    } else {
                        g.invoke_discover_filter_provider_toggle(c);
                    }
                    true // stays open — multi-select
                }
                Action::Back => {
                    g.set_discover_popup_open("".into());
                    true
                }
                _ => true,
            }
        }
        _ => false, // popup-open had an unrecognized value — shouldn't happen
    }
}

/// `discover-filter-bar-active` (no popup open) — the pill row itself.
/// Pill order: 0=Type 1=Genre 2=Sort 3=Rating 4=Year 5=Provider 6=Clear
/// (matches `discover-filter-bar-focused`'s own doc comment in
/// app_state.slint and `discover.slint`'s left-to-right rendering order).
fn handle_key_discover_filter_bar(action: &Action, g: &AppState) -> bool {
    match action {
        Action::Left => {
            let f = g.get_discover_filter_bar_focused();
            if f > 0 {
                g.set_discover_filter_bar_focused(f - 1);
            }
            true
        }
        Action::Right => {
            let f = g.get_discover_filter_bar_focused();
            if f < 6 {
                g.set_discover_filter_bar_focused(f + 1);
            }
            true
        }
        Action::Up => {
            g.set_discover_filter_bar_active(false);
            g.set_discover_header_focused(true);
            true
        }
        Action::Down => {
            g.set_discover_filter_bar_active(false);
            // Filtered-browse (query empty, >=1 filter active) uses the flat
            // discover-results grid, not the landing-row models — same
            // routing fix as handle_key's own `landing` check above.
            if g.get_discover_query().as_str().is_empty() && !g.get_discover_filters_active() {
                if let Some(first) = landing_row_lens(g).iter().position(|&n| n > 0) {
                    g.set_focused_section(first as i32);
                    g.set_discover_landing_card(0);
                }
            } else if g.get_discover_results().row_count() > 0 {
                g.set_focused_section(0);
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
            }
            true
        }
        Action::Back => {
            g.set_discover_filter_bar_active(false);
            true
        }
        Action::Confirm => {
            match g.get_discover_filter_bar_focused() {
                0 => open_discover_popup(g, "type"),
                1 => open_discover_popup(g, "genre"),
                2 => open_discover_popup(g, "sort"),
                3 => open_discover_popup(g, "rating"),
                4 => open_discover_popup(g, "year"),
                5 => open_discover_popup(g, "provider"),
                6 => g.invoke_discover_filter_clear(),
                _ => {}
            }
            true
        }
        _ => true,
    }
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
    if !g.get_discover_popup_open().as_str().is_empty() {
        return handle_key_discover_popup(action, g);
    }
    if g.get_discover_filter_bar_active() {
        return handle_key_discover_filter_bar(action, g);
    }

    let fs = g.get_focused_section();
    // Filtered-browse (query empty, >=1 filter active) renders into the same
    // discover-results grid search uses, not the landing-row models — must
    // route through the flat-grid dispatch below, not handle_key_landing.
    let landing = g.get_discover_query().as_str().is_empty() && !g.get_discover_filters_active();

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
            Action::Up => {
                g.set_discover_filter_bar_active(true);
                true
            }
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
                g.set_discover_filter_bar_active(true);
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
                g.set_discover_filter_bar_active(true);
            }
            true
        }
        Action::Down => {
            if (fs as usize) + 1 < lens.len() {
                let nf = fs + 1;
                g.set_focused_section(nf);
                g.set_discover_landing_card(g.get_discover_landing_card().min((lens[nf as usize] - 1).max(0)));
                debug!("seerr: landing down fs={fs}->{nf} lens={lens:?} card={}", g.get_discover_landing_card());
                true
            } else {
                false // last row — let focus_bar_on_down handle it
            }
        }
        Action::Confirm => {
            let c = g.get_discover_landing_card().max(0);
            // Real bug caught in review before shipping: handle_key_landing
            // is one generic function shared by all 8 rows, deriving
            // media_type from whatever card is focused with no per-card
            // special case — without this check, Enter on the "Coming Up"
            // row's trailing sentinel (no real tmdb id) would try to open a
            // Discover item for garbage data instead of the calendar
            // (Watchlist + Release Calendar, 2026-07-18).
            if fs as usize == LANDING_ROW_COMING_UP && c == count - 1 && count > 0 {
                g.invoke_open_calendar();
            } else if c < count {
                if let Some(card) = landing_row_get(g, fs as usize).row_data(c as usize) {
                    let media_type = if card.item_type.as_str() == "DiscoverMovie" { "movie" } else { "tv" };
                    g.invoke_open_discover_item(media_type.into(), card.id);
                }
            }
            true
        }
        Action::OpenContextMenu => {
            let c = g.get_discover_landing_card().max(0);
            // Same sentinel special-case as Confirm above — right-click/`C`
            // on the fake last card must be inert, not open a context menu
            // for nothing.
            if fs as usize == LANDING_ROW_COMING_UP && c == count - 1 && count > 0 {
                return true;
            }
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

/// Which of zone 0's (button row) 3 possible slots exist for the current
/// item — same "gaps are fine" idiom as `existing_zones`/
/// `existing_discover_menu_rows`: 0=Request (at least one tier still ""),
/// 1=Trailer (found + yt-dlp available), 2=⋮ More (a request already
/// exists). Slot values are indices into `request-detail-btn-focused`, not
/// a positional/visual-order constraint — `request_detail.slint` renders
/// them in this same 0/1/2 order, so here they also happen to match, unlike
/// the Discover context menu's row 5 (see context_menu.rs's own note on
/// that).
fn existing_detail_btn_slots(g: &AppState) -> Vec<i32> {
    let mut slots = Vec::new();
    if g.get_request_detail_status().as_str() == "" || g.get_request_detail_status_4k().as_str() == "" {
        slots.push(0);
    }
    if !g.get_request_detail_trailer_url().as_str().is_empty() && g.get_yt_dlp_available() {
        slots.push(1);
    }
    if !g.get_request_detail_request_id().as_str().is_empty() {
        slots.push(2);
    }
    slots.push(3); // Watchlist (2026-07-18) — always visible, unlike Request
    slots
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

    // Zone 0: Request button (visible while at least one tier is still
    // requestable) + up to two tier-status pills (non-interactive) + an
    // independent Trailer button (Watch Trailer is unrelated to request
    // status — see request-detail-trailer-url's own doc comment) + ⋮ More
    // (only once a request exists). Confirm on Request opens the Request
    // Options modal rather than submitting directly; 4K/tags/seasons are
    // configured there, not on this page.
    let slots = existing_detail_btn_slots(g);
    // Clamp request-detail-btn-focused to whatever's actually present.
    // Recomputed on every zone-0 key press rather than only at zone-entry,
    // so it self-corrects regardless of which transition landed here —
    // same "gaps are fine" idiom as existing_discover_menu_rows's own
    // Up/Down, generalized from the original hardcoded Request/Trailer
    // binary once ⋮ More became a third possible slot (2026-07-18).
    if !slots.contains(&g.get_request_detail_btn_focused()) {
        g.set_request_detail_btn_focused(slots.first().copied().unwrap_or(0));
    }
    let btn_focused = g.get_request_detail_btn_focused();
    let slot_pos = slots.iter().position(|&s| s == btn_focused).unwrap_or(0);
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
        Action::Left => match slot_pos.checked_sub(1).and_then(|i| slots.get(i)) {
            Some(&prev) => {
                g.set_request_detail_btn_focused(prev);
                true
            }
            None => false,
        },
        Action::Right => match slots.get(slot_pos + 1) {
            Some(&next) => {
                g.set_request_detail_btn_focused(next);
                true
            }
            None => false,
        },
        Action::Confirm => {
            match btn_focused {
                0 => g.invoke_open_request_options(),
                1 => g.invoke_play_trailer(),
                2 => g.invoke_open_discover_menu_from_detail(),
                _ => {}
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
