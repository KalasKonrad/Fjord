// ── fjord-seerr · lib.rs ─────────────────────────────────────────────────────
//   re-exports  SeerrClient, SeerrAuth (client), all models
// ─────────────────────────────────────────────────────────────────────────────
pub mod client;
pub mod models;

pub use client::{SeerrAuth, SeerrClient};
pub use models::{
    MediaInfo, MediaRequest, MediaStatus, MovieDetails, QuickConnect, QuickConnectStatus,
    SearchResponse, SearchResult, Season, SeasonsSelector, ServiceServer, ServiceServerDetails,
    StatusInfo, Tag, TvDetails, User,
};
