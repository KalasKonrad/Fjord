// ── fjord-api · lib.rs ───────────────────────────────────────────────────────
//   re-exports  authenticate + the Quick Connect calls (auth), JellyfinClient (client) + its Bonfire/JellyProfiles
//               plugin methods (bonfire), all models
// ─────────────────────────────────────────────────────────────────────────────
pub mod auth;
pub mod bonfire;
pub mod client;
pub mod models;

pub use auth::{
    QuickConnectStatus, authenticate, authenticate_with_quick_connect, quick_connect_enabled,
    quick_connect_initiate, quick_connect_status,
};
pub use bonfire::bonfire_profile_image_url;
pub use client::JellyfinClient;
