// ── fjord-player · lib.rs ────────────────────────────────────────────────────
//   re-exports  MpvRenderCtx, Player, PlayerConfig, PollResult, StatsData, TrackInfo
// ─────────────────────────────────────────────────────────────────────────────
pub mod mpv;
pub use mpv::{MpvRenderCtx, Player, PlayerConfig, PollResult, StatsData, TrackInfo};
