// ── fjord-api · models/system.rs ─────────────────────────────────────────────
//   SystemInfo  GET /System/Info/Public response (ServerName, Version)
// ─────────────────────────────────────────────────────────────────────────────
use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(rename_all = "PascalCase")]
pub struct SystemInfo {
    pub server_name: String,
    pub version:     String,
}
