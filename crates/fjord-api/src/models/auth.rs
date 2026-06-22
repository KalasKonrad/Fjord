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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auth_response_deserializes() {
        let json = r#"{"AccessToken":"tok123","User":{"Id":"user-uuid","Name":"Alice"}}"#;
        let resp: AuthResponse = serde_json::from_str(json).unwrap();
        assert_eq!(resp.access_token, "tok123");
        assert_eq!(resp.user.id, "user-uuid");
        assert_eq!(resp.user.name, "Alice");
    }
}
