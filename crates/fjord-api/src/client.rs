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
    /// Fetches the first page to get total count, then remaining pages in parallel.
    pub async fn get_all_items(
        &self,
        on_progress: impl Fn(usize) + Send + Clone + 'static,
    ) -> Result<Vec<MediaItem>> {
        const PAGE: usize = 1000;

        let first = self.get_items_response(0, PAGE).await?;
        let total = first.total_record_count as usize;
        let mut all = first.items;
        on_progress(all.len());

        if all.len() >= total {
            return Ok(all);
        }

        let loaded = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(all.len()));
        let sem    = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let mut set = tokio::task::JoinSet::new();
        let mut start = PAGE;
        while start < total {
            let this      = self.clone();
            let on_p      = on_progress.clone();
            let loaded    = std::sync::Arc::clone(&loaded);
            let sem       = std::sync::Arc::clone(&sem);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let page = this.get_items_page(start, PAGE).await?;
                let n = loaded.fetch_add(page.len(), std::sync::atomic::Ordering::Relaxed) + page.len();
                on_p(n);
                Ok::<Vec<MediaItem>, anyhow::Error>(page)
            });
            start += PAGE;
        }

        while let Some(res) = set.join_next().await {
            all.extend(res??);
        }

        all.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(all)
    }

    async fn get_items_response(&self, start_index: usize, limit: usize) -> Result<ItemsResponse> {
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
                "Overview,RunTimeTicks,SeriesName,IndexNumber,ParentIndexNumber,ProductionYear",
            )
            .append_pair("StartIndex", &start_index.to_string())
            .append_pair("Limit", &limit.to_string());

        Ok(self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<ItemsResponse>()
            .await?)
    }

    async fn get_items_page(&self, start_index: usize, limit: usize) -> Result<Vec<MediaItem>> {
        Ok(self.get_items_response(start_index, limit).await?.items)
    }

    /// Download raw poster image bytes for a single item.
    pub async fn fetch_poster_bytes(&self, item_id: &str) -> Result<Vec<u8>> {
        let mut url = self
            .server_url
            .join(&format!("/Items/{}/Images/Primary", item_id))?;
        url.query_pairs_mut()
            .append_pair("fillWidth", "280")
            .append_pair("quality", "80");

        let bytes = self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;

        Ok(bytes.to_vec())
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

    /// Items currently in progress (IsResumable) — for "Continue Watching" rows.
    pub async fn get_continue_watching(&self) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("Filters", "IsResumable")
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Movie,Episode")
            .append_pair("SortBy", "DatePlayed")
            .append_pair("SortOrder", "Descending")
            .append_pair("Fields", "SeriesName,IndexNumber,ParentIndexNumber")
            .append_pair("Limit", "15");
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

    /// Next unwatched episode in each series — for "Next Up" rows.
    pub async fn get_next_up(&self) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join("/Shows/NextUp")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("Fields", "SeriesName,IndexNumber,ParentIndexNumber")
            .append_pair("Limit", "15");
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

    /// Most recently added items.  Pass `"Movie"` or `"Episode"` to narrow by type.
    pub async fn get_recently_added(&self, item_type: Option<&str>) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items", self.user_id))?;
        let types = item_type.unwrap_or("Movie,Episode");
        url.query_pairs_mut()
            .append_pair("SortBy", "DateCreated")
            .append_pair("SortOrder", "Descending")
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", types)
            .append_pair("Fields", "SeriesName,IndexNumber,ParentIndexNumber")
            .append_pair("Limit", "15");
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

    /// Items never started (IsUnplayed, not resumable).  Pass `"Movie"` or `"Episode"` to filter.
    pub async fn get_unwatched(&self, item_type: Option<&str>) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items", self.user_id))?;
        let types = item_type.unwrap_or("Movie,Episode");
        url.query_pairs_mut()
            .append_pair("Filters", "IsUnplayed")
            .append_pair("IsPlayed", "false")
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", types)
            .append_pair("Fields", "SeriesName,IndexNumber,ParentIndexNumber")
            .append_pair("SortBy", "DateCreated")
            .append_pair("SortOrder", "Descending")
            .append_pair("Limit", "15");
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
}
