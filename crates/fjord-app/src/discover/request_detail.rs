// ── fjord-app · discover/request_detail.rs ───────────────────────────────────
//   find_local_item             matches a Seerr/TMDB result to the local library by
//                              ProviderIds["Tmdb"] — no server-side Jellyfin lookup exists,
//                              so this scans the already-cached all_movies/all_series;
//                              pub(crate) as of 2026-07-20 — also reused by
//                              resync_jellyfin_watchlist_stars/discover_toggle_watchlist's own
//                              success handler for the in-library watchlist star (see below)
//   open_discover_item         find_local_item hit -> detail::open_detail (the real
//                              Jellyfin item) instead of the Seerr flow below; else
//                              fetch movie/tv detail + poster + backdrop + available tags/
//                              quality profiles for BOTH quality tiers (best-effort, silently
//                              empty on failure — see available_request_options_both_tiers) in
//                              parallel, then cast/crew portraits + season posters (TMDB,
//                              bounded concurrency, same JoinSet+Semaphore shape as detail.rs's
//                              Jellyfin cast fetch); generation-guarded, populates RequestDetailScreen.
//                              Profile row 0 is always a synthetic "Default" entry (id 0)
//                              prepended so the picker has an explicit "no explicit choice"
//                              option, not just whatever's focused first.
//   build_cast_list/format_rating  Seerr credits -> capped cast+crew rows (2 Director/
//                              3 Writer/12 top-billed cast, same shape as detail.rs's
//                              Jellyfin cast) / TMDB voteAverage -> "★ 7.9" badge text
//   build_tag_profile_items    one quality tier's raw Seerr tags/profiles -> Slint TagItem/
//                              ProfileItem models; shared by both tiers in open_discover_item
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
//   open_discover_item_ex/PostOpenAction/open_request_options_modal  open_discover_item is now a
//                              thin wrapper around this with PostOpenAction::None; ::OpenRequestOptions
//                              (Discover context menu's "Request" row) opens the modal the instant the
//                              fetch lands; ::EditRequest(id) additionally fetches GET /request/{id}
//                              fresh (SeerrClient::get_request) and pre-selects its profile/tags/seasons,
//                              setting request-options-editing so the modal hides Quality and Confirm
//                              PUTs via submit_edit_request instead of POSTing via submit_request
//                              (2026-07-18)
//   season_request_status      per-season request-status pill for Series Missing Seasons —
//                              deliberately restricted to MediaCard's existing pill vocabulary
//                              ("requested"/"processing") rather than inventing new label text,
//                              which would silently render as an empty pill (confirmed by reading
//                              widgets.slint directly — its ternary only matches 4 literal strings,
//                              it does NOT render arbitrary text as an earlier draft assumed)
//   PostOpenAction::OpenRequestOptionsPreselect  new variant — opens the Request Options modal
//                              for a genuinely new request (unlike EditRequest) but pre-checks
//                              only the given season numbers instead of tv_fields' all-checked
//                              default; mirrors EditRequest's own post-hoc season-override pattern
//   open_series_request_detail  Series Missing Seasons' entry point into RequestDetailScreen —
//                              always check_local_library:false (the series obviously exists
//                              locally already; the normal redirect would just bounce back to the
//                              same Detail page with no Seerr request UI at all)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Request detail ──────────────────────────────────────────────────────────

// pub(crate) since 2026-08-13 — person.rs's TMDB-only person screen reuses
// this same base + cache-key format ("person-{id}") for its own portrait
// fetch, matching (and free-riding on the disk cache of) the identical
// fetch this file's own RequestDetailScreen CastRow portrait fetch does.
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

/// "2026-07-14" -> "July 14, 2026"; empty/unparseable input -> "". A hand-
/// rolled month-name table would duplicate what `chrono` (already a
/// workspace dependency, used elsewhere for wall-clock formatting) does
/// correctly for free.
// pub(crate) since 2026-08-06 (Seerr Blocklist support) — blocklist.rs
// reuses it for the Manage Blocklist screen's "Blocklisted on <date>" line.
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
    /// Same `""`/`"requested"`/`"processing"`/`"partial"`/`"available"`/
    /// `"blocklisted"` vocabulary as `CardItem.availability` (via
    /// `availability_tag`) — the base/2K tier's status only (see the
    /// Blocklist eligibility design decision in CLAUDE.md for why 4K isn't
    /// checked separately). This is the field the Blocklist button/row
    /// actually gates on, deliberately NOT `status_label` above, which
    /// mixes in request-workflow labels ("Needs Approval"/"Declined") that
    /// have nothing to do with blocklist eligibility. 2026-08-06, Seerr
    /// Blocklist support.
    availability: &'static str,
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
fn tier_status_label(
    status: Option<MediaStatus>,
    request: Option<&fjord_seerr::MediaRequest>,
) -> String {
    if status == Some(MediaStatus::Available) {
        return "Available".to_string();
    }
    // Real bug fixed 2026-08-06 (Seerr Blocklist support): this function
    // previously had no arm for Blocklisted at all, falling through to the
    // final `_ => String::new()` — identical to a never-touched item, so
    // RequestDetailScreen's Request button (gated on this string being
    // empty) incorrectly still showed for a blocklisted title. Available
    // and Blocklisted are mutually exclusive server-side, so checking this
    // right after Available (rather than at the very end) is just for
    // readability, not correctness.
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

/// Per-season request status for the Series "Missing Seasons" row
/// (2026-07-29, Deep Seerr integration) — `(availability_label, request_id,
/// pending, mine)` for whichever active request (if any) covers this season
/// number, regardless of tier (2K vs 4K isn't distinguished per season here
/// — a deliberate simplification: this pill only ever needs to say "already
/// requested/pending", not track two independent tiers per season).
/// `availability_label` is deliberately restricted to the exact same
/// lowercase vocabulary `MediaCard`'s pill in `widgets.slint` already
/// recognizes ("requested"/"processing") — confirmed by reading that
/// component directly (its pill ternary only matches 4 specific literal
/// strings, it does NOT render arbitrary text as an earlier draft of this
/// plan assumed) rather than inventing new label text that would silently
/// render as an empty pill bubble. Status 3 (Declined) and 4 (Failed) are
/// both excluded from "active" — a failed request doesn't block treating
/// the season as available to request again, which is arguably the more
/// useful behavior than a static "Failed" pill with no action anyway. No
/// per-season Jellyfin-fulfillment field exists anywhere in Seerr's API
/// (confirmed: `Media.getMedia` doesn't eager-load `seasons`) — this is
/// request state only, which is all this row's own existence needs, since
/// comparing local season folders against TMDB's list already establishes
/// non-ownership independently of anything this function reports.
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
    /// Series "Missing Seasons" row (2026-07-29, Deep Seerr integration) —
    /// opens the modal for a genuinely NEW request (unlike `EditRequest`,
    /// nothing here is being edited), but pre-selects only the given season
    /// numbers instead of `tv_fields`' own all-checked default, so clicking
    /// a missing season doesn't re-request seasons already owned.
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

/// Series "Missing Seasons" row entry point (2026-07-29, Deep Seerr
/// integration) — always `check_local_library: false`, unlike
/// `open_discover_item`: the series obviously already exists locally (we're
/// viewing its own screen), so the normal in-library redirect would just
/// bounce straight back to the same Detail page with no Seerr request UI at
/// all, defeating the entire point of this action. `preselect_seasons`:
/// `Some(seasons)` opens the Request Options modal pre-checked to exactly
/// those season numbers (a season with no existing covering request);
/// `None` just shows RequestDetailScreen normally (a season that already
/// has one — its own ⋮ More button is the correct place to Edit/Cancel it,
/// not a bespoke season-scoped context menu, which would need to smuggle a
/// season number through fields shaped for a tmdb id and risks a real
/// id-type mismatch for zero real benefit here).
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
        // Loading overlay while the fetch is in flight — RequestDetailScreen
        // has no local cache the way Jellyfin's item_detail_cache gives the
        // native detail screens a fast path. Real bug, live-reported
        // 2026-08-21 ("if you open an item and the load is quick you get a
        // quic flash of the loding then it flash again as the item get
        // shown... its also a bit jaring") — this used to show the overlay
        // unconditionally, the instant this function was called; a
        // genuinely fast fetch then replaced it with real content only a
        // handful of frames later, reading as two visual events back to
        // back rather than one clean transition. Deferred below instead —
        // see the matching comment right after this block — a fetch that's
        // still slow gets the exact same spinner it always did, just not
        // shown until it's actually worth showing.
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
            // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09) —
            // show-request-detail is set true synchronously at open time,
            // before this fetch even starts; reset_session_state now
            // correctly clears it back to false on a sign-out/profile
            // switch, but without this check this closure could still
            // silently repopulate the (now-hidden) screen's fields with
            // the OUTGOING Seerr connection's data.
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
            // Apply the remembered Quality/Profile/Tags preference
            // (2026-08-12, "seerr always remember what you hade chosen last
            // time so it shuld mirror it") as this item's starting point —
            // PostOpenAction::EditRequest's own match arm below overwrites
            // these with the real existing request's own actual values
            // afterward, correctly taking precedence when that's the action.
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
            // Show the screen and clear the loading overlay now that the
            // primary content above has actually landed — matches every
            // other detail-style screen's own pattern (open_detail/spawn_main
            // etc: app-content-loading while fetching, show_X deferred to the
            // commit). Real bug, live-reported 2026-08-12: this used to be
            // set unconditionally at OPEN time (before the fetch even
            // started), with no loading overlay at all — a blank page for
            // however long the fetch took, inconsistent with every native
            // detail screen. Placed BEFORE the match below (not after) so a
            // failure in one of match's own optional follow-up actions (e.g.
            // EditRequest's own fetch) still leaves the screen showing its
            // already-successfully-loaded primary content instead of leaving
            // it hidden/blank on top of a real early return.
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
