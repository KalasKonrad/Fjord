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
    #[serde(rename = "Recap", default)]
    pub recap: Segment,
    #[serde(rename = "Preview", default)]
    pub preview: Segment,
    #[serde(rename = "Commercial", default)]
    pub commercial: Segment,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_all_segments() {
        let json = r#"{
            "Introduction":{"Start":0.0,"End":92.5},
            "Credits":{"Start":1340.0,"End":1380.0},
            "Recap":{"Start":0.0,"End":45.0},
            "Preview":{"Start":1350.0,"End":1370.0},
            "Commercial":{"Start":600.0,"End":630.0}
        }"#;
        let ts: EpisodeTimestamps = serde_json::from_str(json).unwrap();
        assert!(ts.introduction.valid());
        assert_eq!(ts.introduction.end, 92.5);
        assert!(ts.credits.valid());
        assert_eq!(ts.credits.start, 1340.0);
        assert!(ts.recap.valid());
        assert!(ts.preview.valid());
        assert!(ts.commercial.valid());
    }

    #[test]
    fn timestamps_empty_object_all_invalid() {
        let ts: EpisodeTimestamps = serde_json::from_str(r#"{}"#).unwrap();
        assert!(!ts.introduction.valid());
        assert!(!ts.credits.valid());
        assert!(!ts.recap.valid());
        assert!(!ts.preview.valid());
        assert!(!ts.commercial.valid());
    }

    #[test]
    fn segment_valid_boundary() {
        let zero    = Segment { start: 0.0, end: 0.0 };
        let nonzero = Segment { start: 10.0, end: 92.5 };
        assert!(!zero.valid());
        assert!(nonzero.valid());
    }
}
