// ── fjord-api · models/plugins.rs ────────────────────────────────────────────
//   PluginInfo  GET /Plugins response entry (Name, Id, Status) — Bonfire Phase 1,
//               2026-08-09: shared plugin-availability registry (Bonfire gate
//               fast-path, Intro Skipper presence check). Field shape best-effort
//               from Jellyfin's documented API — not yet live-verified against a
//               real server response, unlike most other API surfaces this project
//               checks directly; only Name/Id/Status are modeled since that's all
//               FjordState.available_plugins actually needs.
// ─────────────────────────────────────────────────────────────────────────────
use serde::Deserialize;

#[derive(Deserialize, Debug, Clone)]
#[serde(rename_all = "PascalCase")]
pub struct PluginInfo {
    pub name:   String,
    pub id:     String,
    #[serde(default)]
    pub status: String,
}
