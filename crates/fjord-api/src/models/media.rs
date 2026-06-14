// ── fjord-api · models/media.rs ──────────────────────────────────────────────
//   ItemsResponse   envelope for GET /Users/{id}/Items responses
//   UserData        played status, resume position, unplayed count, is_favorite
//   MediaItem       full item: id, name, type, series info, user data, runtime
// ─────────────────────────────────────────────────────────────────────────────
use serde::{Deserialize, Serialize};

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
    #[serde(rename = "IsFavorite", default)]
    pub is_favorite: bool,
    #[serde(rename = "UnplayedItemCount", default)]
    pub unplayed_item_count: i32,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PersonInfo {
    #[serde(rename = "Id", default)]
    pub id: String,
    #[serde(rename = "Name", default)]
    pub name: String,
    #[serde(rename = "Role", default)]
    pub role: String,
    #[serde(rename = "Type", default)]
    pub person_type: String,
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
    #[serde(rename = "SeriesId", default)]
    pub series_id: Option<String>,
    #[serde(rename = "SeriesName")]
    pub series_name: Option<String>,
    #[serde(rename = "SeasonName")]
    pub season_name: Option<String>,
    #[serde(rename = "IndexNumber")]
    pub index_number: Option<u32>,
    #[serde(rename = "ParentIndexNumber")]
    pub parent_index_number: Option<u32>,
    #[serde(rename = "UserData", default)]
    pub user_data: UserData,
    // Detail fields — only populated via get_item_detail()
    #[serde(rename = "Genres", default)]
    pub genres: Vec<String>,
    #[serde(rename = "OfficialRating", default)]
    pub official_rating: Option<String>,
    #[serde(rename = "CommunityRating", default)]
    pub community_rating: Option<f32>,
    #[serde(rename = "BackdropImageTags", default)]
    pub backdrop_image_tags: Vec<String>,
    #[serde(rename = "People", default)]
    pub people: Vec<PersonInfo>,
}

impl MediaItem {
    pub fn resume_pct(&self) -> f32 {
        match self.run_time_ticks {
            Some(total) if total > 0 => {
                let pos = self.user_data.playback_position_ticks;
                (pos as f32 / total as f32).clamp(0.0, 1.0)
            }
            _ => 0.0,
        }
    }

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

    pub fn runtime_string(&self) -> Option<String> {
        let ticks = self.run_time_ticks?;
        let total_mins = (ticks / 600_000_000) as u32;
        let h = total_mins / 60;
        let m = total_mins % 60;
        Some(if h > 0 { format!("{}h {}m", h, m) } else { format!("{}m", m) })
    }
}
