// ── fjord-player · lib.rs ────────────────────────────────────────────────────
//   re-exports  MpvRenderCtx, Player, PlayerConfig, PollResult, StatsData, TrackInfo, redact_api_key
// ─────────────────────────────────────────────────────────────────────────────
pub mod mpv;
pub use mpv::{redact_api_key, MpvRenderCtx, Player, PlayerConfig, PollResult, StatsData, TrackInfo};
