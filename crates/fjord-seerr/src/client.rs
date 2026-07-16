// ── fjord-seerr · client.rs ──────────────────────────────────────────────────
//   SeerrAuth          ApiKey(String) | Session(String) — the "connect.sid=…"
//                      cookie pair, not just the raw value; one branch point,
//                      every authenticated request attaches whichever it holds
//   SeerrClient        base_url + auth; 30s timeout, mirrors JellyfinClient::new
//     status           get_status (associated fn, unauthenticated — /status, version + reachability)
//     auth (assoc fns) sign_in_jellyfin, sign_in_local, quick_connect_initiate/
//                      check/authenticate — each returns (SeerrAuth, User) on
//                      success so the caller can build a real SeerrClient
//     session          logout
//     content          search, get_movie, get_tv, create_request (tags: Vec<i64>, is_4k,
//                      profile_id — all three undocumented in the OpenAPI spec, confirmed
//                      from Seerr's TS source)
//     discover         discover_trending, discover_movies(_upcoming), discover_tv(_upcoming) —
//                      Discover screen's no-query landing rows, all reuse SearchResponse
//     tags/profiles    service_servers/pick_default_server/fetch_server_options — building
//                      blocks; available_request_options_both_tiers(media_type) fetches the
//                      regular AND 4K tier's tags + quality profiles in one round of calls
//                      (dedups the detail fetch when both tiers share one server) so the
//                      Request Options modal's Quality toggle can swap between them with no
//                      re-fetch; ([], []) per tier (not Err) when no default server configured
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::{anyhow, Result};
use percent_encoding::{utf8_percent_encode, NON_ALPHANUMERIC};
use reqwest::header::{HeaderMap, SET_COOKIE};
use serde_json::json;
use url::Url;

use crate::models::{
    MediaRequest, MovieDetails, Profile, QuickConnect, QuickConnectStatus, SearchResponse,
    SeasonsSelector, ServiceServer, ServiceServerDetails, StatusInfo, Tag, TvDetails, User,
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
        Ok(Self { http: new_http()?, base_url, auth })
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
        Ok(new_http()?.get(url).send().await?.error_for_status()?.json().await?)
    }

    // ── Auth: Jellyfin username/password ────────────────────────────────────
    pub async fn sign_in_jellyfin(base_url: &Url, username: &str, password: &str) -> Result<(SeerrAuth, User)> {
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
    pub async fn sign_in_local(base_url: &Url, email: &str, password: &str) -> Result<(SeerrAuth, User)> {
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
        Ok(new_http()?.post(url).send().await?.error_for_status()?.json().await?)
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

    pub async fn quick_connect_authenticate(base_url: &Url, secret: &str) -> Result<(SeerrAuth, User)> {
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
        self.authed(self.http.post(url)).send().await?.error_for_status()?;
        Ok(())
    }

    // ── Content ───────────────────────────────────────────────────────────
    /// Query is percent-encoded by hand (`%20` for spaces) rather than via
    /// `query_pairs_mut()`, which follows the WHATWG application/x-www-form-
    /// urlencoded serializer and always encodes space as `+` — real bug,
    /// found live: Seerr's `/search` route (confirmed from its actual
    /// source) reads `req.query.query` and passes it straight to TMDB with
    /// no `+`-to-space decoding anywhere in that path, so any multi-word
    /// search 400'd. `%20` round-trips correctly through every hop, since
    /// RFC 3986 percent-decoding is unambiguous — `+` only means space under
    /// the specific form-urlencoded convention, which nothing here honors.
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

    /// Shared by the 5 `/discover/*` landing-row endpoints below — all return
    /// the exact same `{page, totalPages, totalResults, results}` shape as
    /// `/search` (confirmed from the OpenAPI spec), just pre-filtered/sorted
    /// server-side instead of query-driven.
    async fn discover_list(&self, path: &str, page: u32) -> Result<SearchResponse> {
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

    pub async fn discover_trending(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/trending", page).await
    }
    pub async fn discover_movies(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/movies", page).await
    }
    pub async fn discover_movies_upcoming(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/movies/upcoming", page).await
    }
    pub async fn discover_tv(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/tv", page).await
    }
    pub async fn discover_tv_upcoming(&self, page: u32) -> Result<SearchResponse> {
        self.discover_list("/discover/tv/upcoming", page).await
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

    async fn service_servers(&self, kind: &str) -> Result<Vec<ServiceServer>> {
        let url = api_url(&self.base_url, &format!("/service/{kind}"))?;
        Ok(self.authed(self.http.get(url)).send().await?.error_for_status()?.json().await?)
    }

    /// Prefers a server matching the given quality tier (an admin can
    /// configure a dedicated 4K Radarr/Sonarr instance alongside the regular
    /// one, each independently marked `isDefault`) — falls back to any
    /// default server if no tier-specific match exists, so single-instance
    /// setups (the common case) are unaffected.
    fn pick_default_server(servers: &[ServiceServer], is_4k: bool) -> Option<i64> {
        servers
            .iter()
            .find(|s| s.is_default && s.is4k == is_4k)
            .map(|s| s.id)
            .or_else(|| servers.iter().find(|s| s.is_default).map(|s| s.id))
    }

    /// Empty lists (not an error) when there's no default server configured
    /// — not every Seerr instance has Radarr/Sonarr wired up, and a plain
    /// request without tags/an explicit profile is still perfectly valid.
    /// Genuine failures (network, permissions — the `/service/*` endpoints
    /// may require elevated permissions on some instances) propagate as
    /// `Err`; callers should treat that as "nothing available" too rather
    /// than blocking the request flow on it.
    async fn fetch_server_options(&self, kind: &str, server_id: Option<i64>) -> Result<(Vec<Tag>, Vec<Profile>)> {
        let Some(server_id) = server_id else {
            return Ok((Vec::new(), Vec::new()));
        };
        let url = api_url(&self.base_url, &format!("/service/{kind}/{server_id}"))?;
        let details: ServiceServerDetails =
            self.authed(self.http.get(url)).send().await?.error_for_status()?.json().await?;
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
        let kind = if media_type == "movie" { "radarr" } else { "sonarr" };
        let servers = self.service_servers(kind).await?;
        let regular_id = Self::pick_default_server(&servers, false);
        let fourk_id = Self::pick_default_server(&servers, true);
        if fourk_id == regular_id {
            let opts = self.fetch_server_options(kind, regular_id).await?;
            Ok((opts.clone(), opts))
        } else {
            tokio::try_join!(self.fetch_server_options(kind, regular_id), self.fetch_server_options(kind, fourk_id))
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
