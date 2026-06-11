use anyhow::Result;
use serde_json::json;
use url::Url;

use crate::models::{ItemsResponse, MediaItem};

#[derive(Clone)]
pub struct JellyfinClient {
    http: reqwest::Client,
    pub server_url: Url,
    pub user_id: String,
    pub token: String,
}

impl JellyfinClient {
    pub fn new(server_url: Url, user_id: String, token: String) -> Self {
        Self {
            http: reqwest::Client::new(),
            server_url,
            user_id,
            token,
        }
    }

    fn auth_header(&self) -> String {
        format!(
            r#"MediaBrowser Client="Fjord", Device="Linux", DeviceId="fjord-00000000-0000-0000-0000-000000000001", Version="0.1.0", Token="{}""#,
            self.token
        )
    }

    /// Returns all movies and episodes across all libraries, sorted by name.
    pub async fn get_all_items(&self) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Movie,Episode")
            .append_pair("SortBy", "SortName")
            .append_pair("SortOrder", "Ascending")
            .append_pair(
                "Fields",
                "Overview,RunTimeTicks,SeriesName,IndexNumber,ParentIndexNumber",
            )
            .append_pair("Limit", "2000");

        let resp = self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<ItemsResponse>()
            .await?;

        Ok(resp.items)
    }

    /// Direct-play URL: mpv can open this as-is.
    pub fn direct_play_url(&self, item_id: &str) -> String {
        format!(
            "{}/Videos/{}/stream?static=true&api_key={}",
            self.server_url.as_str().trim_end_matches('/'),
            item_id,
            self.token
        )
    }

    pub async fn report_playback_start(&self, item_id: &str) -> Result<()> {
        let url = self.server_url.join("/Sessions/Playing")?;
        self.http
            .post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "ItemId": item_id, "CanSeek": true, "IsPaused": false }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn report_playback_progress(
        &self,
        item_id: &str,
        position_ticks: i64,
    ) -> Result<()> {
        let url = self.server_url.join("/Sessions/Playing/Progress")?;
        self.http
            .post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "ItemId": item_id, "PositionTicks": position_ticks, "IsPaused": false }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn report_playback_stopped(&self, item_id: &str, position_ticks: i64) -> Result<()> {
        let url = self.server_url.join("/Sessions/Playing/Stopped")?;
        self.http
            .post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "ItemId": item_id, "PositionTicks": position_ticks }))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }
}
