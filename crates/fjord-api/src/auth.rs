// ── fjord-api · auth.rs ──────────────────────────────────────────────────────
//   authenticate  POST /Users/AuthenticateByName → AuthResponse (token + user id)
//   Quick Connect (sign in by approving a code in another Jellyfin app; verified against
//   jellyfin/jellyfin 10.9 → 12.x and a live 12.4 server, see JELLYFIN.md):
//     quick_connect_enabled           GET /QuickConnect/Enabled → bool (anonymous)
//     quick_connect_initiate          POST /QuickConnect/Initiate → QuickConnectResult (code + secret)
//     quick_connect_status            GET /QuickConnect/Connect?secret= → QuickConnectStatus
//                                     (404 = expired/unknown, 401 = Quick Connect turned off)
//     authenticate_with_quick_connect POST /Users/AuthenticateWithQuickConnect → AuthResponse
//   endpoint / anonymous_auth_header / checked  shared by the calls above
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::Result;
use reqwest::StatusCode;
use serde_json::json;
use url::Url;

use crate::models::{AuthResponse, QuickConnectResult};

/// `server_url` + `path`, keeping any base path on the server URL (reverse-proxy subpath setups) —
/// a leading-slash join would discard it (CR10-14).
fn endpoint(server_url: &Url, path: &str) -> Result<Url> {
    let mut base = server_url.clone();
    if !base.path().ends_with('/') {
        let p = format!("{}/", base.path());
        base.set_path(&p);
    }
    Ok(base.join(path)?)
}

/// The Authorization header before there is a token. Quick Connect needs every field: the server
/// creates the new session for this DeviceId/Device/Client/Version and refuses a request without
/// them (400).
fn anonymous_auth_header(device_id: &str) -> String {
    format!(
        r#"MediaBrowser Client="Fjord", Device="Linux", DeviceId="{device_id}", Version="0.1.0""#
    )
}

/// A non-2xx answer as an error carrying the server's own text ("HTTP 401 Unauthorized — Quick
/// connect is disabled"), so logs and short_error show the reason.
async fn checked(resp: reqwest::Response) -> Result<reqwest::Response> {
    let status = resp.status();
    if status.is_success() {
        return Ok(resp);
    }
    let body = resp.text().await.unwrap_or_default();
    let body: String = body.trim().chars().take(200).collect();
    if body.is_empty() {
        anyhow::bail!("HTTP {status}");
    }
    anyhow::bail!("HTTP {status} — {body}")
}

pub async fn authenticate(
    http: &reqwest::Client,
    server_url: &Url,
    username: &str,
    password: &str,
    device_id: &str,
) -> Result<AuthResponse> {
    let url = endpoint(server_url, "Users/AuthenticateByName")?;
    let resp = http
        .post(url)
        .header("Authorization", anonymous_auth_header(device_id))
        .json(&json!({ "Username": username, "Pw": password }))
        .send()
        .await?
        .error_for_status()?
        .json::<AuthResponse>()
        .await?;

    Ok(resp)
}

/// Whether the server allows Quick Connect (Dashboard → General, on by default).
pub async fn quick_connect_enabled(http: &reqwest::Client, server_url: &Url) -> Result<bool> {
    let url = endpoint(server_url, "QuickConnect/Enabled")?;
    let resp = checked(http.get(url).send().await?).await?;
    Ok(resp.json::<bool>().await?)
}

/// Starts a Quick Connect request: the code to show and the secret to poll with. It lives 10
/// minutes on the server.
pub async fn quick_connect_initiate(
    http: &reqwest::Client,
    server_url: &Url,
    device_id: &str,
) -> Result<QuickConnectResult> {
    let url = endpoint(server_url, "QuickConnect/Initiate")?;
    let resp = http
        .post(url)
        .header("Authorization", anonymous_auth_header(device_id))
        .send()
        .await?;
    Ok(checked(resp).await?.json::<QuickConnectResult>().await?)
}

/// Where a Quick Connect request stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QuickConnectStatus {
    /// Nobody has entered the code yet.
    Waiting,
    /// Approved — `authenticate_with_quick_connect` now returns the token.
    Approved,
    /// The server doesn't know the secret any more (10 minutes passed, or it restarted).
    Expired,
    /// Quick Connect was turned off on the server meanwhile.
    Disabled,
}

pub async fn quick_connect_status(
    http: &reqwest::Client,
    server_url: &Url,
    device_id: &str,
    secret: &str,
) -> Result<QuickConnectStatus> {
    let mut url = endpoint(server_url, "QuickConnect/Connect")?;
    url.query_pairs_mut().append_pair("secret", secret);
    let resp = http
        .get(url)
        .header("Authorization", anonymous_auth_header(device_id))
        .send()
        .await?;
    match resp.status() {
        StatusCode::NOT_FOUND => return Ok(QuickConnectStatus::Expired),
        StatusCode::UNAUTHORIZED => return Ok(QuickConnectStatus::Disabled),
        _ => {}
    }
    let result = checked(resp).await?.json::<QuickConnectResult>().await?;
    Ok(if result.authenticated {
        QuickConnectStatus::Approved
    } else {
        QuickConnectStatus::Waiting
    })
}

/// The approved request's session: the same answer as a password login. Before approval the
/// server answers 404.
pub async fn authenticate_with_quick_connect(
    http: &reqwest::Client,
    server_url: &Url,
    device_id: &str,
    secret: &str,
) -> Result<AuthResponse> {
    let url = endpoint(server_url, "Users/AuthenticateWithQuickConnect")?;
    let resp = http
        .post(url)
        .header("Authorization", anonymous_auth_header(device_id))
        .json(&json!({ "Secret": secret }))
        .send()
        .await?;
    Ok(checked(resp).await?.json::<AuthResponse>().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_keeps_a_base_path() {
        let base = Url::parse("https://example.com/jellyfin").unwrap();
        assert_eq!(
            endpoint(&base, "QuickConnect/Enabled").unwrap().as_str(),
            "https://example.com/jellyfin/QuickConnect/Enabled"
        );
        let root = Url::parse("http://192.0.2.1:8096/").unwrap();
        assert_eq!(
            endpoint(&root, "Users/AuthenticateByName")
                .unwrap()
                .as_str(),
            "http://192.0.2.1:8096/Users/AuthenticateByName"
        );
    }
}
