// ── fjord-seerr · client.rs ──────────────────────────────────────────────────
//   SeerrAuth          ApiKey(String) | Session(String) (the whole "connect.sid=…" cookie pair);
//                      every authenticated request attaches whichever it holds
//                      (is_session_auth / auth_method_tag / auth_secret for persisting it)
//   SeerrClient        base_url + auth, 30 s timeout (like JellyfinClient::new)
//     status           get_status (unauthenticated /status: version + reachability)
//     auth (assoc fns) sign_in_jellyfin, sign_in_local, quick_connect_initiate/check/authenticate
//                      — each returns (SeerrAuth, User); logout
//     content          search (hand-encoded %20), get_movie, get_tv, get_collection,
//                      get_movie/tv_recommendations + _similar (related_list; /search envelope),
//                      get_person, get_person_combined_credits
//     discover         discover_trending, discover_movies(_upcoming), discover_tv(_upcoming),
//                      discover_movies_filtered/discover_tv_filtered (discover_list +
//                      DiscoverFilters); get_movie/tv_genres and get_movie/tv_watch_providers for
//                      the filter chips
//     watchlist        get_watchlist, add_watchlist/remove_watchlist (per user, independent of
//                      requests)
//     blocklist        get_blocklist, add_blocklist/remove_blocklist (per server; remove also
//                      deletes the Media row), add/remove_blocklist_collection (whole TMDB
//                      collection, resolved server-side)
//     user settings    get_current_user (/auth/me — session or API key),
//                      get_watch_provider_regions, get_languages, get_user_settings/
//                      update_user_settings (GET-mutate-POST of the whole object;
//                      isOwnProfileOrAdmin on the server)
//     requests         create_request (tags, is_4k, profile_id — undocumented, from Seerr's
//                      source), requested_not_available (the Discover "Requested" row; per-tier
//                      status), list_requests, get_request, delete_request (owner while Pending,
//                      MANAGE_REQUESTS always), approve_request/decline_request
//                      (set_request_status), update_request (tier not editable; tags/profileId
//                      always sent)
//     tags/profiles    service_servers / pick_default_server / fetch_server_options;
//                      available_request_options_both_tiers fetches both tiers' tags + quality
//                      profiles in one round (one detail fetch when they share a server), so the
//                      Quality toggle needs no re-fetch; ([], []) per tier without a default server
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::{Result, anyhow};
use percent_encoding::{NON_ALPHANUMERIC, utf8_percent_encode};
use reqwest::header::{HeaderMap, SET_COOKIE};
use serde_json::json;
use url::Url;

use crate::models::{
    BlocklistResponse, Collection, CombinedCredits, DiscoverFilters, Genre, Language, MediaRequest,
    MediaStatus, MovieDetails, PersonDetails, Profile, QuickConnect, QuickConnectStatus, Region,
    SearchResponse, SeasonsSelector, ServiceServer, ServiceServerDetails, StatusInfo, Tag,
    TvDetails, User, UserGeneralSettings, WatchProviderDetail, WatchlistResponse,
};

#[derive(Clone, Debug)]
pub enum SeerrAuth {
    ApiKey(String),
    /// The full "connect.sid=<value>" pair, ready to send as-is in a Cookie header.
    Session(String),
}

#[derive(Clone)]
pub struct SeerrClient {
    http: reqwest::Client,
    base_url: Url,
    auth: SeerrAuth,
}

fn new_http() -> Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()?)
}

/// Builds `{base}/api/v1{path}` preserving any base path (reverse-proxy
/// subpath setups), same reasoning as JellyfinClient::api_url.
fn api_url(base: &Url, path: &str) -> Result<Url> {
    let mut base = base.clone();
    let existing = base.path().trim_end_matches('/');
    base.set_path(&format!("{existing}/api/v1/"));
    Ok(base.join(path.trim_start_matches('/'))?)
}

/// Finds the `connect.sid=…` pair among possibly-multiple Set-Cookie headers
/// on a login response. Returns the name=value segment only (attributes like
/// Path/HttpOnly/SameSite are for the browser cookie jar, not relevant when
/// we're manually echoing this back in a Cookie header ourselves).
fn extract_session_cookie(headers: &HeaderMap) -> Option<String> {
    headers.get_all(SET_COOKIE).iter().find_map(|v| {
        let s = v.to_str().ok()?;
        let pair = s.split(';').next()?.trim();
        if pair.starts_with("connect.sid=") {
            Some(pair.to_string())
        } else {
            None
        }
    })
}

impl SeerrClient {
    pub fn new(base_url: Url, auth: SeerrAuth) -> Result<Self> {
        Ok(Self {
            http: new_http()?,
            base_url,
            auth,
        })
    }

    fn auth_header(&self) -> (&'static str, String) {
        match &self.auth {
            SeerrAuth::ApiKey(key) => ("X-Api-Key", key.clone()),
            SeerrAuth::Session(cookie) => ("Cookie", cookie.clone()),
        }
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        let (name, value) = self.auth_header();
        req.header(name, value)
    }

    /// Unauthenticated status/version check — GET /status has `security: []`
    /// in the Seerr API spec, so this works before any credentials are
    /// entered (used by ConnectSeerrScreen to sanity-check a URL before
    /// login) and also to show Seerr's own version in Settings, the same way
    /// Jellyfin's server-version is shown.
    pub async fn get_status(base_url: &Url) -> Result<StatusInfo> {
        let url = api_url(base_url, "/status")?;
        Ok(new_http()?
            .get(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    // ── Auth: Jellyfin username/password ────────────────────────────────────
    pub async fn sign_in_jellyfin(
        base_url: &Url,
        username: &str,
        password: &str,
    ) -> Result<(SeerrAuth, User)> {
        let url = api_url(base_url, "/auth/jellyfin")?;
        let resp = new_http()?
            .post(url)
            .json(&json!({ "username": username, "password": password }))
            .send()
            .await?
            .error_for_status()?;
        let cookie = extract_session_cookie(resp.headers())
            .ok_or_else(|| anyhow!("Seerr did not return a session cookie"))?;
        let user: User = resp.json().await?;
        Ok((SeerrAuth::Session(cookie), user))
    }

    // ── Auth: local Seerr account (email/password) ──────────────────────────
    pub async fn sign_in_local(
        base_url: &Url,
        email: &str,
        password: &str,
    ) -> Result<(SeerrAuth, User)> {
        let url = api_url(base_url, "/auth/local")?;
        let resp = new_http()?
            .post(url)
            .json(&json!({ "email": email, "password": password }))
            .send()
            .await?
            .error_for_status()?;
        let cookie = extract_session_cookie(resp.headers())
            .ok_or_else(|| anyhow!("Seerr did not return a session cookie"))?;
        let user: User = resp.json().await?;
        Ok((SeerrAuth::Session(cookie), user))
    }

    // ── Auth: Jellyfin Quick Connect (passwordless PIN pairing) ─────────────
    pub async fn quick_connect_initiate(base_url: &Url) -> Result<QuickConnect> {
        let url = api_url(base_url, "/auth/jellyfin/quickconnect/initiate")?;
        Ok(new_http()?
            .post(url)
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Returns `Ok(false)` while still waiting, `Ok(true)` once approved.
    /// A `404` means the Quick Connect session expired — surfaced as an Err
    /// so the caller can distinguish "keep polling" from "start over".
    pub async fn quick_connect_check(base_url: &Url, secret: &str) -> Result<bool> {
        let mut url = api_url(base_url, "/auth/jellyfin/quickconnect/check")?;
        url.query_pairs_mut().append_pair("secret", secret);
        let resp = new_http()?.get(url).send().await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(anyhow!("Quick Connect session expired"));
        }
        let status: QuickConnectStatus = resp.error_for_status()?.json().await?;
        Ok(status.authenticated)
    }

    pub async fn quick_connect_authenticate(
        base_url: &Url,
        secret: &str,
    ) -> Result<(SeerrAuth, User)> {
        let url = api_url(base_url, "/auth/jellyfin/quickconnect/authenticate")?;
        let resp = new_http()?
            .post(url)
            .json(&json!({ "secret": secret }))
            .send()
            .await?
            .error_for_status()?;
        let cookie = extract_session_cookie(resp.headers())
            .ok_or_else(|| anyhow!("Seerr did not return a session cookie"))?;
        let user: User = resp.json().await?;
        Ok((SeerrAuth::Session(cookie), user))
    }

    /// No-op for API-key auth (nothing server-side to clear); best-effort for
    /// session auth, matching the rest of this crate's "log and move on"
    /// error handling for non-critical calls.
    pub async fn logout(&self) -> Result<()> {
        if matches!(self.auth, SeerrAuth::ApiKey(_)) {
            return Ok(());
        }
        let url = api_url(&self.base_url, "/auth/logout")?;
        self.authed(self.http.post(url))
            .send()
            .await?
            .error_for_status()?;
        Ok(())
    }

    // ── Content ───────────────────────────────────────────────────────────
    // The query is percent-encoded by hand (`%20` for spaces): query_pairs_mut() encodes a space
    // as `+`, and Seerr's /search passes req.query.query to TMDB without decoding `+` — every
    // multi-word search 400'd.
    pub async fn search(&self, query: &str, page: u32) -> Result<SearchResponse> {
        let mut url = api_url(&self.base_url, "/search")?;
        let encoded_query = utf8_percent_encode(query, NON_ALPHANUMERIC);
        url.set_query(Some(&format!("query={encoded_query}&page={page}")));
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Shared by the 5 unfiltered /discover/* landing-row endpoints (with
    /// `DiscoverFilters::default()`) and discover_movies_filtered/discover_tv_filtered — all
    /// return the /search envelope `{page, totalPages, totalResults, results}`. Filter fields are
    /// appended only when set; multi-value ids are pipe-joined (OR — see DiscoverFilters).
    async fn discover_list(
        &self,
        path: &str,
        page: u32,
        filters: &DiscoverFilters,
    ) -> Result<SearchResponse> {
        let mut url = api_url(&self.base_url, path)?;
        url.query_pairs_mut().append_pair("page", &page.to_string());
        if let Some(ids) = &filters.genre_ids
            && !ids.is_empty()
        {
            let joined = ids.iter().map(i64::to_string).collect::<Vec<_>>().join("|");
            url.query_pairs_mut().append_pair("genre", &joined);
        }
        if let Some(ids) = &filters.provider_ids
            && !ids.is_empty()
        {
            let joined = ids.iter().map(i64::to_string).collect::<Vec<_>>().join("|");
            url.query_pairs_mut().append_pair("watchProviders", &joined);
        }
        if let Some(region) = &filters.watch_region {
            url.query_pairs_mut().append_pair("watchRegion", region);
        }
        if let Some(sort) = filters.sort {
            url.query_pairs_mut().append_pair("sortBy", sort);
        }
        if let Some(v) = filters.vote_average_gte {
            url.query_pairs_mut()
                .append_pair("voteAverageGte", &v.to_string());
        }
        if let Some((key, val)) = &filters.date_gte {
            url.query_pairs_mut().append_pair(key, val);
        }
        if let Some((key, val)) = &filters.date_lte {
            url.query_pairs_mut().append_pair(key, val);
        }
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn discover_trending(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/trending", page, &DiscoverFilters::default())
            .await
    }
    pub async fn discover_movies(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/movies", page, &DiscoverFilters::default())
            .await
    }
    pub async fn discover_movies_upcoming(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list(
            "/discover/movies/upcoming",
            page,
            &DiscoverFilters::default(),
        )
        .await
    }
    pub async fn discover_tv(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/tv", page, &DiscoverFilters::default())
            .await
    }
    pub async fn discover_tv_upcoming(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/tv/upcoming", page, &DiscoverFilters::default())
            .await
    }

    /// Discover filters (2026-07-18) — the only two endpoints that accept
    /// `DiscoverFilters` with genuine content; see that struct's own doc
    /// comment for why `/search` can't take any of this.
    pub async fn discover_movies_filtered(
        &self,
        page: u32,
        filters: &DiscoverFilters,
    ) -> Result<SearchResponse> {
        self.discover_list("/discover/movies", page, filters).await
    }
    pub async fn discover_tv_filtered(
        &self,
        page: u32,
        filters: &DiscoverFilters,
    ) -> Result<SearchResponse> {
        self.discover_list("/discover/tv", page, filters).await
    }

    /// `GET /genres/movie` / `GET /genres/tv` — confirmed from Seerr's real
    /// source (`server/routes/index.ts`) to return a plain `[{id, name}]`
    /// array, not wrapped in `{genres: [...]}` despite that being TMDB's
    /// own raw shape (Seerr's route handler already unwraps it). Populates
    /// the Discover Genre filter's chip picker.
    pub async fn get_movie_genres(&self) -> Result<Vec<Genre>> {
        let url = api_url(&self.base_url, "/genres/movie")?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
    pub async fn get_tv_genres(&self) -> Result<Vec<Genre>> {
        let url = api_url(&self.base_url, "/genres/tv")?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `GET /watchproviders/movies`/`GET /watchproviders/tv` — distinct
    /// from `get_watch_provider_regions` above (that lists REGIONS; these
    /// list the actual streaming services available within one region).
    /// Populates the Discover Provider filter's chip picker.
    pub async fn get_movie_watch_providers(
        &self,
        watch_region: &str,
    ) -> Result<Vec<WatchProviderDetail>> {
        let mut url = api_url(&self.base_url, "/watchproviders/movies")?;
        url.query_pairs_mut()
            .append_pair("watchRegion", watch_region);
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }
    pub async fn get_tv_watch_providers(
        &self,
        watch_region: &str,
    ) -> Result<Vec<WatchProviderDetail>> {
        let mut url = api_url(&self.base_url, "/watchproviders/tv")?;
        url.query_pairs_mut()
            .append_pair("watchRegion", watch_region);
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    async fn list_requests(&self, media_type: &str, take: u32) -> Result<Vec<MediaRequest>> {
        #[derive(serde::Deserialize)]
        struct RequestsResponse {
            results: Vec<MediaRequest>,
        }
        let mut url = api_url(&self.base_url, "/request")?;
        url.query_pairs_mut()
            .append_pair("take", &take.to_string())
            .append_pair("filter", "all")
            .append_pair("sort", "added")
            .append_pair("sortDirection", "desc")
            .append_pair("mediaType", media_type);
        let resp: RequestsResponse = self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok(resp.results)
    }

    /// Requests still on the way — not declined, not yet available/deleted — for the Discover
    /// "Requested" row: `(movies, tv)`, one `GET /request?mediaType=` each (MediaRequest has no
    /// type field). Filtered client-side (Seerr's `filter` enum mixes approval and fulfillment
    /// state): `MediaRequest.status == 3` is DECLINED; fulfillment is `MediaInfo.status4k` when
    /// `r.is4k`, else `MediaInfo.status` — the two tiers are independent (status can be Unknown
    /// while status4k is Available). AVAILABLE/DELETED (5/7, see MediaStatus) are excluded. A
    /// request without `media` is kept rather than hidden.
    pub async fn requested_not_available(
        &self,
        take_per_type: u32,
    ) -> Result<(Vec<MediaRequest>, Vec<MediaRequest>)> {
        let keep = |r: &MediaRequest| {
            if r.status == 3 {
                return false;
            }
            let Some(m) = r.media.as_ref() else {
                return true;
            };
            let relevant = if r.is4k { m.status4k() } else { m.status() };
            !matches!(
                relevant,
                Some(MediaStatus::Available | MediaStatus::Deleted)
            )
        };
        let (movies, tv) = tokio::try_join!(
            self.list_requests("movie", take_per_type),
            self.list_requests("tv", take_per_type)
        )?;
        Ok((
            movies.into_iter().filter(keep).collect(),
            tv.into_iter().filter(keep).collect(),
        ))
    }

    pub async fn get_movie(&self, tmdb_id: i64) -> Result<MovieDetails> {
        let url = api_url(&self.base_url, &format!("/movie/{tmdb_id}"))?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    pub async fn get_tv(&self, tmdb_id: i64) -> Result<TvDetails> {
        let url = api_url(&self.base_url, &format!("/tv/{tmdb_id}"))?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Shared by the 4 recommendations/similar endpoints — Seerr's movie.ts/tv.ts return the same
    /// envelope as /search (mapMovieResult/mapTvResult), so SearchResponse/SearchResult are
    /// reused.
    async fn related_list(&self, path: &str, page: u32) -> Result<SearchResponse> {
        let mut url = api_url(&self.base_url, path)?;
        url.query_pairs_mut().append_pair("page", &page.to_string());
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Backs the Detail screen's "Recommended" row (2026-07-29).
    pub async fn get_movie_recommendations(
        &self,
        tmdb_id: i64,
        page: u32,
    ) -> Result<SearchResponse> {
        self.related_list(&format!("/movie/{tmdb_id}/recommendations"), page)
            .await
    }
    pub async fn get_movie_similar(&self, tmdb_id: i64, page: u32) -> Result<SearchResponse> {
        self.related_list(&format!("/movie/{tmdb_id}/similar"), page)
            .await
    }
    /// Backs the Series screen's "Recommended" row (2026-07-29).
    pub async fn get_tv_recommendations(&self, tmdb_id: i64, page: u32) -> Result<SearchResponse> {
        self.related_list(&format!("/tv/{tmdb_id}/recommendations"), page)
            .await
    }
    pub async fn get_tv_similar(&self, tmdb_id: i64, page: u32) -> Result<SearchResponse> {
        self.related_list(&format!("/tv/{tmdb_id}/similar"), page)
            .await
    }

    /// `GET /collection/{id}` — full TMDB collection membership, backing the
    /// Collection screen's "Missing From This Collection" row (2026-07-29).
    /// See `Collection`'s own doc comment for the verified response shape.
    pub async fn get_collection(&self, collection_id: i64) -> Result<Collection> {
        let url = api_url(&self.base_url, &format!("/collection/{collection_id}"))?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `GET /person/{id}/combined_credits` — the person's full TMDB filmography (the Person
    /// screen's "Other Work" row and the TMDB-only person screen).
    pub async fn get_person_combined_credits(&self, person_id: i64) -> Result<CombinedCredits> {
        let url = api_url(
            &self.base_url,
            &format!("/person/{person_id}/combined_credits"),
        )?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `GET /person/{id}` — TMDB name/biography/photo, for a Discover cast member with no local
    /// Jellyfin Person (see PersonDetails).
    pub async fn get_person(&self, person_id: i64) -> Result<PersonDetails> {
        let url = api_url(&self.base_url, &format!("/person/{person_id}"))?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `GET /discover/watchlist?page=` — the connected user's Watchlist (Seerr's local table for
    /// non-Plex auth, i.e. all of Fjord's sign-in methods — see WatchlistResponse).
    pub async fn get_watchlist(&self, page: u32) -> Result<WatchlistResponse> {
        let mut url = api_url(&self.base_url, "/discover/watchlist")?;
        url.query_pairs_mut().append_pair("page", &page.to_string());
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `POST /watchlist` — add an item. `ratingKey` is Plex-specific and
    /// deliberately omitted (confirmed optional in `watchlistCreate`'s real
    /// zod schema). Watchlist + Release Calendar, 2026-07-18.
    pub async fn add_watchlist(&self, tmdb_id: i64, media_type: &str, title: &str) -> Result<()> {
        let url = api_url(&self.base_url, "/watchlist")?;
        let body = json!({ "tmdbId": tmdb_id, "mediaType": media_type, "title": title });
        let resp = self.authed(self.http.post(url)).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("add_watchlist failed: {status} — {body}"));
        }
        Ok(())
    }

    /// `DELETE /watchlist/{tmdbId}?mediaType=`. Watchlist + Release
    /// Calendar, 2026-07-18.
    pub async fn remove_watchlist(&self, tmdb_id: i64, media_type: &str) -> Result<()> {
        let mut url = api_url(&self.base_url, &format!("/watchlist/{tmdb_id}"))?;
        url.query_pairs_mut().append_pair("mediaType", media_type);
        let resp = self.authed(self.http.delete(url)).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("remove_watchlist failed: {status} — {body}"));
        }
        Ok(())
    }

    /// `GET /blocklist?take=&skip=`. Global per-server list, not per-user —
    /// see `BlocklistItem`'s own doc comment. 2026-08-06, Seerr Blocklist
    /// support.
    pub async fn get_blocklist(&self, take: u32, skip: u32) -> Result<BlocklistResponse> {
        let mut url = api_url(&self.base_url, "/blocklist")?;
        url.query_pairs_mut()
            .append_pair("take", &take.to_string())
            .append_pair("skip", &skip.to_string());
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `POST /blocklist` — needs MANAGE_BLOCKLIST. `user_id` (FjordState.seerr_user_id, from
    /// get_current_user) is required in the body; the server doesn't infer it.
    pub async fn add_blocklist(
        &self,
        tmdb_id: i64,
        media_type: &str,
        title: &str,
        user_id: i64,
    ) -> Result<()> {
        let url = api_url(&self.base_url, "/blocklist")?;
        let body =
            json!({ "tmdbId": tmdb_id, "mediaType": media_type, "title": title, "user": user_id });
        let resp = self.authed(self.http.post(url)).json(&body).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("add_blocklist failed: {status} — {body}"));
        }
        Ok(())
    }

    /// `DELETE /blocklist/{tmdbId}?mediaType=` — also deletes the underlying Media row, so the
    /// item reverts to untouched/Unknown.
    pub async fn remove_blocklist(&self, tmdb_id: i64, media_type: &str) -> Result<()> {
        let mut url = api_url(&self.base_url, &format!("/blocklist/{tmdb_id}"))?;
        url.query_pairs_mut().append_pair("mediaType", media_type);
        let resp = self.authed(self.http.delete(url)).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("remove_blocklist failed: {status} — {body}"));
        }
        Ok(())
    }

    /// `POST /blocklist/collection/{id}` — no body; Seerr resolves the full
    /// TMDB collection membership server-side from the id alone and
    /// blocklists every part. 2026-08-06, Seerr Blocklist support.
    pub async fn add_blocklist_collection(&self, collection_id: i64) -> Result<()> {
        let url = api_url(
            &self.base_url,
            &format!("/blocklist/collection/{collection_id}"),
        )?;
        let resp = self.authed(self.http.post(url)).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "add_blocklist_collection failed: {status} — {body}"
            ));
        }
        Ok(())
    }

    /// `DELETE /blocklist/collection/{id}` — symmetric with the add above;
    /// not wired to a UI action yet (v1 only adds), kept for completeness.
    /// 2026-08-06, Seerr Blocklist support.
    pub async fn remove_blocklist_collection(&self, collection_id: i64) -> Result<()> {
        let url = api_url(
            &self.base_url,
            &format!("/blocklist/collection/{collection_id}"),
        )?;
        let resp = self.authed(self.http.delete(url)).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!(
                "remove_blocklist_collection failed: {status} — {body}"
            ));
        }
        Ok(())
    }

    /// `GET /auth/me` — the authenticated user, for session cookies AND API keys (a key resolves
    /// to its owner); the one way to learn "who am I" for every sign-in method (API-key auth has
    /// no sign-in response). get_user_settings/update_user_settings are keyed by this id.
    pub async fn get_current_user(&self) -> Result<User> {
        let url = api_url(&self.base_url, "/auth/me")?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `GET /watchproviders/regions` — genuinely unauthenticated on Seerr's
    /// side (confirmed from `server/routes/index.ts`), sent through
    /// `authed()` anyway for consistency with the rest of this client.
    pub async fn get_watch_provider_regions(&self) -> Result<Vec<Region>> {
        let url = api_url(&self.base_url, "/watchproviders/regions")?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `GET /languages` — authenticated (any permission level, confirmed
    /// from `isAuthenticated()` with no explicit `Permission` argument),
    /// TMDB's full language list. See `Language`'s own doc comment for why
    /// this one list backs both the Discover Language and Display Language
    /// pickers in fjord-app rather than hardcoding Seerr's own separate,
    /// smaller UI-locale set for the latter.
    pub async fn get_languages(&self) -> Result<Vec<Language>> {
        let url = api_url(&self.base_url, "/languages")?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `GET /user/{id}/settings/main` — gated by Seerr's own
    /// `isOwnProfileOrAdmin()`, not `Permission.ADMIN` (confirmed from
    /// source — see `UserGeneralSettings`'s own doc comment). Any user can
    /// read/write their own settings here regardless of Seerr permission
    /// level, as long as `user_id` matches whoever `get_current_user`
    /// resolves to.
    pub async fn get_user_settings(&self, user_id: i64) -> Result<UserGeneralSettings> {
        let url = api_url(&self.base_url, &format!("/user/{user_id}/settings/main"))?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `POST /user/{id}/settings/main` — `settings` must be the full, already-fetched struct,
    /// mutated by the caller (see UserGeneralSettings). On failure the response body goes into
    /// the error: Seerr returns a real JSON message on a 500 (e.g. a NOT NULL violation).
    pub async fn update_user_settings(
        &self,
        user_id: i64,
        settings: &UserGeneralSettings,
    ) -> Result<()> {
        let url = api_url(&self.base_url, &format!("/user/{user_id}/settings/main"))?;
        let resp = self
            .authed(self.http.post(url).json(settings))
            .send()
            .await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("update_user_settings failed: {status} — {body}"));
        }
        Ok(())
    }

    /// `is_4k` maps to the request body's `is4k` field. `tags`/`profile_id`
    /// map to `tags: number[]` / `profileId: number` — none of these three
    /// appear in the published OpenAPI spec (same gap as `media_type`
    /// elsewhere in this crate — confirmed directly from Seerr's TypeScript
    /// source, not assumed). There is still no discrete "HDR" request flag:
    /// HDR (and codec, audio format, everything else about the eventual
    /// file) is baked into whichever Radarr/Sonarr quality profile ends up
    /// selected, not something a request itself specifies beyond `profileId`.
    /// A 4K request only succeeds if the Seerr admin has a 4K server
    /// configured; otherwise it fails server-side and surfaces through the
    /// normal error path.
    pub async fn create_request(
        &self,
        media_type: &str, // "movie" | "tv"
        tmdb_id: i64,
        seasons: Option<SeasonsSelector>,
        is_4k: bool,
        tags: Vec<i64>,
        profile_id: Option<i64>,
    ) -> Result<MediaRequest> {
        let url = api_url(&self.base_url, "/request")?;
        let mut body = json!({ "mediaType": media_type, "mediaId": tmdb_id, "is4k": is_4k });
        if let Some(seasons) = seasons {
            body["seasons"] = serde_json::to_value(seasons)?;
        }
        if !tags.is_empty() {
            body["tags"] = serde_json::to_value(tags)?;
        }
        if let Some(profile_id) = profile_id {
            body["profileId"] = serde_json::to_value(profile_id)?;
        }
        Ok(self
            .authed(self.http.post(url).json(&body))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `GET /request/{id}` — a fresh profile_id/tags/seasons when Edit Request opens (one extra
    /// round trip on a rare action, instead of a snapshot from the Requested row).
    pub async fn get_request(&self, request_id: i64) -> Result<MediaRequest> {
        let url = api_url(&self.base_url, &format!("/request/{request_id}"))?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// `DELETE /request/{id}` — Cancel Request. Seerr (server/routes/request.ts): the owner only
    /// while Pending; MANAGE_REQUESTS any status. Not pre-checked here — the menu only offers it
    /// when local state allows, and a 403 from drifted state surfaces as a toast.
    pub async fn delete_request(&self, request_id: i64) -> Result<()> {
        let url = api_url(&self.base_url, &format!("/request/{request_id}"))?;
        let resp = self.authed(self.http.delete(url)).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("delete_request failed: {status} — {body}"));
        }
        Ok(())
    }

    /// `POST /request/{id}/approve` / `/decline` — admin-only
    /// (`MANAGE_REQUESTS`, enforced server-side), no body. Discover context
    /// menu, 2026-07-18.
    pub async fn approve_request(&self, request_id: i64) -> Result<()> {
        self.set_request_status(request_id, "approve").await
    }
    pub async fn decline_request(&self, request_id: i64) -> Result<()> {
        self.set_request_status(request_id, "decline").await
    }
    async fn set_request_status(&self, request_id: i64, status: &str) -> Result<()> {
        let url = api_url(&self.base_url, &format!("/request/{request_id}/{status}"))?;
        let resp = self.authed(self.http.post(url)).send().await?;
        if !resp.status().is_success() {
            let status_code = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("{status}_request failed: {status_code} — {body}"));
        }
        Ok(())
    }

    /// `PUT /request/{id}` — Edit Request. Body like create_request minus mediaType/mediaId/is4k:
    /// the tier can't be edited (request.ts never assigns is4k) — switching tiers means cancel +
    /// new request. `media_type` stays a Rust parameter because TV requires a non-empty `seasons`
    /// (the server rejects an empty one: "Missing seasons …"). Merging with sibling requests for
    /// other seasons is done server-side.
    pub async fn update_request(
        &self,
        request_id: i64,
        media_type: &str, // "movie" | "tv"
        seasons: Option<SeasonsSelector>,
        tags: Vec<i64>,
        profile_id: Option<i64>,
    ) -> Result<()> {
        let url = api_url(&self.base_url, &format!("/request/{request_id}"))?;
        let mut body = json!({ "mediaType": media_type });
        if media_type == "tv" {
            let Some(seasons) = seasons else {
                return Err(anyhow!("update_request: seasons required for a tv request"));
            };
            body["seasons"] = serde_json::to_value(seasons)?;
        }
        // Always sent, including `[]` (the user cleared every tag): Edit sets exactly this state,
        // and the PUT handler assigns req.body.tags unconditionally — an omitted key would assign
        // `undefined`.
        body["tags"] = serde_json::to_value(&tags)?;
        // Same "always explicit" reasoning as tags above — `null` for the
        // synthetic "Default" (0) selection, not an omitted key, since the
        // handler unconditionally assigns `request.profileId = req.body.profileId`.
        body["profileId"] = match profile_id {
            Some(id) => serde_json::to_value(id)?,
            None => serde_json::Value::Null,
        };
        let resp = self.authed(self.http.put(url).json(&body)).send().await?;
        if !resp.status().is_success() {
            let status = resp.status();
            let err_body = resp.text().await.unwrap_or_default();
            return Err(anyhow!("update_request failed: {status} — {err_body}"));
        }
        Ok(())
    }

    async fn service_servers(&self, kind: &str) -> Result<Vec<ServiceServer>> {
        let url = api_url(&self.base_url, &format!("/service/{kind}"))?;
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
    }

    /// Three-step cascade, each step only reached if the previous finds
    /// nothing: (1) a server matching the tier AND marked `isDefault` — the
    /// expected case when an admin runs multiple servers per tier and picks
    /// one as default; (2) *any* server matching the tier, regardless of
    /// `isDefault` — a lone dedicated 4K (or lone regular) instance doesn't
    /// strictly need its own `isDefault` flag set to be the only sensible
    /// choice for that tier, and step (1) alone would otherwise silently
    /// fall through to step (3) and return the *other* tier's server; (3)
    /// any default server at all, regardless of tier — the single combined-
    /// instance setup, where both tiers legitimately share one server.
    fn pick_default_server(servers: &[ServiceServer], is_4k: bool) -> Option<i64> {
        servers
            .iter()
            .find(|s| s.is_default && s.is4k == is_4k)
            .or_else(|| servers.iter().find(|s| s.is4k == is_4k))
            .or_else(|| servers.iter().find(|s| s.is_default))
            .map(|s| s.id)
    }

    /// Empty lists (not an error) when no default server is configured — a request without tags
    /// or an explicit profile is valid. Real failures (network, permissions — /service/* may need
    /// elevated rights) are `Err`; callers treat them as "nothing available" too.
    async fn fetch_server_options(
        &self,
        kind: &str,
        server_id: Option<i64>,
    ) -> Result<(Vec<Tag>, Vec<Profile>)> {
        let Some(server_id) = server_id else {
            return Ok((Vec::new(), Vec::new()));
        };
        let url = api_url(&self.base_url, &format!("/service/{kind}/{server_id}"))?;
        let details: ServiceServerDetails = self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        Ok((details.tags, details.profiles))
    }

    /// Fetches the default Radarr (movie) / Sonarr (tv) server's configured
    /// tags and quality profiles for **both** quality tiers in one round of
    /// calls — `(regular_tier, 4k_tier)` — so the Request Options modal's
    /// Quality toggle can switch between them instantly with no re-fetch or
    /// race condition on rapid toggling. The common single-instance setup
    /// (both tiers resolve to the same server) only costs the one
    /// `/service/{kind}` list call, not a duplicate detail fetch — the two
    /// detail fetches only both run (in parallel) when a genuinely separate
    /// 4K instance exists.
    pub async fn available_request_options_both_tiers(
        &self,
        media_type: &str,
    ) -> Result<((Vec<Tag>, Vec<Profile>), (Vec<Tag>, Vec<Profile>))> {
        let kind = if media_type == "movie" {
            "radarr"
        } else {
            "sonarr"
        };
        let servers = self.service_servers(kind).await?;
        let regular_id = Self::pick_default_server(&servers, false);
        let fourk_id = Self::pick_default_server(&servers, true);
        // Temporary diagnostic for a live report of identical tags/profiles
        // across both tiers despite the user's Seerr admin showing genuinely
        // different profile/tag sets for 2K vs 4K — logs exactly what
        // /service/{kind} returned so the real cause (wrong is4k/isDefault
        // matching here vs. a server-side quirk) can be confirmed from
        // fjord.log rather than guessed again.
        tracing::debug!(
            "seerr: {kind} servers: {:?} -> regular_id={regular_id:?} fourk_id={fourk_id:?}",
            servers
                .iter()
                .map(|s| (s.id, s.is_default, s.is4k))
                .collect::<Vec<_>>()
        );
        if fourk_id == regular_id {
            let opts = self.fetch_server_options(kind, regular_id).await?;
            Ok((opts.clone(), opts))
        } else {
            tokio::try_join!(
                self.fetch_server_options(kind, regular_id),
                self.fetch_server_options(kind, fourk_id)
            )
        }
    }

    /// True when the underlying auth is a session cookie (as opposed to a
    /// static API key) — used by callers to decide whether a 401 means
    /// "session expired, prompt reconnect" vs. "key was revoked/invalid".
    pub fn is_session_auth(&self) -> bool {
        matches!(self.auth, SeerrAuth::Session(_))
    }

    pub fn auth_method_tag(&self) -> &'static str {
        match self.auth {
            SeerrAuth::ApiKey(_) => "apikey",
            SeerrAuth::Session(_) => "session",
        }
    }

    /// The raw secret to persist to Config — the API key itself, or the
    /// session cookie pair. Callers store this under `seerr_api_key` or
    /// `seerr_session_cookie` respectively based on `auth_method_tag()`.
    pub fn auth_secret(&self) -> &str {
        match &self.auth {
            SeerrAuth::ApiKey(k) => k,
            SeerrAuth::Session(c) => c,
        }
    }
}
