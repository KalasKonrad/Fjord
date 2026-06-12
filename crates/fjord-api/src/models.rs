use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    #[serde(rename = "AccessToken")]
    pub access_token: String,
    #[serde(rename = "User")]
    pub user: UserDto,
}

#[derive(Debug, Deserialize)]
pub struct UserDto {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct ItemsResponse {
    #[serde(rename = "Items")]
    pub items: Vec<MediaItem>,
    #[serde(rename = "TotalRecordCount")]
    pub total_record_count: u32,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct UserData {
    #[serde(rename = "PlaybackPositionTicks", default)]
    pub playback_position_ticks: i64,
    #[serde(rename = "Played", default)]
    pub played: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct MediaItem {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Type")]
    pub item_type: String,
    #[serde(rename = "Overview")]
    pub overview: Option<String>,
    #[serde(rename = "ProductionYear")]
    pub production_year: Option<u32>,
    #[serde(rename = "RunTimeTicks")]
    pub run_time_ticks: Option<i64>,
    #[serde(rename = "SeriesName")]
    pub series_name: Option<String>,
    #[serde(rename = "IndexNumber")]
    pub index_number: Option<u32>,
    #[serde(rename = "ParentIndexNumber")]
    pub parent_index_number: Option<u32>,
    #[serde(rename = "UserData", default)]
    pub user_data: UserData,
}

impl MediaItem {
    /// Returns the resume position in seconds, or None if the item hasn't been started.
    pub fn resume_position_secs(&self) -> Option<f64> {
        let ticks = self.user_data.playback_position_ticks;
        if ticks > 0 { Some(ticks as f64 / 10_000_000.0) } else { None }
    }

    pub fn display_name(&self) -> String {
        match self.item_type.as_str() {
            "Episode" => {
                let s = self.parent_index_number.unwrap_or(0);
                let e = self.index_number.unwrap_or(0);
                let series = self.series_name.as_deref().unwrap_or("?");
                format!("{} S{:02}E{:02} — {}", series, s, e, self.name)
            }
            _ => match self.production_year {
                Some(y) => format!("{} ({})", self.name, y),
                None => self.name.clone(),
            },
        }
    }
}
