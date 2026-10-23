// ── fjord-api · models/intro.rs ──────────────────────────────────────────────
//   Segment           a skippable segment (start/end in seconds); valid when end > 0
//   EpisodeTimestamps an episode's skippable segments (intro, credits, recap, preview,
//                     commercial) — Intro Skipper's GET /Episode/{id}/Timestamps shape;
//                     from_media_segments builds it from Jellyfin's media segments
//   MediaSegment / MediaSegmentsResponse  GET /MediaSegments/{id} (Jellyfin 10.10+)
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
    pub fn valid(&self) -> bool {
        self.end > 0.0
    }
}

/// Response from `GET /Episode/{id}/Timestamps` (Intro Skipper v2+ plugin), also built from
/// Jellyfin's media segments (`from_media_segments`).
#[derive(Debug, Clone, Deserialize, Default)]
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

/// One entry of `GET /MediaSegments/{itemId}` (Jellyfin 10.10+; Intro Skipper and other providers
/// fill it). `Type`: Unknown, Commercial, Preview, Recap, Outro, Intro; ticks are 100 ns.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaSegment {
    #[serde(rename = "Type", default)]
    pub kind: String,
    #[serde(rename = "StartTicks", default)]
    pub start_ticks: i64,
    #[serde(rename = "EndTicks", default)]
    pub end_ticks: i64,
}

/// `GET /MediaSegments/{itemId}` — a QueryResult.
#[derive(Debug, Clone, Deserialize)]
pub struct MediaSegmentsResponse {
    #[serde(rename = "Items", default)]
    pub items: Vec<MediaSegment>,
}

impl EpisodeTimestamps {
    /// The media segments as Intro Skipper's timestamps — Intro, Outro (= credits), Recap,
    /// Preview, Commercial; the first of each kind, Unknown ignored. None when none is usable.
    pub fn from_media_segments(segments: &[MediaSegment]) -> Option<Self> {
        let mut ts = Self::default();
        for m in segments {
            let slot = match m.kind.as_str() {
                "Intro" => &mut ts.introduction,
                "Outro" => &mut ts.credits,
                "Recap" => &mut ts.recap,
                "Preview" => &mut ts.preview,
                "Commercial" => &mut ts.commercial,
                _ => continue,
            };
            if !slot.valid() {
                *slot = Segment {
                    start: m.start_ticks as f64 / 10_000_000.0,
                    end: m.end_ticks as f64 / 10_000_000.0,
                };
            }
        }
        let any = [
            &ts.introduction,
            &ts.credits,
            &ts.recap,
            &ts.preview,
            &ts.commercial,
        ]
        .iter()
        .any(|s| s.valid());
        any.then_some(ts)
    }
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

    /// The shape Jellyfin 12.2 returns with Intro Skipper 12.0.5 (verified live 2026-10-23; ids
    /// replaced).
    #[test]
    fn media_segments_map_to_the_timestamps() {
        let json = r#"{"Items":[
            {"Id":"a","ItemId":"b","Type":"Intro","StartTicks":682357067,"EndTicks":1289618292},
            {"Id":"c","ItemId":"b","Type":"Outro","StartTicks":29748878751,"EndTicks":30105600000},
            {"Id":"d","ItemId":"b","Type":"Unknown","StartTicks":0,"EndTicks":50000000}
        ],"TotalRecordCount":3,"StartIndex":0}"#;
        let r: MediaSegmentsResponse = serde_json::from_str(json).unwrap();
        let ts = EpisodeTimestamps::from_media_segments(&r.items).unwrap();
        assert!((ts.introduction.start - 68.2357067).abs() < 1e-6);
        assert!((ts.introduction.end - 128.9618292).abs() < 1e-6);
        assert!((ts.credits.start - 2974.8878751).abs() < 1e-6);
        assert!(!ts.recap.valid() && !ts.preview.valid() && !ts.commercial.valid());
        // Nothing usable (none, or only Unknown) → None, so the caller tries the plugin's endpoint.
        assert!(EpisodeTimestamps::from_media_segments(&[]).is_none());
        assert!(EpisodeTimestamps::from_media_segments(&r.items[2..]).is_none());
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
        let zero = Segment {
            start: 0.0,
            end: 0.0,
        };
        let nonzero = Segment {
            start: 10.0,
            end: 92.5,
        };
        assert!(!zero.valid());
        assert!(nonzero.valid());
    }
}
