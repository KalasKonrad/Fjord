// ── fjord-seerr · models.rs ──────────────────────────────────────────────────
//   MediaStatus       MediaInfo.status: 1=Unknown 2=Pending 3=Processing
//                     4=PartiallyAvailable 5=Available 6=Deleted
//   MediaInfo         status + tmdbId, present only once Seerr has seen an item
//   SearchResponse/SearchResult  GET /search — mediaType discriminates movie/tv/person
//   MovieDetails/TvDetails       GET /movie/{id}, /tv/{id}
//   Season                       TvDetails.seasons — TMDB-shape, no per-season
//                                 Jellyfin-availability field in the published spec
//   SeasonsSelector              POST /request body's `seasons`: array or "all"
//   MediaRequest                 POST /request response
//   User                         auth response — id/displayName for "Connected as X"
//   QuickConnect                 POST /auth/jellyfin/quickconnect/initiate response
//   StatusInfo                   GET /status response — version, shown in Settings sidebar
//   Tag                          Radarr/Sonarr tag {id, label} — GET /service/{radarr|sonarr}/{id}'s
//                                 `tags` field, NOT in the published OpenAPI spec (confirmed from
//                                 Seerr's actual TypeScript source, same class of gap as media_type below)
//   ServiceServer                GET /service/{radarr|sonarr} list entry — only `id`/`isDefault` used
//                                 (find the default server to fetch tags for; no per-server picker in v1)
//   ServiceServerDetails         GET /service/{radarr|sonarr}/{id} — only `tags` extracted; every other
//                                 field (profiles, rootFolders, server, languageProfiles) ignored
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[repr(u8)]
pub enum MediaStatus {
    Unknown = 1,
    Pending = 2,
    Processing = 3,
    PartiallyAvailable = 4,
    Available = 5,
    Deleted = 6,
}

impl MediaStatus {
    pub fn from_code(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Unknown),
            2 => Some(Self::Pending),
            3 => Some(Self::Processing),
            4 => Some(Self::PartiallyAvailable),
            5 => Some(Self::Available),
            6 => Some(Self::Deleted),
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
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Genre {
    pub id: i64,
    pub name: String,
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
    pub media_info: Option<MediaInfo>,
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
    pub media_info: Option<MediaInfo>,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaRequest {
    pub id: i64,
    pub status: u8,
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

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceServer {
    pub id: i64,
    #[serde(default)]
    pub is_default: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServiceServerDetails {
    #[serde(default)]
    pub tags: Vec<Tag>,
}
