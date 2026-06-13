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
    let url = server_url.join("/Users/AuthenticateByName")?;

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
