// ── fjord-api · models/intro.rs ──────────────────────────────────────────────
//   IntroTimestamps  intro segment bounds from the Intro Skipper plugin
//                   Valid defaults to true; numeric fields default to 0.0 if absent
// ─────────────────────────────────────────────────────────────────────────────
use serde::Deserialize;

fn default_true() -> bool { true }

/// Response from the Intro Skipper plugin: `/Episode/{id}/IntroTimestamps`
#[derive(Debug, Clone, Deserialize)]
pub struct IntroTimestamps {
    #[serde(rename = "Valid", default = "default_true")]
    pub valid: bool,
    #[serde(rename = "IntroStart", default)]
    pub intro_start: f64,
    #[serde(rename = "IntroEnd", default)]
    pub intro_end: f64,
    #[serde(rename = "ShowSkipPromptAt", default)]
    pub show_skip_prompt_at: f64,
    #[serde(rename = "HideSkipPromptAt", default)]
    pub hide_skip_prompt_at: f64,
}
