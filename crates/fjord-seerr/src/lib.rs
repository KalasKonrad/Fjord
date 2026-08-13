// ── fjord-seerr · lib.rs ─────────────────────────────────────────────────────
//   re-exports  SeerrClient, SeerrAuth (client), all models
// ─────────────────────────────────────────────────────────────────────────────
pub mod client;
pub mod models;

pub use client::{SeerrAuth, SeerrClient};
pub use models::{
    BlocklistItem, BlocklistResponse, Cast, Collection, CombinedCredits, Credits, Crew, DiscoverFilters,
    Genre, MediaInfo, MediaRequest, MediaStatus, MovieCollectionRef, MovieDetails, Network, NextEpisode,
    PageInfo, PersonCreditCast, PersonCreditCrew, PersonDetails, ProductionCountry, Profile, QuickConnect,
    QuickConnectStatus, RegionReleases, Region, ReleaseDateEntry, ReleaseDatesResult, SearchResponse,
    SearchResult, Season, SeasonsSelector, ServiceServer, ServiceServerDetails, StatusInfo, Tag, TvDetails,
    User, UserGeneralSettings, Video, WatchProviderDetail, WatchProviderEntry, WatchlistItem,
    WatchlistResponse,
};
