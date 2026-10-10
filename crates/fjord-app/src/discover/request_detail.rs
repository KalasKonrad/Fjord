// ── fjord-app · discover/request_detail.rs ───────────────────────────────────
//   find_local_item            Seerr/TMDB result → local library item by ProviderIds["Tmdb"] (scans the
//                              cached all_movies/all_series — Jellyfin has no such query); also used
//                              for the in-library watchlist star
//   open_discover_item / open_discover_item_ex / PostOpenAction / open_request_options_modal
//                              a local match opens the Jellyfin item (detail::open_detail); otherwise
//                              fetch movie/tv detail, poster, backdrop and both tiers' tags/quality
//                              profiles (best-effort), then cast portraits + season posters (bounded
//                              concurrency); generation-guarded. PostOpenAction: OpenRequestOptions
//                              (open the modal when the fetch lands), EditRequest(id) (fetch the request,
//                              pre-select its profile/tags/seasons; the modal hides Quality and PUTs),
//                              OpenRequestOptionsPreselect(seasons) (new request, only those seasons)
//   open_series_request_detail Series "Missing Seasons" entry (check_local_library: false)
//   build_cast_list / format_rating  Seerr credits → capped cast + crew rows; voteAverage → "★ 7.9"
//   format_date_pretty / language_display_name / country_flag_emoji  metadata panel formatting
//   build_tag_profile_items    one tier's tags/profiles → TagItem/ProfileItem (row 0 = synthetic "Default")
//   DetailFields / movie_fields / tv_fields  everything one fetch fills in, per media type
//   tier_status_label          one tier's display status (fulfilment + approval workflow) → request-
//                              detail-status/-4k, the poster badge, tier pills, Request button visibility
//   tier_request / pick_primary_request  a tier's MediaRequest (MediaInfo.requests, detail endpoints
//                              only); the one request ⋮ More acts on (4K wins a tie)
//   season_request_status      per-season pill for Series "Missing Seasons" (MediaCard's vocabulary only)
//   resolve_providers / format_countries  "Currently Streaming On" + production countries
//   fix_detail_btn_focus       keeps request-detail-btn-focused on an existing button
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Request detail ──────────────────────────────────────────────────────────

// Also used by person.rs's TMDB-only person screen (same base + "person-{id}" cache
// key, so it shares this CastRow portrait cache).
pub(crate) const TMDB_PROFILE_BASE: &str = "https://image.tmdb.org/t/p/w185";

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
    let Some(credits) = credits else {
        return Vec::new();
    };
    let mut seen_ids: std::collections::HashSet<i64> = Default::default();
    let mut out: Vec<CreditRow> = Vec::new();
    for c in credits
        .crew
        .iter()
        .filter(|c| c.job.as_deref() == Some("Director"))
        .take(2)
    {
        if seen_ids.insert(c.id) {
            out.push((
                c.id.to_string(),
                c.name.clone(),
                "Director".to_string(),
                c.profile_path.clone(),
            ));
        }
    }
    for c in credits
        .crew
        .iter()
        .filter(|c| matches!(c.job.as_deref(), Some("Writer") | Some("Screenplay")))
        .take(3)
    {
        if seen_ids.insert(c.id) {
            out.push((
                c.id.to_string(),
                c.name.clone(),
                "Writer".to_string(),
                c.profile_path.clone(),
            ));
        }
    }
    let mut cast_sorted: Vec<&fjord_seerr::Cast> = credits.cast.iter().collect();
    cast_sorted.sort_by_key(|c| c.order.unwrap_or(i64::MAX));
    for c in cast_sorted.into_iter().take(12) {
        if seen_ids.insert(c.id) {
            let role = c.character.clone().unwrap_or_default();
            out.push((
                c.id.to_string(),
                c.name.clone(),
                role,
                c.profile_path.clone(),
            ));
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

/// Also used by blocklist.rs ("Blocklisted on <date>").
pub(crate) fn format_date_pretty(iso: &str) -> String {
    chrono::NaiveDate::parse_from_str(iso, "%Y-%m-%d")
        .map(|d| d.format("%B %-d, %Y").to_string())
        .unwrap_or_default()
}

/// TMDB's `original_language` is an ISO 639-1 code ("en", "ja", ...) with no
/// display name in the response itself. Small hardcoded table, same idiom
/// (and same language set) as `playback.rs::sub_lang_code`'s reverse
/// mapping — a full ISO-639 name table would be a lot of data for a
/// cosmetic label; anything outside this common set just shows its raw code
/// uppercased rather than silently blank.
fn language_display_name(code: &str) -> String {
    match code {
        "en" => "English".into(),
        "de" => "German".into(),
        "fr" => "French".into(),
        "ja" => "Japanese".into(),
        "es" => "Spanish".into(),
        "it" => "Italian".into(),
        "pt" => "Portuguese".into(),
        "ru" => "Russian".into(),
        "ko" => "Korean".into(),
        "zh" => "Chinese".into(),
        "nl" => "Dutch".into(),
        "sv" => "Swedish".into(),
        "pl" => "Polish".into(),
        "cs" => "Czech".into(),
        "ar" => "Arabic".into(),
        "tr" => "Turkish".into(),
        "fi" => "Finnish".into(),
        "da" => "Danish".into(),
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
fn build_tag_profile_items(
    options: (Vec<fjord_seerr::Tag>, Vec<fjord_seerr::Profile>),
) -> TagProfileItems {
    let (tags, profiles) = options;
    let tags = tags
        .into_iter()
        .map(|t| TagItem {
            id: t.id as i32,
            label: t.label.as_str().into(),
            selected: false,
        })
        .collect();
    let mut profiles: Vec<ProfileItem> = std::iter::once(ProfileItem {
        id: 0,
        name: "Default".into(),
    })
    .chain(profiles.into_iter().map(|p| ProfileItem {
        id: p.id as i32,
        name: p.name.as_str().into(),
    }))
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

/// Picks the `flatrate` (subscription-included) providers for one region
/// out of `MovieDetails`/`TvDetails.watch_providers` — an empty result just
/// means nothing streams there (or the title has no watch-provider data at
/// all, common for less mainstream/older content), not an error.
fn resolve_providers(
    providers: &[fjord_seerr::WatchProviderEntry],
    region: &str,
) -> Vec<ProviderRow> {
    providers
        .iter()
        .find(|p| p.iso_3166_1 == region)
        .map(|p| {
            p.flatrate
                .iter()
                .map(|d| (d.id, d.name.clone(), d.logo_path.clone()))
                .collect()
        })
        .unwrap_or_default()
}

/// "🇺🇸 United States\n🇬🇧 United Kingdom" — see request-detail-production-
/// countries' own doc comment in app_state.slint for why this is a single
/// newline-joined string rather than a list model.
fn format_countries(countries: &[fjord_seerr::ProductionCountry]) -> String {
    countries
        .iter()
        .map(|c| format!("{} {}", country_flag_emoji(&c.iso_3166_1), c.name))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Keeps the button cursor on a real stop when the Trailer slot disappears
/// from existing_detail_btn_slots (check pending/failed).
pub(crate) fn fix_detail_btn_focus(g: &AppState) {
    let slots = existing_detail_btn_slots(g);
    if !slots.contains(&g.get_request_detail_btn_focused())
        && let Some(&first) = slots.first()
    {
        g.set_request_detail_btn_focused(first);
    }
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
    trailer_candidates: Vec<String>,
    // Watchlist + Release Calendar (2026-07-18) — MovieDetails/TvDetails.
    // onUserWatchlist verbatim.
    on_watchlist: bool,
    /// `CardItem.availability`'s vocabulary (""/requested/processing/partial/available/
    /// blocklisted, via `availability_tag`) for the base/2K tier — what the Blocklist
    /// button/row gates on (not `status_label`, which mixes in workflow labels like
    /// "Needs Approval"/"Declined").
    availability: &'static str,
}

/// One tier's status label, combining Seerr's two independent signals — `MediaStatus`
/// (fulfilment: is the file there) and the request's `MediaRequestStatus` (workflow:
/// approved yet?) — so "needs approval" and "approved, waiting on Radarr/Sonarr" read
/// differently. `request.status == 3` is `MediaRequestStatus::Declined` (raw int, like
/// `requested_not_available`).
fn tier_status_label(
    status: Option<MediaStatus>,
    request: Option<&fjord_seerr::MediaRequest>,
) -> String {
    if status == Some(MediaStatus::Available) {
        return "Available".to_string();
    }
    // Blocklisted → its own label, so the Request button (shown only while this is "")
    // stays hidden. Available and Blocklisted are exclusive server-side; the order here is
    // for readability.
    if status == Some(MediaStatus::Blocklisted) {
        return "Blocklisted".to_string();
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
fn tier_request(
    mi: Option<&fjord_seerr::MediaInfo>,
    is4k: bool,
) -> Option<&fjord_seerr::MediaRequest> {
    mi?.requests.iter().find(|r| r.is4k == is4k)
}

/// Per-season request status for the Series "Missing Seasons" row:
/// `(availability_label, request_id, pending, mine)` for the active request covering
/// this season, either tier (one pill per season is enough). The label uses only
/// MediaCard's pill vocabulary ("requested"/"processing" — widgets.slint renders just
/// 4 literals). Declined (3) and Failed (4) don't count as active, so the season can be
/// requested again. Seerr has no per-season fulfilment field (`Media.getMedia` doesn't
/// load `seasons`); non-ownership comes from comparing local seasons with TMDB's list.
pub(crate) fn season_request_status(
    requests: &[fjord_seerr::MediaRequest],
    season_number: u32,
    my_user_id: Option<i64>,
) -> Option<(String, String, bool, bool)> {
    let r = requests.iter().find(|r| {
        (r.status == 1 || r.status == 2 || r.status == 5)
            && r.seasons.iter().any(|s| s.season_number == season_number)
    })?;
    let label = if r.status == 1 {
        "requested"
    } else {
        "processing"
    }; // 1=Pending, 2=Approved/5=Completed
    let mine = my_user_id
        .zip(r.requested_by.as_ref().map(|rb| rb.id))
        .map(|(mine, theirs)| mine == theirs)
        .unwrap_or(true);
    Some((label.to_string(), r.id.to_string(), r.is_pending(), mine))
}

/// The ONE `(request_id, pending, mine)` this page's ⋮ More acts on (Edit/Cancel/
/// Approve/Decline). With active requests on both tiers, the 4K one wins — an
/// arbitrary, documented tiebreak rather than a per-tier action UI.
fn pick_primary_request(
    req_2k: Option<&fjord_seerr::MediaRequest>,
    req_4k: Option<&fjord_seerr::MediaRequest>,
    my_user_id: Option<i64>,
) -> (String, bool, bool) {
    match req_4k.or(req_2k) {
        Some(r) => {
            let mine = my_user_id
                .zip(r.requested_by.as_ref().map(|rb| rb.id))
                .map(|(mine, theirs)| mine == theirs)
                .unwrap_or(true);
            (r.id.to_string(), r.is_pending(), mine)
        }
        None => (String::new(), false, false),
    }
}

fn movie_fields(d: MovieDetails, region: &str, my_user_id: Option<i64>) -> DetailFields {
    let year = d
        .release_date
        .as_deref()
        .filter(|s| s.len() >= 4)
        .map(|s| &s[..4])
        .unwrap_or("");
    let genres = d
        .genres
        .iter()
        .map(|g| g.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let cast = build_cast_list(&d.credits);
    let providers = resolve_providers(&d.watch_providers, region);
    let trailer_candidates = trailer_candidates(&d.related_videos);
    let req_2k = tier_request(d.media_info.as_ref(), false);
    let req_4k = tier_request(d.media_info.as_ref(), true);
    let status_label = tier_status_label(d.media_info.as_ref().and_then(|mi| mi.status()), req_2k);
    let status4k_label =
        tier_status_label(d.media_info.as_ref().and_then(|mi| mi.status4k()), req_4k);
    let (request_id, request_pending, request_mine) =
        pick_primary_request(req_2k, req_4k, my_user_id);
    let availability = availability_tag(d.media_info.as_ref().and_then(|mi| mi.status()));
    DetailFields {
        title: d.title,
        meta: if genres.is_empty() {
            year.to_string()
        } else {
            format!("{year} · {genres}")
        },
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
        date_value: d
            .release_date
            .as_deref()
            .map(format_date_pretty)
            .unwrap_or_default(),
        next_air_date: String::new(),
        original_language: language_display_name(&d.original_language),
        production_countries: format_countries(&d.production_countries),
        network: String::new(),
        providers,
        trailer_candidates,
        on_watchlist: d.on_user_watchlist,
        availability,
    }
}

fn tv_fields(d: TvDetails, region: &str, my_user_id: Option<i64>) -> DetailFields {
    let year = d
        .first_air_date
        .as_deref()
        .filter(|s| s.len() >= 4)
        .map(|s| &s[..4])
        .unwrap_or("");
    let genres = d
        .genres
        .iter()
        .map(|g| g.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
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
            let name = if s.name.is_empty() {
                format!("Season {}", s.season_number)
            } else {
                s.name.clone()
            };
            (s.season_number as i32, name, s.episode_count as i32, true) // default all-checked, per plan decision 2
        })
        .collect();
    let providers = resolve_providers(&d.watch_providers, region);
    let network = d
        .networks
        .iter()
        .map(|n| n.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let next_air_date = d
        .next_episode_to_air
        .as_ref()
        .and_then(|e| e.air_date.as_deref())
        .map(format_date_pretty)
        .unwrap_or_default();
    let trailer_candidates = trailer_candidates(&d.related_videos);
    let req_2k = tier_request(d.media_info.as_ref(), false);
    let req_4k = tier_request(d.media_info.as_ref(), true);
    let status_label = tier_status_label(d.media_info.as_ref().and_then(|mi| mi.status()), req_2k);
    let status4k_label =
        tier_status_label(d.media_info.as_ref().and_then(|mi| mi.status4k()), req_4k);
    let (request_id, request_pending, request_mine) =
        pick_primary_request(req_2k, req_4k, my_user_id);
    let availability = availability_tag(d.media_info.as_ref().and_then(|mi| mi.status()));
    DetailFields {
        title: d.name,
        meta: if genres.is_empty() {
            year.to_string()
        } else {
            format!("{year} · {genres}")
        },
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
        date_value: d
            .first_air_date
            .as_deref()
            .map(format_date_pretty)
            .unwrap_or_default(),
        next_air_date,
        original_language: language_display_name(&d.original_language),
        production_countries: format_countries(&d.production_countries),
        network,
        providers,
        trailer_candidates,
        on_watchlist: d.on_user_watchlist,
        availability,
    }
}

/// Matches a Seerr/TMDB result to the local library by provider id, so a card that's
/// already in the library opens the real item (playable, with progress/favourite
/// state) instead of the request page. Client-side: Jellyfin has no "find by provider
/// id" query, so this scans the cached `all_movies`/`all_series` for a
/// `ProviderIds["Tmdb"]` match. A miss falls through to the Seerr detail flow.
pub(crate) fn find_local_item(
    state: &Arc<Mutex<FjordState>>,
    media_type: &str,
    tmdb_id_str: &str,
) -> Option<(String, String)> {
    let s = state.lock().unwrap();
    let items = if media_type == "movie" {
        &s.all_movies
    } else {
        &s.all_series
    };
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
pub(crate) enum PostOpenAction {
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
    /// Series "Missing Seasons": opens the modal for a NEW request with only these
    /// seasons pre-selected (not `tv_fields`' all-checked default), so owned seasons
    /// aren't requested again.
    OpenRequestOptionsPreselect(Vec<u32>),
}

/// Resets zone/focus and opens the Request Options modal — shared by the
/// Request button's own callback and `PostOpenAction`'s two variants above,
/// so both entry points stay in sync.
pub(crate) fn open_request_options_modal(g: &AppState) {
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
    open_discover_item_ex(
        media_type,
        tmdb_id_str,
        state,
        ww,
        rt,
        PostOpenAction::None,
        true,
    );
}

/// Series "Missing Seasons" entry point — `check_local_library: false` (the series is
/// local, so the in-library redirect would just bounce back to its own page).
/// `preselect_seasons`: `Some(seasons)` opens Request Options pre-checked to those
/// seasons (no covering request yet); `None` shows RequestDetailScreen (the season
/// has a request — its ⋮ More edits/cancels it).
pub(crate) fn open_series_request_detail(
    tmdb_id_str: String,
    preselect_seasons: Option<Vec<u32>>,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let action = match preselect_seasons {
        Some(seasons) => PostOpenAction::OpenRequestOptionsPreselect(seasons),
        None => PostOpenAction::None,
    };
    open_discover_item_ex("tv".to_string(), tmdb_id_str, state, ww, rt, action, false);
}

/// `check_local_library`: `true` for View Details/Request/Edit Request (redirect to
/// the Jellyfin item when the library has it); `false` only for the context menu's
/// "View Request" row — a requested item can also be partly in the library (a series
/// missing seasons), and that row must reach the request page.
pub(crate) fn open_discover_item_ex(
    media_type: String,
    tmdb_id_str: String,
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    post_action: PostOpenAction,
    check_local_library: bool,
) {
    if check_local_library
        && let Some((id, item_type)) = find_local_item(&state, &media_type, &tmdb_id_str)
    {
        crate::detail::open_detail(id, item_type, state, ww, rt);
        return;
    }
    let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else {
        return;
    };
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        return;
    };
    let is_session_auth = client.is_session_auth();

    let generation = {
        let Some(w) = ww.upgrade() else { return };
        let g = AppState::get(&w);
        let next = g.get_request_detail_open_gen() + 1;
        g.set_request_detail_open_gen(next);
        // Loading overlay while fetching (no local cache here), but deferred (see below):
        // showing it at once made a fast fetch flash the spinner and then the content.
        g.set_app_loading_progress(0.0);
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
        g.set_request_detail_availability("".into());
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
        // Land on the button row (Request), not Back, like the other detail screens.
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
        g.set_request_detail_providers(ModelRc::new(VecModel::from(
            Vec::<StreamingProvider>::new(),
        )));
        g.set_request_detail_trailer_url("".into());
        g.set_request_detail_trailer_state("".into()); // set again once the fetch lands
        g.set_request_detail_btn_focused(0);
        g.set_show_request_options(false); // defensive — shouldn't still be open across items
        g.set_request_options_editing(false);
        g.set_request_options_editing_request_id("".into());
        // NOT set here — deferred to the commit closure below, once the
        // primary fetch has actually landed (see this function's own
        // app-content-loading comment above and the commit closure's
        // matching comment further down).
        next
    };

    // Delayed spinner-show — see this block's own comment above. Only
    // actually flips app-content-loading on if, once the short window
    // elapses, this exact open (generation still matches — a newer open, or this
    // same one having already superseded itself, both correctly skip it)
    // hasn't already finished (show-request-detail still false). A fetch
    // faster than the window never shows a spinner at all; one slower
    // than it shows the identical overlay this always had, just not
    // pre-emptively for the common fast case.
    {
        let ww_spin = ww.clone();
        rt.spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww_spin.upgrade() else { return };
                let g = AppState::get(&w);
                if g.get_request_detail_open_gen() == generation && !g.get_show_request_detail() {
                    g.set_app_content_loading(true);
                }
            });
        });
    }

    let media_type2 = media_type.clone();
    let rt_trailer = rt.clone();
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
                    client
                        .get_movie(tmdb_id)
                        .await
                        .map(|d| movie_fields(d, &region, my_user_id))
                } else {
                    client
                        .get_tv(tmdb_id)
                        .await
                        .map(|d| tv_fields(d, &region, my_user_id))
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
                // A failed detail fetch never reaches the commit closure that
                // would otherwise clear this — same fix shape as detail.rs's
                // own open_detail on its equivalent error path.
                let ww_err = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww_err.upgrade() {
                        AppState::get(&w).set_app_content_loading(false);
                    }
                });
                return;
            }
        };
        // Best-effort: no tags/profiles configured, or no permission to read
        // /service/* on this account, are both "just don't show that
        // picker," not a reason to fail opening the item.
        let ((tags, profiles), (tags_4k, profiles_4k)): (TagProfileItems, TagProfileItems) =
            match options_result {
                Ok((regular, fourk)) => (
                    build_tag_profile_items(regular),
                    build_tag_profile_items(fourk),
                ),
                Err(e) => {
                    debug!("seerr: couldn't fetch tags/profiles for {media_type2}: {e:#}");
                    ((Vec::new(), Vec::new()), (Vec::new(), Vec::new()))
                }
            };

        let Ok(http) = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
        else {
            return;
        };
        let cache_prefix = if media_type2 == "movie" {
            "movie"
        } else {
            "tv"
        };
        let poster_buf = if let Some(p) = &fields.poster_path {
            fetch_tmdb_image(
                &http,
                TMDB_POSTER_BASE,
                p,
                &format!("{cache_prefix}-{tmdb_id}"),
            )
            .await
            .and_then(|b| decode_poster_buffer(&b))
        } else {
            None
        };
        let backdrop_buf = if let Some(p) = &fields.backdrop_path {
            fetch_tmdb_image(
                &http,
                TMDB_BACKDROP_BASE,
                p,
                &format!("{cache_prefix}-{tmdb_id}-bg"),
            )
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
        let mut portrait_tasks: tokio::task::JoinSet<(
            usize,
            Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
        )> = tokio::task::JoinSet::new();
        for (idx, (_, _, _, profile_path)) in fields.cast.iter().enumerate() {
            let Some(path) = profile_path.clone() else {
                continue;
            };
            let http = http.clone();
            let sem = Arc::clone(&sem);
            let person_id = fields.cast[idx].0.clone();
            portrait_tasks.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let bytes = fetch_tmdb_image(
                    &http,
                    TMDB_PROFILE_BASE,
                    &path,
                    &format!("person-{person_id}"),
                )
                .await;
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
        let mut season_tasks: tokio::task::JoinSet<(
            usize,
            Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
        )> = tokio::task::JoinSet::new();
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
        let mut provider_tasks: tokio::task::JoinSet<(
            usize,
            Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
        )> = tokio::task::JoinSet::new();
        for (idx, (provider_id, _, logo_path)) in fields.providers.iter().enumerate() {
            let Some(path) = logo_path.clone() else {
                continue;
            };
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
            if g.get_request_detail_open_gen() != generation {
                return; // superseded by a rapid re-open of a different item
            }
            // show-request-detail is set at open time; reset_session_state clears it on
            // sign-out/profile switch, and this guard keeps the fetch from refilling the hidden
            // screen with the previous Seerr connection's data.
            if !crate::seerr_session_current(&state, &client) {
                return;
            }
            g.set_request_detail_title(fields.title.as_str().into());
            g.set_request_detail_meta(fields.meta.as_str().into());
            g.set_request_detail_overview(fields.overview.as_str().into());
            g.set_request_detail_rating(fields.rating.as_str().into());
            g.set_request_detail_status(fields.status_label.as_str().into());
            g.set_request_detail_status_4k(fields.status4k_label.as_str().into());
            g.set_request_detail_availability(fields.availability.into());
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
                    CastMember {
                        id: id.as_str().into(),
                        name: name.as_str().into(),
                        role: role.as_str().into(),
                        photo,
                        has_photo,
                    }
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
                    SeasonItem {
                        season_number,
                        name: name.as_str().into(),
                        episode_count,
                        selected,
                        poster,
                        has_poster,
                    }
                })
                .collect();
            g.set_request_detail_seasons(ModelRc::new(VecModel::from(seasons)));
            g.set_request_detail_tags(ModelRc::new(VecModel::from(tags)));
            g.set_request_detail_profiles(ModelRc::new(VecModel::from(profiles)));
            g.set_request_detail_tags_alt(ModelRc::new(VecModel::from(tags_4k)));
            g.set_request_detail_profiles_alt(ModelRc::new(VecModel::from(profiles_4k)));
            // Start from the remembered Quality/Profile/Tags (Request Options' "remember last
            // choice"); EditRequest's arm below overrides them with the request's real values.
            let remembered = {
                let s = state.lock().unwrap();
                if media_type2 == "movie" {
                    s.config.active().request_pref_movie.clone()
                } else {
                    s.config.active().request_pref_tv.clone()
                }
            };
            {
                let model = g.get_request_detail_tags();
                for i in 0..model.row_count() {
                    if let Some(mut t) = model.row_data(i) {
                        t.selected = remembered.tag_ids_2k.contains(&(t.id as i64));
                        model.set_row_data(i, t);
                    }
                }
                let model_alt = g.get_request_detail_tags_alt();
                for i in 0..model_alt.row_count() {
                    if let Some(mut t) = model_alt.row_data(i) {
                        t.selected = remembered.tag_ids_4k.contains(&(t.id as i64));
                        model_alt.set_row_data(i, t);
                    }
                }
            }
            g.set_request_detail_selected_profile_id(remembered.profile_id_2k);
            g.set_request_detail_selected_profile_id_alt(remembered.profile_id_4k);
            if remembered.want_4k {
                set_quality(&g, true);
            }
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
                    StreamingProvider {
                        id: id as i32,
                        name: name.as_str().into(),
                        logo,
                        has_logo,
                    }
                })
                .collect();
            g.set_request_detail_providers(ModelRc::new(VecModel::from(providers)));
            start_trailer_check(
                &state,
                &ww,
                &rt_trailer,
                generation,
                fields.trailer_candidates,
            );
            if let Some(buf) = poster_buf {
                g.set_request_detail_poster(slint::Image::from_rgba8(buf));
                g.set_request_detail_has_poster(true);
            }
            if let Some(buf) = backdrop_buf {
                g.set_request_detail_backdrop(slint::Image::from_rgba8(buf));
                g.set_request_detail_has_backdrop(true);
            }
            // Show the screen and clear the overlay now that the main content has landed (like
            // the native detail screens). Before the match below, so a failing follow-up action
            // (e.g. EditRequest's fetch) still leaves the loaded content visible.
            g.set_show_request_detail(true);
            g.set_app_content_loading(false);
            g.set_app_loading_progress(0.0);
            w.invoke_grab_keyboard_focus();
            match post_action {
                PostOpenAction::None => {}
                PostOpenAction::OpenRequestOptions => open_request_options_modal(&g),
                PostOpenAction::EditRequest(id) => {
                    match editing_request_result {
                        Some(Ok(r)) => {
                            if r.is4k {
                                set_quality(&g, true);
                            }
                            g.set_request_detail_selected_profile_id(
                                r.profile_id.unwrap_or(0) as i32
                            );
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
                            g.set_request_options_editing_request_id(
                                id.to_string().as_str().into(),
                            );
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
                PostOpenAction::OpenRequestOptionsPreselect(wanted_seasons) => {
                    let wanted: std::collections::HashSet<u32> =
                        wanted_seasons.into_iter().collect();
                    let model = g.get_request_detail_seasons();
                    for i in 0..model.row_count() {
                        if let Some(mut s) = model.row_data(i) {
                            s.selected = wanted.contains(&(s.season_number as u32));
                            model.set_row_data(i, s);
                        }
                    }
                    open_request_options_modal(&g);
                }
            }
        });
    });
}
