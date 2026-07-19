// ── fjord-seerr · models.rs ──────────────────────────────────────────────────
//   MediaStatus       MediaInfo.status: 1=Unknown 2=Pending 3=Processing
//                     4=PartiallyAvailable 5=Available 6=Blocklisted 7=Deleted
//                     (verified against Seerr's real server/constants/media.ts
//                     after a live bug — see MediaStatus's own doc comment)
//   MediaInfo         status + status4k (tracked completely independently by Seerr — see
//                     status4k's own doc comment, real bug fixed 2026-07-18) + tmdbId,
//                     present only once Seerr has seen an item; requests (only populated
//                     on the single-item detail endpoints, see its own doc comment)
//   SearchResponse/SearchResult  GET /search — mediaType discriminates movie/tv/person;
//                                 genreIds/voteAverage/popularity added 2026-07-18 for
//                                 client-side filtering+sorting of search results AND
//                                 merge-sorting the filtered-browse view's Type=All movie+TV
//                                 interleave (Discover filters — /search itself accepts no
//                                 filter query params at all, see DiscoverFilters' own doc comment)
//   DiscoverFilters               GET /discover/movies GET /discover/tv's real filter query
//                                 params (genre/watchProviders/sortBy/voteAverageGte/date
//                                 range — confirmed from Seerr's real route source, 2026-07-18);
//                                 sort/date_gte/date_lte are pre-resolved to the correct
//                                 value+key name per media type by the caller, since movies/TV
//                                 genuinely differ there (primary_release_date vs first_air_date);
//                                 date_lte added 2026-07-18 for "New in Theaters"' upper bound
//   WatchlistResponse/WatchlistItem  GET /discover/watchlist — local (non-Plex) per-user
//                                 Watchlist, independent of Requests (2026-07-18, Watchlist +
//                                 Release Calendar); no poster/richer data, same per-item-
//                                 detail-fetch situation as a bare MediaRequest
//   MovieDetails/TvDetails       GET /movie/{id}, /tv/{id} — voteAverage + credits (Cast/Crew)
//                                 confirmed present in the OpenAPI spec but not deserialized
//                                 until the RequestDetailScreen redesign (2026-07-16);
//                                 onUserWatchlist (both) + releases (MovieDetails only, see
//                                 ReleaseDatesResult below) added 2026-07-18
//   ReleaseDatesResult/RegionReleases/ReleaseDateEntry  MovieDetails.releases — TMDB's raw
//                                 per-region theatrical(3)/digital(4)/physical(5) release-date
//                                 breakdown, forwarded verbatim by Seerr; TV has no equivalent
//                                 (2026-07-18, Watchlist + Release Calendar)
//   Season                       TvDetails.seasons — TMDB-shape, no per-season
//                                 Jellyfin-availability field in the published spec.
//                                 posterPath also present in the spec, same
//                                 previously-undeserialized-field situation as above
//   Credits/Cast/Crew            MovieDetails/TvDetails.credits — cast (id/name/character/
//                                 order/profilePath) + crew (id/name/job/department/profilePath)
//   SeasonsSelector              POST /request body's `seasons`: array or "all"
//   MediaRequest                 POST /request response + GET /request list entries (media/
//                                 created_at/requested_by/profile_id/tags/seasons only populated
//                                 by the latter — Discover "Requested" row + context menu);
//                                 is4k picks which of media's status/status4k is the relevant
//                                 fulfillment status (2026-07-18); status: 1=Pending 2=Approved
//                                 3=Declined 4=Failed 5=Completed (real enum, confirmed from
//                                 Seerr's source, 2026-07-18); is_pending() checks status==1
//   RequestedBy                  MediaRequest.requestedBy — id only, ownership check for
//                                 Edit/Cancel Request (2026-07-18)
//   SeasonRequestNumber          MediaRequest.seasons entry — Seerr's own tracked per-season
//                                 request state (seasonNumber only), NOT Season above (TMDB
//                                 metadata) — pre-fills Edit Request's season picker (2026-07-18)
//   User                         auth response — id/displayName for "Connected as X";
//                                 permissions bitmask (can_manage_requests(): MANAGE_REQUESTS
//                                 bit 16 OR the ADMIN bit 2, which bypasses every permission
//                                 check server-side and is what the owner account actually
//                                 carries — fixed 2026-07-18, see the impl's own doc comment)
//                                 gates Approve/Decline/admin-Cancel in the Discover context
//                                 menu (2026-07-18)
//   QuickConnect                 POST /auth/jellyfin/quickconnect/initiate response
//   StatusInfo                   GET /status response — version, shown in Settings sidebar
//   Tag                          Radarr/Sonarr tag {id, label} — GET /service/{radarr|sonarr}/{id}'s
//                                 `tags` field, NOT in the published OpenAPI spec (confirmed from
//                                 Seerr's actual TypeScript source, same class of gap as media_type below)
//   Profile                      Radarr/Sonarr quality profile {id, name} — same endpoint's `profiles`
//                                 field; spec shows it as a single object with no array wrapper, but
//                                 Seerr's TypeScript source confirms it's really QualityProfile[]
//   ServiceServer                GET /service/{radarr|sonarr} list entry — `id`/`isDefault`/`is4k`
//                                 (find the default server for a given quality tier to fetch tags/
//                                 profiles for; no per-server picker in v1)
//   ServiceServerDetails         GET /service/{radarr|sonarr}/{id} — `tags` + `profiles` extracted;
//                                 every other field (rootFolders, server, languageProfiles) ignored
//   ProductionCountry/Network/NextEpisode/WatchProviderEntry/WatchProviderDetail
//                                 MovieDetails/TvDetails' status/originalLanguage/
//                                 productionCountries/networks/nextEpisodeToAir/watchProviders —
//                                 confirmed present in Seerr's real server/models/{Movie,Tv,common}.ts
//                                 (not in the published OpenAPI spec, same class of gap as Tag/Profile
//                                 above); added for the request-detail metadata panel (2026-07-17);
//                                 NextEpisode extended with episode_number/name/season_number
//                                 2026-07-18 for the "Coming Up" calendar entry label
//   Video                         MovieDetails/TvDetails.relatedVideos entry — YouTube trailer/
//                                 teaser/clip links (kind + already-fully-formed url); Watch Trailer
//                                 feature (2026-07-17)
//   Region                        GET /watchproviders/regions list entry — populates the Streaming
//                                 Region picker (Settings -> Integrations)
//   Language                      GET /languages list entry (TMDB's full ~180-entry list) — backs
//                                 BOTH the Discover Language and Display Language pickers (Settings
//                                 -> Integrations, 2026-07-17); Discover Region deliberately NOT
//                                 mirrored — confirmed dead in Seerr itself, discover.ts's
//                                 createTmdbWithRegionLanguage reads user.settings.streamingRegion
//                                 for its "discoverRegion" TMDB param, never discoverRegion
//   UserGeneralSettings           GET/POST /user/{id}/settings/main — gated by Seerr's own
//                                 isOwnProfileOrAdmin(), NOT Permission.ADMIN (confirmed from source,
//                                 corrected a wrong earlier assumption that this needed admin rights)
//                                 — used to read/write the CONNECTED user's own streamingRegion, which
//                                 resolve_streaming_region (discover.rs) also reads from for "Currently
//                                 Streaming On." POST overwrites the whole object, no partial patch —
//                                 every field skip_serializing_if=is_none (a real 500 live-reproduced
//                                 otherwise — locale is a NOT NULL DB column, see this struct's own
//                                 doc comment, 2026-07-17)
//
// Every Deserialize struct below carries #[serde(rename_all = "camelCase")] —
// Seerr's JSON is camelCase throughout (mediaType, posterPath, totalResults,
// displayName, ...), confirmed directly from the OpenAPI spec. Real bug, found
// live via the fjord.log warning this crate's own logging added: without this,
// serde requires an exact field-name match, so any REQUIRED multi-word field
// (SearchResult.media_type) failed deserialization outright — but every
// Option<...> field with #[serde(default)] (MovieDetails.poster_path etc.)
// would have failed *silently* instead, just quietly staying None even when
// the server sent real data. rename_all fixes both classes at once.
use serde::{Deserialize, Serialize};

/// Confirmed directly against Seerr's real source (`server/constants/
/// media.ts`) after a live report of "Deleted" items surviving the
/// Discover "Requested" row's filter — the previously-modeled 6-value
/// enum (`...Available=5, Deleted=6`) was simply wrong past `Available`:
/// the real enum has a `Blocklisted` value at 6 that was never
/// represented at all, pushing the real `Deleted` to 7. Every request
/// this crate had actually seen with a real status of 7 (Deleted) was
/// silently falling through `from_code` to `None` — indistinguishable
/// from a genuinely unrecognized code — so `requested_not_available`'s
/// exclusion check (`Some(Available | Deleted)`) never matched it and
/// deleted-but-still-request-tracked items stayed listed as "not yet
/// available."
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[repr(u8)]
pub enum MediaStatus {
    Unknown = 1,
    Pending = 2,
    Processing = 3,
    PartiallyAvailable = 4,
    Available = 5,
    Blocklisted = 6,
    Deleted = 7,
}

impl MediaStatus {
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Unknown),
            2 => Some(Self::Pending),
            3 => Some(Self::Processing),
            4 => Some(Self::PartiallyAvailable),
            5 => Some(Self::Available),
            6 => Some(Self::Blocklisted),
            7 => Some(Self::Deleted),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaInfo {
    #[serde(default)]
    pub tmdb_id: Option<i64>,
    pub status: u8,
    /// The 4K tier's own fulfillment status, tracked entirely separately
    /// from `status` (confirmed live, 2026-07-18, against a real account
    /// where almost every request is `is4k` — many items had `status: 1`
    /// (Unknown, the non-4K tier was never requested) alongside a genuinely
    /// `status4k: 5` (Available) or still-`status4k: 3` (Processing)).
    /// `requested_not_available`'s original filter checked `status` alone
    /// regardless of which tier was actually requested, which is why
    /// already-fulfilled 4K requests kept showing in the Discover
    /// "Requested" row — see that function's own doc comment.
    #[serde(default)]
    pub status4k: Option<u8>,
    /// Only populated on the single-item detail endpoints (`GET /movie/
    /// {id}`/`GET /tv/{id}`) — confirmed from Seerr's real source
    /// (`Media.getMedia`, `server/entity/Media.ts`): `relations: {
    /// requests: true, issues: true }`. The list-style endpoints (`/search`,
    /// `/discover/*`) use `Media.getRelatedMedia` instead, which only joins
    /// `watchlists` — `requests` stays empty there, not because no request
    /// exists, but because that query never asked for it. Added 2026-07-18
    /// to let the Discover detail page show a tier-aware, approval-aware
    /// status (`RequestDetailScreen`'s poster badge and status pills) —
    /// picks the request matching a given `is4k` tier via `.iter().find()`.
    #[serde(default)]
    pub requests: Vec<MediaRequest>,
}

impl MediaInfo {
    pub fn status(&self) -> Option<MediaStatus> {
        MediaStatus::from_code(self.status)
    }
    pub fn status4k(&self) -> Option<MediaStatus> {
        self.status4k.and_then(MediaStatus::from_code)
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResponse {
    pub page: u32,
    pub total_pages: u32,
    pub total_results: u32,
    pub results: Vec<SearchResult>,
}

/// Flattened over MovieResult/TvResult/PersonResult — discriminated by
/// `media_type` at the point of use. `title` (movie) and `name` (tv) are
/// merged into one `title` field here since Fjord never needs to distinguish
/// them beyond display; `person` results carry neither and are filtered out
/// by the caller (v1 shows movies/TV only).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    pub id: i64,
    pub media_type: String, // "movie" | "tv" | "person"
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub first_air_date: Option<String>,
    #[serde(default)]
    pub media_info: Option<MediaInfo>,
    /// TMDB genre ids on the raw multi-search/discover result — needed for
    /// client-side genre filtering of search results, since `/search`
    /// itself accepts no filter params at all (see `DiscoverFilters`' own
    /// doc comment). Movie and TV genre id spaces don't fully overlap, but
    /// that's only relevant when building a filter's own selectable list
    /// (`Genre`/`GenreItem`), not when reading this field back.
    #[serde(default)]
    pub genre_ids: Vec<i64>,
    /// TMDB average rating (0-10) — needed for client-side rating filtering
    /// of search results, same reason as `genre_ids` above.
    #[serde(default)]
    pub vote_average: Option<f64>,
    /// TMDB's own relevance ranking — needed to interleave movie and TV
    /// results into one genuinely popularity-sorted grid when the filtered-
    /// browse view's Type filter is "All" (two separate `/discover/movies`/
    /// `/discover/tv` responses, each already sorted by this same value on
    /// TMDB's side, merged client-side by comparing it directly rather than
    /// assuming a naive round-robin zip approximates the real ranking).
    #[serde(default)]
    pub popularity: Option<f64>,
}

impl SearchResult {
    pub fn display_title(&self) -> &str {
        self.title.as_deref().or(self.name.as_deref()).unwrap_or("")
    }
    pub fn year(&self) -> Option<&str> {
        self.release_date
            .as_deref()
            .or(self.first_air_date.as_deref())
            .filter(|d| d.len() >= 4)
            .map(|d| &d[..4])
    }
}

/// `GET /discover/watchlist` — same `{page, totalPages, totalResults,
/// results}` shape family as `SearchResponse` (confirmed
/// `server/interfaces/api/discoverInterfaces.ts`'s `WatchlistResponse`).
/// For a non-Plex user (every one of Fjord's 4 auth methods), this is the
/// LOCAL Watchlist table, not a Plex-synced one (confirmed
/// `server/routes/discover.ts`).
#[derive(Debug, Clone, Deserialize)]
pub struct WatchlistResponse {
    pub page: u32,
    pub total_pages: u32,
    pub total_results: u32,
    pub results: Vec<WatchlistItem>,
}

/// One row — no poster/richer data (confirmed
/// `server/interfaces/api/discoverInterfaces.ts`'s `WatchlistItem`), same
/// "needs its own per-item detail fetch" situation as a `MediaRequest` from
/// `GET /request`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchlistItem {
    pub id: i64,
    pub tmdb_id: i64,
    pub media_type: String, // "movie" | "tv"
    #[serde(default)]
    pub title: String,
}

/// `GET /discover/movies`/`GET /discover/tv`'s real filter query params
/// (confirmed from Seerr's actual route source, `server/routes/
/// discover.ts` — the OpenAPI spec has been wrong/incomplete before, see
/// this crate's own history of re-verifying against real source rather
/// than the spec). `GET /search` accepts NONE of these — only `query`/
/// `page`/`language` — so this struct is only ever used against the two
/// `/discover/*` endpoints, never search. All fields optional; a `Some`
/// field is appended to the query string, `None` is omitted entirely
/// (matching every other optional-query-param pattern already used
/// elsewhere in this crate, e.g. `create_request`'s `tags`/`profileId`).
///
/// `sort`/`date_gte` are pre-resolved to the correct literal TMDB
/// parameter VALUE (e.g. `"primary_release_date.desc"`) and QUERY KEY
/// NAME (`primaryReleaseDateGte` for movies vs `firstAirDateGte` for TV)
/// respectively by the caller — this struct doesn't know which media type
/// it's being used for, and movies/TV genuinely use different names for
/// their date-range/date-sort params (confirmed from the real route
/// source), so resolving that here would need a media-type parameter this
/// struct has no other use for.
#[derive(Debug, Clone, Default)]
pub struct DiscoverFilters {
    /// Multiple ids are pipe-joined (OR logic) at request-build time —
    /// TMDB's `with_genres`/`with_watch_providers` both take the same
    /// comma=AND / pipe=OR convention (confirmed: Seerr passes `genre`/
    /// `watchProviders` straight through to TMDB with no server-side
    /// transform).
    pub genre_ids: Option<Vec<i64>>,
    pub provider_ids: Option<Vec<i64>>,
    pub watch_region: Option<String>,
    /// Already the correct TMDB sort key for the target media type, e.g.
    /// `"popularity.desc"` or `"primary_release_date.desc"` — see this
    /// struct's own doc comment.
    pub sort: Option<&'static str>,
    pub vote_average_gte: Option<f32>,
    /// Already the correct query KEY NAME for the target media type
    /// (`primaryReleaseDateGte` vs `firstAirDateGte`) paired with its
    /// value — see this struct's own doc comment.
    pub date_gte: Option<(&'static str, String)>,
    /// Mirrors `date_gte` exactly (`primaryReleaseDateLte`/
    /// `firstAirDateLte`) — added 2026-07-18 for the "New in Theaters" row,
    /// which needs an upper bound too (without one, `date_gte` alone would
    /// also match future not-yet-released titles).
    pub date_lte: Option<(&'static str, String)>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Season {
    pub season_number: u32,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub episode_count: u32,
    #[serde(default)]
    pub air_date: Option<String>,
    #[serde(default)]
    pub poster_path: Option<String>,
}

/// A single cast member from MovieDetails/TvDetails.credits.cast — `order`
/// is TMDB's own top-billed-first ranking (lower = more prominent).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cast {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub character: Option<String>,
    #[serde(default)]
    pub order: Option<i64>,
    #[serde(default)]
    pub profile_path: Option<String>,
}

/// A single crew member from MovieDetails/TvDetails.credits.crew — `job`
/// ("Director", "Writer", "Screenplay", ...) is what Fjord filters on.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Crew {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub job: Option<String>,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Credits {
    #[serde(default)]
    pub cast: Vec<Cast>,
    #[serde(default)]
    pub crew: Vec<Crew>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Genre {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProductionCountry {
    pub iso_3166_1: String,
    pub name: String,
}

/// TV's `networks` field (Movie has no equivalent — production companies
/// are a different, unrelated field neither crate consumer needs).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Network {
    pub id: i64,
    pub name: String,
}

/// TV's `nextEpisodeToAir` — `air_date` was the only field this crate's
/// consumer needed originally; `episode_number`/`name`/`season_number`
/// added 2026-07-18 for the "Coming Up" calendar entry label (all already
/// present in the real `TmdbTvEpisodeResult` shape TMDB returns, confirmed
/// from Seerr's own `server/api/themoviedb/interfaces.ts`, just unread
/// until now — `overview`/`still_path` also exist there but aren't
/// consumed by anything yet, so left unmodeled, same "only what's
/// consumed" style as `Video`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextEpisode {
    #[serde(default)]
    pub air_date: Option<String>,
    #[serde(default)]
    pub episode_number: Option<i64>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub season_number: Option<i64>,
}

/// `MovieDetails.releases` (2026-07-18, Watchlist + Release Calendar) — the
/// raw TMDB `release_dates` shape, forwarded verbatim by Seerr's own
/// `mapMovieDetails` (confirmed `server/models/Movie.ts`: `releases:
/// movie.release_dates`). TV has no equivalent — TMDB doesn't track
/// per-episode release types, only `nextEpisodeToAir.airDate` above.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseDatesResult {
    #[serde(default)]
    pub results: Vec<RegionReleases>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RegionReleases {
    pub iso_3166_1: String,
    #[serde(default)]
    pub release_dates: Vec<ReleaseDateEntry>,
}

/// `certification`/`note`/`iso_639_1` exist in the real response too but
/// aren't consumed — only what a calendar entry needs. `release_type`'s
/// real TMDB meaning (confirmed from Seerr's own frontend,
/// `src/components/MovieDetails/index.tsx`): 1=Premiere, 2=Theatrical
/// (limited), 3=Theatrical, 4=Digital, 5=Physical, 6=TV — Seerr's own UI
/// only ever shows 3/4/5, which is exactly the cinema/streaming/physical
/// split this crate's own consumer wants.
#[derive(Debug, Clone, Deserialize)]
pub struct ReleaseDateEntry {
    #[serde(rename = "type")]
    pub release_type: i32,
    pub release_date: String,
}

/// One region's entry in `MovieDetails`/`TvDetails.watchProviders` —
/// `buy`/`rent`/`link` exist in the real response too but aren't modeled
/// here, only `flatrate` (subscription-included streaming, what "Currently
/// Streaming On" means).
#[derive(Debug, Clone, Deserialize)]
pub struct WatchProviderEntry {
    pub iso_3166_1: String,
    #[serde(default)]
    pub flatrate: Vec<WatchProviderDetail>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchProviderDetail {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub logo_path: Option<String>,
}

/// `MovieDetails`/`TvDetails.relatedVideos` entry — YouTube trailer/teaser/
/// clip links. Confirmed from Seerr's real source (`server/models/
/// common.ts`'s `mapVideos`/`siteUrlCreator`) that `url` is already a
/// fully-formed `https://www.youtube.com/watch?v={key}` link, and `site`
/// is always `"YouTube"` in practice (the mapper's own type signature only
/// ever maps that one site) — so only `kind`/`url` are modeled, same
/// "only what's consumed" style as `NextEpisode`. `kind` distinguishes
/// `"Trailer"`/`"Teaser"`/`"Clip"`/`"Featurette"`/etc; `#[serde(rename)]`
/// since `type` is a Rust keyword.
#[derive(Debug, Clone, Deserialize)]
pub struct Video {
    #[serde(rename = "type")]
    pub kind: String,
    pub url: String,
}

/// `GET /watchproviders/regions` list entry — every region TMDB has
/// watch-provider data for; used to populate the Streaming Region picker
/// (Settings -> Integrations).
#[derive(Debug, Clone, Deserialize)]
pub struct Region {
    pub iso_3166_1: String,
    pub english_name: String,
}

/// `GET /languages` list entry — TMDB's full language list (confirmed via
/// Seerr's real source, `server/api/themoviedb/index.ts::getLanguages` ->
/// TMDB's `/configuration/languages`, ~180 entries). Backs BOTH the
/// Discover Language (`originalLanguage` filter) and Display Language
/// (`locale`) pickers in fjord-app, deliberately sharing one fetched list
/// rather than hardcoding Seerr's own separate, much smaller (~40-entry)
/// UI-translation locale set (`src/context/LanguageContext.tsx`) for
/// Display Language — Fjord never renders Seerr's own web UI text, so the
/// only real effect `locale` has here is as the default TMDB `language`
/// query param on movie/tv/search calls (confirmed from
/// `server/middleware/auth.ts`'s `req.locale = user.settings.locale` and
/// `server/routes/movie.ts`'s `language: query.language ?? req.locale`),
/// which the fuller TMDB list serves just as well.
#[derive(Debug, Clone, Deserialize)]
pub struct Language {
    pub iso_639_1: String,
    pub english_name: String,
}

/// `GET`/`POST /user/{id}/settings/main`'s "general" shape — only the
/// fields this crate's consumer round-trips. **The POST handler
/// unconditionally overwrites `username`/`email`/etc. from the body with
/// no partial-patch semantics** (confirmed from Seerr's real source, not
/// assumed) — a caller changing just one field (e.g. `streaming_region`)
/// must `GET` this struct first, mutate the one field, and `POST` the
/// whole thing back; constructing one from scratch with the rest left at
/// `Default`/`None` would blank out the user's username/email server-side.
///
/// **Every field is `skip_serializing_if = "Option::is_none"` on the way
/// out — this is load-bearing, not cosmetic.** Live-reproduced: for an
/// account that has never saved anything under Seerr's own Settings ->
/// General (a real, unremarkable state — confirmed via `GET /auth/me`
/// returning `"settings": null` for such a user), `GET .../settings/main`
/// simply omits keys like `locale` entirely rather than returning them as
/// `null`, so this struct deserializes them as `None`. Seerr's
/// `user_settings.locale` DB column is `NOT NULL` with an empty-string
/// default — sending it back as JSON `null` (which plain `Option<String>`
/// serialization does unconditionally) reaches the SQL layer unchanged and
/// the whole write 500s: `{"message":"SQLITE_CONSTRAINT: NOT NULL
/// constraint failed: user_settings.locale"}` (the exact body, captured by
/// hand-crafting the same POST directly against a live instance — the
/// generic `error_for_status()` Fjord's own client used at the time threw
/// away this message entirely, showing only "500 Internal Server Error"
/// with no indication of why). Omitting the key outright (confirmed live
/// against the same instance) lets Seerr fall back to its own column
/// default instead, which succeeds. Applied to every field, not just
/// `locale` — the same class of NOT NULL mismatch could exist on any of
/// these columns on a different Seerr version/install, and omitting an
/// unset field is also just correct: this client never has an opinion on a
/// field it never received a real value for.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UserGeneralSettings {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locale: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub discover_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub streaming_region: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub original_language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watchlist_sync_movies: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watchlist_sync_tv: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovieDetails {
    pub id: i64,
    pub title: String,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default)]
    pub backdrop_path: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub genres: Vec<Genre>,
    #[serde(default)]
    pub vote_average: Option<f64>,
    #[serde(default)]
    pub credits: Option<Credits>,
    #[serde(default)]
    pub media_info: Option<MediaInfo>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub original_language: String,
    #[serde(default)]
    pub production_countries: Vec<ProductionCountry>,
    #[serde(default)]
    pub watch_providers: Vec<WatchProviderEntry>,
    #[serde(default)]
    pub related_videos: Vec<Video>,
    /// Already computed server-side (confirmed `server/routes/movie.ts`:
    /// `onUserWatchlist: userWatchlist`) — zero extra network calls to know
    /// watchlist state on the detail page (2026-07-18, Watchlist + Release
    /// Calendar).
    #[serde(default)]
    pub on_user_watchlist: bool,
    /// TMDB's per-region theatrical/digital/physical release dates,
    /// forwarded verbatim by Seerr — see `ReleaseDatesResult`'s own doc
    /// comment. TV has no equivalent (2026-07-18, Watchlist + Release
    /// Calendar).
    #[serde(default)]
    pub releases: Option<ReleaseDatesResult>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TvDetails {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default)]
    pub backdrop_path: Option<String>,
    #[serde(default)]
    pub first_air_date: Option<String>,
    #[serde(default)]
    pub genres: Vec<Genre>,
    #[serde(default)]
    pub seasons: Vec<Season>,
    #[serde(default)]
    pub vote_average: Option<f64>,
    #[serde(default)]
    pub credits: Option<Credits>,
    #[serde(default)]
    pub media_info: Option<MediaInfo>,
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub original_language: String,
    #[serde(default)]
    pub production_countries: Vec<ProductionCountry>,
    #[serde(default)]
    pub next_episode_to_air: Option<NextEpisode>,
    #[serde(default)]
    pub networks: Vec<Network>,
    #[serde(default)]
    pub watch_providers: Vec<WatchProviderEntry>,
    #[serde(default)]
    pub related_videos: Vec<Video>,
    /// See `MovieDetails.on_user_watchlist`'s own doc comment (confirmed
    /// `server/routes/tv.ts`: `onUserWatchlist: userWatchlist`) — 2026-07-18.
    #[serde(default)]
    pub on_user_watchlist: bool,
}

/// POST /request body's `seasons` field — either a specific list of season
/// numbers or the literal string "all" (Seerr's own shorthand for every
/// season). Serializes untagged so the wire shape matches exactly.
#[derive(Debug, Clone, Serialize)]
#[serde(untagged)]
pub enum SeasonsSelector {
    Numbers(Vec<u32>),
    All(&'static str), // always constructed as All("all")
}

impl SeasonsSelector {
    pub fn all() -> Self {
        Self::All("all")
    }
}

/// `status` is the *request* (approval workflow) state — real enum,
/// confirmed from Seerr's own source (`server/constants/media.ts`,
/// `MediaRequestStatus`): 1=PENDING, 2=APPROVED, 3=DECLINED, 4=FAILED,
/// 5=COMPLETED — a different enum from `MediaInfo.status` (fulfillment
/// state: Unknown/Pending/Processing/PartiallyAvailable/Available/
/// Blocklisted/Deleted). `media`/`created_at` are only populated by `GET
/// /request` (the create-request response doesn't need them) — `#[serde(default)]`
/// so both endpoints deserialize into the same struct. `requested_by`/
/// `profile_id`/`tags`/`seasons` are all already present on the same `GET
/// /request` response (confirmed from Seerr's route source —
/// `leftJoinAndSelect`s `requestedBy`/`seasons`, and `profileId`/`tags` are
/// plain unguarded columns on the entity), added 2026-07-18 for the
/// Discover context menu's Edit/Cancel/Approve/Decline actions — no new
/// network call needed to support them.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRequest {
    pub id: i64,
    pub status: u8,
    /// Which tier THIS request is for — confirmed live (2026-07-18) this is
    /// the field that must pick which of `MediaInfo.status`/`status4k` is
    /// the relevant fulfillment status, not `media.status` alone. Rust
    /// field name matches the JSON key verbatim (`is4k`, already valid
    /// snake_case — no `rename_all` transform needed or relied on, same
    /// reasoning as `status4k` above).
    #[serde(default)]
    pub is4k: bool,
    #[serde(default)]
    pub media: Option<MediaInfo>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub requested_by: Option<RequestedBy>,
    #[serde(default)]
    pub profile_id: Option<i64>,
    #[serde(default)]
    pub tags: Option<Vec<i64>>,
    #[serde(default)]
    pub seasons: Vec<SeasonRequestNumber>,
}

impl MediaRequest {
    pub fn is_pending(&self) -> bool {
        self.status == 1
    }
}

/// Minimal nested shape of `MediaRequest.requestedBy` — only the id is
/// needed (the Discover context menu's ownership check for Edit/Cancel),
/// not the full `User` shape.
#[derive(Debug, Clone, Deserialize)]
pub struct RequestedBy {
    pub id: i64,
}

/// One entry of `MediaRequest.seasons` — a *different* shape from `Season`
/// above (TMDB's own per-season metadata: name/posterPath/episodeCount).
/// This is Seerr's own tracked per-season request state; only the season
/// number is needed here, to pre-fill the Edit Request season picker.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SeasonRequestNumber {
    pub season_number: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct User {
    pub id: i64,
    #[serde(default)]
    pub username: Option<String>,
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    /// Plain bitmask, confirmed from Seerr's real source
    /// (`server/entity/User.ts`: `@Column({type: 'integer', default: 0})
    /// public permissions = 0;` — no `select:false`/exclusion, genuinely
    /// returned by `/auth/me`, which Fjord already calls). `MANAGE_REQUESTS
    /// = 16` (`server/lib/permissions.ts`) is the literal bit Fjord's
    /// Approve/Decline/Cancel context-menu rows care about.
    #[serde(default)]
    pub permissions: u32,
}

impl User {
    pub fn label(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.username.clone())
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| format!("user #{}", self.id))
    }

    /// **Real bug, live-reported 2026-07-18** ("on requested 4k items I
    /// only got detail on the context menu"): this originally checked bit
    /// 16 (`MANAGE_REQUESTS`) alone. But Seerr's own `hasPermission()`
    /// (`server/lib/permissions.ts`) treats the `ADMIN` bit (2) as a
    /// universal bypass for every permission check — `!!(value &
    /// Permission.ADMIN) || !!(value & total)` — and the owner/first-admin
    /// account is provisioned with exactly `permissions: Permission.ADMIN`
    /// (confirmed from `server/routes/auth.ts`'s account-creation paths),
    /// not the literal `MANAGE_REQUESTS` bit. On a personal single-user
    /// Seerr instance the connected account is almost always this owner
    /// account, so the old bit-16-only check made `can_manage_requests()`
    /// false for the one account most likely to actually have the
    /// server-side permission — Approve/Decline (and the admin bypass on
    /// Cancel) silently never appeared. Mirrors the real OR-bypass exactly.
    pub fn can_manage_requests(&self) -> bool {
        self.permissions & (2 | 16) != 0
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickConnect {
    pub code: String,
    pub secret: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QuickConnectStatus {
    pub authenticated: bool,
}

/// GET /status — unauthenticated. Only `version` is used today (Settings
/// sidebar); the other fields Seerr returns (commitTag, updateAvailable,
/// commitsBehind, restartRequired) are ignored (serde drops unknown-to-us
/// fields silently, no `deny_unknown_fields`).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StatusInfo {
    pub version: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Tag {
    pub id: i64,
    pub label: String,
}

/// A Radarr/Sonarr quality profile ("720p/1080p", "WEB-1080p", "Remux-2160p",
/// whatever the admin named it) — `GET /service/{radarr|sonarr}/{id}`'s
/// `profiles` field, same undocumented-in-the-spec situation as `Tag` above
/// (the spec shows it as a single `ServiceProfile` object with no `type:
/// array` wrapper; confirmed via Seerr's actual TypeScript source
/// (`QualityProfile[]` in `serviceInterfaces.ts`) that it's really an array).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Profile {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceServer {
    pub id: i64,
    #[serde(default)]
    pub is_default: bool,
    // Whether this server entry is the 4K-tier instance (an admin can
    // configure a separate Radarr/Sonarr server dedicated to 4K, each with
    // its own isDefault flag) — used to pick the tags/profiles matching
    // whichever quality tier a request is actually going to.
    #[serde(default)]
    pub is4k: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceServerDetails {
    #[serde(default)]
    pub tags: Vec<Tag>,
    #[serde(default)]
    pub profiles: Vec<Profile>,
}
