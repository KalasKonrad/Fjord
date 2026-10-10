// ── fjord-player · lib.rs ────────────────────────────────────────────────────
//   re-exports  MpvRenderCtx, Player, PlayerConfig, PollResult, SourceHdrMetadata,
//               StatsData, TrackInfo, redact_api_key
// ─────────────────────────────────────────────────────────────────────────────
pub mod mpv;
pub use mpv::{
    MpvRenderCtx, Player, PlayerConfig, PollResult, SourceHdrMetadata, StatsData, TrackInfo,
    redact_api_key,
};
