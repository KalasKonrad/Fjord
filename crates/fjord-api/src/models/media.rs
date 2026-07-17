// ── fjord-api · models/media.rs ──────────────────────────────────────────────
//   ItemsResponse   envelope for GET /Users/{id}/Items responses
//   UserData        played status, resume position, unplayed count, is_favorite
//   StudioInfo      studio name (from Studios array in item detail)
//   MediaItem       full item: id, name, type, series info, user data, runtime, image_tags, status/end_date,
//                   date_created (WS delta-sync Recently Added ordering), season_id (episode → season routing),
//                   provider_ids ("Tmdb"/"Imdb"/"Tvdb" -> value, movies/series only — Discover TMDB-match);
//                   helpers: primary_image_tag(), card_title(), card_subtitle() (Jellyfin-style card rows);
//                   detail fields: genres, rating, backdrop, people, taglines, studios, recursive_item_count
//                   music fields: album_artist, album (track → parent album name, index_number = track #)
//                   playlist fields: media_type, playlist_item_id (entry id for removal), child_count
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
pub struct StudioInfo {
    #[serde(rename = "Name", default)]
    pub name: String,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct PersonInfo {
    #[serde(rename = "Id", default)]
    pub id: String,
    #[serde(rename = "PrimaryImageTag", default)]
    pub primary_image_tag: Option<String>,
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
    // External ids ("Tmdb"/"Imdb"/"Tvdb" -> value) — used to match a Seerr/
    // TMDB search result back to the corresponding local library item
    // (Discover screen: "already in your library" -> open the real item
    // instead of the Seerr request-detail page). Only populated on movies/
    // series (get_all_movies/get_all_series request the ProviderIds field);
    // Jellyfin has no server-side "find item by provider id" query, so this
    // is matched client-side against the already-cached library list.
    #[serde(rename = "ProviderIds", default)]
    pub provider_ids: std::collections::HashMap<String, String>,
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
    #[serde(rename = "Taglines", default)]
    pub taglines: Vec<String>,
    #[serde(rename = "Studios", default)]
    pub studios: Vec<StudioInfo>,
    #[serde(rename = "RecursiveItemCount", default)]
    pub recursive_item_count: Option<u32>,
    // Series airing info (default-included when set): "Continuing" / "Ended".
    #[serde(rename = "Status", default)]
    pub status: Option<String>,
    #[serde(rename = "EndDate", default)]
    pub end_date: Option<String>,
    // Image tags — hash per image type ("Primary", …); changes when the
    // artwork is replaced server-side. Included by default in item responses.
    #[serde(rename = "ImageTags", default)]
    pub image_tags: std::collections::HashMap<String, String>,
    // Music fields — only present on MusicAlbum / Audio items
    #[serde(rename = "AlbumArtist", default)]
    pub album_artist: Option<String>,
    #[serde(rename = "Album", default)]
    pub album: Option<String>,
    #[serde(rename = "AlbumId", default)]
    pub album_id: Option<String>,
    // Playlist fields — MediaType distinguishes audio playlists ("Audio") from
    // video ones; PlaylistItemId identifies an entry inside a playlist (needed
    // for removal — one item can appear multiple times); ChildCount = # entries.
    #[serde(rename = "MediaType", default)]
    pub media_type: Option<String>,
    #[serde(rename = "PlaylistItemId", default)]
    pub playlist_item_id: Option<String>,
    #[serde(rename = "ChildCount", default)]
    pub child_count: Option<u32>,
    // Library view type ("movies" / "tvshows" / "music" …) — only present on
    // /Users/{id}/Views entries; used to resolve the music library id.
    #[serde(rename = "CollectionType", default)]
    pub collection_type: Option<String>,
    // ISO 8601 UTC timestamp ("...Z" suffixed) — Jellyfin always emits this
    // format, so plain string comparison sorts chronologically with no date-
    // parsing dependency needed. Used to insert WS-added items into
    // Recently Added rows at the correct position instead of a full re-fetch.
    #[serde(rename = "DateCreated", default)]
    pub date_created: Option<String>,
    // Parent season id — only present on Episode items. Used to route a
    // WS-added/updated episode into the right series_episode_cache entry.
    #[serde(rename = "SeasonId", default)]
    pub season_id: Option<String>,
}

impl MediaItem {
    /// Server-side hash of the primary image; changes when the artwork changes.
    /// Used to revalidate the on-disk poster cache.
    pub fn primary_image_tag(&self) -> Option<&str> {
        self.image_tags.get("Primary").map(|s| s.as_str())
    }

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
            "Audio" => self.name.clone(),
            "MusicAlbum" => match self.album_artist.as_deref() {
                Some(artist) => format!("{} — {}", artist, self.name),
                None => self.name.clone(),
            },
            _ => match self.production_year {
                Some(y) => format!("{} ({})", self.name, y),
                None => self.name.clone(),
            },
        }
    }

    /// First text row on a media card (Jellyfin style): series name for
    /// episodes, item name otherwise.
    pub fn card_title(&self) -> String {
        match self.item_type.as_str() {
            "Episode" => self.series_name.clone().unwrap_or_else(|| self.name.clone()),
            _         => self.name.clone(),
        }
    }

    /// Second text row on a media card (Jellyfin style):
    ///   Episode     → "S1:E3 - Spring Broken"
    ///   Series      → "2019 - Present" / "2019 - 2023" / "2019"
    ///   MusicAlbum  → album artist (year fallback)
    ///   MusicArtist → ""
    ///   otherwise   → year
    pub fn card_subtitle(&self) -> String {
        match self.item_type.as_str() {
            "Episode" => {
                let s = self.parent_index_number.unwrap_or(0);
                let e = self.index_number.unwrap_or(0);
                if s > 0 || e > 0 { format!("S{}:E{} - {}", s, e, self.name) } else { self.name.clone() }
            }
            "Series" => {
                let Some(start) = self.production_year else { return String::new() };
                let end_year = self.end_date.as_deref()
                    .and_then(|d| d.get(..4))
                    .and_then(|y| y.parse::<u32>().ok());
                match self.status.as_deref() {
                    Some("Continuing") => format!("{} - Present", start),
                    _ => match end_year {
                        Some(end) if end > start => format!("{} - {}", start, end),
                        _ => start.to_string(),
                    },
                }
            }
            "MusicAlbum" => self.album_artist.clone().unwrap_or_else(||
                self.production_year.map(|y| y.to_string()).unwrap_or_default()),
            "MusicArtist" => String::new(),
            "Playlist" => match self.child_count {
                Some(1) => "1 track".to_string(),
                Some(n) => format!("{} tracks", n),
                None    => String::new(),
            },
            _ => self.production_year.map(|y| y.to_string()).unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn media_item_minimal() {
        let json = r#"{"Id":"abc","Name":"Test Movie","Type":"Movie"}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.id, "abc");
        assert_eq!(item.name, "Test Movie");
        assert_eq!(item.item_type, "Movie");
        assert!(item.overview.is_none());
        assert!(!item.user_data.played);
        assert!(item.genres.is_empty());
        assert_eq!(item.resume_pct(), 0.0);
    }

    #[test]
    fn media_item_episode_display_name_and_resume() {
        let json = r#"{
            "Id":"ep1","Name":"Pilot","Type":"Episode",
            "IndexNumber":1,"ParentIndexNumber":1,
            "SeriesId":"s1","SeriesName":"My Show",
            "RunTimeTicks":27000000000,
            "UserData":{"PlaybackPositionTicks":13500000000,"Played":false,"IsFavorite":false,"UnplayedItemCount":0}
        }"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.display_name(), "My Show S01E01 — Pilot");
        let pct = item.resume_pct();
        assert!((pct - 0.5).abs() < 0.001, "resume_pct={pct}");
        assert_eq!(item.resume_position_secs(), Some(1350.0));
    }

    #[test]
    fn media_item_detail_fields() {
        let json = r#"{
            "Id":"m1","Name":"A Film","Type":"Movie","ProductionYear":2020,
            "Genres":["Drama","Thriller"],
            "OfficialRating":"R",
            "CommunityRating":7.8,
            "BackdropImageTags":["tag1"],
            "People":[{"Id":"p1","Name":"Alice","Role":"","Type":"Director"}],
            "Taglines":["Life is short"],
            "Studios":[{"Name":"Studio A"}],
            "UserData":{}
        }"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.genres, vec!["Drama", "Thriller"]);
        assert_eq!(item.official_rating.as_deref(), Some("R"));
        assert!((item.community_rating.unwrap() - 7.8).abs() < 0.001);
        assert_eq!(item.people[0].person_type, "Director");
        assert_eq!(item.taglines[0], "Life is short");
        assert_eq!(item.studios[0].name, "Studio A");
        assert_eq!(item.display_name(), "A Film (2020)");
    }

    #[test]
    fn user_data_all_defaults() {
        let json = r#"{"Id":"x","Name":"X","Type":"Movie","UserData":{}}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.user_data.playback_position_ticks, 0);
        assert!(!item.user_data.played);
        assert!(!item.user_data.is_favorite);
        assert_eq!(item.user_data.unplayed_item_count, 0);
    }

    #[test]
    fn runtime_string_hours_and_minutes() {
        let json = r#"{"Id":"","Name":"","Type":"Movie","RunTimeTicks":54000000000}"#;
        let item: MediaItem = serde_json::from_str(json).unwrap();
        assert_eq!(item.runtime_string(), Some("1h 30m".to_string()));
    }
}
