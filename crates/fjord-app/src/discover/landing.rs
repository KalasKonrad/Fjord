// ── fjord-app · discover/landing.rs ──────────────────────────────────────────
//   LANDING_ROW_NEW_IN_THEATERS / LANDING_ROW_COMING_UP  named landing-row indices
//   landing_row_get/_set/_lens the 9 landing-row models (0=Trending … 8=Watchlist), explicit arms
//                              (no catch-all), shared by the fetch and handle_key's landing branch
//   movie_details_to_meta / tv_details_to_meta  Requested-row card meta from a detail fetch
//   RequestEntry / request_entry  one request for the Requested row: status vs status4k by the
//                              request's tier ("requested" when that status is Unknown), plus
//                              other_tier_available ("Available in 2K/4K") and other_tier_requested
//                              ("Also requested in 2K/4K", via dual_tier_tmdb_ids)
//   dual_tier_tmdb_ids         tmdb ids with active, unfulfilled requests in BOTH tiers
//   fetch_requested_row / refresh_requested_row  the Requested row (refreshed after a new request)
//   fetch_new_in_theaters      canned filter preset (primary release date in the last ~45 days,
//                              popularity) — an approximation, Seerr can't filter by release type
//   ensure_discover_landing    all landing rows in parallel, once per session; text first, then
//                              posters; dedups the discovery rows against Requested; fills
//                              discover_known_requests and then rebuilds the calendar
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Landing rows (Trending / Popular / Upcoming, shown when query == "") ───

// Row indices, named — used by handle_key_landing's sentinel special-case
// (Watchlist + Release Calendar, 2026-07-18) so that check doesn't depend
// on a bare literal matching this match's own row ordering.
pub(crate) const LANDING_ROW_NEW_IN_THEATERS: usize = 6;

pub(crate) const LANDING_ROW_COMING_UP: usize = 7;
// Row 8 = Watchlist, appended (landing-row indices are only ever appended, never
// inserted). No named const: it has no sentinel or keyboard special case. Not deduped
// against the discovery rows — only Requested dedups (see below).

// Explicit `7 =>`/`8 =>` arms, no catch-all `_ =>`: a catch-all would silently
// alias a new row to an existing one instead of failing to compile.
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

/// One kept request's raw fields + which endpoint its detail fetch needs.
/// `is4k`/`other_tier_available` let the card say "4K Requested" + "Available in 2K"
/// instead of a tier-blind "Requested" (`status` vs `status4k` must follow the request's
/// tier — see `requested_not_available` in fjord-seerr). `request_id`/`pending`/`mine`
/// feed the context menu's Edit/Cancel/Approve/Decline rows; `pending`/`mine` are the
/// request's workflow state, not media availability.
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
    // A row here is always an active request for this tier, but Seerr can report the
    // tier's media status as Unknown long after the request (seen on 3 of 49 real 4K
    // requests — likely a series whose top-level status wasn't recomputed). Show
    // "requested" rather than a blank pill.
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

/// Re-fetches just the Requested row and replaces `discover-requested` — after a new
/// request (Request button, context-menu Request/Edit). A brand-new request was never
/// in the row, so `patch_discover_card_availability` had nothing to patch, and the row
/// otherwise loads once per session. Re-fetching this one row is cheap for an
/// infrequent action.
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
        // Refresh discover_known_requests here too, so a request made this session is known
        // everywhere at once.
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

/// "New in Theaters" — an APPROXIMATION, not a verified "still showing" signal:
/// Seerr's `/discover/movies` can't filter by release type (fixed query allowlist), so
/// this uses `primaryReleaseDateGte`/`Lte` over roughly the last 6 weeks (a wide
/// release's primary TMDB date is usually the theatrical one). Reuses
/// `discover_movies_filtered` with a canned preset.
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

/// Fetches the landing rows in parallel, once per session
/// (`FjordState.discover_landing_fetched`, reset on disconnect/reconnect/sign-out — a
/// different server means a different catalog). Two-phase commit like
/// `spawn_discover_search`: text cards first, posters patched in as they arrive. Row 5
/// (Requested) is built differently (`fetch_requested_row`) but folds into the same
/// `metas_per_row`/`poster_jobs` shape.
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

        // Items in the Requested row don't also show in Trending/Popular/Upcoming (search
        // is untouched). Only Requested dedups — the same title in Trending and Popular is
        // normal for a discovery page. Keyed on (item_type, tmdb id): a movie and a series
        // can share a tmdb id.
        let requested_keys: std::collections::HashSet<(&'static str, String)> =
            requested.iter().map(|(m, _)| (m.item_type, m.id.clone())).collect();

        // So an already-requested item still shown in another row offers Edit/Cancel/View
        // Request (see FjordState.discover_known_requests). Built before `requested` is moved
        // into row 5.
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
        // Rebuild the calendar now that discover_known_requests is filled:
        // ensure_discover_watchlist's own call usually runs first (its fetch is instant, this
        // join is a network round trip) and found no request candidates, so without this
        // Coming Up stayed empty for a session with no watchlist items. Spawned, so its
        // detail fetches don't delay this screen's commit.
        tokio::spawn(build_calendar_entries(Arc::clone(&state2), ww.clone()));

        let mut metas_per_row: Vec<Vec<DiscoverCardMeta>> = Vec::with_capacity(6);
        // (row, idx-within-row, item_type, tmdb_id, poster_path)
        let mut poster_jobs: Vec<(usize, usize, String, String, String)> = Vec::new();
        let mut first_error: Option<anyhow::Error> = None;
        for (row, r) in responses.into_iter().enumerate() {
            match r {
                Ok(resp) => {
                    // metas and poster jobs come from ONE filter pass: building them with two different
                    // filters (search_result_to_meta also drops blocklisted items) shifted every later
                    // poster in the row by one after a single blocklisted entry.
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
            // Session guard: a sign-out/profile switch mid-fetch must not land the previous
            // Seerr connection's rows in the new session.
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
