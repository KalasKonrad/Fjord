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
//     user settings    get_current_user (GET /auth/me, works for session or API-key auth),
//                      get_watch_provider_regions (GET /watchproviders/regions, unauthenticated),
//                      get_user_settings/update_user_settings (GET/POST /user/{id}/settings/main
//                      — gated by isOwnProfileOrAdmin(), not admin permission, confirmed from
//                      source; used both by resolve_streaming_region's read path and the
//                      Settings -> Integrations -> Streaming Region write path in fjord-app)
//     discover         discover_trending, discover_movies(_upcoming), discover_tv(_upcoming) —
//                      Discover screen's no-query landing rows, all reuse SearchResponse
//     requests         requested_not_available(take_per_type) — (movies, tv) MediaRequests
//                      still on the way (not declined, not already available/deleted),
//                      for the Discover "Requested" landing row; list_requests is the
//                      shared per-mediaType GET /request helper
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
    Language, MediaRequest, MediaStatus, MovieDetails, Profile, QuickConnect, QuickConnectStatus,
    Region, SearchResponse, SeasonsSelector, ServiceServer, ServiceServerDetails, StatusInfo, Tag,
    TvDetails, User, UserGeneralSettings,
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
        let resp: RequestsResponse =
            self.authed(self.http.get(url)).send().await?.error_for_status()?.json().await?;
        Ok(resp.results)
    }

    /// Requests that are still on the way — neither declined nor already
    /// fully available/deleted — for the Discover "Requested" landing row.
    /// `(movies, tv)`, one `GET /request?mediaType=...` call each so the
    /// caller knows each result's type by construction (`MediaRequest`
    /// itself carries no type field to infer it from). Filtered client-side
    /// rather than relying on Seerr's own `filter` query enum, whose exact
    /// semantics blend request-approval state and media-fulfillment state in
    /// ways not worth depending on precisely — `MediaRequest.status == 3` is
    /// DECLINED, `MediaInfo.status` AVAILABLE/DELETED (5/7 — see
    /// `MediaStatus`'s own doc comment for the live-confirmed numbering,
    /// which is not 5/6 as originally assumed) are both excluded using the
    /// same `MediaStatus` enum already modeled elsewhere in this crate. A
    /// request with no linked `media` (shouldn't happen in practice, but the
    /// field is `Option`) is kept rather than dropped — erring toward
    /// showing it over silently hiding a real request.
    pub async fn requested_not_available(&self, take_per_type: u32) -> Result<(Vec<MediaRequest>, Vec<MediaRequest>)> {
        let keep = |r: &MediaRequest| {
            r.status != 3
                && !r
                    .media
                    .as_ref()
                    .is_some_and(|m| matches!(m.status(), Some(MediaStatus::Available | MediaStatus::Deleted)))
        };
        let (movies, tv) =
            tokio::try_join!(self.list_requests("movie", take_per_type), self.list_requests("tv", take_per_type))?;
        Ok((movies.into_iter().filter(keep).collect(), tv.into_iter().filter(keep).collect()))
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

    /// `GET /auth/me` — the currently authenticated user. Works uniformly
    /// for session-cookie AND API-key auth (Seerr resolves an API key to
    /// its "owner" user internally) — unlike the 4 sign-in flows' own
    /// returned `User` (only 3 of which produce one; API-key auth has
    /// none), this is the one way to learn "who am I" regardless of which
    /// of Fjord's connection methods was used. Needed for
    /// `get_user_settings`/`update_user_settings` below, which are keyed
    /// by user id.
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

    /// `POST /user/{id}/settings/main` — see `UserGeneralSettings`'s own
    /// doc comment for why `settings` must be the full, already-fetched
    /// struct (mutated in place by the caller) rather than one built from
    /// scratch with unrelated fields left `None`.
    ///
    /// Reads the response body on failure rather than calling
    /// `error_for_status()` directly — Seerr returns a real JSON error
    /// message on a 500 (e.g. a SQL constraint violation), and the plain
    /// `error_for_status()` this used before discarded it, surfacing only
    /// "500 Internal Server Error" with no indication of why. That gap is
    /// what turned a one-field `NOT NULL` mismatch (see the doc comment on
    /// `UserGeneralSettings`) into a multi-round-trip live debugging
    /// session instead of an immediately obvious error.
    pub async fn update_user_settings(&self, user_id: i64, settings: &UserGeneralSettings) -> Result<()> {
        let url = api_url(&self.base_url, &format!("/user/{user_id}/settings/main"))?;
        let resp = self.authed(self.http.post(url).json(settings)).send().await?;
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

    async fn service_servers(&self, kind: &str) -> Result<Vec<ServiceServer>> {
        let url = api_url(&self.base_url, &format!("/service/{kind}"))?;
        Ok(self.authed(self.http.get(url)).send().await?.error_for_status()?.json().await?)
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
        // Temporary diagnostic for a live report of identical tags/profiles
        // across both tiers despite the user's Seerr admin showing genuinely
        // different profile/tag sets for 2K vs 4K — logs exactly what
        // /service/{kind} returned so the real cause (wrong is4k/isDefault
        // matching here vs. a server-side quirk) can be confirmed from
        // fjord.log rather than guessed again.
        tracing::debug!(
            "seerr: {kind} servers: {:?} -> regular_id={regular_id:?} fourk_id={fourk_id:?}",
            servers.iter().map(|s| (s.id, s.is_default, s.is4k)).collect::<Vec<_>>()
        );
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
