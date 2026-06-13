use serde::Deserialize;

/// Response from the Intro Skipper plugin: `/Episode/{id}/IntroTimestamps`
#[derive(Debug, Clone, Deserialize)]
pub struct IntroTimestamps {
    #[serde(rename = "Valid")]
    pub valid: bool,
    #[serde(rename = "IntroStart")]
    pub intro_start: f64,
    #[serde(rename = "IntroEnd")]
    pub intro_end: f64,
    #[serde(rename = "ShowSkipPromptAt")]
    pub show_skip_prompt_at: f64,
    #[serde(rename = "HideSkipPromptAt")]
    pub hide_skip_prompt_at: f64,
}
