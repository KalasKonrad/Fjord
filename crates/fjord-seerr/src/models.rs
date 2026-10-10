// ── fjord-seerr · models.rs ────────────────────────────────────────────────
//   MediaStatus       MediaInfo.status: 1 Unknown 2 Pending 3 Processing 4 PartiallyAvailable 5 Available
//                     6 Blocklisted 7 Deleted (Seerr server/constants/media.ts)
//   MediaInfo         status + status4k (independent per tier), tmdbId; requests only on detail endpoints
//   SearchResponse/SearchResult  GET /search (mediaType movie/tv/person; genreIds/voteAverage/popularity
//                     for client-side filtering and sorting)
//   DiscoverFilters   GET /discover/movies|tv filter params; sort/date keys pre-resolved per media type
//   WatchlistResponse/WatchlistItem  GET /discover/watchlist (the local per-user watchlist)
//   BlocklistResponse/BlocklistItem/PageInfo  GET /blocklist (own {pageInfo} envelope; global per server)
//   MovieDetails/TvDetails  GET /movie|tv/{id}: credits, voteAverage, onUserWatchlist, releases + collection
//                     (movies), status/originalLanguage/productionCountries/networks/nextEpisodeToAir/
//                     watchProviders, relatedVideos
//   MovieCollectionRef / Collection  belongs_to_collection; GET /collection/{id} (parts = SearchResult)
//   PersonDetails / PersonCreditCast / PersonCreditCrew / CombinedCredits  GET /person/{id}[/combined_credits]
//   ReleaseDatesResult/RegionReleases/ReleaseDateEntry  per-region theatrical(3)/digital(4)/physical(5) dates
//   Season, Credits/Cast/Crew, SeasonsSelector ("all" or a list of season numbers)
//   MediaRequest      POST /request response + GET /request entries; status 1 Pending 2 Approved 3 Declined
//                     4 Failed 5 Completed (is_pending); is4k picks status vs status4k
//   RequestedBy / SeasonRequestNumber  who requested (Edit/Cancel ownership); requested seasons
//   User              auth response; can_manage_requests / can_manage_blocklist (ADMIN bypasses both)
//   QuickConnect, StatusInfo, Tag, Profile, ServiceServer, ServiceServerDetails  Radarr/Sonarr default
//                     server tags + quality profiles (not in the OpenAPI spec; from Seerr's source)
//   ProductionCountry/Network/NextEpisode/WatchProviderEntry/WatchProviderDetail, Video, Region, Language
//   UserGeneralSettings  GET/POST /user/{id}/settings/main (own profile or admin; POST replaces the object)
//
// Every Deserialize struct carries #[serde(rename_all = "camelCase")]: Seerr's JSON is camelCase, and
// without it a required field fails while Option fields silently stay None.
// ─────────────────────────────────────────────────────────────────────────────
use serde::{Deserialize, Serialize};

/// Seerr's real enum (server/constants/media.ts) — 6 is Blocklisted and Deleted is 7. (An
/// older model had Deleted at 6, so real Deleted items fell through `from_code` and
/// stayed in the Requested row.)
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
    /// The 4K tier's fulfilment status, independent of `status` (an item can be status 1
    /// Unknown with status4k 5 Available). Pick by the request's tier (`MediaRequest.is4k`)
    /// — see `requested_not_available`.
    #[serde(default)]
    pub status4k: Option<u8>,
    /// Only on the single-item detail endpoints (`GET /movie|tv/{id}` — Media.getMedia joins
    /// requests). List endpoints (`/search`, `/discover/*` — Media.getRelatedMedia) only join
    /// watchlists, so `requests` is empty there even when requests exist. Feeds the detail
    /// page's tier- and approval-aware status.
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

/// `GET /discover/watchlist` — {page, totalPages, totalResults, results}
/// (discoverInterfaces.ts). For non-Plex users (all of Fjord's auth methods) this is the
/// LOCAL watchlist table (routes/discover.ts). camelCase like everything else.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
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

/// `GET /blocklist`'s own pagination envelope — genuinely different shape
/// from `SearchResponse`/`WatchlistResponse`'s `{page,totalPages,
/// totalResults}` (confirmed from Seerr's real `server/interfaces/api/
/// common.ts` `PaginatedResponse` + `server/routes/blocklist.ts`'s response
/// construction), so it gets its own struct rather than reusing theirs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PageInfo {
    pub pages: u32,
    pub page: u32,
    pub results: u32,
    pub page_size: u32,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlocklistResponse {
    pub page_info: PageInfo,
    pub results: Vec<BlocklistItem>,
}

/// One blocklisted title — no poster (blocklistInterfaces.ts). `user`/`created_at` give
/// "Blocklisted by X on Y" with no extra call. `blocklisted_tags` (Sonarr/Radarr
/// auto-blocklist by tag) is modeled but not shown.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BlocklistItem {
    pub tmdb_id: i64,
    pub media_type: String, // "movie" | "tv"
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub user: Option<User>,
    #[serde(default)]
    pub blocklisted_tags: Option<String>,
}

/// `GET /discover/movies` / `/discover/tv` filter params, from Seerr's route source
/// (server/routes/discover.ts — the OpenAPI spec has been wrong before). `GET /search`
/// takes none of them (only query/page/language). `Some` fields go into the query
/// string, `None` is omitted. `sort`/`date_gte` are resolved by the caller to the TMDB
/// VALUE ("primary_release_date.desc") and the per-type KEY (primaryReleaseDateGte vs
/// firstAirDateGte) — this struct doesn't know its media type.
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
    /// Like `date_gte` (primaryReleaseDateLte / firstAirDateLte) — the upper bound
    /// "New in Theaters" needs.
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

/// TV's `nextEpisodeToAir`: air_date + episode_number/name/season_number for the
/// Coming Up label (TMDB's TmdbTvEpisodeResult; overview/still_path left out).
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

/// `MovieDetails.releases` — TMDB's raw release_dates, forwarded by Seerr's
/// mapMovieDetails (models/Movie.ts). TV has no equivalent (only nextEpisodeToAir).
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

/// `GET`/`POST /user/{id}/settings/main` ("general") — only the fields Fjord round-trips.
/// **The POST overwrites username/email/… from the body (no partial patch):** GET, change
/// one field, POST the whole thing back; building one from scratch would blank the
/// user's username/email.
///
/// **Every field is `skip_serializing_if = "Option::is_none"` — load-bearing.** For a user
/// who never saved Seerr's General settings, GET omits keys like `locale`; sending them
/// back as `null` hits NOT NULL columns and the write 500s ("SQLITE_CONSTRAINT: NOT NULL
/// constraint failed: user_settings.locale"). Omitted, Seerr uses its column default.
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
    /// Computed server-side (routes/movie.ts: onUserWatchlist) — watchlist state with no
    /// extra call.
    #[serde(default)]
    pub on_user_watchlist: bool,
    /// TMDB's per-region release dates — see `ReleaseDatesResult`. TV has none.
    #[serde(default)]
    pub releases: Option<ReleaseDatesResult>,
    /// The movie's TMDB collection (`belongs_to_collection` → {id, name, posterPath,
    /// backdropPath}, models/Movie.ts) — how a local BoxSet's TMDB collection id is found,
    /// from any member's already-fetched details.
    #[serde(default)]
    pub collection: Option<MovieCollectionRef>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovieCollectionRef {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default)]
    pub backdrop_path: Option<String>,
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

/// GET /collection/{id} — the full TMDB collection, diffed against a local BoxSet for
/// the Collection screen's "Missing From This Collection" row. `parts` reuses
/// `SearchResult` (mapCollection uses the same mapMovieResult as /search).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Collection {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub overview: Option<String>,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default)]
    pub backdrop_path: Option<String>,
    #[serde(default)]
    pub parts: Vec<SearchResult>,
}

/// GET /person/{id}/combined_credits — a person's full TMDB filmography (Person screen's
/// "Other Work" row). Cast and crew share base fields plus `character` (cast) or
/// `department`+`job` (crew) (models/Person.ts); `media_type` is optional on some old
/// entries. No `media_info`: this join is watchlist-only (like /search), so request state
/// is patched client-side.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonCreditCast {
    pub id: i64,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub first_air_date: Option<String>,
    #[serde(default)]
    pub character: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonCreditCrew {
    pub id: i64,
    #[serde(default)]
    pub media_type: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub poster_path: Option<String>,
    #[serde(default)]
    pub release_date: Option<String>,
    #[serde(default)]
    pub first_air_date: Option<String>,
    #[serde(default)]
    pub department: Option<String>,
    #[serde(default)]
    pub job: Option<String>,
}

/// GET /person/{id} — TMDB bio/portrait via Seerr's mapPersonDetails (checked in
/// models/Person.ts, not against a live instance). Only what the TMDB-only person
/// screen needs is modeled.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PersonDetails {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub biography: Option<String>,
    #[serde(default)]
    pub profile_path: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CombinedCredits {
    pub id: i64,
    #[serde(default)]
    pub cast: Vec<PersonCreditCast>,
    #[serde(default)]
    pub crew: Vec<PersonCreditCrew>,
}

impl PersonCreditCast {
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

impl PersonCreditCrew {
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

/// `status` = the REQUEST's workflow state (MediaRequestStatus, constants/media.ts):
/// 1 PENDING 2 APPROVED 3 DECLINED 4 FAILED 5 COMPLETED — not `MediaInfo.status`
/// (fulfilment). `media`/`created_at` only come with GET /request (`#[serde(default)]`,
/// so both endpoints deserialize here); `requested_by`/`profile_id`/`tags`/`seasons` are on
/// the same response and feed the context menu's Edit/Cancel/Approve/Decline.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRequest {
    pub id: i64,
    pub status: u8,
    /// Which tier THIS request is for — decides between `MediaInfo.status` and `status4k`.
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

    /// MANAGE_REQUESTS (bit 16) OR ADMIN (bit 2): Seerr's hasPermission() (lib/permissions.ts)
    /// treats ADMIN as a bypass for every permission, and the owner account carries exactly
    /// ADMIN — bit 16 alone hid Approve/Decline from the account most likely to have them.
    pub fn can_manage_requests(&self) -> bool {
        self.permissions & (2 | 16) != 0
    }

    /// Same ADMIN bypass, with MANAGE_BLOCKLIST = 268435456 — a separate bit (lib/permissions.ts).
    pub fn can_manage_blocklist(&self) -> bool {
        self.permissions & (2 | 268_435_456) != 0
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
