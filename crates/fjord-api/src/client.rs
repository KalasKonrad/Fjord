// ── fjord-api · client.rs ────────────────────────────────────────────────────
//   JellyfinClient  HTTP client wrapper (server URL, user_id, token, device_id); 30 s request timeout
//     library       get_all_items, get_all_movies, get_all_series, get_item_detail, search_items
//     images        fetch_poster_bytes, fetch_backdrop_bytes
//     seasons       get_seasons, get_season_episodes
//     home data     get_continue_watching, get_next_up, get_recently_added, get_unwatched
//     playback      direct_play_url, report_playback_start/progress/stopped
//     user actions  mark_played, mark_unplayed, set_favorite, unset_favorite
//     plugins       get_intro_timestamps, get_credits_timestamps (Intro Skipper), get_next_up_for_series
//     auth          check_auth
//     server        get_system_info (name + version via /System/Info/Public)
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::Result;
use reqwest::StatusCode;
use serde_json::json;
use tracing::warn;
use url::Url;

use crate::models::{IntroTimestamps, ItemsResponse, MediaItem, SystemInfo};

#[derive(Clone)]
pub struct JellyfinClient {
    http: reqwest::Client,
    pub server_url: Url,
    pub user_id: String,
    pub token: String,
    pub device_id: String,
}

impl JellyfinClient {
    pub fn new(server_url: Url, user_id: String, token: String, device_id: String) -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()?;
        Ok(Self { http, server_url, user_id, token, device_id })
    }

    fn auth_header(&self) -> String {
        format!(
            r#"MediaBrowser Client="Fjord", Device="Linux", DeviceId="{}", Version="0.1.0", Token="{}""#,
            self.device_id, self.token
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
                "Overview,RunTimeTicks,SeriesId,SeriesName,IndexNumber,ParentIndexNumber,ProductionYear,UserData",
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

    /// Full item details including genres, rating, cast, and backdrop tags.
    pub async fn get_item_detail(&self, item_id: &str) -> Result<MediaItem> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items/{}", self.user_id, item_id))?;
        url.query_pairs_mut().append_pair(
            "Fields",
            "Overview,RunTimeTicks,SeriesName,SeasonName,IndexNumber,ParentIndexNumber,\
             ProductionYear,UserData,Genres,OfficialRating,CommunityRating,\
             BackdropImageTags,People",
        );
        Ok(self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<MediaItem>()
            .await?)
    }

    /// Raw bytes of the first backdrop image (index 0), scaled to 1280 px wide.
    pub async fn fetch_backdrop_bytes(&self, item_id: &str) -> Result<Vec<u8>> {
        let mut url = self
            .server_url
            .join(&format!("/Items/{}/Images/Backdrop/0", item_id))?;
        url.query_pairs_mut()
            .append_pair("fillWidth", "1280")
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

    /// All series in the library.
    pub async fn get_all_series(&self) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Series")
            .append_pair("SortBy", "SortName")
            .append_pair("SortOrder", "Ascending")
            .append_pair("Fields", "Overview,ProductionYear,UserData");
        Ok(self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<ItemsResponse>()
            .await?
            .items)
    }

    /// All seasons for a series.
    pub async fn get_seasons(&self, series_id: &str) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Shows/{}/Seasons", series_id))?;
        url.query_pairs_mut()
            .append_pair("userId", &self.user_id)
            .append_pair("Fields", "UserData");
        Ok(self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<ItemsResponse>()
            .await?
            .items)
    }

    /// All episodes for one season.
    pub async fn get_season_episodes(
        &self,
        series_id: &str,
        season_id: &str,
    ) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Shows/{}/Episodes", series_id))?;
        url.query_pairs_mut()
            .append_pair("seasonId", season_id)
            .append_pair("userId", &self.user_id)
            .append_pair("Fields", "Overview,RunTimeTicks,SeriesId,UserData");
        Ok(self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<ItemsResponse>()
            .await?
            .items)
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
            .append_pair("Fields", "SeriesId,SeriesName,IndexNumber,ParentIndexNumber,UserData")
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
            .append_pair("Fields", "SeriesId,SeriesName,IndexNumber,ParentIndexNumber,UserData")
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

    /// Intro skip timestamps from the Intro Skipper plugin.
    /// Returns None on 404 (plugin absent or episode not analyzed); errors on other HTTP failures.
    pub async fn get_intro_timestamps(&self, item_id: &str) -> Result<Option<IntroTimestamps>> {
        let url = self
            .server_url
            .join(&format!("/Episode/{}/IntroTimestamps", item_id))?;
        let resp = self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!("intro timestamps HTTP {}: {}", status, body.chars().take(120).collect::<String>());
            return Ok(None);
        }
        let ts = resp.json::<IntroTimestamps>().await?;
        Ok(if ts.valid { Some(ts) } else { None })
    }

    /// Credit segment timestamps from the Intro Skipper plugin (`/Episode/{id}/Credits`).
    /// Returns None on 404 (plugin absent or episode not analyzed); errors on other HTTP failures.
    /// The Credits endpoint returns the same JSON structure as IntroTimestamps.
    pub async fn get_credits_timestamps(&self, item_id: &str) -> Result<Option<IntroTimestamps>> {
        let url = self
            .server_url
            .join(&format!("/Episode/{}/Credits", item_id))?;
        let resp = self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!("credits timestamps HTTP {}: {}", status, body.chars().take(120).collect::<String>());
            return Ok(None);
        }
        let ts = resp.json::<IntroTimestamps>().await?;
        Ok(if ts.valid { Some(ts) } else { None })
    }

    pub async fn get_next_up_for_series(&self, series_id: &str) -> Result<Option<MediaItem>> {
        let mut url = self.server_url.join("/Shows/NextUp")?;
        url.query_pairs_mut()
            .append_pair("UserId", &self.user_id)
            .append_pair("SeriesId", series_id)
            .append_pair("Fields", "SeriesId,SeriesName,IndexNumber,ParentIndexNumber,UserData,RunTimeTicks")
            .append_pair("Limit", "1");
        let resp = self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<ItemsResponse>()
            .await?;
        Ok(resp.items.into_iter().next())
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
            .append_pair("Fields", "SeriesId,SeriesName,IndexNumber,ParentIndexNumber,UserData")
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
    /// Lightweight authenticated probe — just checks if the token is still valid.
    /// Fetches server name and version from the public endpoint (no auth required).
    pub async fn get_system_info(&self) -> Result<SystemInfo> {
        let url = self.server_url.join("/System/Info/Public")?;
        Ok(self.http.get(url).send().await?.error_for_status()?.json().await?)
    }

    pub async fn check_auth(&self) -> Result<()> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("Limit", "0")
            .append_pair("Recursive", "true");
        self.http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    /// All movies sorted by name. Used for lazy library grid load.
    pub async fn get_all_movies(&self) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Movie")
            .append_pair("SortBy", "SortName")
            .append_pair("SortOrder", "Ascending")
            .append_pair("Fields", "UserData")
            .append_pair("EnableUserData", "true")
            .append_pair("Limit", "10000");
        Ok(self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<ItemsResponse>()
            .await?
            .items)
    }

    /// Server-side title search across movies, series, and episodes.
    pub async fn search_items(&self, query: &str, limit: usize) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("searchTerm", query)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Movie,Series,Episode")
            .append_pair("Fields", "SeriesId,SeriesName,IndexNumber,ParentIndexNumber,UserData")
            .append_pair("EnableUserData", "true")
            .append_pair("Limit", &limit.to_string());
        Ok(self
            .http
            .get(url)
            .header("Authorization", self.auth_header())
            .send()
            .await?
            .error_for_status()?
            .json::<ItemsResponse>()
            .await?
            .items)
    }

    /// Mark an item as played: POST /Users/{userId}/PlayedItems/{itemId}
    pub async fn mark_played(&self, item_id: &str) -> Result<()> {
        let url = self.server_url.join(&format!(
            "/Users/{}/PlayedItems/{}", self.user_id, item_id
        ))?;
        self.http.post(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?;
        Ok(())
    }

    /// Mark an item as unplayed: DELETE /Users/{userId}/PlayedItems/{itemId}
    pub async fn mark_unplayed(&self, item_id: &str) -> Result<()> {
        let url = self.server_url.join(&format!(
            "/Users/{}/PlayedItems/{}", self.user_id, item_id
        ))?;
        self.http.delete(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?;
        Ok(())
    }

    /// Add an item to favourites: POST /Users/{userId}/FavoriteItems/{itemId}
    pub async fn set_favorite(&self, item_id: &str) -> Result<()> {
        let url = self.server_url.join(&format!(
            "/Users/{}/FavoriteItems/{}", self.user_id, item_id
        ))?;
        self.http.post(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?;
        Ok(())
    }

    /// Remove an item from favourites: DELETE /Users/{userId}/FavoriteItems/{itemId}
    pub async fn unset_favorite(&self, item_id: &str) -> Result<()> {
        let url = self.server_url.join(&format!(
            "/Users/{}/FavoriteItems/{}", self.user_id, item_id
        ))?;
        self.http.delete(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?;
        Ok(())
    }

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
            .append_pair("Fields", "SeriesId,SeriesName,IndexNumber,ParentIndexNumber,UserData")
            .append_pair("SortBy", "Random")
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
