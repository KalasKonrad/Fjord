// ── fjord-api · models/bonfire.rs ────────────────────────────────────────────
//   BonfireProfile         GET /plugins/profiles/list entry
//   SwitchResult            POST /plugins/profiles/switch response
//   CreateProfileRequest    POST /plugins/profiles/create body
//   CreateProfileResult     POST /plugins/profiles/create response
//   UpdateProfileRequest    POST /plugins/profiles/update body
//   BonfireLibrary          GET /plugins/profiles/libraries entry
//   BonfireDevice           GET /plugins/profiles/devices entry
//   BonfireGroupMember      shared shape for ownedMembers (status) / members (generate)
//   BonfireGroupStatus      GET /plugins/profiles/bonfire/status response
//   BonfireGroupInfo        POST /plugins/profiles/bonfire/generate response
//   BonfireJoinResult       POST /plugins/profiles/bonfire/join response
//   BonfirePreferences      GET/POST /plugins/profiles/preferences body+response (switcherMode: "gate"|"native")
//   AdminMasterUser/AdminSubProfile/AdminMappings  GET /plugins/profiles/admin/mappings response
//   AuditLogEntry           GET /plugins/profiles/admin/audit-logs entry
//
// Field NAMES below are quoted from the plugin's own real developer-api.md
// (github.com/AHouseOfBards/Bonfire-JellyProfiles, fetched 2026-08-09), but
// the CASING was wrong from the start — this whole module originally
// assumed camelCase ("a third-party plugin, its own convention," reasoning
// by analogy with Seerr, a genuinely different Node.js/TypeScript service)
// with a caveat that it was "not yet live-verified against a real
// Bonfire-enabled server." That caveat turned out to matter: live-tested
// 2026-08-12 against a real server (4 real profiles, one master + three
// sub-profiles) via a one-off diagnostic query (raw HTTP + the already-
// decrypted token from this dev machine's own saved config.json, same
// technique this project has used before for Seerr endpoints — see
// CLAUDE.md), and the real response is **PascalCase** throughout
// (`ProfileUserId`, not `profileUserId`) — the same convention every OTHER
// Jellyfin-core-facing struct in this crate already correctly uses
// (`models/system.rs`, `models/plugins.rs`). This makes sense in hindsight:
// Bonfire is a Jellyfin plugin running on Jellyfin's own ASP.NET Core
// pipeline, not a standalone service with its own serialization choice —
// there was never a real reason to expect it to diverge from Jellyfin's own
// convention, the developer-api.md's camelCase-looking examples
// notwithstanding. This was the actual root cause of "Bonfire doesn't
// work" — `bonfire_list_profiles()` failed to deserialize on every call
// with "missing field `profileUserId`" (silently swallowed by
// `sync_bonfire_subprofiles`'s own `Err => debug!(...); return;` — visible
// only once that function's logging was added, see its own doc comment in
// profile.rs), so `Config.profiles` could never grow past 1 and the picker
// could never trigger, regardless of the auto-login wiring fix from the
// day before. The rest of this module (17 endpoints total) has NOT been
// individually live-verified beyond this one — the casing fix applies
// uniformly since it's a property of the whole plugin, not a per-endpoint
// quirk, but per-field required/optional mismatches on the other 16
// remain unconfirmed until each is actually exercised live.
// ─────────────────────────────────────────────────────────────────────────────
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BonfireProfile {
    pub profile_user_id: String,
    pub profile_name: String,
    pub avatar_initial: String,
    pub avatar_color: String,
    pub requires_pin: bool,
    pub has_pin: bool,
    pub is_master: bool,
    pub lockout_minutes: i64,
    // Master-only — confirmed absent entirely on every sub-profile entry in
    // a real /list response (a required-but-sometimes-missing field is
    // exactly what surfaced this whole struct's casing bug in the first
    // place, see this file's own header comment). Currently unread anywhere
    // in fjord-app, so 0 for a sub-profile is harmless.
    #[serde(default)] pub max_sub_profiles: i64,
    #[serde(default)] pub enabled_folders: Vec<String>,
    #[serde(default)] pub blocked_tags: Vec<String>,
    #[serde(default)] pub allowed_tags: Vec<String>,
    pub bypass_pin_on_local_network: bool,
    #[serde(default)] pub allowed_device_ids: Vec<String>,
    pub is_bonfire: bool,
    #[serde(default)] pub profile_image: String,
    pub master_user_id: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct SwitchResult {
    pub active_profile_token: String,
    pub jellyfin_user_id: String,
}

/// Every field but `profile_name` is genuinely optional per the plugin's own
/// docs ("conditional" / unmarked-required) — `skip_serializing_if` so an
/// unset field is omitted from the request body entirely rather than sent
/// as JSON `null`. This project has already been burned once by the
/// opposite choice: Seerr's `NOT NULL` columns 500ing on an explicit `null`
/// for a field the client had no real value for (see CLAUDE.md's Streaming
/// Region write-path note) — applying that same lesson here pre-emptively
/// rather than waiting to rediscover it against a second API.
#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct CreateProfileRequest {
    pub profile_name: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub avatar_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub max_parental_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub enabled_folders: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub blocked_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub allowed_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub lockout_minutes: Option<i64>,
    // Required only when the caller (a master profile) itself has a PIN set.
    #[serde(skip_serializing_if = "Option::is_none")] pub master_pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub bypass_pin_on_local_network: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub allowed_device_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub profile_image: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct CreateProfileResult {
    pub profile_user_id: String,
    pub profile_name: String,
}

/// Same shape as `CreateProfileRequest` plus the target `profile_id`, minus
/// nothing — kept as a distinct type rather than reusing `CreateProfileRequest`
/// with a bolted-on optional id, since the two requests are semantically
/// different (create vs. update) even though every other field lines up.
#[derive(Serialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct UpdateProfileRequest {
    pub profile_id: String,
    pub profile_name: String,
    #[serde(skip_serializing_if = "Option::is_none")] pub pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub avatar_color: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub max_parental_rating: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub enabled_folders: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub blocked_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub allowed_tags: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub lockout_minutes: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")] pub master_pin: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")] pub bypass_pin_on_local_network: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")] pub allowed_device_ids: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")] pub profile_image: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BonfireLibrary {
    pub id: String,
    pub name: String,
    pub collection_type: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BonfireDevice {
    pub device_id: String,
    pub device_name: String,
    pub client: String,
    pub last_seen: String,
    pub master_user_id: String,
}

/// Shared shape for both `bonfire/status`'s `ownedMembers` and
/// `bonfire/generate`'s `members` arrays.
#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BonfireGroupMember {
    pub user_id: String,
    pub username: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BonfireGroupStatus {
    pub is_owner: bool,
    #[serde(default)] pub owned_code: Option<String>,
    #[serde(default)] pub owned_members: Vec<BonfireGroupMember>,
    pub is_member: bool,
    #[serde(default)] pub joined_owner_name: Option<String>,
    #[serde(default)] pub joined_owner_id: Option<String>,
    pub hide_my_sub_profiles_from_others: bool,
    pub hide_others_sub_profiles_from_me: bool,
    pub allow_household_lan_bypass: bool,
    pub is_administrator: bool,
    pub has_pin: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BonfireGroupInfo {
    pub group_id: String,
    pub bonfire_code: String,
    #[serde(default)] pub members: Vec<BonfireGroupMember>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct BonfireJoinResult {
    pub message: String,
    pub owner_name: String,
}

/// switcherMode: "gate" (always show the profile picker) | "native" (defer
/// to Fjord's own launch-policy setting instead) — exact semantics per
/// mode not fully spelled out in the plugin's own docs; modeled as a plain
/// String rather than an enum so an unrecognized future value round-trips
/// instead of failing to deserialize.
#[derive(Deserialize, Serialize, Debug, Clone, Default)]
#[serde(rename_all = "PascalCase")]
pub struct BonfirePreferences {
    pub switcher_mode: String,
    #[serde(default)] pub master_user_id: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct AdminMasterUser {
    pub profile_user_id: String,
    pub profile_name: String,
    pub requires_pin: bool,
    pub max_profiles: i64,
    #[serde(default)] pub limit_override: Option<i64>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct AdminSubProfile {
    pub profile_user_id: String,
    pub profile_name: String,
    pub master_name: String,
    pub master_user_id: String,
    pub requires_pin: bool,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct AdminMappings {
    #[serde(default)] pub master_users: Vec<AdminMasterUser>,
    #[serde(default)] pub sub_profiles: Vec<AdminSubProfile>,
    pub injection_succeeded: bool,
    pub is_version_stale: bool,
    #[serde(default)] pub index_path: String,
    #[serde(default)] pub failure_reason: Option<String>,
    #[serde(default)] pub service_account: Option<String>,
    pub is_windows: bool,
    #[serde(default)] pub plugin_version: String,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct AuditLogEntry {
    pub timestamp: String,
    pub master_username: String,
    pub target_username: String,
    pub device_name: String,
    pub client: String,
    pub ip_address: String,
}
