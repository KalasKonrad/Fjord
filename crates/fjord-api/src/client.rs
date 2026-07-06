// ── fjord-api · client.rs ────────────────────────────────────────────────────
//   JellyfinClient  HTTP client wrapper (server URL, user_id, token, device_id); 30 s request timeout
//     library       get_all_items, get_all_movies, get_all_series (all paginated), get_item_detail, search_items,
//                   get_similar_items, get_all_boxsets, get_boxset_items, get_person_filmography
//     images        fetch_poster_bytes, fetch_backdrop_bytes
//     seasons       get_seasons, get_season_episodes, get_series_episodes (all eps, airing order)
//     home data     get_continue_watching, get_next_up, get_recently_added, get_unwatched,
//                   get_recently_added_collections, get_unwatched_collections
//     music         get_recently_added_albums, get_recently_played_albums, get_album_tracks,
//                   get_album_artists, get_artist_albums, get_all_albums, get_lyrics
//     favorites     get_favorites(item_types) — IsFavorite filter for any item type(s)
//     playback      direct_play_url, report_playback_start/progress/stopped
//     user actions  mark_played, mark_unplayed, set_favorite, unset_favorite
//     plugins       get_episode_timestamps (Intro Skipper v2+: intro+credits in one call), get_next_up_for_series
//     auth          check_auth
//     server        get_system_info (name + version via /System/Info/Public)
//     websocket     ws_url() → ws[s]://host/socket?api_key=…&deviceId=…
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::Result;
use reqwest::StatusCode;
use serde_json::json;
use tracing::{debug, warn};
use url::Url;

use crate::models::{EpisodeTimestamps, ItemsResponse, MediaItem, SystemInfo};

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

    /// Paginated fetch for a single item type with custom fields.
    /// Fetches the first page to get the total count, then remaining pages in parallel (4 concurrent).
    /// Guarantees all items are returned regardless of the server's MaxPageSize.
    async fn get_all_paged(&self, include_types: &str, fields: &str) -> Result<Vec<MediaItem>> {
        const PAGE: usize = 1000;

        let first = self.get_typed_page_response(0, PAGE, include_types, fields).await?;
        let total = first.total_record_count as usize;
        let mut all = first.items;

        if all.len() >= total {
            all.sort_by(|a, b| a.name.cmp(&b.name));
            return Ok(all);
        }

        let sem = std::sync::Arc::new(tokio::sync::Semaphore::new(4));
        let mut set = tokio::task::JoinSet::new();
        let mut start = PAGE;
        while start < total {
            let this  = self.clone();
            let it    = include_types.to_string();
            let fi    = fields.to_string();
            let sem   = std::sync::Arc::clone(&sem);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                this.get_typed_page(start, PAGE, &it, &fi).await
            });
            start += PAGE;
        }
        while let Some(res) = set.join_next().await { all.extend(res??); }

        all.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(all)
    }

    async fn get_typed_page_response(&self, start: usize, limit: usize, include_types: &str, fields: &str) -> Result<ItemsResponse> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("Recursive",        "true")
            .append_pair("IncludeItemTypes", include_types)
            .append_pair("SortBy",           "SortName")
            .append_pair("SortOrder",        "Ascending")
            .append_pair("Fields",           fields)
            .append_pair("StartIndex",       &start.to_string())
            .append_pair("Limit",            &limit.to_string());
        Ok(self.http.get(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?
            .json::<ItemsResponse>().await?)
    }

    async fn get_typed_page(&self, start: usize, limit: usize, include_types: &str, fields: &str) -> Result<Vec<MediaItem>> {
        Ok(self.get_typed_page_response(start, limit, include_types, fields).await?.items)
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
             BackdropImageTags,People,Taglines,Studios,RecursiveItemCount",
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
        self.get_all_paged("Series", "Overview,ProductionYear,UserData").await
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

    /// All episodes of a series in airing order (no season filter). Used to
    /// resolve "the episode after X" without server-side state changes.
    pub async fn get_series_episodes(&self, series_id: &str) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Shows/{}/Episodes", series_id))?;
        url.query_pairs_mut()
            .append_pair("userId", &self.user_id)
            .append_pair("Fields", "SeriesId,SeriesName,IndexNumber,ParentIndexNumber,UserData,RunTimeTicks");
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
    /// Combined intro + credits timestamps from the Intro Skipper v2+ plugin.
    /// Single call to `GET /Episode/{id}/Timestamps`; returns None on 404.
    pub async fn get_episode_timestamps(&self, item_id: &str) -> Result<Option<EpisodeTimestamps>> {
        let url = self
            .server_url
            .join(&format!("/Episode/{}/Timestamps", item_id))?;
        debug!("episode timestamps GET {}", url);
        let resp = self
            .http
            .get(url.clone())
            .header("Authorization", self.auth_header())
            .send()
            .await?;
        let status = resp.status();
        if status == StatusCode::NOT_FOUND {
            debug!("episode timestamps 404: {}", url);
            return Ok(None);
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            warn!("episode timestamps HTTP {}: {}", status, body.chars().take(120).collect::<String>());
            return Ok(None);
        }
        Ok(Some(resp.json::<EpisodeTimestamps>().await?))
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
            .append_pair("Filters", "IsUnplayed")
            .append_pair("IsPlayed", "false")
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
        self.get_all_paged("Movie", "UserData").await
    }

    /// 15 most recently added BoxSets (no IsUnplayed filter — shows newly added regardless of status).
    pub async fn get_recently_added_collections(&self) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("IncludeItemTypes", "BoxSet")
            .append_pair("Recursive",        "true")
            .append_pair("Fields",           "ProductionYear,UserData")
            .append_pair("SortBy",           "DateCreated")
            .append_pair("SortOrder",        "Descending")
            .append_pair("Limit",            "15");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// Up to 15 unwatched BoxSets in random order.
    pub async fn get_unwatched_collections(&self) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("IncludeItemTypes", "BoxSet")
            .append_pair("Recursive",        "true")
            .append_pair("Fields",           "ProductionYear,UserData")
            .append_pair("Filters",          "IsUnplayed")
            .append_pair("SortBy",           "Random")
            .append_pair("Limit",            "15");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// All BoxSets in the library (Id + Name only — for building the collection membership map).
    pub async fn get_all_boxsets(&self) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("IncludeItemTypes", "BoxSet")
            .append_pair("Recursive", "true")
            .append_pair("Fields", "Id,Name,ProductionYear,UserData")
            .append_pair("SortBy", "SortName");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// All items in a BoxSet with metadata for the collection SectionRow.
    pub async fn get_boxset_items(&self, boxset_id: &str) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("ParentId", boxset_id)
            .append_pair("Fields", "ProductionYear,UserData")
            .append_pair("SortBy", "ProductionYear")
            .append_pair("SortOrder", "Ascending");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// Items similar to the given item (same type). Limit 12, includes production year + user data.
    pub async fn get_similar_items(&self, item_id: &str) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Items/{}/Similar", item_id))?;
        url.query_pairs_mut()
            .append_pair("userId", &self.user_id)
            .append_pair("Limit", "12")
            .append_pair("Fields", "ProductionYear,UserData");
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

    /// Movies and series a person appears in, newest first.
    pub async fn get_person_filmography(&self, person_id: &str) -> Result<Vec<MediaItem>> {
        let mut url = self
            .server_url
            .join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("PersonIds", person_id)
            .append_pair("Recursive", "true")
            .append_pair("IncludeItemTypes", "Movie,Series")
            .append_pair("Fields", "ProductionYear,UserData")
            .append_pair("SortBy", "PremiereDate")
            .append_pair("SortOrder", "Descending")
            .append_pair("Limit", "24");
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

    /// 15 most recently added MusicAlbum items.
    pub async fn get_recently_added_albums(&self) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("IncludeItemTypes", "MusicAlbum")
            .append_pair("Recursive",        "true")
            .append_pair("Fields",           "ProductionYear,UserData,AlbumArtist")
            .append_pair("SortBy",           "DateCreated")
            .append_pair("SortOrder",        "Descending")
            .append_pair("Limit",            "15");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// 15 most recently played MusicAlbum items (by DatePlayed descending).
    pub async fn get_recently_played_albums(&self) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("IncludeItemTypes", "MusicAlbum")
            .append_pair("Recursive",        "true")
            .append_pair("Fields",           "ProductionYear,UserData,AlbumArtist")
            .append_pair("Filters",          "IsPlayed")
            .append_pair("SortBy",           "DatePlayed")
            .append_pair("SortOrder",        "Descending")
            .append_pair("Limit",            "15");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// Items marked as favourite for the given item type(s) (e.g. "Movie", "Series", "MusicAlbum").
    /// Returns up to 30, sorted by name.
    pub async fn get_favorites(&self, item_types: &str) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("IncludeItemTypes", item_types)
            .append_pair("Recursive",        "true")
            .append_pair("Fields",           "ProductionYear,UserData,AlbumArtist,SeriesId,SeriesName,IndexNumber,ParentIndexNumber")
            .append_pair("Filters",          "IsFavorite")
            .append_pair("SortBy",           "SortName")
            .append_pair("SortOrder",        "Ascending")
            .append_pair("Limit",            "30");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// All album artists in the library, sorted by name.
    pub async fn get_album_artists(&self) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join("/Artists/AlbumArtists")?;
        url.query_pairs_mut()
            .append_pair("userId",    &self.user_id)
            .append_pair("Recursive", "true")
            .append_pair("Fields",    "Overview,UserData")
            .append_pair("SortBy",    "SortName")
            .append_pair("SortOrder", "Ascending");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// All MusicAlbum items for a given artist, sorted by year ascending.
    pub async fn get_artist_albums(&self, artist_id: &str) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("ArtistIds",        artist_id)
            .append_pair("IncludeItemTypes", "MusicAlbum")
            .append_pair("Recursive",        "true")
            .append_pair("Fields",           "ProductionYear,UserData,AlbumArtist")
            .append_pair("SortBy",           "ProductionYear")
            .append_pair("SortOrder",        "Ascending");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// All MusicAlbum items in the library, sorted by SortName ascending.
    pub async fn get_all_albums(&self) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("IncludeItemTypes", "MusicAlbum")
            .append_pair("Recursive",        "true")
            .append_pair("Fields",           "ProductionYear,UserData,AlbumArtist,Overview")
            .append_pair("SortBy",           "SortName")
            .append_pair("SortOrder",        "Ascending");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// All Audio tracks in an album, sorted by track number (IndexNumber).
    pub async fn get_album_tracks(&self, album_id: &str) -> Result<Vec<MediaItem>> {
        let mut url = self.server_url.join(&format!("/Users/{}/Items", self.user_id))?;
        url.query_pairs_mut()
            .append_pair("ParentId",         album_id)
            .append_pair("IncludeItemTypes", "Audio")
            .append_pair("Fields",           "RunTimeTicks,UserData,IndexNumber,AlbumArtist,Album")
            .append_pair("SortBy",           "IndexNumber")
            .append_pair("SortOrder",        "Ascending");
        Ok(self.http.get(url).header("Authorization", self.auth_header())
            .send().await?.error_for_status()?.json::<ItemsResponse>().await?.items)
    }

    /// Lyrics for an Audio item (Jellyfin 10.9+).  Returns None when the server
    /// returns 404 (track has no lyrics or server version is older).
    /// Each entry is `(start_ms, text)`.  `start_ms == 0` for unsynced lines.
    pub async fn get_lyrics(&self, item_id: &str) -> Result<Option<Vec<(u64, String)>>> {
        #[derive(serde::Deserialize)]
        struct LyricLine {
            #[serde(rename = "Start")]  start: Option<u64>,
            #[serde(rename = "Text")]   text:  String,
        }
        #[derive(serde::Deserialize)]
        struct LyricsResponse {
            #[serde(rename = "Lyrics")] lyrics: Vec<LyricLine>,
        }

        let url = self.server_url.join(&format!("/Audio/{}/Lyrics", item_id))?;
        let resp = self.http.get(url)
            .header("Authorization", self.auth_header())
            .send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND { return Ok(None); }
        let data: LyricsResponse = resp.error_for_status()?.json().await?;
        let lines = data.lyrics.into_iter()
            .map(|l| (l.start.unwrap_or(0), l.text))
            .collect();
        Ok(Some(lines))
    }

    /// WebSocket URL for real-time events: http(s) → ws(s), path /socket.
    /// Connect with: `tokio_tungstenite::connect_async(client.ws_url())`.
    pub fn ws_url(&self) -> String {
        let base = self.server_url.as_str().trim_end_matches('/');
        let scheme = if base.starts_with("https://") { "wss" } else { "ws" };
        let host_path = base
            .trim_start_matches("https://")
            .trim_start_matches("http://");
        format!(
            "{}://{}/socket?api_key={}&deviceId={}",
            scheme, host_path, self.token, self.device_id
        )
    }
}
