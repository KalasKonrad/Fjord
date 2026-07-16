// ── fjord-seerr · lib.rs ─────────────────────────────────────────────────────
//   re-exports  SeerrClient, SeerrAuth (client), all models
// ─────────────────────────────────────────────────────────────────────────────
pub mod client;
pub mod models;

pub use client::{SeerrAuth, SeerrClient};
pub use models::{
    Cast, Credits, Crew, MediaInfo, MediaRequest, MediaStatus, MovieDetails, Profile,
    QuickConnect, QuickConnectStatus, SearchResponse, SearchResult, Season, SeasonsSelector,
    ServiceServer, ServiceServerDetails, StatusInfo, Tag, TvDetails, User,
};
