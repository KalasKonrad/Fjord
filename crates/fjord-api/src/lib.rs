// ── fjord-api · lib.rs ───────────────────────────────────────────────────────
//   re-exports  authenticate (auth), JellyfinClient (client), all models
// ─────────────────────────────────────────────────────────────────────────────
pub mod auth;
pub mod client;
pub mod models;

pub use auth::authenticate;
pub use client::JellyfinClient;
