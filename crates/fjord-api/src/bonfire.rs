// ── fjord-api · bonfire.rs ───────────────────────────────────────────────────
//   impl JellyfinClient (Bonfire/JellyProfiles plugin, /plugins/profiles/*) —
//     profiles     list_profiles (404 → Ok(vec![]), the plugin-absent signal), switch_profile,
//                  verify_pin, create_profile, update_profile, delete_profile
//     libraries/devices  list_libraries, list_devices, delete_device
//     bonfire group  bonfire_status, bonfire_settings, bonfire_generate, bonfire_join,
//                  bonfire_kick, bonfire_leave, bonfire_delete_group
//     preferences  get_preferences/set_preferences (switcherMode: "gate"|"native")
//     admin        admin_mappings, admin_reset_pin, admin_set_profile_limit, admin_audit_logs
//   All endpoints live on the SAME Jellyfin server as every other JellyfinClient call (a plugin
//   extends the core API), so this is an `impl JellyfinClient` block sharing auth_header()/api_url(),
//   not a separate client type — unlike fjord-seerr, which talks to a genuinely different service.
//   Deliberately excluded (per the plan): GET /plugins/profiles/profiles.js (web-client injection
//   script, irrelevant to a native client) and POST /plugins/profiles/admin/retry-injection (fixes
//   that same injection mechanism). GET /plugins/profiles/image/{profileId} is an unauthenticated
//   image URL, consumed directly by callers, no client method needed.
// ─────────────────────────────────────────────────────────────────────────────
use anyhow::{anyhow, Result};
use reqwest::StatusCode;
use serde_json::json;

use crate::client::JellyfinClient;
use crate::models::{
    AdminMappings, AuditLogEntry, BonfireDevice, BonfireGroupInfo, BonfireGroupStatus,
    BonfireJoinResult, BonfireLibrary, BonfirePreferences, BonfireProfile, CreateProfileRequest,
    CreateProfileResult, SwitchResult, UpdateProfileRequest,
};

impl JellyfinClient {
    /// All profiles under this master account (`GET /plugins/profiles/list`).
    /// A 404 means the plugin isn't installed — the caller's signal to hide
    /// the whole Bonfire feature, same graceful-absence shape as
    /// `get_episode_timestamps` uses for Intro Skipper.
    pub async fn bonfire_list_profiles(&self) -> Result<Vec<BonfireProfile>> {
        let url = self.api_url("/plugins/profiles/list")?;
        let resp = self.http.get(url).header("Authorization", self.auth_header()).send().await?;
        if resp.status() == StatusCode::NOT_FOUND {
            return Ok(Vec::new());
        }
        Ok(resp.error_for_status()?.json::<Vec<BonfireProfile>>().await?)
    }

    /// `pin` only needs to be `Some` when the target profile's own
    /// `requires_pin` is true — sent as `null`-omitted (not `null`) either
    /// way via `skip_serializing_if`, matching the request-body discipline
    /// this module uses throughout.
    pub async fn bonfire_switch_profile(&self, profile_id: &str, pin: Option<&str>) -> Result<SwitchResult> {
        let url = self.api_url("/plugins/profiles/switch")?;
        let mut body = json!({ "profileId": profile_id });
        if let Some(p) = pin { body["pin"] = json!(p); }
        Ok(self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await?
            .error_for_status()?
            .json::<SwitchResult>().await?)
    }

    /// `Ok(())` on a correct PIN (200); a wrong PIN or any other failure
    /// surfaces as an `Err` via `error_for_status` — the plugin's own docs
    /// don't document a distinct status code for "wrong PIN" specifically.
    pub async fn bonfire_verify_pin(&self, profile_id: &str, pin: &str) -> Result<()> {
        let url = self.api_url("/plugins/profiles/verify-pin")?;
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "profileId": profile_id, "pin": pin }))
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn bonfire_create_profile(&self, req: &CreateProfileRequest) -> Result<CreateProfileResult> {
        let url = self.api_url("/plugins/profiles/create")?;
        Ok(self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(req)
            .send().await?
            .error_for_status()?
            .json::<CreateProfileResult>().await?)
    }

    pub async fn bonfire_update_profile(&self, req: &UpdateProfileRequest) -> Result<()> {
        let url = self.api_url("/plugins/profiles/update")?;
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(req)
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    /// `master_pin` is required only when the calling master profile itself
    /// has a PIN set (same conditional shape as `switch_profile`'s own `pin`).
    pub async fn bonfire_delete_profile(&self, profile_id: &str, master_pin: Option<&str>) -> Result<()> {
        let url = self.api_url("/plugins/profiles/delete")?;
        let mut body = json!({ "profileId": profile_id });
        if let Some(p) = master_pin { body["masterPin"] = json!(p); }
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn bonfire_list_libraries(&self) -> Result<Vec<BonfireLibrary>> {
        let url = self.api_url("/plugins/profiles/libraries")?;
        Ok(self.http.get(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?
            .json::<Vec<BonfireLibrary>>().await?)
    }

    pub async fn bonfire_list_devices(&self) -> Result<Vec<BonfireDevice>> {
        let url = self.api_url("/plugins/profiles/devices")?;
        Ok(self.http.get(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?
            .json::<Vec<BonfireDevice>>().await?)
    }

    pub async fn bonfire_delete_device(&self, device_id: &str) -> Result<()> {
        let url = self.api_url("/plugins/profiles/devices/delete")?;
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "deviceId": device_id }))
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn bonfire_status(&self) -> Result<BonfireGroupStatus> {
        let url = self.api_url("/plugins/profiles/bonfire/status")?;
        Ok(self.http.get(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?
            .json::<BonfireGroupStatus>().await?)
    }

    pub async fn bonfire_settings(
        &self,
        hide_my_sub_profiles_from_others: bool,
        hide_others_sub_profiles_from_me: bool,
        allow_household_lan_bypass: Option<bool>,
    ) -> Result<()> {
        let url = self.api_url("/plugins/profiles/bonfire/settings")?;
        let mut body = json!({
            "hideMySubProfilesFromOthers": hide_my_sub_profiles_from_others,
            "hideOthersSubProfilesFromMe": hide_others_sub_profiles_from_me,
        });
        if let Some(b) = allow_household_lan_bypass { body["allowHouseholdLanBypass"] = json!(b); }
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn bonfire_generate(&self) -> Result<BonfireGroupInfo> {
        let url = self.api_url("/plugins/profiles/bonfire/generate")?;
        Ok(self.http.post(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?
            .json::<BonfireGroupInfo>().await?)
    }

    pub async fn bonfire_join(&self, code: &str) -> Result<BonfireJoinResult> {
        let url = self.api_url("/plugins/profiles/bonfire/join")?;
        Ok(self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "code": code }))
            .send().await?
            .error_for_status()?
            .json::<BonfireJoinResult>().await?)
    }

    pub async fn bonfire_kick(&self, member_id: &str) -> Result<()> {
        let url = self.api_url("/plugins/profiles/bonfire/kick")?;
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "memberId": member_id }))
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn bonfire_leave(&self) -> Result<()> {
        let url = self.api_url("/plugins/profiles/bonfire/leave")?;
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn bonfire_delete_group(&self) -> Result<()> {
        let url = self.api_url("/plugins/profiles/bonfire/delete-group")?;
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn bonfire_get_preferences(&self) -> Result<BonfirePreferences> {
        let url = self.api_url("/plugins/profiles/preferences")?;
        Ok(self.http.get(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?
            .json::<BonfirePreferences>().await?)
    }

    /// `switcher_mode` must be `"gate"` or `"native"` per the plugin's own
    /// docs — not validated client-side, an unrecognized value is the
    /// server's own 400 to raise, not ours to pre-empt.
    pub async fn bonfire_set_preferences(&self, switcher_mode: &str) -> Result<BonfirePreferences> {
        let url = self.api_url("/plugins/profiles/preferences")?;
        Ok(self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "switcherMode": switcher_mode }))
            .send().await?
            .error_for_status()?
            .json::<BonfirePreferences>().await?)
    }

    pub async fn bonfire_admin_mappings(&self) -> Result<AdminMappings> {
        let url = self.api_url("/plugins/profiles/admin/mappings")?;
        Ok(self.http.get(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?
            .json::<AdminMappings>().await?)
    }

    pub async fn bonfire_admin_reset_pin(&self, profile_id: &str) -> Result<()> {
        let url = self.api_url("/plugins/profiles/admin/reset-pin")?;
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&json!({ "profileId": profile_id }))
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    /// `max_profiles: None` presumably clears an override back to the
    /// plugin's own default — not explicitly documented, and there's no way
    /// to confirm without a live admin-permissioned server; flagged here
    /// rather than assumed silently correct.
    pub async fn bonfire_admin_set_profile_limit(&self, user_id: &str, max_profiles: Option<u32>) -> Result<()> {
        let url = self.api_url("/plugins/profiles/admin/set-profile-limit")?;
        let mut body = json!({ "userId": user_id });
        if let Some(m) = max_profiles { body["maxProfiles"] = json!(m); }
        self.http.post(url)
            .header("Authorization", self.auth_header())
            .json(&body)
            .send().await?
            .error_for_status()?;
        Ok(())
    }

    pub async fn bonfire_admin_audit_logs(&self) -> Result<Vec<AuditLogEntry>> {
        let url = self.api_url("/plugins/profiles/admin/audit-logs")?;
        Ok(self.http.get(url)
            .header("Authorization", self.auth_header())
            .send().await?
            .error_for_status()?
            .json::<Vec<AuditLogEntry>>().await?)
    }
}

/// Builds the (unauthenticated) profile-avatar image URL — `GET
/// /plugins/profiles/image/{profileId}`, consumed directly as an `<Image>`
/// source the same way poster/backdrop URLs already are elsewhere in this
/// app, no client method needed for a plain GET with no auth header.
pub fn bonfire_profile_image_url(client: &JellyfinClient, profile_id: &str) -> Result<url::Url> {
    client.api_url(&format!("/plugins/profiles/image/{}", profile_id))
        .map_err(|e| anyhow!("bonfire_profile_image_url: {e}"))
}
