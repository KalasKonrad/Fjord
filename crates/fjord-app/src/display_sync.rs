// ── fjord-app · display_sync.rs ─────────────────────────────────────────────
//   Native resolution/refresh-rate/HDR/WCG matching to source, replacing the
//   external `media_display_sync` Python script's job for Fjord's own
//   playback (2026-09-18). Ports that script's proven `kscreen-doctor`
//   mode-selection mechanism, not its detection mechanism — Fjord already
//   knows synchronously, from its own mpv instance, exactly what's playing
//   the instant VideoReconfig fires, so none of the script's own external
//   polling/timeout/grace-period machinery is needed. KDE Plasma Wayland
//   only; degrades to a silent no-op wherever `kscreen-doctor` isn't found
//   (X11, other Wayland compositors — Fjord ships `fjord-x11.desktop`).
//
//   HdrMode / WcgMode      parsed from Config.device.display_sync_hdr_mode/
//                          _wcg_mode ("yes"/"no"/"always", "auto"/"yes"/"no")
//   DisplaySyncSettings    everything compute_target_mode/sync_to_source need,
//                          extracted from DeviceConfig ONCE per trigger (not
//                          cloning the whole DeviceConfig every 16ms tick)
//   compute_target_mode    pure, unit-tested port of the proven script's own
//                          media_display_sync.py:202-237 fps->Hz cadence
//                          table + fallback chain (exact -> closest Hz at the
//                          same resolution -> the configured default mode)
//   get_supported_modes    kscreen-doctor -o output parse -> {(res, hz)} —
//                          plain string parsing, no regex dependency added
//                          for this one narrow, well-known CLI format
//   supported_resolutions_ derived, sorted Vecs over get_supported_modes'
//     and_hz                own set — backs the "Default resolution"/
//                          "Default refresh rate" Settings dropdowns
//                          (main.rs), fetched at startup and again whenever
//                          Output changes, replacing 2026-09-18's original
//                          fixed 3-resolution/7-Hz compile-time lists (a
//                          real dev-machine report: too few choices, and
//                          none of them guaranteed to be modes the actual
//                          display supports)
//   list_output_names      Settings-dropdown option list AND (via its own
//                          length at the call site) the one-shot "exactly
//                          one candidate" pre-fill check — never read at
//                          runtime by this module itself, only at startup
//                          (main.rs)
//   apply_display_mode/    thin kscreen-doctor wrappers — best-effort logged,
//   apply_display_color    a missing binary is a silent one-time-logged no-op
//   sync_to_source         the real per-item orchestration: get supported
//                          modes, compute target, apply mode+scale (+3s
//                          settle) and HDR/WCG only when they actually
//                          changed from FjordState's own "what's currently
//                          applied" tracking — called from wire_mpv_timer's
//                          own hook (playback.rs), which is also what
//                          sequences this to complete BEFORE HDR Stage 3's
//                          negotiation ever runs when both are enabled (see
//                          CLAUDE.md's dated section for the real race this
//                          avoids — kscreen-doctor's own HDR toggle and
//                          hdr.rs's Wayland surface negotiation are two
//                          different, both-heavyweight operations that must
//                          not fire concurrently)
//   revert_to_default      called from the 3 genuine-stop call sites
//                          (quit_cleanup, do_stop_playback, wire_mpv_timer's
//                          natural-EOF branch once nothing turns out to be
//                          next) — never from a replace-in-place teardown
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashSet;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use fjord_player::SourceHdrMetadata;

use crate::config::{DeviceConfig, FjordState};

const RES_4K: &str = "3840x2160";

/// Parsed from `Config.device.display_sync_hdr_mode`. No "manual" value —
/// `display_sync_enabled=false` already IS "never touch HDR/WCG".
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HdrMode {
    Yes,
    No,
    Always,
}

impl HdrMode {
    fn parse(s: &str) -> Self {
        match s {
            "no" => HdrMode::No,
            "always" => HdrMode::Always,
            _ => HdrMode::Yes, // "yes" and any unrecognized value
        }
    }
}

/// Parsed from `Config.device.display_sync_wcg_mode`.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum WcgMode {
    Auto,
    Yes,
    No,
}

impl WcgMode {
    fn parse(s: &str) -> Self {
        match s {
            "yes" => WcgMode::Yes,
            "no" => WcgMode::No,
            _ => WcgMode::Auto,
        }
    }
}

/// Everything `compute_target_mode`/`sync_to_source` need, extracted from
/// `DeviceConfig` once per trigger rather than cloning the whole (much
/// larger, mostly-unrelated) struct on every 16ms tick.
#[derive(Clone)]
pub(crate) struct DisplaySyncSettings {
    pub screen_name: String,
    pub default_resolution: String,
    pub default_hz: String,
    pub scale_4k: String,
    pub scale_1080p: String,
    pub sync_resolution: bool,
    pub sync_refresh_rate: bool,
    /// true = "stay_4k" (pick the closest supported Hz at 4K for an unusual
    /// framerate instead of dropping resolution), false = "fallback"
    /// (default — matches the proven script exactly).
    pub odd_fps_stay_4k: bool,
    pub hdr_mode: HdrMode,
    pub wcg_mode: WcgMode,
}

impl DisplaySyncSettings {
    pub(crate) fn from_device_config(c: &DeviceConfig) -> Self {
        Self {
            screen_name: c.display_sync_screen_name.clone(),
            default_resolution: c.display_sync_default_resolution.clone(),
            default_hz: c.display_sync_default_hz.clone(),
            scale_4k: c.display_sync_scale_4k.clone(),
            scale_1080p: c.display_sync_scale_1080p.clone(),
            sync_resolution: c.display_sync_sync_resolution,
            sync_refresh_rate: c.display_sync_sync_refresh_rate,
            odd_fps_stay_4k: c.display_sync_4k_odd_fps_mode == "stay_4k",
            hdr_mode: HdrMode::parse(&c.display_sync_hdr_mode),
            wcg_mode: WcgMode::parse(&c.display_sync_wcg_mode),
        }
    }

    fn scale_for(&self, resolution: &str) -> String {
        if resolution.starts_with("3840") {
            self.scale_4k.clone()
        } else {
            self.scale_1080p.clone()
        }
    }
}

// ── mode selection (pure, unit-tested) ──────────────────────────────────────

/// Direct port of the proven external script's own fps->Hz cadence table +
/// fallback chain (`media_display_sync.py:202-237`, `display_modes.py`),
/// parameterized by Fjord's own Settings fields instead of the script's
/// hardcoded behavior. `fps` is mpv's raw `estimated-vf-fps` reading —
/// rounded once, here, matching the proven script's own `round(fps)` before
/// its cadence match.
pub(crate) fn compute_target_mode(
    width: i64,
    fps: f64,
    cfg: &DisplaySyncSettings,
    supported: &HashSet<(String, String)>,
) -> (String, String) {
    let fps_r = fps.round() as i64;
    let is_4k = cfg.sync_resolution && width >= 3840;

    let (mut target_res, mut target_hz) = if is_4k {
        match fps_r {
            23 | 24 => (RES_4K.to_string(), "23.98".to_string()),
            25 => (RES_4K.to_string(), "25.00".to_string()),
            29 | 30 => (RES_4K.to_string(), "29.97".to_string()),
            _ if cfg.odd_fps_stay_4k => {
                // Stay at 4K — the fallback chain below picks the closest
                // supported Hz to the real content fps at this resolution
                // (this exact value is virtually never itself a supported
                // mode, which is what deliberately triggers that chain).
                (RES_4K.to_string(), format!("{fps:.2}"))
            }
            _ => (cfg.default_resolution.clone(), "59.94".to_string()),
        }
    } else {
        let hz = match fps_r {
            23 | 24 => "23.98",
            25 | 50 => "50.00",
            29 | 30 => "29.97",
            59 | 60 => "59.94",
            _ => cfg.default_hz.as_str(),
        };
        (cfg.default_resolution.clone(), hz.to_string())
    };

    if !cfg.sync_refresh_rate {
        target_hz = cfg.default_hz.clone();
    }
    if !cfg.sync_resolution {
        target_res = cfg.default_resolution.clone();
    }

    // Fallback chain: exact mode -> closest Hz at the same resolution ->
    // the configured default mode entirely. Skipped when `supported` is
    // empty (couldn't query modes at all — don't second-guess a computed
    // target against no information).
    if !supported.is_empty() && !supported.contains(&(target_res.clone(), target_hz.clone())) {
        let target_f = target_hz.parse::<f64>().unwrap_or(0.0);
        let same_res: Vec<&(String, String)> =
            supported.iter().filter(|(r, _)| *r == target_res).collect();
        if same_res.is_empty() {
            target_res = cfg.default_resolution.clone();
            target_hz = cfg.default_hz.clone();
        } else {
            // Safe to unwrap: same_res is non-empty in this branch.
            let closest = same_res
                .iter()
                .min_by(|a, b| {
                    let da = (a.1.parse::<f64>().unwrap_or(f64::MAX) - target_f).abs();
                    let db = (b.1.parse::<f64>().unwrap_or(f64::MAX) - target_f).abs();
                    da.partial_cmp(&db).unwrap_or(std::cmp::Ordering::Equal)
                })
                .unwrap();
            target_hz = closest.1.clone();
        }
    }

    (target_res, target_hz)
}

// ── kscreen-doctor plumbing ──────────────────────────────────────────────────

/// Strips SGR ANSI color codes (`\x1b[...m`) from `kscreen-doctor`'s own
/// colorized output — a plain state-machine walk rather than a `regex`
/// dependency, since this tool's output only ever uses this one escape
/// shape (confirmed live on this dev machine).
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            for c2 in chars.by_ref() {
                if c2 == 'm' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

static WARNED_MISSING_BINARY: AtomicBool = AtomicBool::new(false);

fn run_kscreen(args: &[String]) {
    match Command::new("kscreen-doctor").args(args).output() {
        Ok(o) if o.status.success() => {}
        Ok(o) => tracing::warn!(
            "display_sync: kscreen-doctor {:?} failed: {}",
            args,
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !WARNED_MISSING_BINARY.swap(true, Ordering::Relaxed) {
                tracing::info!(
                    "display_sync: kscreen-doctor not found (not on KDE Plasma Wayland) — \
                     display sync will silently no-op for the rest of this session"
                );
            }
        }
        Err(e) => tracing::warn!("display_sync: could not run kscreen-doctor: {e}"),
    }
}

fn kscreen_doctor_o() -> Option<String> {
    match Command::new("kscreen-doctor").arg("-o").output() {
        Ok(o) if o.status.success() => Some(strip_ansi(&String::from_utf8_lossy(&o.stdout))),
        Ok(o) => {
            tracing::warn!(
                "display_sync: kscreen-doctor -o failed: {}",
                String::from_utf8_lossy(&o.stderr)
            );
            None
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            if !WARNED_MISSING_BINARY.swap(true, Ordering::Relaxed) {
                tracing::info!(
                    "display_sync: kscreen-doctor not found (not on KDE Plasma Wayland) — \
                     display sync will silently no-op for the rest of this session"
                );
            }
            None
        }
        Err(e) => {
            tracing::warn!("display_sync: could not run kscreen-doctor -o: {e}");
            None
        }
    }
}

/// `kscreen-doctor -o` output parse -> the set of `(resolution, hz)` modes
/// the named output supports. `hz` stays in the real fractional form
/// (`"23.98"`) matching the tool's own display — a *separate*, integer-
/// rounded form is only ever needed for the literal `mode.<res>@<hz>`
/// argument (`apply_display_mode`), never for matching/logging. `pub(crate)`
/// (not just used internally by `compute_target_mode`'s own fallback chain)
/// since `supported_resolutions_and_hz` below is a thin derived view over
/// this same parse, not a second one.
pub(crate) fn get_supported_modes(screen: &str) -> HashSet<(String, String)> {
    let Some(output) = kscreen_doctor_o() else {
        return HashSet::new();
    };
    let mut modes = HashSet::new();
    let mut in_section = false;
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Output:") {
            in_section = rest.split_whitespace().any(|w| w == screen);
        } else if in_section {
            if let Some(rest) = trimmed.strip_prefix("Modes:") {
                for tok in rest.split_whitespace() {
                    // token shape: "N:WxH@HZ[*][!]"
                    let Some((_, after_colon)) = tok.split_once(':') else { continue };
                    let Some((res, hz_raw)) = after_colon.split_once('@') else { continue };
                    let hz: String =
                        hz_raw.chars().filter(|c| c.is_ascii_digit() || *c == '.').collect();
                    if !res.is_empty() && !hz.is_empty() {
                        modes.insert((res.to_string(), hz));
                    }
                }
            }
        }
    }
    modes
}

/// Distinct resolutions and Hz values `screen` genuinely supports, each in a
/// sensible dropdown order — resolutions by pixel count descending (largest,
/// most likely intentional choice first), Hz ascending. Backs the Settings
/// screen's "Default resolution"/"Default refresh rate" dynamic dropdowns
/// (`main.rs`, same shape as the Output row's own `list_output_names` fetch)
/// — a plain derived view over `get_supported_modes`'s own already-parsed
/// set, not a second `kscreen-doctor` shell-out. Both empty when the query
/// itself failed (missing binary, unknown output name) — callers leave
/// whatever the dropdown already showed untouched in that case, the same
/// "don't clear a working value over a transient/absent query" precedent
/// `list_output_names`'s own screen-name pre-fill already follows.
pub(crate) fn supported_resolutions_and_hz(screen: &str) -> (Vec<String>, Vec<String>) {
    let modes = get_supported_modes(screen);
    let mut resolutions: Vec<String> =
        modes.iter().map(|(res, _)| res.clone()).collect::<HashSet<_>>().into_iter().collect();
    resolutions.sort_by_key(|res| std::cmp::Reverse(pixel_count(res)));
    let mut hz: Vec<String> =
        modes.iter().map(|(_, hz)| hz.clone()).collect::<HashSet<_>>().into_iter().collect();
    hz.sort_by(|a, b| {
        a.parse::<f64>()
            .unwrap_or(0.0)
            .partial_cmp(&b.parse::<f64>().unwrap_or(0.0))
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    (resolutions, hz)
}

fn pixel_count(res: &str) -> u64 {
    res.split_once('x')
        .and_then(|(w, h)| Some((w.parse::<u64>().ok()?, h.parse::<u64>().ok()?)))
        .map(|(w, h)| w * h)
        .unwrap_or(0)
}

/// Every output name currently reported both `enabled` and `connected` by
/// `kscreen-doctor -o`, in the order it lists them. Used by
/// `list_output_names` below for both the Settings dropdown's full option
/// list AND (by checking the returned `Vec`'s own length at the call site,
/// `main.rs`'s startup fetch) the one-shot "exactly one candidate" pre-fill
/// check — deliberately one shell-out serving both purposes rather than two.
fn enabled_connected_outputs() -> Vec<String> {
    let Some(output) = kscreen_doctor_o() else { return Vec::new() };
    let mut candidates = Vec::new();
    let mut current: Option<String> = None;
    let (mut enabled, mut connected) = (false, false);
    let flush = |current: &mut Option<String>, enabled: bool, connected: bool, out: &mut Vec<String>| {
        if let (Some(name), true, true) = (current.take(), enabled, connected) {
            out.push(name);
        }
    };
    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(rest) = trimmed.strip_prefix("Output:") {
            flush(&mut current, enabled, connected, &mut candidates);
            current = rest.split_whitespace().nth(1).map(str::to_string);
            enabled = false;
            connected = false;
        } else if trimmed == "enabled" {
            enabled = true;
        } else if trimmed == "connected" {
            connected = true;
        }
    }
    flush(&mut current, enabled, connected, &mut candidates);
    candidates
}

/// Settings-dropdown option list for `display_sync_screen_name` — every
/// currently enabled+connected output. Read only at app startup (`main.rs`),
/// never at runtime by `display_sync.rs` itself (see
/// `DeviceConfig.display_sync_screen_name`'s own doc comment for why
/// re-detecting on every playback would be wrong the moment a second output
/// exists) — the caller also uses this same list's length to decide whether
/// to auto-pre-fill an still-empty stored value (exactly one candidate) or
/// leave it for the user to pick explicitly (zero or 2+ candidates).
pub(crate) fn list_output_names() -> Vec<String> {
    enabled_connected_outputs()
}

fn apply_display_mode(screen: &str, resolution: &str, hz_frac: &str, scale: &str) {
    let hz_int = hz_frac.parse::<f64>().map(|f| f.round() as i64).unwrap_or(60);
    tracing::info!("display_sync: setting display mode: {resolution}@{hz_int} on {screen}");
    run_kscreen(&[format!("output.{screen}.mode.{resolution}@{hz_int}")]);
    run_kscreen(&[format!("output.{screen}.scale.{scale}")]);
}

fn apply_display_color(screen: &str, hdr: bool, wcg: bool) {
    tracing::info!(
        "display_sync: setting HDR {} / WCG {} on {screen}",
        if hdr { "on" } else { "off" },
        if wcg { "on" } else { "off" }
    );
    run_kscreen(&[format!("output.{screen}.hdr.{}", if hdr { "enable" } else { "disable" })]);
    run_kscreen(&[format!("output.{screen}.wcg.{}", if wcg { "enable" } else { "disable" })]);
}

// ── orchestration ────────────────────────────────────────────────────────────

/// The real per-item orchestration — called once per playback item, from
/// `wire_mpv_timer`'s own trigger, only after that trigger has already
/// claimed both one-shot flags synchronously (see the trigger's own doc
/// comment in `playback.rs` for the re-entry bug this ordering avoids).
/// Only ever touches the physical output when something actually needs to
/// change, tracked via `FjordState.display_sync_current_mode`/`_current_hdr`
/// — a same-mode item (e.g. back-to-back episodes) is a cheap no-op.
pub(crate) async fn sync_to_source(
    state: Arc<Mutex<FjordState>>,
    dims: (i64, i64, f64),
    meta: SourceHdrMetadata,
    cfg: DisplaySyncSettings,
) {
    if cfg.screen_name.is_empty() {
        tracing::warn!("display_sync: enabled but no output configured — skipping");
        return;
    }
    let (width, _height, fps) = dims;
    let screen = cfg.screen_name.clone();

    let screen_for_modes = screen.clone();
    let supported = tokio::task::spawn_blocking(move || get_supported_modes(&screen_for_modes))
        .await
        .unwrap_or_default();

    let (target_res, target_hz) = compute_target_mode(width, fps, &cfg, &supported);

    let is_hdr_content = meta.gamma == "pq" || meta.gamma == "hlg";
    let want_hdr = match cfg.hdr_mode {
        HdrMode::Always => true,
        HdrMode::No => false,
        HdrMode::Yes => is_hdr_content,
    };
    let want_wcg = match cfg.wcg_mode {
        WcgMode::Yes => true,
        WcgMode::No => false,
        WcgMode::Auto => want_hdr,
    };

    let (mode_changed, hdr_changed) = {
        let s = state.lock().unwrap();
        let mode_changed =
            s.display_sync_current_mode.as_ref() != Some(&(target_res.clone(), target_hz.clone()));
        let hdr_changed = s.display_sync_current_hdr != Some(want_hdr);
        (mode_changed, hdr_changed)
    };

    if mode_changed {
        let scale = cfg.scale_for(&target_res);
        let (screen2, res2, hz2, scale2) = (screen.clone(), target_res.clone(), target_hz.clone(), scale);
        tokio::task::spawn_blocking(move || apply_display_mode(&screen2, &res2, &hz2, &scale2))
            .await
            .ok();
        // "Allow HDMI link to renegotiate" — the proven script's own
        // load-bearing settle delay after every real mode-set.
        tokio::time::sleep(Duration::from_secs(3)).await;
        state.lock().unwrap().display_sync_current_mode = Some((target_res, target_hz));
    }

    // HDR/WCG are only ever re-applied together, only when HDR's own
    // effective value changed — matching the proven script's own behavior
    // exactly (it never tracks WCG independently, and never re-asserts
    // color state on a bare mode switch that didn't also change HDR).
    if hdr_changed {
        let screen3 = screen.clone();
        tokio::task::spawn_blocking(move || apply_display_color(&screen3, want_hdr, want_wcg))
            .await
            .ok();
        state.lock().unwrap().display_sync_current_hdr = Some(want_hdr);
    }
}

/// Called from the 3 genuine-stop call sites (`quit_cleanup`,
/// `do_stop_playback`, `wire_mpv_timer`'s natural-EOF branch once its own
/// deferred check confirms nothing started next) — never from a
/// replace-in-place teardown, which always proceeds straight into a new
/// item's own `sync_to_source` instead. A no-op whenever the feature is off
/// or nothing was ever actually applied this session (the common case for
/// most stops — e.g. quitting without having played anything since launch).
pub(crate) async fn revert_to_default(state: Arc<Mutex<FjordState>>) {
    let (screen, default_res, default_hz, needs_revert) = {
        let s = state.lock().unwrap();
        if !s.config.device.display_sync_enabled {
            return;
        }
        let needs_revert =
            s.display_sync_current_mode.is_some() || s.display_sync_current_hdr == Some(true);
        (
            s.config.device.display_sync_screen_name.clone(),
            s.config.device.display_sync_default_resolution.clone(),
            s.config.device.display_sync_default_hz.clone(),
            needs_revert,
        )
    };
    if !needs_revert || screen.is_empty() {
        return;
    }

    let scale = if default_res.starts_with("3840") {
        state.lock().unwrap().config.device.display_sync_scale_4k.clone()
    } else {
        state.lock().unwrap().config.device.display_sync_scale_1080p.clone()
    };
    let (screen2, res2, hz2, scale2) = (screen.clone(), default_res.clone(), default_hz.clone(), scale);
    tokio::task::spawn_blocking(move || apply_display_mode(&screen2, &res2, &hz2, &scale2))
        .await
        .ok();
    let screen3 = screen.clone();
    tokio::task::spawn_blocking(move || apply_display_color(&screen3, false, false))
        .await
        .ok();

    let mut s = state.lock().unwrap();
    // The display genuinely IS at the default mode/HDR-off now — recording
    // that (rather than resetting to None) means the next default-mode item
    // correctly skips a redundant re-apply too.
    s.display_sync_current_mode = Some((default_res, default_hz));
    s.display_sync_current_hdr = Some(false);
}

// ── tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> DisplaySyncSettings {
        DisplaySyncSettings {
            screen_name: "HDMI-A-1".into(),
            default_resolution: "1920x1080".into(),
            default_hz: "59.94".into(),
            scale_4k: "1.0".into(),
            scale_1080p: "1.0".into(),
            sync_resolution: true,
            sync_refresh_rate: true,
            odd_fps_stay_4k: false,
            hdr_mode: HdrMode::Yes,
            wcg_mode: WcgMode::Auto,
        }
    }

    fn no_supported() -> HashSet<(String, String)> {
        HashSet::new()
    }

    #[test]
    fn four_k_cadence_matches() {
        let cfg = settings();
        assert_eq!(
            compute_target_mode(3840, 23.976, &cfg, &no_supported()),
            (RES_4K.to_string(), "23.98".to_string())
        );
        assert_eq!(
            compute_target_mode(3840, 24.0, &cfg, &no_supported()),
            (RES_4K.to_string(), "23.98".to_string())
        );
        assert_eq!(
            compute_target_mode(3840, 25.0, &cfg, &no_supported()),
            (RES_4K.to_string(), "25.00".to_string())
        );
        assert_eq!(
            compute_target_mode(3840, 29.97, &cfg, &no_supported()),
            (RES_4K.to_string(), "29.97".to_string())
        );
    }

    #[test]
    fn four_k_odd_fps_falls_back_by_default() {
        let cfg = settings();
        assert_eq!(
            compute_target_mode(3840, 48.0, &cfg, &no_supported()),
            ("1920x1080".to_string(), "59.94".to_string())
        );
    }

    #[test]
    fn four_k_odd_fps_stays_4k_when_configured() {
        let mut cfg = settings();
        cfg.odd_fps_stay_4k = true;
        let supported: HashSet<(String, String)> =
            [(RES_4K.to_string(), "48.00".to_string()), (RES_4K.to_string(), "60.00".to_string())]
                .into_iter()
                .collect();
        assert_eq!(
            compute_target_mode(3840, 48.0, &cfg, &supported),
            (RES_4K.to_string(), "48.00".to_string())
        );
    }

    #[test]
    fn non_4k_cadence_matches() {
        let cfg = settings();
        assert_eq!(
            compute_target_mode(1920, 23.976, &cfg, &no_supported()),
            ("1920x1080".to_string(), "23.98".to_string())
        );
        assert_eq!(
            compute_target_mode(1920, 25.0, &cfg, &no_supported()),
            ("1920x1080".to_string(), "50.00".to_string())
        );
        assert_eq!(
            compute_target_mode(1920, 50.0, &cfg, &no_supported()),
            ("1920x1080".to_string(), "50.00".to_string())
        );
        assert_eq!(
            compute_target_mode(1920, 60.0, &cfg, &no_supported()),
            ("1920x1080".to_string(), "59.94".to_string())
        );
        // Unusual, non-cadence-matched fps — falls back to the configured
        // default Hz, resolution unaffected (this isn't the 4K odd-fps case).
        assert_eq!(
            compute_target_mode(1920, 48.0, &cfg, &no_supported()),
            ("1920x1080".to_string(), "59.94".to_string())
        );
    }

    #[test]
    fn sync_resolution_disabled_always_pins_default() {
        let mut cfg = settings();
        cfg.sync_resolution = false;
        // Real 4K content — would normally switch to 3840x2160, but pinned
        // to the configured default instead; Hz still varies by cadence.
        assert_eq!(
            compute_target_mode(3840, 23.976, &cfg, &no_supported()),
            ("1920x1080".to_string(), "23.98".to_string())
        );
    }

    #[test]
    fn sync_refresh_rate_disabled_always_pins_default_hz() {
        let mut cfg = settings();
        cfg.sync_refresh_rate = false;
        assert_eq!(
            compute_target_mode(3840, 23.976, &cfg, &no_supported()),
            (RES_4K.to_string(), "59.94".to_string())
        );
    }

    #[test]
    fn fallback_chain_closest_hz_at_same_resolution() {
        let cfg = settings();
        let supported: HashSet<(String, String)> = [
            (RES_4K.to_string(), "24.00".to_string()),
            (RES_4K.to_string(), "60.00".to_string()),
        ]
        .into_iter()
        .collect();
        // Wants 23.98 (not present); closest at the same resolution is 24.00.
        assert_eq!(
            compute_target_mode(3840, 23.976, &cfg, &supported),
            (RES_4K.to_string(), "24.00".to_string())
        );
    }

    #[test]
    fn fallback_chain_no_mode_at_resolution_uses_default() {
        let cfg = settings();
        let supported: HashSet<(String, String)> =
            [("1280x720".to_string(), "60.00".to_string())].into_iter().collect();
        assert_eq!(
            compute_target_mode(3840, 23.976, &cfg, &supported),
            ("1920x1080".to_string(), "59.94".to_string())
        );
    }

    #[test]
    fn strip_ansi_removes_sgr_codes() {
        assert_eq!(strip_ansi("\u{1b}[01;32mOutput:\u{1b}[0;0m 1 HDMI-A-2"), "Output: 1 HDMI-A-2");
        assert_eq!(strip_ansi("plain text, no codes"), "plain text, no codes");
    }
}
