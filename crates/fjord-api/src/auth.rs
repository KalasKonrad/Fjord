use anyhow::Result;
use serde_json::json;
use url::Url;

use crate::models::AuthResponse;

const AUTH_HEADER: &str =
    r#"MediaBrowser Client="Fjord", Device="Linux", DeviceId="fjord-00000000-0000-0000-0000-000000000001", Version="0.1.0""#;

pub async fn authenticate(
    http: &reqwest::Client,
    server_url: &Url,
    username: &str,
    password: &str,
) -> Result<AuthResponse> {
    let url = server_url.join("/Users/AuthenticateByName")?;

    let resp = http
        .post(url)
        .header("Authorization", AUTH_HEADER)
        .json(&json!({ "Username": username, "Pw": password }))
        .send()
        .await?
        .error_for_status()?
        .json::<AuthResponse>()
        .await?;

    Ok(resp)
}
