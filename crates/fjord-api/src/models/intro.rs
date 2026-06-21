// ── fjord-api · models/intro.rs ──────────────────────────────────────────────
//   Segment           a skippable segment (start/end in seconds); valid when end > 0
//   EpisodeTimestamps combined Introduction + Credits segments from Intro Skipper v2+
//                     endpoint: GET /Episode/{id}/Timestamps
// ─────────────────────────────────────────────────────────────────────────────
use serde::Deserialize;

/// A single skippable segment from the Intro Skipper plugin.
/// `end > 0.0` means the segment was detected.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct Segment {
    #[serde(rename = "Start", default)]
    pub start: f64,
    #[serde(rename = "End", default)]
    pub end: f64,
}

impl Segment {
    pub fn valid(&self) -> bool { self.end > 0.0 }
}

/// Response from `GET /Episode/{id}/Timestamps` (Intro Skipper v2+ plugin).
#[derive(Debug, Clone, Deserialize)]
pub struct EpisodeTimestamps {
    #[serde(rename = "Introduction", default)]
    pub introduction: Segment,
    #[serde(rename = "Credits", default)]
    pub credits: Segment,
}
