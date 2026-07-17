// ── fjord-seerr · models.rs ──────────────────────────────────────────────────
//   MediaStatus       MediaInfo.status: 1=Unknown 2=Pending 3=Processing
//                     4=PartiallyAvailable 5=Available 6=Blocklisted 7=Deleted
//                     (verified against Seerr's real server/constants/media.ts
//                     after a live bug — see MediaStatus's own doc comment)
//   MediaInfo         status + tmdbId, present only once Seerr has seen an item
//   SearchResponse/SearchResult  GET /search — mediaType discriminates movie/tv/person
//   MovieDetails/TvDetails       GET /movie/{id}, /tv/{id} — voteAverage + credits (Cast/Crew)
//                                 confirmed present in the OpenAPI spec but not deserialized
//                                 until the RequestDetailScreen redesign (2026-07-16)
//   Season                       TvDetails.seasons — TMDB-shape, no per-season
//                                 Jellyfin-availability field in the published spec.
//                                 posterPath also present in the spec, same
//                                 previously-undeserialized-field situation as above
//   Credits/Cast/Crew            MovieDetails/TvDetails.credits — cast (id/name/character/
//                                 order/profilePath) + crew (id/name/job/department/profilePath)
//   SeasonsSelector              POST /request body's `seasons`: array or "all"
//   MediaRequest                 POST /request response + GET /request list entries (media/
//                                 created_at only populated by the latter — Discover "Requested" row)
//   User                         auth response — id/displayName for "Connected as X"
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
//                                 above); added for the request-detail metadata panel (2026-07-17)
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
}

impl MediaInfo {
    pub fn status(&self) -> Option<MediaStatus> {
        MediaStatus::from_code(self.status)
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

/// TV's `nextEpisodeToAir` — only the one field this crate's consumer
/// needs out of the full episode shape.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NextEpisode {
    #[serde(default)]
    pub air_date: Option<String>,
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

/// `status` is the *request* (approval workflow) state: 1=PENDING_APPROVAL,
/// 2=APPROVED, 3=DECLINED — a different enum from `MediaInfo.status`
/// (fulfillment state: Unknown/Pending/Processing/PartiallyAvailable/
/// Available/Deleted). `media`/`created_at` are only populated by `GET
/// /request` (the create-request response doesn't need them) — `#[serde(default)]`
/// so both endpoints deserialize into the same struct.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRequest {
    pub id: i64,
    pub status: u8,
    #[serde(default)]
    pub media: Option<MediaInfo>,
    #[serde(default)]
    pub created_at: Option<String>,
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
}

impl User {
    pub fn label(&self) -> String {
        self.display_name
            .clone()
            .or_else(|| self.username.clone())
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| format!("user #{}", self.id))
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
