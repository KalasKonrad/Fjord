// ── fjord-api · auth.rs ──────────────────────────────────────────────────────
//   authenticate  POST /Users/AuthenticateByName → AuthResponse (token + user id)
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::Result;
use serde_json::json;
use url::Url;

use crate::models::AuthResponse;

pub async fn authenticate(
    http: &reqwest::Client,
    server_url: &Url,
    username: &str,
    password: &str,
    device_id: &str,
) -> Result<AuthResponse> {
    // Preserve any base path on the server URL (reverse-proxy subpath setups) —
    // a leading-slash join would discard it (CR10-14).
    let url = {
        let mut base = server_url.clone();
        if !base.path().ends_with('/') {
            let p = format!("{}/", base.path());
            base.set_path(&p);
        }
        base.join("Users/AuthenticateByName")?
    };

    let auth_header = format!(
        r#"MediaBrowser Client="Fjord", Device="Linux", DeviceId="{device_id}", Version="0.1.0""#
    );

    let resp = http
        .post(url)
        .header("Authorization", auth_header)
        .json(&json!({ "Username": username, "Pw": password }))
        .send()
        .await?
        .error_for_status()?
        .json::<AuthResponse>()
        .await?;

    Ok(resp)
}
