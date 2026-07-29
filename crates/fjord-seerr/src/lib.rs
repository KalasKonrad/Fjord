// ── fjord-seerr · lib.rs ─────────────────────────────────────────────────────
//   re-exports  SeerrClient, SeerrAuth (client), all models
// ─────────────────────────────────────────────────────────────────────────────
pub mod client;
pub mod models;

pub use client::{SeerrAuth, SeerrClient};
pub use models::{
    Cast, Collection, CombinedCredits, Credits, Crew, DiscoverFilters, Genre, MediaInfo, MediaRequest,
    MediaStatus, MovieCollectionRef, MovieDetails, Network, NextEpisode, PersonCreditCast, PersonCreditCrew,
    ProductionCountry, Profile, QuickConnect, QuickConnectStatus, RegionReleases, Region, ReleaseDateEntry,
    ReleaseDatesResult, SearchResponse, SearchResult, Season, SeasonsSelector, ServiceServer,
    ServiceServerDetails, StatusInfo, Tag, TvDetails, User, UserGeneralSettings, Video, WatchProviderDetail,
    WatchProviderEntry, WatchlistItem, WatchlistResponse,
};
