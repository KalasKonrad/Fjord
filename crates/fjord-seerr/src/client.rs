// ── fjord-seerr · client.rs ──────────────────────────────────────────────────
//   SeerrAuth          ApiKey(String) | Session(String) — the "connect.sid=…"
//                      cookie pair, not just the raw value; one branch point,
//                      every authenticated request attaches whichever it holds
//   SeerrClient        base_url + auth; 30s timeout, mirrors JellyfinClient::new
//     status           check_status (associated fn, unauthenticated — /status)
//     auth (assoc fns) sign_in_jellyfin, sign_in_local, quick_connect_initiate/
//                      check/authenticate — each returns (SeerrAuth, User) on
//                      success so the caller can build a real SeerrClient
//     session          logout
//     content          search, get_movie, get_tv, create_request
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::{anyhow, Result};
use reqwest::header::{HeaderMap, SET_COOKIE};
use serde_json::json;
use url::Url;

use crate::models::{
    MediaRequest, MovieDetails, QuickConnect, QuickConnectStatus, SearchResponse, SeasonsSelector,
    TvDetails, User,
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

    /// Unauthenticated reachability check — GET /status has `security: []` in
    /// the Seerr API spec, so this works before any credentials are entered
    /// (used by ConnectSeerrScreen to sanity-check a URL before login).
    pub async fn check_status(base_url: &Url) -> Result<()> {
        let url = api_url(base_url, "/status")?;
        new_http()?.get(url).send().await?.error_for_status()?;
        Ok(())
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
    pub async fn search(&self, query: &str, page: u32) -> Result<SearchResponse> {
        let mut url = api_url(&self.base_url, "/search")?;
        url.query_pairs_mut()
            .append_pair("query", query)
            .append_pair("page", &page.to_string());
        Ok(self
            .authed(self.http.get(url))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
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

    pub async fn create_request(
        &self,
        media_type: &str, // "movie" | "tv"
        tmdb_id: i64,
        seasons: Option<SeasonsSelector>,
    ) -> Result<MediaRequest> {
        let url = api_url(&self.base_url, "/request")?;
        let mut body = json!({ "mediaType": media_type, "mediaId": tmdb_id });
        if let Some(seasons) = seasons {
            body["seasons"] = serde_json::to_value(seasons)?;
        }
        Ok(self
            .authed(self.http.post(url).json(&body))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?)
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
