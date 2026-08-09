// ── fjord-api · lib.rs ───────────────────────────────────────────────────────
//   re-exports  authenticate (auth), JellyfinClient (client) + its Bonfire/JellyProfiles
//               plugin methods (bonfire), all models
// ─────────────────────────────────────────────────────────────────────────────
pub mod auth;
pub mod bonfire;
pub mod client;
pub mod models;

pub use auth::authenticate;
pub use bonfire::bonfire_profile_image_url;
pub use client::JellyfinClient;
