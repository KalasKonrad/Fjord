// ── fjord-api · models/auth.rs ───────────────────────────────────────────────
//   AuthResponse  top-level login response (AccessToken + User)
//   UserDto       user id + display name from login response
// ─────────────────────────────────────────────────────────────────────────────
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    #[serde(rename = "AccessToken")]
    pub access_token: String,
    #[serde(rename = "User")]
    pub user: UserDto,
}

#[derive(Debug, Deserialize)]
pub struct UserDto {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
}
