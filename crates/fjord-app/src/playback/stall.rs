// ── fjord-app · playback/stall.rs ─────────────────────────────────────────
//   next_stall_step/StallStep  shared reload-or-give-up step for a stall AND a failed open
//                           (PollResult::Failed) of a library item — same budget, same logs
//   STALL_SECS / FIRST_OPEN_STALL_SECS / MAX_STALL_RELOAD_ATTEMPTS_*  stall-recovery budget + toasts
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// Cap on stall-triggered reloads per item (wire_mpv_timer's stall check); past it,
// playback stops cleanly instead of looping. Two budgets, chosen by connection health
// (FjordState.ws_connected / ws_last_keepalive_at — the WebSocket's 30 s keep-alive
// proves the server is reachable whatever one stream is doing):
// - healthy → long budget (~40 s): the stall is most likely a slow server resource,
//   e.g. a library drive waking up — a reload doesn't help, time does;
// - unknown/down → the original short budget (~15 s): a real outage is reported fast.
// Background: DEVLOG → "Playback resilience".
const MAX_STALL_RELOAD_ATTEMPTS_HEALTHY: u32 = 7; // (7+1) × 5s ≈ 40s
const MAX_STALL_RELOAD_ATTEMPTS_UNHEALTHY: u32 = 2; // (2+1) × 5s = 15s, the original budget
pub(crate) const STALL_GIVE_UP_TOAST: &str = "Playback stopped — lost connection to server";
/// Seconds without progress before a reload, normally.
pub(crate) const STALL_SECS: f64 = 5.0;
/// The same for an item's FIRST open while mpv hasn't reached FileLoaded yet
/// (2026-10-08): the server's media disks spin down, and waking them makes
/// the first open hang 5–12 s — a reload at 5 s just restarted a request
/// that was about to succeed. Reloads after that use STALL_SECS again, so a
/// server that's really down still gives up about as fast as before.
pub(crate) const FIRST_OPEN_STALL_SECS: f64 = 15.0;
pub(crate) const FAILED_OPEN_TOAST: &str = "Couldn't play this — the file wouldn't open";
pub(crate) const TRAILER_FAILED_TOAST: &str = "Trailer unavailable";

/// One step of stall recovery for the current item, shared by the stall
/// watchdog and a failed open (PollResult::Failed, 2026-10-04): reload while
/// the item still has attempts left (recorded here), give up past the cap.
/// `NotReloadable` = no Jellyfin item to reload (a trailer).
pub(crate) enum StallStep {
    Reload(QueueItem, Arc<JellyfinClient>, f64),
    GiveUp,
    NotReloadable,
}

pub(crate) fn next_stall_step(
    vs: &mut VideoState,
    connection_likely_healthy: bool,
    why: &str,
) -> StallStep {
    let max_attempts = if connection_likely_healthy {
        MAX_STALL_RELOAD_ATTEMPTS_HEALTHY
    } else {
        MAX_STALL_RELOAD_ATTEMPTS_UNHEALTHY
    };
    let attempts = match &vs.stall_reload_attempts_for {
        Some((id, n)) if vs.item_id.as_deref() == Some(id.as_str()) => *n,
        _ => 0,
    };
    if attempts >= max_attempts {
        warn!(
            "{why} — giving up after {max_attempts} reload attempt(s) (connection_likely_healthy={connection_likely_healthy})"
        );
        vs.stall_last_reload_at = Some(Instant::now());
        return StallStep::GiveUp;
    }
    let (Some(item_id), Some(cli), Some(np)) = (
        vs.item_id.clone(),
        vs.client.clone(),
        vs.now_playing.clone(),
    ) else {
        return StallStep::NotReloadable;
    };
    warn!(
        "{why} — reloading stream (attempt {}/{max_attempts}, connection_likely_healthy={connection_likely_healthy})",
        attempts + 1
    );
    vs.stall_reload_attempts_for = Some((item_id, attempts + 1));
    vs.stall_last_reload_at = Some(Instant::now());
    // pos itself can be unreliable right as a stream breaks (mpv's own
    // position readout can reset toward 0 — the same symptom the duration
    // guard on natural end exists to catch) — resume from the last position
    // we know was read while things were genuinely working.
    let resume_secs = (vs.last_known_pos_ticks as f64) / 10_000_000.0;
    StallStep::Reload(np, cli, resume_secs)
}
