// ── fjord-app · discover/landing.rs ──────────────────────────────────────────
//   ensure_discover_landing    fetches the 5 no-query landing rows (Trending/Popular
//                              Movies/Popular TV/Upcoming Movies/Upcoming TV) once per
//                              session (FjordState.discover_landing_fetched guard), on
//                              first nav arrival at Discover; same text-first-then-posters
//                              two-phase commit as search
//   landing_row_get/_set/_lens  AppState accessors for the 9 fixed landing-row lists
//                              (0=Trending..8=Watchlist), shared by the fetch and by
//                              handle_key's landing branch; deliberately explicit `7 =>`/`8 =>`
//                              arms, not a catch-all — a catch-all here would silently alias a
//                              future 9th row instead of failing to compile (real gap caught
//                              by an independent plan review when Watchlist/row 8 was added,
//                              2026-07-20)
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
//   fetch_new_in_theaters        canned DiscoverFilters preset (primaryReleaseDateGte=today-45d,
//                              primaryReleaseDateLte=today, sort=popularity.desc) over the
//                              existing discover_movies_filtered — an honest approximation,
//                              Seerr's /discover/movies has no verified "still showing" signal
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Landing rows (Trending / Popular / Upcoming, shown when query == "") ───

// Row indices, named — used by handle_key_landing's sentinel special-case
// (Watchlist + Release Calendar, 2026-07-18) so that check doesn't depend
// on a bare literal matching this match's own row ordering.
pub(crate) const LANDING_ROW_NEW_IN_THEATERS: usize = 6;

pub(crate) const LANDING_ROW_COMING_UP: usize = 7;
// Row 8 = Watchlist (2026-07-20), appended at the end — not inserted —
// the established "append, don't insert" rule this codebase already
// follows for landing-row indices, avoiding the renumbering risk a
// mid-list insert would carry. No named const: unlike Coming Up it has no
// sentinel card / no keyboard special-case, so nothing outside
// landing_row_get/_set/_lens needs to know its index by name. Deliberately
// NOT deduped against the other 5 discovery rows either (Trending/Popular/
// Upcoming/New in Theaters) — the existing dedup-against-Requested logic
// below is a special case for Requested specifically, not a general "hide
// personal-list items elsewhere" rule; Coming Up already sets the
// precedent of not needing one.

// landing_row_get/_set deliberately end in explicit `7 =>`/`8 =>` arms, NOT
// a catch-all `_ =>` — a catch-all here would silently alias a future 9th
// row to whichever arm the catch-all resolves to instead of failing to
// compile (real gap caught by an independent plan review before this row
// was added, 2026-07-20).
pub(crate) fn landing_row_get(g: &AppState, idx: usize) -> ModelRc<CardItem> {
    match idx {
        0 => g.get_discover_trending(),
        1 => g.get_discover_popular_movies(),
        2 => g.get_discover_popular_tv(),
        3 => g.get_discover_upcoming_movies(),
        4 => g.get_discover_upcoming_tv(),
        5 => g.get_discover_requested(),
        6 => g.get_discover_new_in_theaters(),
        7 => g.get_discover_coming_up(),
        _ => g.get_discover_watchlist_mixed(),
    }
}

pub(crate) fn landing_row_set(g: &AppState, idx: usize, model: ModelRc<CardItem>) {
    match idx {
        0 => g.set_discover_trending(model),
        1 => g.set_discover_popular_movies(model),
        2 => g.set_discover_popular_tv(model),
        3 => g.set_discover_upcoming_movies(model),
        4 => g.set_discover_upcoming_tv(model),
        5 => g.set_discover_requested(model),
        6 => g.set_discover_new_in_theaters(model),
        7 => g.set_discover_coming_up(model),
        _ => g.set_discover_watchlist_mixed(model),
    }
}

pub(crate) fn landing_row_lens(g: &AppState) -> [i32; 9] {
    std::array::from_fn(|i| landing_row_get(g, i).row_count() as i32)
}

// Both take `&RequestEntry` (not its ~7 fields unpacked as loose scalars —
// clippy's too-many-arguments, and every one of these values already lives
// on `entry` at both call sites in `fetch_requested_row`) rather than the
// per-field signature these had before `request_id`/`request_pending`/
// `request_mine` were added.
fn movie_details_to_meta(
    tmdb_id: i64,
    d: &MovieDetails,
    entry: &RequestEntry,
) -> (DiscoverCardMeta, Option<String>) {
    let year = d
        .release_date
        .as_deref()
        .filter(|s| s.len() >= 4)
        .map(|s| &s[..4])
        .unwrap_or("");
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

fn tv_details_to_meta(
    tmdb_id: i64,
    d: &TvDetails,
    entry: &RequestEntry,
) -> (DiscoverCardMeta, Option<String>) {
    let year = d
        .first_air_date
        .as_deref()
        .filter(|s| s.len() >= 4)
        .map(|s| &s[..4])
        .unwrap_or("");
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

pub(crate) type RequestedRowItem = (DiscoverCardMeta, Option<String>);

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
    media_type: &'static str,
    r: &fjord_seerr::MediaRequest,
    dual_tier_tmdb_ids: &std::collections::HashSet<i64>,
    my_user_id: Option<i64>,
) -> Option<RequestEntry> {
    let media = r.media.as_ref()?;
    let tmdb_id = media.tmdb_id?;
    let (requested_status, other_status) = if r.is4k {
        (media.status4k(), media.status())
    } else {
        (media.status(), media.status4k())
    };
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
    let mine = my_user_id
        .zip(r.requested_by.as_ref().map(|rb| rb.id))
        .map(|(mine, theirs)| mine == theirs)
        .unwrap_or(true);
    debug!(
        "seerr: request_entry {media_type} tmdb={tmdb_id} request_id={} is4k={} status={} pending={} \
         requested_by={:?} my_user_id={my_user_id:?} mine={mine}",
        r.id,
        r.is4k,
        r.status,
        r.is_pending(),
        r.requested_by.as_ref().map(|rb| rb.id),
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
        let Some(tmdb_id) = r.media.as_ref().and_then(|m| m.tmdb_id) else {
            continue;
        };
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
async fn fetch_requested_row(
    client: &fjord_seerr::SeerrClient,
    my_user_id: Option<i64>,
) -> Vec<RequestedRowItem> {
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
        .chain(
            tv.iter()
                .filter_map(|r| request_entry("tv", r, &dual_tv_ids, my_user_id)),
        )
        .collect();
    entries.sort_by(|a, b| b.created_at.cmp(&a.created_at)); // newest requested first
    entries.truncate(20);

    let n = entries.len();
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set: tokio::task::JoinSet<(usize, Option<RequestedRowItem>)> =
        tokio::task::JoinSet::new();
    for (idx, entry) in entries.into_iter().enumerate() {
        let client = client.clone();
        let sem = Arc::clone(&sem);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let item = if entry.media_type == "movie" {
                client
                    .get_movie(entry.tmdb_id)
                    .await
                    .ok()
                    .map(|d| movie_details_to_meta(entry.tmdb_id, &d, &entry))
            } else {
                client
                    .get_tv(entry.tmdb_id)
                    .await
                    .ok()
                    .map(|d| tv_details_to_meta(entry.tmdb_id, &d, &entry))
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
pub(crate) fn refresh_requested_row(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let (client, my_user_id) = {
        let s = state.lock().unwrap();
        let Some(client) = s.seerr_client.clone() else {
            return;
        };
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
            .filter_map(|(idx, (m, poster_path))| {
                poster_path
                    .clone()
                    .map(|p| (idx, m.item_type.to_string(), m.id.clone(), p))
            })
            .collect();
        debug!(
            "seerr: refresh_requested_row -> {} card(s)",
            requested.len()
        );
        // metas (not CardItem) crosses the thread boundary — CardItem carries
        // a slint::Image field and is `!Send` regardless of whether it's
        // populated (same reason ensure_discover_landing's own commit closure
        // builds CardItem only inside invoke_from_event_loop, never before).
        let metas: Vec<DiscoverCardMeta> = requested.into_iter().map(|(m, _)| m).collect();
        let ww2 = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww2.upgrade() {
                let cards: Vec<CardItem> = metas
                    .into_iter()
                    .map(DiscoverCardMeta::into_card_item)
                    .collect();
                AppState::get(&w).set_discover_requested(ModelRc::new(VecModel::from(cards)));
            }
        });
        if poster_jobs.is_empty() {
            return;
        }
        let Ok(http) = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
        else {
            return;
        };
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut set = tokio::task::JoinSet::new();
        for (idx, item_type, tmdb_id, poster_path) in poster_jobs {
            let http = http.clone();
            let sem = Arc::clone(&sem);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let cache_key = format!(
                    "{}-{}",
                    if item_type == "DiscoverMovie" {
                        "movie"
                    } else {
                        "tv"
                    },
                    tmdb_id
                );
                let bytes =
                    fetch_tmdb_image(&http, TMDB_POSTER_BASE, &poster_path, &cache_key).await?;
                let buf = decode_poster_buffer(&bytes)?;
                Some((idx, item_type, tmdb_id, buf))
            });
        }
        while let Some(res) = set.join_next().await {
            let Ok(Some((idx, item_type, tmdb_id, buf))) = res else {
                continue;
            };
            let ww2 = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww2.upgrade() else { return };
                let g = AppState::get(&w);
                let model = g.get_discover_requested();
                let Some(mut card) = model.row_data(idx) else {
                    return;
                };
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
async fn fetch_new_in_theaters(
    client: &fjord_seerr::SeerrClient,
) -> anyhow::Result<fjord_seerr::SearchResponse> {
    let today = chrono::Local::now().date_naive();
    let six_weeks_ago = today - chrono::Duration::days(45);
    let filters = fjord_seerr::DiscoverFilters {
        sort: Some("popularity.desc"),
        date_gte: Some((
            "primaryReleaseDateGte",
            six_weeks_ago.format("%Y-%m-%d").to_string(),
        )),
        date_lte: Some((
            "primaryReleaseDateLte",
            today.format("%Y-%m-%d").to_string(),
        )),
        ..Default::default()
    };
    client.discover_movies_filtered(1, &filters).await
}

pub(crate) fn ensure_discover_landing(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let (client, my_user_id) = {
        let mut s = state.lock().unwrap();
        if s.discover_landing_fetched {
            return;
        }
        let Some(client) = s.seerr_client.clone() else {
            return;
        };
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
                    // Real bug, live-reported 2026-08-12 ("Gran Hermano...
                    // have the Mentalis's poster image" / "Law & order
                    // missing poster on popular tv shows row but have
                    // poster when you go in to the detail"). `metas` used
                    // to be built by filter_map-ing `search_result_to_meta`
                    // over `results` (which drops BLOCKLISTED items, not
                    // just non-movie/tv ones), while the poster job's own
                    // zip separately re-filtered `results` using only a
                    // media_type check — the two filters disagreed on
                    // blocklisted entries, so a single blocklisted item
                    // anywhere in a row's raw results silently shifted
                    // every SUBSEQUENT poster-job pairing in that row by
                    // one position, assigning the wrong title's
                    // poster_path (or none at all, once the shift ran past
                    // the end) to every card after it. Fixed by deriving
                    // metas and their poster jobs from one single filter
                    // pass instead of two independently-filtered views of
                    // the same data — they can no longer drift apart
                    // because there's only one filtering decision left,
                    // made once, per item.
                    let results: Vec<_> = resp.results.into_iter()
                        .filter(|r| {
                            let item_type = if r.media_type == "movie" { "DiscoverMovie" } else { "DiscoverTv" };
                            !requested_keys.contains(&(item_type, r.id.to_string()))
                        })
                        .collect();
                    let mut metas: Vec<DiscoverCardMeta> = Vec::with_capacity(results.len());
                    let mut jobs: Vec<(usize, usize, String, String, String)> = Vec::new();
                    for r in &results {
                        let Some(mut m) = search_result_to_meta(r) else { continue };
                        patch_known_request_state(&mut m, &known);
                        patch_watchlist_state(&mut m, &watchlist_ids);
                        if let Some(p) = r.poster_path.clone() {
                            jobs.push((row, metas.len(), m.item_type.to_string(), m.id.clone(), p));
                        }
                        metas.push(m);
                    }
                    debug!("seerr: landing row {} ({}) -> {} card(s)", row, ROW_NAMES[row], metas.len());
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
                    // Same bug + fix as the rows-0-4 loop above (see its own
                    // comment) — metas and jobs built together in one pass.
                    let results: Vec<_> = resp.results.into_iter()
                        .filter(|r| !requested_keys.contains(&("DiscoverMovie", r.id.to_string())))
                        .collect();
                    let mut metas: Vec<DiscoverCardMeta> = Vec::with_capacity(results.len());
                    let mut jobs: Vec<(usize, usize, String, String, String)> = Vec::new();
                    for r in &results {
                        let Some(mut m) = search_result_to_meta(r) else { continue };
                        patch_known_request_state(&mut m, &known);
                        patch_watchlist_state(&mut m, &watchlist_ids);
                        if let Some(p) = r.poster_path.clone() {
                            jobs.push((row, metas.len(), m.item_type.to_string(), m.id.clone(), p));
                        }
                        metas.push(m);
                    }
                    debug!("seerr: landing row {} ({}) -> {} card(s)", row, ROW_NAMES[row], metas.len());
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
        let state_commit  = Arc::clone(&state2);
        let client_commit = client.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_commit.upgrade() else { return };
            // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09) — this
            // function previously had NO staleness guard of any kind, not
            // even a generation counter; a sign-out/profile-switch (a
            // different Jellyfin user can have a different or no Seerr
            // connection at all) mid-fetch would otherwise land the OLD
            // connection's Discover rows into the new session's AppState.
            if !crate::seerr_session_current(&state_commit, &client_commit) { return; }
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
