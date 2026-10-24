// ── fjord-app · playback/mod.rs ─────────────────────────────────────────────
//   Submodules (re-exported here, so callers keep using crate::playback::*):
//     queue    playlist/queue stepping, repeat/shuffle, resolve_true_next_episode (+ its tests)
//     inhibit  screensaver / KDE power / systemd sleep inhibitors
//     tracks   track list model, time formatting, apply_audio_track
//     render   wire_rendering_notifier + FBO helpers (GL thread)
//     stall    stall-recovery budget + next_stall_step
//     start    start_playback, one function per step
//     timer    wire_mpv_timer (16 ms tick)
//   SkipFadeAudio / arm_skip_fade  the skip-segment fade (PCM volume ramp or passthrough mute)
//   VideoState              mpv Player + MpvRenderCtx, FBOs, playback metadata: playlist/queue/
//                           shuffle/repeat, now_playing, current_is_audio (natural-end advance stays
//                           within a media class), from_detail/-series/-season (screen restored on
//                           stop), playback_generation (stale-result guard), skip-segment state
//                           (incl. the ask-timed pause freeze), credits/Up Next state
//                           (credits_auto_marked_played + credits_mark_threshold — a rewind before
//                           the threshold reverts the mark and hides the banner), next_ep_pending,
//                           chapters + OSD countdowns, stall-recovery fields, is_trailer/trailer_url,
//                           HDR/display-sync one-shot flags, video_on_subsurface
//   tear_down_player        capture ticks, drop render_ctx then player (mpv invariant; a subsurface
//                           player's context is freed on our own GL context), return the stop data;
//                           reports 0 ticks while credits_auto_marked_played so the stop report
//                           can't re-add a resume point
//   quit_cleanup            synchronous stop report + screensaver release after window.run() exits
//   reset_playback_ui       clears all player UI state (video-surface-active first, so the window is
//                           opaque before the next frame; music bar, Now Playing, buffering, overlays)
//   do_stop_playback        user stop: tear down, keep playlist + queue (idle queue panel), reset the
//                           UI, stop report, home refresh
//   reset_video_state_for_playback  the shared "fresh playback" reset for start_playback/play_trailer
//   start_playback (start.rs) open an item: one function per step — see start.rs's header
//   TrailerSource           YouTube (a server-provided link) | Server (a stream URL Fjord builds)
//   play_trailer            Watch Trailer (a YouTube source must pass trailer_url_allowed); no
//                           Jellyfin client/item, so no reporting, auto-advance or played/resume
//                           changes; display sync only when device.display_sync_trailers; a failed
//                           open closes with "Trailer unavailable"
//   start_trailer / TRAILER_OPEN_RETRY_SECS  play_trailer's body; a YouTube first attempt keeps
//                           vs.trailer_retry (title + config) for wire_mpv_timer's one restart (7 s)
// ─────────────────────────────────────────────────────────────────────────────
use std::num::NonZeroU32;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Local;

use fjord_api::{
    JellyfinClient,
    models::{MediaItem, Segment},
};
use fjord_player::{MpvRenderCtx, Player, PlayerConfig, PollResult, TrackInfo};
use slint::platform::WindowEvent;
use slint::{ComponentHandle, Global, LogicalPosition, ModelRc, SharedString, VecModel};
use tracing::{debug, error, info, warn};

use crate::AppState;
use crate::MainWindow;
use crate::TrackEntry;
use crate::config::FjordState;
use crate::home::{fetch_home_data, home_data_sections, push_home_data};
use crate::poster::spawn_poster_loading;
use crate::stats::update_stats_window;

fn ss(s: &str) -> SharedString {
    SharedString::from(s)
}

mod inhibit;
mod queue;
mod render;
mod stall;
mod start;
mod timer;
mod tracks;

pub(crate) use inhibit::*;
pub(crate) use queue::*;
pub(crate) use render::*;
pub(crate) use stall::*;
pub(crate) use start::*;
pub(crate) use timer::*;
pub(crate) use tracks::*;

// ── SkipFadeAudio ────────────────────────────────────────────────────────────
// The audio side of `pending_skip_seek`'s fade-to-black, spanning BOTH halves (fade-out
// before the seek and fade-in after it — `pending_skip_seek` is cleared at the seek).
// Armed by `arm_skip_fade` at the same three sites; processed every tick in
// `wire_mpv_timer` with that tick's fade duration, so audio and video stay in step.
// The mechanism is chosen once at arm time:
//   - PCM: a volume ramp, ORIG_VOLUME → 0 → ORIG_VOLUME, mirroring the picture.
//   - SPDIF passthrough: a raw bitstream can't be volume-ramped (it corrupts the
//     frames — see Player::set_volume), so mute for the whole transition: the receiver
//     sees silence-then-resume, not a content splice. Not verified on real AVR
//     hardware. Off via Settings → Audio → Passthrough → "Mute during skip fade"
//     (`skip_fade_mute_passthrough`): then this is never armed; the video fade still runs.
#[derive(Clone, Copy)]
struct SkipFadeAudio {
    armed_at: Instant,
    passthrough: bool,
    orig_volume: f64, // meaningful only when !passthrough
}

/// Arms both halves of the skip-segment fade (video via `pending_skip_seek`,
/// audio via `skip_fade_audio`) from a single call site, so the three
/// places that trigger a skip (always-skip, ask-timed countdown expiry,
/// manual "Skip" confirm) can't drift out of sync with each other on what
/// each one arms. `g` must reflect the CURRENT window (some call sites
/// already have it in scope; `on_skip_segment` fetches it before locking
/// `vs`, same ordering, no functional difference).
pub(crate) fn arm_skip_fade(vs: &mut VideoState, g: &crate::AppState<'_>, seg_end: f64) {
    let armed_at = Instant::now();
    vs.pending_skip_seek = Some((seg_end, armed_at));
    let passthrough = g.get_audio_passthrough_active();
    // "Mute during skip fade" off + passthrough: nothing for the audio fade to do (no
    // ramp on a bitstream, mute declined), so it isn't armed — the video fade above is
    // armed regardless, and PCM never reaches this check.
    if passthrough && !g.get_settings_skip_fade_mute_passthrough() {
        vs.skip_fade_audio = None;
        return;
    }
    let orig_volume = vs.player.as_ref().map(|p| p.get_volume()).unwrap_or(100.0);
    vs.skip_fade_audio = Some(SkipFadeAudio {
        armed_at,
        passthrough,
        orig_volume,
    });
}

// ── VideoState ────────────────────────────────────────────────────────────────
pub(crate) struct VideoState {
    pub player: Option<Player>,
    // Set by reset_video_state_for_playback whenever a fresh Player is
    // stored, alongside it — the actual mpv `loadfile` for this URL hasn't
    // been issued yet (Player::new() only builds the mpv core, deliberately
    // no longer loading anything itself). wire_rendering_notifier's
    // BeforeRendering handler consumes (takes) this the moment it creates
    // a render context for the current `player`, and only then calls
    // Player::load() — see Player::load()'s own doc comment for the real
    // HTPC race (audio-only-forever, black screen) this two-step split
    // fixes at the root. `None` means either nothing is pending or it was
    // already consumed; a superseding start_playback/play_trailer call
    // simply overwrites it in lockstep with `player` itself, so a rapid
    // double-start can never leave a stale URL pointed at an already-torn-
    // down player (tear_down_player drops `player`/`render_ctx` first).
    pub pending_load_url: Option<String>,
    pub render_ctx: Option<MpvRenderCtx>,
    pub fbos: [u32; 2],
    pub textures: [u32; 2],
    pub fbo_w: u32,
    pub fbo_h: u32,
    pub back: usize,
    pub item_id: Option<String>,
    pub playing_series_id: Option<String>,
    pub client: Option<Arc<JellyfinClient>>,
    pub play_start: Option<Instant>,
    pub decoder_logged: bool,
    // One-shot guard for the video-never-initialized diagnostic (2026-07-29,
    // see Player::has_seen_video_reconfig's doc comment) — fires once at 5s,
    // independent of decoder_logged's own 2s snapshot, since normal 4K HEVC
    // hwdec startup can itself take close to 2s and would false-positive here.
    pub video_init_checked: bool,
    // One-shot guard for hdr.rs's Wayland color-management negotiation
    // (hdr branch, Stage 3) — fires as soon as Player::video_info() is ready (the
    // first VideoReconfig + mpv's report of the format), no artificial delay (unlike
    // video_init_checked above, which is watching for an absence).
    pub hdr_negotiation_attempted: bool,
    // Whether THIS item's FBO uses GL_RGB10_A2 instead of 8-bit RGBA. Set once in
    // reset_video_state_for_playback from the "HDR passthrough" setting, not from the
    // source (create_fbo runs before mpv has decoded anything). See create_fbo.
    pub wide_color_fbo: bool,
    // One-shot guard, paired with hdr_negotiation_attempted above but for
    // the OTHER half: has apply_hdr_output() already been called for this
    // item? Set the moment wire_mpv_timer's poll observes hdr::is_active()
    // for the first time this item. Without this, resetting it to false at
    // the START of a new item (reset_video_state_for_playback) is what
    // guarantees a stale Active status left over from a JUST-torn-down
    // previous item can never be wrongly applied to a brand new item's
    // Player — see hdr::send_command's own doc comment for the synchronous-
    // Idle-reset half of that fix.
    pub hdr_output_applied: bool,
    // Branch B's one-shot guard (display sync), claimed together with
    // hdr_negotiation_attempted, synchronously, the moment its trigger is true — see
    // wire_mpv_timer's display-sync hook.
    pub display_sync_attempted: bool,
    // display-mode-prefetch (2026-09-25) — true from the moment start_playback
    // defers pending_load_url/play_start to wait on display_sync's own
    // pre-decode mode-switch, until that wait resolves (success, no-video-
    // info-found, or timeout) and pending_load_url is finally set. Purely a
    // "should the buffering spinner stay visible" signal (see wire_mpv_timer's
    // buf_active condition) — play_start staying None for the same span is
    // what actually keeps every play_start-gated watchdog/diagnostic quiet
    // during the wait; this flag has no gating role of its own.
    pub display_sync_prestart_active: bool,
    // The display was switched for THIS item before load (sync_before_load
    // completed, 2026-10-06): HDR is then negotiated at the first
    // VideoReconfig instead of waiting for Branch B's post-decode check.
    pub display_presynced: bool,
    pub tracks_loaded: bool,
    pub pos_tick: u32,
    pub controls_idle_ticks: u32,
    pub seek_pending_secs: f64, // accumulated keyboard seek; executed after debounce
    pub seek_pending_ticks: u32, // countdown to execution; 0 = idle
    pub intro_timestamps: Option<Segment>,
    pub recap_timestamps: Option<Segment>,
    pub preview_timestamps: Option<Segment>,
    pub commercial_timestamps: Option<Segment>,
    pub intro_skip_shown: bool,
    pub recap_skip_shown: bool,
    pub preview_skip_shown: bool,
    pub commercial_skip_shown: bool,
    pub skip_segment_end: Option<f64>, // seek target for the currently-shown skip prompt
    pub skip_segment_handled: bool,    // true after always-skip seeked or user dismissed timed
    // A decided but delayed skip-segment seek (seek_target_secs, armed_at). The three
    // skip paths call arm_skip_fade (this + skip_fade_audio) and set skip-fade-active for
    // player.slint's fade-to-black; wire_mpv_timer fires the seek once armed_at is older
    // than skip_fade_ms × the settings-animation-speed multiplier (the same factor the
    // Slint animation uses, so they can't drift). 0 / 0% = an instant seek.
    pub pending_skip_seek: Option<(f64, Instant)>,
    // Audio-side companion — see SkipFadeAudio's own doc comment for the
    // full design (volume ramp for PCM, mute for SPDIF passthrough).
    skip_fade_audio: Option<SkipFadeAudio>,
    pub skip_timed_shown_at: Option<Instant>, // when ask-timed overlay first appeared
    pub skip_timed_prompt_secs: u32,          // configured secs for current ask-timed segment
    // Some(t) while the ask-timed overlay is paused; on resume its duration is folded
    // into skip_timed_shown_at, so a pause freezes the countdown (wire_mpv_timer).
    pub skip_timed_paused_since: Option<Instant>,
    pub credits_start: Option<f64>, // Up Next banner trigger (Credits.start)
    pub next_ep_banner_shown: bool, // prevents re-trigger within same episode
    pub credits_auto_marked_played: bool, // true after the credits-trigger auto-mark-played fires;
    // watched for a rewind past credits_mark_threshold to auto-revert
    pub credits_mark_threshold: Option<f64>, // the position that actually fired the trigger above —
    // credits_start or the dur-30s fallback, whichever fired first
    pub next_ep_pending: Option<MediaItem>, // set by countdown task; taken by natural-end or Play Now
    pub playback_generation: u64, // incremented on each start_playback; guards stale async writes
    pub last_known_pos_ticks: i64, // last successfully-read position (ticks); fallback for tear_down
    pub from_detail: bool, // set by on_play_detail/on_resume_detail; cleared in start_playback
    pub from_series: bool, // set by on_play_series_episode; cleared in start_playback
    pub from_season: bool, // set alongside from_series when show-season was also true
    pub did_render: bool,
    // Set by mpv's update callback (it has a new frame or something to redraw), cleared by each
    // video render — a fresh flag per render context. With `last_plane` (where and how the last
    // frame was drawn) it lets a paused player skip redraws that would draw the same picture.
    pub frame_news: Arc<AtomicBool>,
    pub last_plane: Option<render::PlaneKey>,
    // Logged once per Player::new — distinguishes "render context created"
    // (just means the C object exists) from "a frame actually made it to the
    // screen", so a stall between the two (e.g. mpv waiting on a slow-to-open
    // audio device) is visible in the log instead of looking identical to a
    // normal fast start.
    pub first_frame_logged: bool,
    // Stall detection (wire_mpv_timer): the last position that counted as real forward
    // progress — a rolling check, so a stall anywhere mid-video is caught.
    pub stall_last_progress_pos: f64,
    pub stall_last_progress_at: Option<Instant>,
    // The position on the PREVIOUS tick, whether or not it was progress. A big jump
    // between ticks (a seek either way, a gapless transition, a chapter/skip jump)
    // resets the stall baseline to the new position — after a backward seek the old,
    // higher stall_last_progress_pos would make healthy playback look stalled.
    pub stall_last_tick_pos: Option<f64>,
    // (item_id, attempts_so_far) — NOT reset by reset_video_state_for_playback: a reload
    // starts the SAME item again, and the cap must survive that; a different item reads
    // as 0 attempts (id mismatch). Forgiven (cleared) after sustained healthy playback
    // since the last reload (stall_last_reload_at), so two early stalls don't disable
    // recovery for the rest of a long video; a server that stays down never earns that.
    pub stall_reload_attempts_for: Option<(String, u32)>,
    pub stall_last_reload_at: Option<Instant>,
    /// "Still opening — first-open grace" already logged for this player
    /// (2026-10-08; see FIRST_OPEN_STALL_SECS).
    pub stall_grace_logged: bool,
    pub screensaver_cookie: PlaybackCookies,
    pub chapters: Vec<(f64, String)>, // chapter list; loaded ~2 s after playback start
    pub chapters_loaded: bool,        // true once chapter poll succeeded or timed out
    pub chapter_load_attempts: u32,   // retry counter while count==0 (max 30)
    pub chapter_osd_ticks: u32,       // countdown to hide chapter OSD; 125 ≈ 2 s
    pub delay_osd_ticks: u32,         // countdown to hide sub/audio delay OSD; 125 ≈ 2 s
    // Playlist: ordered track list for album/artist playback (includes currently-playing item).
    // playlist_index is the index of the currently-playing item.
    pub playlist: Vec<QueueItem>,
    pub playlist_index: usize,
    pub shuffle: bool,
    pub shuffle_order: Vec<usize>, // pre-shuffled permutation of 0..playlist.len()
    pub repeat_mode: RepeatMode,
    // Context-menu queue: items enqueued via "Add to Queue" / "Play Next"; play after playlist.
    pub queue: Vec<QueueItem>,
    // True when the current item is Audio; drives the class-gated natural-end
    // advance (audio only follows audio, video only follows video).
    pub current_is_audio: bool,
    // A Watch Trailer session (play_trailer) — no Jellyfin item, so no
    // stall reloads; a failed open closes the player with a toast instead,
    // and display sync only runs for it when Settings says so (2026-10-04).
    // `trailer_url` is the YouTube link, to mark it unplayable on failure.
    pub is_trailer: bool,
    pub trailer_url: Option<String>,
    // A YouTube trailer's first attempt keeps what a restart needs (title + player config):
    // wire_mpv_timer restarts it once if it hasn't opened after TRAILER_OPEN_RETRY_SECS.
    // None on the retry (and for every other kind of playback).
    pub trailer_retry: Option<(String, PlayerConfig)>,
    // The playing item's trickplay thumbnails (trickplay::spawn_fetch); None without any.
    pub trickplay: Option<Arc<crate::trickplay::Session>>,
    // HDR Stage 5 (2026-10-05): this player's render context lives on the
    // video subsurface's GL context (decided once, at render-context
    // creation — see wire_rendering_notifier); and the spot last logged.
    pub video_on_subsurface: bool,
    pub video_spot_logged: Option<crate::video_surface::Spot>,
    pub video_spot_waits: u8,
    pub startup_snapshot_ticks: u32,
    // Snapshot of the currently-playing item, set in start_playback. Used by
    // push_queue_display to render a synthetic now-playing row when the current
    // play is not the playlist row at playlist_index (queue jump, single track).
    pub now_playing: Option<QueueItem>,
    // Since when music has played with the fullscreen Now Playing screen closed (None while it is
    // open or no music plays) — with the last input, drives the auto-open (timer.rs
    // music_auto_open): any close path needs a fresh idle stretch before it opens again.
    pub music_idle_since: Option<Instant>,
    // Lyrics for the current Audio track (populated by get_lyrics; None = no/unknown lyrics).
    pub lyrics: Option<Vec<(u64, String)>>,
    pub lyrics_available: bool,
    // Gapless: the QueueItem appended into mpv's playlist for a seamless
    // transition. Set by the timer's preload check; consumed on TrackChanged;
    // dropped (with Player::cancel_pending) whenever the upcoming order changes.
    pub preloaded_next: Option<QueueItem>,
    // CR11-12: ticks left before retrying a failed append_gapless. Without this,
    // a failure (mpv command error) retried the identical peek+append every 16ms
    // for the whole "within 12s of end" window — up to ~750 wasted IPC calls.
    // Self-decrementing, so no explicit reset is needed when the track changes.
    pub gapless_retry_cooldown: u32,
}

impl Default for VideoState {
    fn default() -> Self {
        Self {
            player: None,
            pending_load_url: None,
            render_ctx: None,
            fbos: [0; 2],
            textures: [0; 2],
            fbo_w: 0,
            fbo_h: 0,
            back: 0,
            item_id: None,
            playing_series_id: None,
            client: None,
            play_start: None,
            decoder_logged: false,
            video_init_checked: false,
            hdr_negotiation_attempted: false,
            wide_color_fbo: false,
            hdr_output_applied: false,
            display_sync_attempted: false,
            display_sync_prestart_active: false,
            display_presynced: false,
            tracks_loaded: false,
            pos_tick: 0,
            controls_idle_ticks: 0,
            seek_pending_secs: 0.0,
            seek_pending_ticks: 0,
            intro_timestamps: None,
            recap_timestamps: None,
            preview_timestamps: None,
            commercial_timestamps: None,
            intro_skip_shown: false,
            recap_skip_shown: false,
            preview_skip_shown: false,
            commercial_skip_shown: false,
            skip_segment_end: None,
            skip_segment_handled: false,
            pending_skip_seek: None,
            skip_fade_audio: None,
            skip_timed_shown_at: None,
            skip_timed_prompt_secs: 8,
            skip_timed_paused_since: None,
            credits_start: None,
            next_ep_banner_shown: false,
            credits_auto_marked_played: false,
            credits_mark_threshold: None,
            next_ep_pending: None,
            playback_generation: 0,
            last_known_pos_ticks: 0,
            from_detail: false,
            from_series: false,
            from_season: false,
            did_render: false,
            frame_news: Arc::new(AtomicBool::new(true)),
            last_plane: None,
            first_frame_logged: false,
            stall_last_progress_pos: 0.0,
            stall_last_progress_at: None,
            stall_last_tick_pos: None,
            stall_reload_attempts_for: None,
            stall_last_reload_at: None,
            stall_grace_logged: false,
            screensaver_cookie: PlaybackCookies::default(),
            chapters: Vec::new(),
            chapters_loaded: false,
            chapter_load_attempts: 0,
            chapter_osd_ticks: 0,
            delay_osd_ticks: 0,
            playlist: Vec::new(),
            playlist_index: 0,
            shuffle: false,
            shuffle_order: Vec::new(),
            repeat_mode: RepeatMode::Off,
            queue: Vec::new(),
            current_is_audio: false,
            is_trailer: false,
            trailer_url: None,
            trailer_retry: None,
            trickplay: None,
            video_on_subsurface: false,
            video_spot_logged: None,
            video_spot_waits: 0,
            startup_snapshot_ticks: 0,
            now_playing: None,
            music_idle_since: None,
            lyrics: None,
            lyrics_available: false,
            preloaded_next: None,
            gapless_retry_cooldown: 0,
        }
    }
}

// ── tear_down_player ──────────────────────────────────────────────────────────
// Capture the final playback position then drop render_ctx before player
// (mpv invariant: MpvRenderCtx must be freed before mpv_terminate_destroy).
// Returns (item_id, client, screensaver_cookie, final_ticks) so the caller
// can send the stop report and release the screensaver inhibitor.
// Call this every time playback ends — normal finish, user stop, or replacement.
pub(crate) fn tear_down_player(
    vs: &mut VideoState,
) -> (
    Option<String>,
    Option<Arc<JellyfinClient>>,
    PlaybackCookies,
    i64,
) {
    vs.preloaded_next = None; // player is going away — pending gapless entry with it
    // get_position() returns 0.0 (via unwrap_or) if time-pos is not yet available
    // (file still loading).  Fall back to the last successfully-read position so
    // a stop-in-first-second doesn't send ticks=0 and wipe the Jellyfin resume point.
    let raw_ticks = vs
        .player
        .as_ref()
        .map(|p| (p.get_position() * 10_000_000.0) as i64)
        .unwrap_or(0);
    let ticks = if raw_ticks > 0 {
        raw_ticks
    } else {
        vs.last_known_pos_ticks
    };
    // If the credits trigger marked this episode played (POST PlayedItems, server
    // position 0) and no rewind reverted it, report 0 ticks: every teardown path comes
    // through here, and the real (nonzero) mpv position would re-add a resume point and
    // put the episode back into Continue Watching moments after the mark.
    let ticks = if vs.credits_auto_marked_played {
        0
    } else {
        ticks
    };
    vs.credits_auto_marked_played = false;
    vs.credits_mark_threshold = None;
    // A render context made on the video subsurface's own GL context must be
    // freed on it (HDR Stage 5) — mpv requires its GL context for that, and
    // before the Player (mpv core) goes.
    if vs.video_on_subsurface {
        if let Some(ctx) = vs.render_ctx.take() {
            crate::video_surface::free_render_ctx(ctx);
        }
        vs.video_on_subsurface = false;
    }
    vs.video_spot_logged = None;
    vs.render_ctx = None;
    vs.player = None;
    vs.pending_load_url = None;
    vs.trailer_retry = None;
    vs.trickplay = None;
    // display-mode-prefetch: a stop during the pre-decode wait must also
    // drop the "Loading…" spinner signal — the deferred task bails without
    // clearing it once it sees vs.player is gone (see start_playback).
    vs.display_sync_prestart_active = false;
    // hdr branch, Stage 3 — unconditional on every teardown path (stop/
    // replaced/natural-end/quit all funnel through this one function), not
    // just the ones that actually had an HDR image description active: the
    // worker itself decides whether a real unset_image_description() wire
    // call is needed, and it's also the one place that resets the on-screen
    // HDR status back to Idle regardless (see HdrStatus's own doc comment
    // for why that reset must be unconditional too). Cheap no-op if the
    // worker was never spawned (X11) or nothing was ever set.
    crate::hdr::send_command(crate::hdr::HdrCommand::Unset);
    (
        vs.item_id.take(),
        vs.client.take(),
        std::mem::take(&mut vs.screensaver_cookie),
        ticks,
    )
}

// ── quit_cleanup ──────────────────────────────────────────────────────────────
// Called from main() after window.run() returns (i.e. the user quit).
// The 16 ms timer has stopped so tear_down_player will never run via the
// normal finished path. We do it here synchronously so the stop report
// reaches Jellyfin before the runtime drops and cancels in-flight tasks.
pub(crate) fn quit_cleanup(
    video: &Arc<Mutex<VideoState>>,
    rt: &tokio::runtime::Runtime,
    state: &Arc<Mutex<FjordState>>,
) {
    let (dropped, dec_dropped) = video
        .lock()
        .unwrap()
        .player
        .as_ref()
        .map(|p| p.get_drop_counts())
        .unwrap_or((0, 0));
    info!(
        "playback stats at quit: frame-drops={} decoder-drops={}",
        dropped, dec_dropped
    );
    let (item_id, client, ss_cookie, final_ticks) = tear_down_player(&mut video.lock().unwrap());
    uninhibit_screensaver(ss_cookie);
    if let (Some(id), Some(cli)) = (item_id, client) {
        info!(
            "quit: sending stop report for {} at {:.1}s",
            id,
            final_ticks as f64 / 10_000_000.0
        );
        // Bound the wait — the HTTP client's own timeout is 30 s, and an
        // unreachable server must not stall app exit that long (CR10-16).
        rt.block_on(async move {
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                cli.report_playback_stopped(&id, final_ticks),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(e)) => warn!("report_playback_stopped (quit) failed: {e}"),
                Err(_) => warn!("report_playback_stopped (quit) timed out after 5 s"),
            }
        });
    }
    // display_sync: revert the physical output before Fjord exits, same
    // reasoning as the bounded report_playback_stopped wait just above —
    // this function's own doc comment already establishes that anything not
    // explicitly awaited here risks being cancelled once the runtime drops
    // right after this function returns, so this is genuinely block_on'd
    // (not fire-and-forget like do_stop_playback's own equivalent call),
    // bounded the same 5s way so an unreachable/hung kscreen-doctor can't
    // stall app exit indefinitely. A no-op fast-return inside
    // revert_to_default when the feature is off or nothing was ever applied
    // this session, so this costs nothing for the overwhelmingly common case.
    let state2 = Arc::clone(state);
    rt.block_on(async move {
        if tokio::time::timeout(
            std::time::Duration::from_secs(5),
            crate::display_sync::revert_to_default(state2),
        )
        .await
        .is_err()
        {
            warn!("display_sync: revert_to_default (quit) timed out after 5 s");
        }
    });
}

// ── reset_playback_ui ─────────────────────────────────────────────────────────
// Clear all player UI state after stop or natural end-of-file.
// Called from do_stop_playback and the finished path in wire_mpv_timer.
pub(crate) fn reset_playback_ui(w: &MainWindow) {
    let g = AppState::get(w);
    // Opaque again before this frame is drawn (HDR Stage 5) — the render
    // callback only runs after femtovg has already cleared the window.
    g.set_video_surface_active(false);
    g.set_is_playing(false);
    g.set_is_audio_playing(false);
    g.set_music_bar_has_art(false);
    g.set_music_bar_paused(false);
    g.set_has_background_player(false);
    g.set_video_behind_ui(false);
    g.set_float_card_focused(-1);
    g.set_music_bar_focused(-1);
    g.set_is_paused(false);
    g.set_stats_visible(false);
    g.set_playback_pos(0.0);
    g.set_playback_time("0:00".into());
    g.set_playback_total("0:00".into());
    g.set_playback_total_secs(0.0);
    g.set_playback_ends_at("".into());
    g.set_seek_hover_time("".into());
    g.set_buffering_active(false);
    g.set_buffering_pct(0);
    g.set_playback_stalled(false);
    g.set_buffered_pos(0.0);
    g.set_sub_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
    g.set_audio_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
    g.set_video_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
    g.set_player_open_panel(0);
    g.set_controls_visible(true);
    g.set_pause_bar_visible(false);
    g.set_seek_osd_visible(false);
    g.set_seek_bar_pos(0.0);
    g.set_seek_bar_time("".into());
    g.set_seek_delta_text("".into());
    g.set_seek_dragging(false);
    g.set_show_skip_segment(false);
    g.set_show_skip_timed(false);
    g.set_skip_fade_active(false);
    g.set_show_next_ep_banner(false);
    g.set_next_ep_ends_at("".into());
    g.set_chapter_marks(ModelRc::new(VecModel::<f32>::default()));
    g.set_chapter_entries(ModelRc::new(VecModel::<TrackEntry>::default()));
    g.set_current_chapter(-1);
    g.set_chapter_osd_visible(false);
    g.set_chapter_osd_text("".into());
    g.set_delay_osd_visible(false);
    g.set_delay_osd_text("".into());
    g.set_sub_delay_ms(0);
    g.set_audio_delay_ms(0);
    g.set_show_lyrics(false);
    g.set_lyrics_available(false);
    g.set_lyrics_active_idx(-1);
    g.set_lyrics_lines(ModelRc::new(VecModel::<crate::LyricEntry>::default()));
    g.set_show_now_playing(false);
    if g.get_playback_from_detail() {
        g.set_show_detail(true);
        g.set_playback_from_detail(false);
        w.invoke_grab_keyboard_focus();
    }
    if g.get_playback_from_series() {
        g.set_show_series(true);
        if g.get_playback_from_season() {
            g.set_show_season(true);
        }
        g.set_playback_from_series(false);
        g.set_playback_from_season(false);
        w.invoke_grab_keyboard_focus();
    }
}

// ── do_stop_playback ──────────────────────────────────────────────────────────
// High-level user-initiated stop: tear down player, reset UI, send stop report,
// refresh home. Does NOT auto-advance — callers that want auto-advance (the
// natural end-of-file path in wire_mpv_timer) handle it after this returns.
pub(crate) fn do_stop_playback(
    video: &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
    state: &Arc<Mutex<FjordState>>,
) {
    let (dropped, dec_dropped) = video
        .lock()
        .unwrap()
        .player
        .as_ref()
        .map(|p| p.get_drop_counts())
        .unwrap_or((0, 0));
    info!(
        "playback stopped: frame-drops={} decoder-drops={}",
        dropped, dec_dropped
    );
    let (item_id, client, ss_cookie, final_ticks) = tear_down_player(&mut video.lock().unwrap());
    uninhibit_screensaver(ss_cookie);

    // display_sync: a genuine user-initiated stop, never a replace-in-place
    // — unambiguous, nothing else is ever about to start. Fire-and-forget
    // (unlike quit_cleanup's own bounded block_on): the app keeps running,
    // so there's no risk of the runtime dropping this task before it
    // finishes. No-op fast-return inside revert_to_default when the feature
    // is off or nothing was ever applied this session.
    rt_handle.spawn(crate::display_sync::revert_to_default(Arc::clone(state)));

    // A user stop keeps the playlist and queue: the panel stays reachable via `q` while
    // idle and Enter resumes from it (Clear All or sign-out empties it).
    // UI updates go through invoke_from_event_loop: reset_session_state calls this from a
    // Tokio worker (switch_to_profile), where `Weak::upgrade()` silently returns None.
    {
        let video2 = Arc::clone(video);
        let ww2 = window_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            reset_playback_ui(&w);
            let g = crate::AppState::get(&w);
            g.set_show_queue_panel(false);
            crate::push_queue_display(&video2.lock().unwrap(), &g);
        });
    }

    // Stop report then home refresh, sequenced so the home fetch happens after Jellyfin
    // has processed the stop — prevents the stopped item reappearing in continue-watching.
    if let (Some(id), Some(cli)) = (item_id, client) {
        let ww = window_weak.clone();
        let rth = rt_handle.clone();
        let state = Arc::clone(state);
        rt_handle.spawn(async move {
            if let Err(e) = cli.report_playback_stopped(&id, final_ticks).await {
                warn!("report_playback_stopped failed: {e}");
            }
            let home_data = fetch_home_data(&cli, Some(&state)).await;
            let sections = home_data_sections(&home_data);
            let ww2 = ww.clone();
            let watchlist = state.lock().unwrap().jellyfin_watchlist_ids.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww2.upgrade() {
                    push_home_data(&w, &home_data, &watchlist);
                }
            });
            spawn_poster_loading(cli, sections, ww, rth, state);
        });
    }
}

// ── reset_video_state_for_playback ────────────────────────────────────────────
// The shared "fresh playback" reset — chapters, stall detection, screensaver
// inhibitor, skip/OSD countdowns, gapless preload — used by `start_playback` and
// `play_trailer`, so a field added for one can't be forgotten in the other. Each
// caller then sets its own `item_id`/`playing_series_id`/`client` (a trailer has none).
fn reset_video_state_for_playback(
    vs: &mut VideoState,
    player: Player,
    config: &PlayerConfig,
    is_episode: bool,
    url: &str,
) {
    vs.player = Some(player);
    // Deliberately NOT loaded yet — Player::new() only builds the mpv core.
    // The actual loadfile is deferred until wire_rendering_notifier's
    // BeforeRendering handler has confirmed a render context exists for
    // THIS player (same-thread, race-free ordering) — see Player::load()'s
    // own doc comment and CLAUDE.md's Known platform issues for the real
    // HTPC bug (audio-only-forever, black screen) this fixes at the root.
    vs.pending_load_url = Some(url.to_string());
    // Not stamped here: each caller sets play_start when decode is actually requested
    // (at once, or after display sync's pre-decode mode switch). It MUST be cleared here,
    // though — the previous item's timestamp would otherwise drive every play_start-gated
    // check (chapter poll, decoder log, VideoReconfig warning, stall watchdog, first-frame
    // log) during the display-sync wait.
    vs.play_start = None;
    vs.first_frame_logged = false;
    vs.startup_snapshot_ticks = 0;
    vs.is_trailer = false; // play_trailer sets these after this reset
    vs.trailer_url = None;
    vs.trailer_retry = None;
    vs.trickplay = None;
    vs.stall_last_progress_pos = config.start_position_secs.unwrap_or(0.0);
    vs.stall_last_progress_at = None; // re-armed on the first tick after this player starts
    // A fresh player means the very first tick has nothing meaningful to
    // compare against — without this, it would compare the new player's
    // start position against a completely unrelated leftover value from
    // whatever was playing (or being seeked in) right before this reset.
    vs.stall_last_tick_pos = None;
    vs.stall_grace_logged = false;
    // stall_reload_attempts_for/stall_last_reload_at deliberately NOT reset
    // here — see their own doc comment on VideoState for why they must
    // survive a same-item reload.
    vs.decoder_logged = false;
    vs.video_init_checked = false;
    vs.hdr_negotiation_attempted = false;
    // hdr branch, Stage 4: decided once, here, from the Settings toggle
    // alone — see wide_color_fbo's own VideoState doc comment for why this
    // can't be conditioned on per-item source eligibility instead.
    vs.wide_color_fbo = config.target_colorspace_hint;
    vs.hdr_output_applied = false;
    vs.display_sync_attempted = false;
    vs.display_sync_prestart_active = false;
    vs.display_presynced = false;
    vs.tracks_loaded = false;
    vs.pos_tick = 0;
    vs.controls_idle_ticks = 0;
    // Seeded with the start position, not 0: a stop (or stall reload) before
    // mpv ever reports a position — e.g. during display_sync's pre-decode
    // wait — would otherwise report 0 to Jellyfin and wipe the resume point.
    vs.last_known_pos_ticks = config
        .start_position_secs
        .map(|s| (s * 10_000_000.0) as i64)
        .unwrap_or(0);
    // For Episodes: intro_timestamps/intro_skip_shown/credits_start were reset
    // before the fetch tasks were spawned — don't clear them here or a fast
    // response would be silently wiped. For everything else (movies, trailers):
    // no such tasks run, so reset explicitly.
    if !is_episode {
        vs.intro_timestamps = None;
        vs.recap_timestamps = None;
        vs.preview_timestamps = None;
        vs.commercial_timestamps = None;
        vs.intro_skip_shown = false;
        vs.recap_skip_shown = false;
        vs.preview_skip_shown = false;
        vs.commercial_skip_shown = false;
        vs.skip_segment_end = None;
        vs.credits_start = None;
    }
    vs.skip_segment_handled = false;
    vs.pending_skip_seek = None;
    // A fresh player has its own fresh volume/mute state (whatever was
    // ramping/muted on the OLD one is gone the instant it's dropped, right
    // above) — no explicit unmute/un-ramp call needed on it, just clear the
    // Rust-side tracking so a stale snapshot doesn't apply to the new one.
    vs.skip_fade_audio = None;
    vs.skip_timed_shown_at = None;
    vs.skip_timed_prompt_secs = 8;
    vs.skip_timed_paused_since = None;
    vs.next_ep_banner_shown = false;
    vs.credits_auto_marked_played = false;
    vs.credits_mark_threshold = None;
    vs.next_ep_pending = None;
    vs.screensaver_cookie = inhibit_screensaver();
    vs.chapters = Vec::new();
    vs.chapters_loaded = false;
    vs.chapter_load_attempts = 0;
    vs.chapter_osd_ticks = 0;
    vs.delay_osd_ticks = 0;
    vs.lyrics = None;
    vs.lyrics_available = false;
    vs.preloaded_next = None; // fresh player — no pending entry
}

/// Where a trailer comes from — decides the URL check in play_trailer.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum TrailerSource {
    /// A YouTube link from the server (Seerr relatedVideos, Jellyfin RemoteTrailers): must pass
    /// discover::trailer_url_allowed; mpv resolves it through yt-dlp.
    YouTube,
    /// A trailer file on the Jellyfin server: the stream URL is built by Fjord
    /// (JellyfinClient::direct_play_url for a validated id), never taken from the server.
    Server,
}

// ── play_trailer ──────────────────────────────────────────────────────────────
// Watch Trailer (Discover, movie detail page, series page). Not routed through `start_playback`:
// that path reports to Jellyfin (playback start/progress/stopped, intro/credits
// fetch, series auto-advance) and needs a real `Arc<JellyfinClient>`. Here
// `vs.client`/`item_id`/`playing_series_id` stay `None`, which is what makes progress
// reports and the series up-next/auto-advance/credits block in wire_mpv_timer skip
// themselves. Everything else comes from `reset_video_state_for_playback`.
pub(crate) fn play_trailer(
    url: String,
    title: String,
    config: PlayerConfig,
    source: TrailerSource,
    video: &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
) {
    start_trailer(
        url,
        title,
        config,
        source,
        false,
        video,
        window_weak,
        rt_handle,
    );
}

/// A YouTube trailer with no FileLoaded this long after starting gets one fresh start (a new
/// yt-dlp resolution): working opens take 1–2 s in the logs, stuck ones ~29 s.
pub(crate) const TRAILER_OPEN_RETRY_SECS: u64 = 7;

/// play_trailer, or (`retry`) wire_mpv_timer's one restart of a trailer that didn't open.
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_trailer(
    url: String,
    title: String,
    config: PlayerConfig,
    source: TrailerSource,
    retry: bool,
    video: &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
) {
    info!(
        "playing trailer ({source:?}{}): {}",
        if retry { ", retry" } else { "" },
        fjord_player::redact_api_key(&url)
    );
    // Server-provided URL: only https YouTube reaches mpv (2026-10-09
    // security review; the callers already filter, this is the last gate).
    if source == TrailerSource::YouTube && !crate::discover::trailer_url_allowed(&url) {
        warn!("trailer not played: not an https YouTube URL");
        crate::show_toast(window_weak.clone(), "Trailer unavailable".into());
        return;
    }

    {
        let mut vs = video.lock().unwrap();
        vs.playback_generation = vs.playback_generation.wrapping_add(1);
    }

    let (dropped, dec_dropped) = video
        .lock()
        .unwrap()
        .player
        .as_ref()
        .map(|p| p.get_drop_counts())
        .unwrap_or((0, 0));
    info!(
        "playback replaced (trailer): frame-drops={} decoder-drops={}",
        dropped, dec_dropped
    );
    let (prev_item_id, prev_client, prev_cookie, prev_ticks) =
        { tear_down_player(&mut video.lock().unwrap()) };
    uninhibit_screensaver(prev_cookie);
    if let (Some(id), Some(cli)) = (prev_item_id, prev_client) {
        rt_handle.spawn(async move {
            if let Err(e) = cli.report_playback_stopped(&id, prev_ticks).await {
                warn!("report_playback_stopped (replaced by trailer) failed: {e}");
            }
        });
    }

    let retry_with =
        (source == TrailerSource::YouTube && !retry).then(|| (title.clone(), config.clone()));
    match Player::new(&config) {
        Ok(player) => {
            {
                let mut vs = video.lock().unwrap();
                reset_video_state_for_playback(&mut vs, player, &config, false, &url);
                vs.trailer_retry = retry_with;
                vs.item_id = None;
                vs.playing_series_id = None;
                vs.client = None;
                vs.is_trailer = true;
                // Only a YouTube link is remembered (to mark it unplayable on failure); a server
                // stream URL carries the token and has nothing to mark.
                vs.trailer_url = (source == TrailerSource::YouTube).then(|| url.clone());
                // Trailers have no video_info/Jellyfin item, so they never
                // go through start_playback's own deferred-load path — stamp
                // play_start immediately here, matching today's exact
                // pre-display-mode-prefetch timing (see
                // reset_video_state_for_playback's own doc comment for why
                // this is no longer done for every caller automatically).
                vs.play_start = Some(Instant::now());
            }
            if let Some(w) = window_weak.upgrade() {
                let g = AppState::get(&w);
                g.set_playing_title(ss(&title));
                g.set_is_audio_playing(false);
                g.set_is_playing(true);
                g.set_has_background_player(false);
                g.set_video_behind_ui(false);
                g.set_is_paused(false);
                g.set_controls_visible(false);
            }
        }
        Err(e) => {
            error!("trailer player init failed: {:#}", e);
            if let Some(w) = window_weak.upgrade() {
                reset_playback_ui(&w);
            }
            crate::show_toast(
                window_weak.clone(),
                "Couldn't play trailer — is yt-dlp installed?".to_string(),
            );
        }
    }
}
