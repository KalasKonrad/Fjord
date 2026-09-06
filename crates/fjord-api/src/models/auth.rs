// ── fjord-api · models/auth.rs ───────────────────────────────────────────────
//   AuthResponse  top-level login response (AccessToken + User)
//   UserDto       user id + display name + admin policy from login/GET /Users/{id}
//   UserPolicy    (Bonfire Phase 6, 2026-09-04) IsAdministrator — genuinely new;
//                 Fjord never modeled Jellyfin's own server-admin flag before this
// ─────────────────────────────────────────────────────────────────────────────
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct AuthResponse {
    #[serde(rename = "AccessToken")]
    pub access_token: String,
    #[serde(rename = "User")]
    pub user: UserDto,
}

// Bonfire Phase 6 (admin actions, 2026-09-04): Bonfire's own admin/*
// endpoints gate on Jellyfin's core Policy.IsAdministrator (verified
// directly against the real plugin controller source — no Bonfire-
// specific permission concept exists at all), which Fjord had never
// modeled anywhere before this — UserDto only ever carried id/name.
// #[serde(default)] on both the field and the struct: a login/user-info
// response missing or reshaping Policy degrades safely to
// is_administrator: false, the correct default for a security-relevant
// flag (never default-open).
#[derive(Debug, Deserialize, Default)]
pub struct UserPolicy {
    #[serde(rename = "IsAdministrator", default)]
    pub is_administrator: bool,
}

#[derive(Debug, Deserialize)]
pub struct UserDto {
    #[serde(rename = "Id")]
    pub id: String,
    #[serde(rename = "Name")]
    pub name: String,
    #[serde(rename = "Policy", default)]
    pub policy: UserPolicy,
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

    #[test]
    fn missing_policy_defaults_to_non_admin() {
        // A response with no Policy at all (the pre-Phase-6 shape every
        // existing fixture/test used) must still parse, and must default
        // to non-admin — never silently grant admin on a shape mismatch.
        let json = r#"{"AccessToken":"tok123","User":{"Id":"user-uuid","Name":"Alice"}}"#;
        let resp: AuthResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.user.policy.is_administrator);
    }

    #[test]
    fn real_policy_shape_deserializes() {
        let json = r#"{"AccessToken":"tok123","User":{"Id":"user-uuid","Name":"Alice","Policy":{"IsAdministrator":true,"IsHidden":false}}}"#;
        let resp: AuthResponse = serde_json::from_str(json).unwrap();
        assert!(resp.user.policy.is_administrator);
    }
}
