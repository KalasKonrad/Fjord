// ── fjord-app · playback/stall.rs ─────────────────────────────────────────
//   next_stall_step/StallStep  shared reload-or-give-up step for a stall AND a failed open
//                           (PollResult::Failed) of a library item — same budget, same logs
//   STALL_SECS / FIRST_OPEN_STALL_SECS / MAX_STALL_RELOAD_ATTEMPTS_*  stall-recovery budget + toasts
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// Cap on stall-triggered stream reloads per item (see the stall-recovery
// check inside wire_mpv_timer) — past this, a still-broken connection stops
// retrying and playback stops cleanly instead of looping forever.
//
// Split into two budgets, 2026-08-28, per a real HTPC log investigation +
// a direct user follow-up question. The original single 2-attempt budget
// gives up after ~15s total ((2+1) reload cycles × the 5s stall threshold
// below) — measured exactly from a real incident where a library drive
// that had spun down took longer than that to wake and start serving
// reads, so every one of 3 straight attempts hit an identical dead stream
// (StartFile, then literally nothing — no FileLoaded event ever arrived)
// before the cap was hit and playback stopped with the "lost connection"
// toast; a manual retry moments later worked instantly once the drive had
// finished waking on its own, unrelated to anything a reload itself does
// differently. A reload doesn't actually help a slow-spin-up stall the way
// it helps a genuinely dropped connection (mpv's own read is blocked on
// the SERVER's disk I/O, not a broken client connection) — what actually
// fixes it is just enough elapsed time.
//
// Simply raising the cap for everyone would mean a GENUINELY dead
// connection also waits the full extended budget before being reported,
// which is a real cost when it's true network failure — the user's own
// direct follow-up ("is there not another way to detect a genuine
// connection issue... wuld it not hit that when the library wuld
// refreshe") pointed at exactly the right existing signal: the
// WebSocket's own already-continuously-running 30s keep-alive is a live,
// independent proof that the Jellyfin SERVER itself is reachable, whether
// or not any particular file's own stream is currently stuck (see
// FjordState.ws_connected/ws_last_keepalive_at's own doc comment). When
// that signal says the connection is healthy, a stall is far more likely
// to be a local/server-side resource being slow (the drive-wake case) than
// a real outage, so the LONG budget applies; when it's stale or the socket
// is down, the connection itself may genuinely be the problem, so the
// ORIGINAL short budget applies instead — a real outage is still reported
// in ~15s, not held up for 40, while a healthy-connection stall gets the
// patience an unrelated slow resource deserves.
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
