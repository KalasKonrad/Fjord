// ── fjord-app · playback.rs ──────────────────────────────────────────────────
//   QueueItem               { id, item_type, series_id, title, audio_meta } — one entry in the playback queue
//   RepeatMode              Off / All / One — queue repeat behaviour
//   VideoState              mpv Player + MpvRenderCtx, GL FBOs, playback metadata
//                           playlist: Vec<QueueItem> — ordered track list for album/artist playback
//                           playlist_index: usize — currently-playing position in playlist
//                           shuffle: bool, shuffle_order: Vec<usize> — shuffled play order
//                           repeat_mode: RepeatMode
//                           queue: Vec<QueueItem> — context-menu enqueue (plays after playlist ends)
//                           keep_playlist: bool — set by playlist-driven callers before start_playback;
//                             consumed there; when false start_playback wipes playlist+queue (CR10-2)
//                           from_detail/from_series/from_season: bool — set before start_playback by
//                             on_play_detail/on_resume_detail / on_play_series_episode; read+cleared in
//                             start_playback to prevent hiding the originating screen; reset_playback_ui
//                             restores show_detail / show_series / show_season on stop
//                           playback_generation: u64 counter incremented each start_playback;
//                             episode timestamps task (Intro Skipper v2+) guards stale generation
//                           skip_segment_handled: true after always-skip seeked or user dismissed timed
//                           skip_timed_shown_at: Instant when ask-timed overlay first appeared
//                           skip_timed_prompt_secs: configured countdown for current ask-timed segment
//                           credits_start: trigger point for Up Next banner (Intro Skipper Credits)
//                           next_ep_banner_shown: guard — fires once per episode
//                           next_ep_pending: next MediaItem; taken by natural-end, Play Now, or cancelled
//                           chapters: Vec<(start_secs, title)> from chapter-list; loaded after 2 s
//                           chapter_osd_ticks: countdown to hide chapter-name OSD (125 = ~2 s)
//                           delay_osd_ticks: countdown to hide sub/audio delay OSD (125 = ~2 s)
//   chapter entries         chapter-entries ([TrackEntry] id=index, label="M:SS  Title") + current-chapter
//                           populated when chapters load; current-chapter tracked in 16 ms timer
//   upcoming_count          queue-count definition: playlist tracks after current + queue items
//   fmt_secs                seconds → "H:MM:SS" / "M:SS"
//   fmt_ends_at             remaining seconds → local wall-clock "HH:MM" (empty when ≤ 0)
//   build_track_model       Vec<TrackInfo> → ModelRc<TrackEntry>; title preferred, falls back to external filename base
//   PlaybackCookies         ScreenSaver cookie + KDE PowerManagement cookie + systemd child
//   inhibit_screensaver     ScreenSaver.Inhibit + KDE PowerManagement.Inhibit + systemd-inhibit child
//   uninhibit_screensaver   release all three (KDE/systemd no-op when unavailable)
//   tear_down_player        capture ticks, drop render_ctx then player (mpv invariant), return stop data
//   do_stop_playback        user stop: tear down, clear playlist+queue (CR10-2), reset UI, stop report, home refresh
//   reset_playback_ui       clear all player UI state incl. buffering + seek-hover + seek-dragging + skip overlays
//   quit_cleanup            synchronous stop report + screensaver release called after window.run() exits
//   start_playback          stop-report previous item first (CR-3), then open URL in mpv; audio_meta: Option<(artist, album_art_id)> drives music bar;
//                           item_type=="Audio" → is-audio-playing=true (music bar, no fullscreen player); generation guards stale writes; show_toast on failure
//   reset_playback_ui       clear all player UI state incl. is-audio-playing + music-bar fields + buffering + skip overlays
//   wire_rendering_notifier GL thread: FBO render + report_swap() for vsync feedback (no stats — moved to timer)
//   wire_mpv_timer          16 ms timer: position (also updates music-bar-pos/elapsed/total when is-audio-playing), stats,
//                           skip segment (4 modes: always-skip/ask/ask-timed/never-skip),
//                           Up Next banner trigger (credits mode: always-skip/ask/never-skip) + configurable countdown;
//                           natural-end fallback: if EOF beats next-up fetch (always-skip race), respawns fetch
// ─────────────────────────────────────────────────────────────────────────────
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use chrono::Local;

use fjord_api::{models::{MediaItem, Segment}, JellyfinClient};
use fjord_player::{MpvRenderCtx, Player, PlayerConfig, PollResult, TrackInfo};
use slint::{ComponentHandle, Global, LogicalPosition, ModelRc, SharedString, VecModel};
use slint::platform::WindowEvent;
use tracing::{debug, error, info, warn};

use crate::config::FjordState;
use crate::home::{fetch_home_data, home_data_sections, push_home_data};
use crate::poster::spawn_poster_loading;
use crate::AppState;
use crate::stats::update_stats_window;
use crate::MainWindow;
use crate::TrackEntry;

fn ss(s: &str) -> SharedString { SharedString::from(s) }

// ── screensaver + display inhibitor ──────────────────────────────────────────

// Holds cookies from both the freedesktop ScreenSaver inhibitor and the KDE
// PowerManagement inhibitor.  Either may be None if the call is unavailable
// (e.g. not running under KDE, or busctl absent).
pub(crate) struct PlaybackCookies {
    freedesktop:   Option<u32>,
    kde_power:     Option<u32>,
    // systemd-logind inhibitor (idle + sleep) held open as a child process.
    // Covers sleep/suspend on GNOME, XFCE, and any systemd-based DE that is
    // not KDE (KDE sleep is already covered by kde_power above).
    systemd_child: Option<std::process::Child>,
}

impl Default for PlaybackCookies {
    fn default() -> Self {
        Self { freedesktop: None, kde_power: None, systemd_child: None }
    }
}

fn busctl_inhibit(service: &str, path: &str, interface: &str, label: &str) -> Option<u32> {
    let out = std::process::Command::new("busctl")
        .args(["call", "--session", service, path, interface, "Inhibit", "ss",
               "Fjord", "Video playback"])
        .output()
        .ok()?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let cookie = stdout.trim().strip_prefix("u ").and_then(|s| s.parse().ok());
    if let Some(c) = cookie {
        info!("{} inhibited (cookie={})", label, c);
    } else {
        debug!("{} inhibit unavailable", label);
    }
    cookie
}

fn busctl_uninhibit(service: &str, path: &str, interface: &str, cookie: u32, label: &str) {
    let _ = std::process::Command::new("busctl")
        .args(["call", "--session", service, path, interface, "UnInhibit", "u",
               &cookie.to_string()])
        .status();
    info!("{} uninhibited (cookie={})", label, cookie);
}

fn inhibit_systemd_sleep() -> Option<std::process::Child> {
    // systemd-logind inhibitor: holds an fd open via a long-lived child process.
    // Blocks idle + sleep on any systemd-based DE (GNOME, XFCE, Cinnamon, …).
    // KDE sleep is already covered by the KDE PowerManagement inhibitor above.
    match std::process::Command::new("systemd-inhibit")
        .args(["--what=idle:sleep", "--who=Fjord", "--why=Video playback", "--mode=block",
               "sleep", "infinity"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    {
        Ok(child) => { info!("systemd sleep inhibited (pid={})", child.id()); Some(child) }
        Err(e)    => { debug!("systemd-inhibit unavailable: {}", e); None }
    }
}

fn inhibit_screensaver() -> PlaybackCookies {
    PlaybackCookies {
        freedesktop: busctl_inhibit(
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
            "ScreenSaver",
        ),
        kde_power: busctl_inhibit(
            "org.kde.PowerManagement",
            "/org/kde/PowerManagement/Inhibit",
            "org.kde.PowerManagement.Inhibition",
            "KDE PowerManagement",
        ),
        systemd_child: inhibit_systemd_sleep(),
    }
}

fn uninhibit_screensaver(mut cookies: PlaybackCookies) {
    if let Some(c) = cookies.freedesktop {
        busctl_uninhibit(
            "org.freedesktop.ScreenSaver",
            "/org/freedesktop/ScreenSaver",
            "org.freedesktop.ScreenSaver",
            c, "ScreenSaver",
        );
    }
    if let Some(c) = cookies.kde_power {
        busctl_uninhibit(
            "org.kde.PowerManagement",
            "/org/kde/PowerManagement/Inhibit",
            "org.kde.PowerManagement.Inhibition",
            c, "KDE PowerManagement",
        );
    }
    if let Some(mut child) = cookies.systemd_child.take() {
        child.kill().ok();
        child.wait().ok();
        info!("systemd sleep inhibitor released");
    }
}

// ── RepeatMode ────────────────────────────────────────────────────────────────
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub(crate) enum RepeatMode {
    #[default]
    Off = 0,
    All = 1,
    One = 2,
}

// ── QueueItem ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug)]
pub(crate) struct QueueItem {
    pub id:         String,
    pub item_type:  String,
    pub series_id:  Option<String>,
    pub title:      String,
    pub audio_meta: Option<(String, String)>, // (artist, album_art_id)
}

// ── upcoming_count ────────────────────────────────────────────────────────────
// Single definition of what queue-count means: tracks still ahead in the
// playlist (after the current one) plus all context-menu queue items (CR10-6).
pub(crate) fn upcoming_count(vs: &VideoState) -> i32 {
    let ahead = if vs.playlist.is_empty() {
        0
    } else {
        vs.playlist.len().saturating_sub(vs.playlist_index + 1)
    };
    (ahead + vs.queue.len()) as i32
}

// ── shuffle_indices ───────────────────────────────────────────────────────────
// LCG Fisher-Yates shuffle of 0..n into a Vec<usize>.
pub(crate) fn shuffle_indices(n: usize) -> Vec<usize> {
    let mut indices: Vec<usize> = (0..n).collect();
    if n <= 1 { return indices; }
    let seed = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64)
        .unwrap_or(12345);
    let mut rng = seed;
    for i in (1..n).rev() {
        rng = rng.wrapping_mul(6364136223846793005u64).wrapping_add(1442695040888963407u64);
        let j = (rng >> 33) as usize % (i + 1);
        indices.swap(i, j);
    }
    indices
}

// ── VideoState ────────────────────────────────────────────────────────────────
pub(crate) struct VideoState {
    pub player:     Option<Player>,
    pub render_ctx: Option<MpvRenderCtx>,
    pub fbos:       [u32; 2],
    pub textures:   [u32; 2],
    pub fbo_w:      u32,
    pub fbo_h:      u32,
    pub back:       usize,
    pub item_id:          Option<String>,
    pub playing_series_id: Option<String>,
    pub client:           Option<Arc<JellyfinClient>>,
    pub play_start:     Option<Instant>,
    pub decoder_logged:     bool,
    pub tracks_loaded:      bool,
    pub pos_tick:           u32,
    pub controls_idle_ticks:  u32,
    pub seek_pending_secs:    f64,  // accumulated keyboard seek; executed after debounce
    pub seek_pending_ticks:   u32,  // countdown to execution; 0 = idle
    pub intro_timestamps:      Option<Segment>,
    pub recap_timestamps:      Option<Segment>,
    pub preview_timestamps:    Option<Segment>,
    pub commercial_timestamps: Option<Segment>,
    pub intro_skip_shown:      bool,
    pub recap_skip_shown:      bool,
    pub preview_skip_shown:    bool,
    pub commercial_skip_shown: bool,
    pub skip_segment_end:      Option<f64>,    // seek target for the currently-shown skip prompt
    pub skip_segment_handled:  bool,           // true after always-skip seeked or user dismissed timed
    pub skip_timed_shown_at:   Option<Instant>, // when ask-timed overlay first appeared
    pub skip_timed_prompt_secs: u32,           // configured secs for current ask-timed segment
    pub credits_start:         Option<f64>,    // Up Next banner trigger (Credits.start)
    pub next_ep_banner_shown:  bool,           // prevents re-trigger within same episode
    pub next_ep_pending:     Option<MediaItem>, // set by countdown task; taken by natural-end or Play Now
    pub playback_generation: u64,              // incremented on each start_playback; guards stale async writes
    pub last_known_pos_ticks: i64,            // last successfully-read position (ticks); fallback for tear_down
    pub from_detail:         bool,             // set by on_play_detail/on_resume_detail; cleared in start_playback
    pub from_series:         bool,             // set by on_play_series_episode; cleared in start_playback
    pub from_season:         bool,             // set alongside from_series when show-season was also true
    pub did_render:          bool,
    pub screensaver_cookie:  PlaybackCookies,
    pub chapters:              Vec<(f64, String)>, // chapter list; loaded ~2 s after playback start
    pub chapters_loaded:       bool,               // true once chapter poll succeeded or timed out
    pub chapter_load_attempts: u32,                // retry counter while count==0 (max 30)
    pub chapter_osd_ticks:     u32,                // countdown to hide chapter OSD; 125 ≈ 2 s
    pub delay_osd_ticks:       u32,                // countdown to hide sub/audio delay OSD; 125 ≈ 2 s
    // Playlist: ordered track list for album/artist playback (includes currently-playing item).
    // playlist_index is the index of the currently-playing item.
    pub playlist:              Vec<QueueItem>,
    pub playlist_index:        usize,
    pub shuffle:               bool,
    pub shuffle_order:         Vec<usize>,  // pre-shuffled permutation of 0..playlist.len()
    pub repeat_mode:           RepeatMode,
    // Context-menu queue: items enqueued via "Add to Queue" / "Play Next"; play after playlist.
    pub queue:                 Vec<QueueItem>,
    // Set by playlist-driven callers (Play All, prev/next track, queue jump, natural-end
    // advance) immediately before start_playback; consumed there. When false, start_playback
    // wipes playlist+queue so a stale album can't resume after an unrelated item ends (CR10-2).
    pub keep_playlist:         bool,
    // Lyrics for the current Audio track (populated by get_lyrics; None = no/unknown lyrics).
    pub lyrics:                Option<Vec<(u64, String)>>,
    pub lyrics_available:      bool,
}

impl Default for VideoState {
    fn default() -> Self {
        Self {
            player: None, render_ctx: None,
            fbos: [0; 2], textures: [0; 2],
            fbo_w: 0, fbo_h: 0, back: 0,
            item_id: None, playing_series_id: None, client: None,
            play_start: None, decoder_logged: false,
            tracks_loaded: false, pos_tick: 0,
            controls_idle_ticks: 0,
            seek_pending_secs: 0.0, seek_pending_ticks: 0,
            intro_timestamps: None, recap_timestamps: None,
            preview_timestamps: None, commercial_timestamps: None,
            intro_skip_shown: false, recap_skip_shown: false,
            preview_skip_shown: false, commercial_skip_shown: false,
            skip_segment_end: None,
            skip_segment_handled: false, skip_timed_shown_at: None, skip_timed_prompt_secs: 8,
            credits_start: None, next_ep_banner_shown: false, next_ep_pending: None,
            playback_generation: 0, last_known_pos_ticks: 0,
            from_detail: false, from_series: false, from_season: false,
            did_render: false, screensaver_cookie: PlaybackCookies::default(),
            chapters: Vec::new(), chapters_loaded: false,
            chapter_load_attempts: 0, chapter_osd_ticks: 0, delay_osd_ticks: 0,
            playlist: Vec::new(), playlist_index: 0,
            shuffle: false, shuffle_order: Vec::new(), repeat_mode: RepeatMode::Off,
            queue: Vec::new(), keep_playlist: false,
            lyrics: None, lyrics_available: false,
        }
    }
}

// ── fmt_secs ──────────────────────────────────────────────────────────────────
pub(crate) fn fmt_secs(secs: f64) -> SharedString {
    if secs <= 0.0 { return "0:00".into(); }
    let s = secs as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let s = s % 60;
    if h > 0 {
        SharedString::from(format!("{}:{:02}:{:02}", h, m, s).as_str())
    } else {
        SharedString::from(format!("{}:{:02}", m, s).as_str())
    }
}

// ── fmt_ends_at ───────────────────────────────────────────────────────────────
pub(crate) fn fmt_ends_at(remaining_secs: f64) -> SharedString {
    if remaining_secs <= 0.0 { return "".into(); }
    let ends = Local::now() + chrono::Duration::seconds(remaining_secs as i64);
    SharedString::from(ends.format("%H:%M").to_string().as_str())
}

// ── sub_lang_code ────────────────────────────────────────────────────────────
fn sub_lang_code(name: &str) -> &str {
    match name {
        "English"    => "en", "German"     => "de", "French"     => "fr",
        "Japanese"   => "ja", "Spanish"    => "es", "Italian"    => "it",
        "Portuguese" => "pt", "Russian"    => "ru", "Korean"     => "ko",
        "Chinese"    => "zh", "Dutch"      => "nl", "Swedish"    => "sv",
        "Polish"     => "pl", "Czech"      => "cs", "Arabic"     => "ar",
        "Turkish"    => "tr", "Finnish"    => "fi", "Danish"     => "da",
        "Norwegian"  => "no",
        _            => "",
    }
}

// ── build_track_model ─────────────────────────────────────────────────────────
pub(crate) fn build_track_model(tracks: &[TrackInfo], kind: &str) -> ModelRc<TrackEntry> {
    let entries: Vec<TrackEntry> = tracks.iter()
        .filter(|t| t.track_type == kind)
        .map(|t| {
            let mut label = String::new();

            // Title first: prefer embedded title, fall back to base filename for external tracks.
            let title = if !t.title.is_empty() {
                t.title.clone()
            } else if !t.external_filename.is_empty() {
                std::path::Path::new(&t.external_filename)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            if !title.is_empty() { label.push_str(&title); }

            // Append type tag for subtitle tracks when the flag is set but the
            // title doesn't already contain a hint (avoids "English (SDH) [SDH]").
            if kind == "sub" {
                let title_lower = title.to_ascii_lowercase();
                if t.hearing_impaired && !title_lower.contains("sdh") && !title_lower.contains("hearing") {
                    if !label.is_empty() { label.push(' '); }
                    label.push_str("[SDH]");
                } else if t.forced && !title_lower.contains("forced") {
                    if !label.is_empty() { label.push(' '); }
                    label.push_str("[Forced]");
                }
            }

            // Language code after title.
            if !t.lang.is_empty() {
                if !label.is_empty() { label.push(' '); }
                label.push_str(&t.lang);
            }

            // Codec last.
            if !t.codec.is_empty() {
                label.push_str(&format!(" ({})", t.codec));
            }
            if label.is_empty() { label = format!("Track {}", t.id); }
            TrackEntry { id: t.id as i32, label: label.into() }
        })
        .collect();
    ModelRc::new(VecModel::from(entries))
}

pub(crate) unsafe fn create_fbo(w: u32, h: u32) -> Option<(u32, u32)> {
    let mut tex = 0u32;
    gl::GenTextures(1, &mut tex);
    gl::BindTexture(gl::TEXTURE_2D, tex);
    gl::TexImage2D(
        gl::TEXTURE_2D, 0, gl::RGBA as i32,
        w as i32, h as i32, 0,
        gl::RGBA, gl::UNSIGNED_BYTE, std::ptr::null(),
    );
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
    gl::BindTexture(gl::TEXTURE_2D, 0);

    let mut fbo = 0u32;
    gl::GenFramebuffers(1, &mut fbo);
    gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
    gl::FramebufferTexture2D(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::TEXTURE_2D, tex, 0);
    let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER);
    gl::BindFramebuffer(gl::FRAMEBUFFER, 0);

    if status != gl::FRAMEBUFFER_COMPLETE {
        tracing::error!("FBO not complete: {:#x}", status);
        gl::DeleteFramebuffers(1, &fbo);
        gl::DeleteTextures(1, &tex);
        return None;
    }
    Some((fbo, tex))
}

pub(crate) unsafe fn delete_fbo(fbo: u32, tex: u32) {
    if fbo != 0 { gl::DeleteFramebuffers(1, &fbo); }
    if tex != 0 { gl::DeleteTextures(1, &tex); }
}

// ── tear_down_player ──────────────────────────────────────────────────────────
// Capture the final playback position then drop render_ctx before player
// (mpv invariant: MpvRenderCtx must be freed before mpv_terminate_destroy).
// Returns (item_id, client, screensaver_cookie, final_ticks) so the caller
// can send the stop report and release the screensaver inhibitor.
// Call this every time playback ends — normal finish, user stop, or replacement.
pub(crate) fn tear_down_player(vs: &mut VideoState)
    -> (Option<String>, Option<Arc<JellyfinClient>>, PlaybackCookies, i64)
{
    // get_position() returns 0.0 (via unwrap_or) if time-pos is not yet available
    // (file still loading).  Fall back to the last successfully-read position so
    // a stop-in-first-second doesn't send ticks=0 and wipe the Jellyfin resume point.
    let raw_ticks = vs.player.as_ref()
        .map(|p| (p.get_position() * 10_000_000.0) as i64)
        .unwrap_or(0);
    let ticks = if raw_ticks > 0 { raw_ticks } else { vs.last_known_pos_ticks };
    vs.render_ctx = None;
    vs.player     = None;
    (vs.item_id.take(), vs.client.take(), std::mem::take(&mut vs.screensaver_cookie), ticks)
}

// ── quit_cleanup ──────────────────────────────────────────────────────────────
// Called from main() after window.run() returns (i.e. the user quit).
// The 16 ms timer has stopped so tear_down_player will never run via the
// normal finished path. We do it here synchronously so the stop report
// reaches Jellyfin before the runtime drops and cancels in-flight tasks.
pub(crate) fn quit_cleanup(video: &Arc<Mutex<VideoState>>, rt: &tokio::runtime::Runtime) {
    let (dropped, dec_dropped) = video.lock().unwrap().player.as_ref()
        .map(|p| p.get_drop_counts()).unwrap_or((0, 0));
    info!("playback stats at quit: frame-drops={} decoder-drops={}", dropped, dec_dropped);
    let (item_id, client, ss_cookie, final_ticks) = tear_down_player(&mut video.lock().unwrap());
    uninhibit_screensaver(ss_cookie);
    if let (Some(id), Some(cli)) = (item_id, client) {
        info!("quit: sending stop report for {} at {:.1}s", id, final_ticks as f64 / 10_000_000.0);
        // Bound the wait — the HTTP client's own timeout is 30 s, and an
        // unreachable server must not stall app exit that long (CR10-16).
        rt.block_on(async move {
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                cli.report_playback_stopped(&id, final_ticks),
            ).await {
                Ok(Ok(()))  => {}
                Ok(Err(e))  => warn!("report_playback_stopped (quit) failed: {e}"),
                Err(_)      => warn!("report_playback_stopped (quit) timed out after 5 s"),
            }
        });
    }
}

// ── reset_playback_ui ─────────────────────────────────────────────────────────
// Clear all player UI state after stop or natural end-of-file.
// Called from do_stop_playback and the finished path in wire_mpv_timer.
pub(crate) fn reset_playback_ui(w: &MainWindow) {
    let g = AppState::get(w);
    g.set_is_playing(false);
    g.set_is_playing_audio(false);
    g.set_is_audio_playing(false);
    g.set_music_bar_has_art(false);
    g.set_music_bar_paused(false);
    g.set_playing_has_album_art(false);
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
    video:       &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle:   &tokio::runtime::Handle,
) {
    let (dropped, dec_dropped) = video.lock().unwrap().player.as_ref()
        .map(|p| p.get_drop_counts()).unwrap_or((0, 0));
    info!("playback stopped: frame-drops={} decoder-drops={}", dropped, dec_dropped);
    let (item_id, client, ss_cookie, final_ticks) = tear_down_player(&mut video.lock().unwrap());
    uninhibit_screensaver(ss_cookie);

    // User-initiated stop discards the playlist and queue — otherwise a later
    // unrelated natural-end would resume the stale album (CR10-2).
    {
        let mut vs = video.lock().unwrap();
        vs.playlist.clear();
        vs.playlist_index = 0;
        vs.queue.clear();
        vs.shuffle_order.clear();
    }

    if let Some(w) = window_weak.upgrade() {
        reset_playback_ui(&w);
        let g = crate::AppState::get(&w);
        g.set_show_queue_panel(false);
        crate::push_queue_display(&video.lock().unwrap(), &g);
    }

    // Stop report then home refresh, sequenced so the home fetch happens after Jellyfin
    // has processed the stop — prevents the stopped item reappearing in continue-watching.
    if let (Some(id), Some(cli)) = (item_id, client) {
        let ww  = window_weak.clone();
        let rth = rt_handle.clone();
        rt_handle.spawn(async move {
            if let Err(e) = cli.report_playback_stopped(&id, final_ticks).await {
                warn!("report_playback_stopped failed: {e}");
            }
            let home_data = fetch_home_data(&cli).await;
            let sections  = home_data_sections(&home_data);
            let ww2 = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww2.upgrade() { push_home_data(&w, &home_data); }
            });
            spawn_poster_loading(cli, sections, ww, rth);
        });
    }
}

// ── start_playback ────────────────────────────────────────────────────────────
pub(crate) fn start_playback(
    url:         String,
    item_id:     String,
    item_type:   &str,
    title:       String,
    config:      PlayerConfig,
    client:      Arc<JellyfinClient>,
    series_id:   Option<String>,
    // (artist, album_art_id) — populated for Audio items; drives the music bar
    audio_meta:  Option<(String, String)>,
    video:       &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle:   &tokio::runtime::Handle,
) {
    info!("starting playback: {} — {}", item_id, fjord_player::redact_api_key(&url));

    // Increment generation before spawning tasks so stale responses from a prior
    // episode can be detected and discarded even if they arrive after Player::new.
    let my_gen = {
        let mut vs = video.lock().unwrap();
        vs.playback_generation = vs.playback_generation.wrapping_add(1);
        vs.playback_generation
    };

    // Track whether this play started from the detail/series/season page so reset_playback_ui
    // can restore the correct screen on stop.
    let (from_detail, from_series) = {
        let mut vs = video.lock().unwrap();
        let fd = vs.from_detail;  vs.from_detail = false;
        let fs = vs.from_series;  vs.from_series = false;
        vs.from_season = false;
        (fd, fs)
    };
    if let Some(w) = window_weak.upgrade() {
        let g = AppState::get(&w);
        if !from_detail { g.set_show_detail(false); }
        // Series/season have no inline-video slot — always hide.
        // on_play_series_episode already set playback_from_series/season directly on the
        // UI thread; only clear them when this is NOT a series play (from_series = false
        // means a different source, e.g. home screen or context menu).
        g.set_show_series(false);
        g.set_show_season(false);
        g.set_playback_from_detail(from_detail);
        if !from_series {
            g.set_playback_from_series(false);
            g.set_playback_from_season(false);
        }
    }

    // Consume keep_playlist: plays not initiated from the playlist/queue wipe any
    // leftover playlist so a stale album can't resume after this item ends (CR10-2).
    let keep_playlist = {
        let mut vs = video.lock().unwrap();
        let keep = vs.keep_playlist;
        vs.keep_playlist = false;
        if !keep {
            vs.playlist.clear();
            vs.playlist_index = 0;
            vs.queue.clear();
            vs.shuffle_order.clear();
        }
        keep
    };
    if !keep_playlist {
        if let Some(w) = window_weak.upgrade() {
            let g = AppState::get(&w);
            g.set_show_queue_panel(false);
            crate::push_queue_display(&video.lock().unwrap(), &g);
        }
    }

    if item_type == "Episode" {
        // Reset intro/credits state before spawning fetch tasks so that if the response
        // arrives before Player::new completes the result is not wiped by the init block.
        {
            let mut vs = video.lock().unwrap();
            vs.intro_timestamps = None;
            vs.recap_timestamps = None;
            vs.preview_timestamps = None;
            vs.commercial_timestamps = None;
            vs.intro_skip_shown = false;
            vs.recap_skip_shown = false;
            vs.preview_skip_shown = false;
            vs.commercial_skip_shown = false;
            vs.skip_segment_end = None;
            vs.credits_start    = None;
        }

        // Intro + credits timestamps (Intro Skipper v2+: single call returns both)
        let client_ts  = Arc::clone(&client);
        let video_ts   = Arc::clone(video);
        let item_id_ts = item_id.clone();
        rt_handle.spawn(async move {
            match client_ts.get_episode_timestamps(&item_id_ts).await {
                Ok(Some(ts)) => {
                    let mut vs = video_ts.lock().unwrap();
                    if vs.playback_generation != my_gen {
                        debug!("episode timestamps for {} arrived late — discarding", item_id_ts);
                        return;
                    }
                    let mut any = false;
                    if ts.introduction.valid() {
                        info!("intro: start={:.1}s end={:.1}s", ts.introduction.start, ts.introduction.end);
                        vs.intro_timestamps = Some(ts.introduction.clone());
                        any = true;
                    }
                    if ts.recap.valid() {
                        info!("recap: start={:.1}s end={:.1}s", ts.recap.start, ts.recap.end);
                        vs.recap_timestamps = Some(ts.recap.clone());
                        any = true;
                    }
                    if ts.preview.valid() {
                        info!("preview: start={:.1}s end={:.1}s", ts.preview.start, ts.preview.end);
                        vs.preview_timestamps = Some(ts.preview.clone());
                        any = true;
                    }
                    if ts.commercial.valid() {
                        info!("commercial: start={:.1}s end={:.1}s", ts.commercial.start, ts.commercial.end);
                        vs.commercial_timestamps = Some(ts.commercial.clone());
                        any = true;
                    }
                    if ts.credits.valid() {
                        info!("credits start: {:.1}s", ts.credits.start);
                        vs.credits_start = Some(ts.credits.start);
                        any = true;
                    }
                    if !any {
                        info!("no segments for {} (plugin absent or episode not analyzed)", item_id_ts);
                    }
                }
                Ok(None) => info!("no episode timestamps for {} (plugin absent or episode not analyzed)", item_id_ts),
                Err(e)   => warn!("episode timestamps fetch failed: {:#}", e),
            }
        });
    }

    let (dropped, dec_dropped) = video.lock().unwrap().player.as_ref()
        .map(|p| p.get_drop_counts()).unwrap_or((0, 0));
    info!("playback replaced: frame-drops={} decoder-drops={}", dropped, dec_dropped);
    let (prev_item_id, prev_client, prev_cookie, prev_ticks) = {
        tear_down_player(&mut video.lock().unwrap())
    };
    uninhibit_screensaver(prev_cookie);
    if let (Some(id), Some(cli)) = (prev_item_id, prev_client) {
        rt_handle.spawn(async move {
            if let Err(e) = cli.report_playback_stopped(&id, prev_ticks).await {
                warn!("report_playback_stopped (replaced) failed: {e}");
            }
        });
    }

    // Send start report only after the previous stop has been dispatched (CR-3).
    {
        let client2  = Arc::clone(&client);
        let item_id2 = item_id.clone();
        rt_handle.spawn(async move {
            if let Err(e) = client2.report_playback_start(&item_id2).await {
                warn!("report_playback_start failed: {e}");
            }
        });
    }

    let client_art = Arc::clone(&client);
    let item_id_art = item_id.clone();
    let is_audio    = item_type == "Audio";

    match Player::new(&url, &config) {
        Ok(player) => {
            {
                let mut vs               = video.lock().unwrap();
                vs.player                = Some(player);
                vs.item_id               = Some(item_id);
                vs.playing_series_id     = series_id;
                vs.client                = Some(client);
                vs.play_start            = Some(Instant::now());
                vs.decoder_logged        = false;
                vs.tracks_loaded         = false;
                vs.pos_tick              = 0;
                vs.controls_idle_ticks   = 0;
                vs.last_known_pos_ticks  = 0;
                // For Episodes: intro_timestamps/intro_skip_shown/credits_start were reset
                // before the fetch tasks were spawned — don't clear them here or a fast
                // response would be silently wiped.
                // For non-episodes (movies): no tasks run, so reset explicitly.
                if item_type != "Episode" {
                    vs.intro_timestamps = None;
                    vs.recap_timestamps = None;
                    vs.preview_timestamps = None;
                    vs.commercial_timestamps = None;
                    vs.intro_skip_shown = false;
                    vs.recap_skip_shown = false;
                    vs.preview_skip_shown = false;
                    vs.commercial_skip_shown = false;
                    vs.skip_segment_end = None;
                    vs.credits_start    = None;
                }
                vs.skip_segment_handled   = false;
                vs.skip_timed_shown_at    = None;
                vs.skip_timed_prompt_secs = 8;
                vs.next_ep_banner_shown  = false;
                vs.next_ep_pending       = None;
                vs.screensaver_cookie    = inhibit_screensaver();
                vs.chapters              = Vec::new();
                vs.chapters_loaded       = false;
                vs.chapter_load_attempts = 0;
                vs.chapter_osd_ticks     = 0;
                vs.delay_osd_ticks       = 0;
                vs.lyrics                = None;
                vs.lyrics_available      = false;
            }
            if let Some(w) = window_weak.upgrade() {
                let g = AppState::get(&w);
                g.set_playing_title(ss(&title));
                g.set_is_playing_audio(is_audio);
                g.set_playing_has_album_art(false);
                if is_audio {
                    // Audio-only: show music bar, not the fullscreen player.
                    let (artist, album_art_id) = audio_meta
                        .as_ref()
                        .map(|(a, i)| (a.as_str(), i.as_str()))
                        .unwrap_or(("", ""));
                    g.set_is_audio_playing(true);
                    g.set_is_playing(false);
                    g.set_has_background_player(false);
                    g.set_video_behind_ui(false);
                    g.set_music_bar_title(ss(&title));
                    g.set_music_bar_artist(artist.into());
                    g.set_music_bar_album_id(album_art_id.into());
                    g.set_music_bar_has_art(false);
                    g.set_music_bar_paused(false);
                    g.set_music_bar_pos(0.0);
                    g.set_music_bar_elapsed("0:00".into());
                    g.set_music_bar_total("0:00".into());
                    // Clear lyrics for the new track; lyrics fetch will re-populate.
                    g.set_lyrics_available(false);
                    g.set_show_lyrics(false);
                    g.set_lyrics_active_idx(-1);
                    g.set_lyrics_lines(ModelRc::new(VecModel::<crate::LyricEntry>::default()));
                } else {
                    // Video: fullscreen player as before.
                    g.set_is_audio_playing(false);
                    g.set_is_playing(true);
                    g.set_has_background_player(false);
                    g.set_video_behind_ui(false);
                    g.set_is_paused(false);
                    g.set_controls_visible(false);
                }
            }
            // For audio tracks: fetch album art for music bar (and player background).
            if is_audio {
                let ww_art = window_weak.clone();
                let art_id = audio_meta.as_ref().map(|(_, i)| i.clone()).unwrap_or_else(|| item_id_art.clone());
                rt_handle.spawn(async move {
                    if let Some(bytes) = crate::poster::fetch_poster_cached(&client_art, &art_id).await {
                        if let Some(spb) = crate::poster::decode_poster_buffer(&bytes) {
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww_art.upgrade() {
                                    let g = AppState::get(&w);
                                    if g.get_is_audio_playing() {
                                        let img = slint::Image::from_rgba8(spb);
                                        g.set_music_bar_art(img.clone());
                                        g.set_music_bar_has_art(true);
                                        g.set_playing_album_art(img);
                                        g.set_playing_has_album_art(true);
                                    }
                                }
                            });
                        }
                    }
                });

                // Fetch lyrics (Jellyfin 10.9+; gracefully absent when 404).
                let client_lyr  = Arc::clone(&video.lock().unwrap().client.as_ref().expect("client just set"));
                let item_id_lyr = item_id_art.clone();
                let video_lyr   = Arc::clone(video);
                let ww_lyr      = window_weak.clone();
                rt_handle.spawn(async move {
                    match client_lyr.get_lyrics(&item_id_lyr).await {
                        Ok(Some(lines)) => {
                            // Check generation hasn't moved.
                            let gen_ok = {
                                let vs = video_lyr.lock().unwrap();
                                vs.playback_generation == my_gen
                            };
                            if !gen_ok { return; }
                            {
                                let mut vs = video_lyr.lock().unwrap();
                                vs.lyrics           = Some(lines.clone());
                                vs.lyrics_available = true;
                            }
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww_lyr.upgrade() {
                                    let g = AppState::get(&w);
                                    if g.get_is_audio_playing() {
                                        use slint::{ModelRc, VecModel};
                                        let entries: Vec<crate::LyricEntry> = lines.into_iter()
                                            .map(|(ms, text)| crate::LyricEntry {
                                                text:     text.as_str().into(),
                                                start_ms: ms as i32,
                                            })
                                            .collect();
                                        g.set_lyrics_lines(ModelRc::new(VecModel::from(entries)));
                                        g.set_lyrics_available(true);
                                        g.set_lyrics_active_idx(-1);
                                    }
                                }
                            });
                        }
                        Ok(None) => {
                            debug!("no lyrics for {} (not found or server too old)", item_id_lyr);
                        }
                        Err(e) => {
                            debug!("lyrics fetch failed for {}: {:#}", item_id_lyr, e);
                        }
                    }
                });
            }
        }
        Err(e) => {
            error!("player init failed: {:#}", e);
            // Clear timestamp fields so a fast async response for this failed item
            // can't leave stale segment data for a subsequent play.
            {
                let mut vs = video.lock().unwrap();
                vs.intro_timestamps      = None;
                vs.recap_timestamps      = None;
                vs.preview_timestamps    = None;
                vs.commercial_timestamps = None;
                vs.credits_start         = None;
            }
            if let Some(w) = window_weak.upgrade() {
                reset_playback_ui(&w);
            }
            crate::show_toast(window_weak.clone(), "Couldn't start playback — check your server connection".to_string());
        }
    }
}

// ── playlist_prev / playlist_next ────────────────────────────────────────────
// Called by queue-prev-track / queue-next-track callbacks in main.rs.
// prev: if pos < 2 s and index > 0 → go back; else restart current.
// next: advance to next in playlist (or queue if no playlist).
// Returns Some(QueueItem) if a new item should start; None if nothing to do.

pub(crate) fn playlist_prev(vs: &mut VideoState) -> Option<QueueItem> {
    if vs.playlist.is_empty() { return None; }
    let pos = vs.player.as_ref().map(|p| p.get_position()).unwrap_or(0.0);
    let new_idx = if pos < 2.0 && vs.playlist_index > 0 {
        if vs.shuffle && !vs.shuffle_order.is_empty() {
            let cur_pos = vs.shuffle_order.iter()
                .position(|&i| i == vs.playlist_index)
                .unwrap_or(0);
            if cur_pos > 0 {
                let prev_idx = vs.shuffle_order[cur_pos - 1];
                vs.playlist_index = prev_idx;
                prev_idx
            } else {
                vs.playlist_index
            }
        } else {
            vs.playlist_index -= 1;
            vs.playlist_index
        }
    } else {
        // Restart current track (seek to 0 instead of re-starting via start_playback).
        // Return None so caller just seeks, unless pos < 2 and no prev.
        if pos < 2.0 {
            return None; // already at start, nothing to go back to
        }
        return None; // seek-to-0 handled by caller
    };
    Some(vs.playlist[new_idx].clone())
}

pub(crate) fn playlist_next(vs: &mut VideoState) -> Option<QueueItem> {
    let len = vs.playlist.len();
    if len == 0 {
        // Fall back to context-menu queue.
        return if vs.queue.is_empty() { None } else { Some(vs.queue.remove(0)) };
    }
    let next_idx = if vs.shuffle && !vs.shuffle_order.is_empty() {
        let cur_pos = vs.shuffle_order.iter()
            .position(|&i| i == vs.playlist_index)
            .unwrap_or(0);
        let next_pos = cur_pos + 1;
        match vs.repeat_mode {
            RepeatMode::Off => vs.shuffle_order.get(next_pos).copied()?,
            RepeatMode::All | RepeatMode::One => vs.shuffle_order[next_pos % len],
        }
    } else {
        let next = vs.playlist_index + 1;
        match vs.repeat_mode {
            RepeatMode::Off => {
                if next < len { next } else { return None; }
            }
            RepeatMode::All | RepeatMode::One => next % len,
        }
    };
    vs.playlist_index = next_idx;
    Some(vs.playlist[next_idx].clone())
}

pub(crate) fn toggle_shuffle(vs: &mut VideoState) {
    vs.shuffle = !vs.shuffle;
    if vs.shuffle && !vs.playlist.is_empty() {
        vs.shuffle_order = shuffle_indices(vs.playlist.len());
        // Move current item to position 0 in shuffle order so next advances naturally.
        if let Some(pos) = vs.shuffle_order.iter().position(|&i| i == vs.playlist_index) {
            vs.shuffle_order.swap(0, pos);
        }
    }
}

// ── wire_rendering_notifier ───────────────────────────────────────────────────
pub(crate) fn wire_rendering_notifier(
    window: &MainWindow,
    video:  Arc<Mutex<VideoState>>,
) {
    let video_rn  = video;
    let window_rn = window.as_weak();

    window.window().set_rendering_notifier({
        let mut gl_loaded = false;

        move |state_rn, api| {
            match state_rn {
                slint::RenderingState::RenderingSetup => {
                    if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                        if !gl_loaded {
                            gl::load_with(|name| {
                                let cname = std::ffi::CString::new(name).unwrap();
                                get_proc_address(cname.as_c_str())
                            });
                            gl_loaded = true;
                            info!("OpenGL loaded");
                        }
                    }
                }

                slint::RenderingState::BeforeRendering => {
                    let Some(win) = window_rn.upgrade() else { return; };
                    let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api else { return; };

                    let mut vs = video_rn.lock().unwrap();
                    vs.did_render = false;

                    if vs.fbos[0] != 0 && vs.player.is_none() {
                        unsafe {
                            delete_fbo(vs.fbos[0], vs.textures[0]);
                            delete_fbo(vs.fbos[1], vs.textures[1]);
                        }
                        vs.fbos = [0; 2]; vs.textures = [0; 2];
                        vs.fbo_w = 0; vs.fbo_h = 0;
                        return;
                    }

                    if vs.player.is_none() { return; }

                    if vs.render_ctx.is_none() {
                        let handle = vs.player.as_ref().unwrap().raw_handle_ptr();
                        match unsafe { MpvRenderCtx::new(handle, get_proc_address) } {
                            Ok(mut ctx) => {
                                let ww = window_rn.clone();
                                ctx.set_update_callback(move || {
                                    let ww2 = ww.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(w) = ww2.upgrade() {
                                            w.window().request_redraw();
                                        }
                                    });
                                });
                                vs.render_ctx = Some(ctx);
                                info!("mpv render context created");
                            }
                            Err(e) => { error!("MpvRenderCtx::new: {:#}", e); return; }
                        }
                    }

                    let phys = win.window().size();
                    let w = phys.width.max(1);
                    let h = phys.height.max(1);

                    if vs.fbos[0] == 0 || vs.fbo_w != w || vs.fbo_h != h {
                        unsafe {
                            delete_fbo(vs.fbos[0], vs.textures[0]);
                            delete_fbo(vs.fbos[1], vs.textures[1]);
                        }
                        let r0 = unsafe { create_fbo(w, h) };
                        let r1 = unsafe { create_fbo(w, h) };
                        match (r0, r1) {
                            (Some((f0, t0)), Some((f1, t1))) => {
                                vs.fbos = [f0, f1]; vs.textures = [t0, t1];
                                vs.fbo_w = w; vs.fbo_h = h; vs.back = 0;
                            }
                            (p0, p1) => {
                                if let Some((f, t)) = p0 { unsafe { delete_fbo(f, t); } }
                                if let Some((f, t)) = p1 { unsafe { delete_fbo(f, t); } }
                                vs.fbos = [0; 2]; vs.textures = [0; 2];
                                return;
                            }
                        }
                    }

                    if let Some(ctx) = vs.render_ctx.as_ref() {
                        let b = vs.back;
                        if let Err(e) = ctx.render(vs.fbos[b] as i32, w as i32, h as i32, true) {
                            warn!("mpv render: {:#}", e);
                        } else {
                            vs.did_render = true;
                        }

                        if let Some(tex_id) = NonZeroU32::new(vs.textures[b]) {
                            let size = euclid::default::Size2D::new(w, h);
                            let img = unsafe {
                                slint::BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(tex_id, size)
                                    .origin(slint::BorrowedOpenGLTextureOrigin::BottomLeft)
                                    .build()
                            };
                            AppState::get(&win).set_video_frame(img);
                        }

                        vs.back = 1 - b;
                    }
                }

                slint::RenderingState::AfterRendering => {
                    let vs = video_rn.lock().unwrap();
                    if vs.did_render {
                        if let Some(ctx) = vs.render_ctx.as_ref() {
                            ctx.report_swap();
                        }
                    }
                }

                slint::RenderingState::RenderingTeardown => {
                    let mut vs = video_rn.lock().unwrap();
                    vs.render_ctx = None;
                    unsafe {
                        delete_fbo(vs.fbos[0], vs.textures[0]);
                        delete_fbo(vs.fbos[1], vs.textures[1]);
                    }
                    vs.fbos = [0; 2]; vs.textures = [0; 2];
                }

                _ => {}
            }
        }
    }).ok();
}

// ── wire_mpv_timer ────────────────────────────────────────────────────────────
pub(crate) fn wire_mpv_timer(
    window_weak:    slint::Weak<MainWindow>,
    video:          Arc<Mutex<VideoState>>,
    state:          Arc<Mutex<FjordState>>,
    rt_handle:      tokio::runtime::Handle,
    controls_show:  Arc<AtomicBool>,
    seek_suppress:  Arc<AtomicU32>,
) -> slint::Timer {
    let video_timer  = video;
    let window_timer = window_weak;
    let state_timer  = state;

    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
        let (finished, banner_trigger) = {
            let mut vs = video_timer.lock().unwrap();
            let mut banner_trigger: Option<(String, Option<Arc<JellyfinClient>>, u32, bool)> = None;

            if vs.player.is_some() {
                let elapsed_ok = vs.play_start.map_or(false, |t| t.elapsed() >= Duration::from_secs(2));

                if elapsed_ok && !vs.decoder_logged {
                    if let Some(p) = vs.player.as_ref() {
                        p.log_decoder_info();
                        p.apply_auto_vf();
                    }
                    vs.decoder_logged = true;
                }

                // ── Chapter list loading ──────────────────────────────────────
                // Poll chapter-list/count after the 2 s decoder-logged gate.
                // Retry up to 30 ticks (~480 ms) to handle containers where the
                // chapter metadata appears slightly after the first track data.
                // A count of 0 after 30 attempts is treated as "no chapters".
                if elapsed_ok && !vs.chapters_loaded {
                    if let Some(p) = vs.player.as_ref() {
                        let count = p.get_chapter_count();
                        if count > 0 {
                            let dur      = p.get_duration();
                            let chapters = p.get_chapters();
                            info!("loaded {} chapters", chapters.len());
                            let marks: Vec<f32> = if dur > 0.0 {
                                chapters.iter().map(|(t, _)| (t / dur) as f32).collect()
                            } else {
                                vec![]
                            };
                            if let Some(w) = window_timer.upgrade() {
                                let g = AppState::get(&w);
                                g.set_chapter_marks(
                                    ModelRc::new(VecModel::from(marks)),
                                );
                                let entries: Vec<TrackEntry> = chapters.iter().enumerate().map(|(i, (t, title))| {
                                    let ts = fmt_secs(*t).to_string();
                                    let label = if title.is_empty() {
                                        ts
                                    } else {
                                        format!("{ts}  {title}")
                                    };
                                    TrackEntry { id: i as i32, label: label.into() }
                                }).collect();
                                g.set_chapter_entries(ModelRc::new(VecModel::from(entries)));
                            }
                            vs.chapters = chapters;
                            vs.chapters_loaded = true;
                        } else if vs.chapter_load_attempts >= 30 {
                            debug!("no chapters after 30 attempts");
                            vs.chapters_loaded = true;
                        } else {
                            vs.chapter_load_attempts += 1;
                        }
                    }
                }

                // ── Chapter OSD countdown ─────────────────────────────────────
                if vs.chapter_osd_ticks > 0 {
                    vs.chapter_osd_ticks -= 1;
                    if vs.chapter_osd_ticks == 0 {
                        if let Some(w) = window_timer.upgrade() {
                            AppState::get(&w).set_chapter_osd_visible(false);
                        }
                    }
                }

                // ── Current chapter tracking ─────────────────────────────────
                if vs.chapters_loaded && !vs.chapters.is_empty() {
                    if let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                        let pos  = p.get_position();
                        let new_ch = vs.chapters.iter().rposition(|(t, _)| pos >= *t)
                            .map(|i| i as i32).unwrap_or(-1);
                        let g = AppState::get(&w);
                        if g.get_current_chapter() != new_ch {
                            g.set_current_chapter(new_ch);
                        }
                    }
                }

                // ── Sub / audio delay OSD countdown ───────────────────────────
                if vs.delay_osd_ticks > 0 {
                    vs.delay_osd_ticks -= 1;
                    if vs.delay_osd_ticks == 0 {
                        if let Some(w) = window_timer.upgrade() {
                            AppState::get(&w).set_delay_osd_visible(false);
                        }
                    }
                }
                if elapsed_ok && !vs.tracks_loaded {
                    if let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                        let tracks = p.get_tracks();
                        // Retry next tick if mpv hasn't parsed the track list yet.
                        if !tracks.is_empty() {
                            debug!("track-list ({} entries):", tracks.len());
                            for t in &tracks {
                                debug!("  [{:>2}] {:5}  selected={}  lang={:5}  title={:?}  codec={}",
                                    t.id, t.track_type, t.selected, t.lang, t.title, t.codec);
                            }
                            let sub_model   = build_track_model(&tracks, "sub");
                            let audio_model = build_track_model(&tracks, "audio");
                            let video_model = build_track_model(&tracks, "video");
                            let mut cur_sub = tracks.iter().find(|t| t.track_type == "sub" && t.selected).map(|t| t.id).unwrap_or(0);
                            let cur_audio = tracks.iter().find(|t| t.track_type == "audio" && t.selected).map(|t| t.id).unwrap_or(1);
                            let cur_video = tracks.iter().find(|t| t.track_type == "video" && t.selected).map(|t| t.id).unwrap_or(1);
                            debug!("active tracks: sub={} audio={} video={}", cur_sub, cur_audio, cur_video);
                            let g = AppState::get(&w);

                            // Subtitle auto-select: global off → force 0; else try primary then fallback.
                            if !g.get_settings_sub_enabled() {
                                if let Some(p) = vs.player.as_ref() { p.set_sub_track(0); }
                                cur_sub = 0;
                            } else {
                                let pref1 = g.get_settings_sub_lang().to_string();
                                let pref2 = g.get_settings_sub_lang2().to_string();
                                let sub_type = g.get_settings_sub_type().to_string();
                                let codes: Vec<&str> = [pref1.as_str(), pref2.as_str()]
                                    .iter().map(|n| sub_lang_code(n)).filter(|c| !c.is_empty()).collect();
                                if !codes.is_empty() {
                                    // 0=Normal, 1=SDH, 2=Forced — type priority per preference.
                                    let kind_of = |t: &TrackInfo| -> u8 {
                                        if t.hearing_impaired { 1 } else if t.forced { 2 } else { 0 }
                                    };
                                    let priority: &[u8] = match sub_type.as_str() {
                                        "Forced"           => &[2, 0, 1],
                                        "Hearing Impaired" => &[1, 0, 2],
                                        _                  => &[0, 1, 2], // Normal / Any / empty
                                    };
                                    // Outer loop: type priority; inner loop: language codes.
                                    // A preferred-type match in pref1_lang beats a fallback-type
                                    // match in either language.
                                    let found = priority.iter().find_map(|&want_kind| {
                                        codes.iter().find_map(|code| {
                                            tracks.iter().find(|t| {
                                                t.track_type == "sub"
                                                && t.lang.to_ascii_lowercase().starts_with(code)
                                                && kind_of(t) == want_kind
                                            })
                                        })
                                    });
                                    if let Some(t) = found {
                                        info!("auto-selected sub {} (lang={} forced={} hi={}) pref_lang={:?}/{:?} pref_type={:?}",
                                            t.id, t.lang, t.forced, t.hearing_impaired, pref1, pref2, sub_type);
                                        if let Some(p) = vs.player.as_ref() { p.set_sub_track(t.id); }
                                        cur_sub = t.id;
                                    }
                                    // No match → leave mpv default unchanged
                                }
                            }

                            // Audio language auto-select: if preference set, pick first matching track.
                            let audio_lang_pref = g.get_settings_audio_lang().to_string();
                            let audio_code = sub_lang_code(&audio_lang_pref);
                            if !audio_code.is_empty() {
                                let audio_tracks: Vec<_> = tracks.iter()
                                    .filter(|t| t.track_type == "audio").collect();
                                if audio_tracks.len() > 1 {
                                    let found = audio_tracks.iter().find(|t| {
                                        t.lang.to_ascii_lowercase().starts_with(audio_code)
                                    });
                                    if let Some(t) = found {
                                        info!("auto-selected audio {} (lang={}) pref={:?}", t.id, t.lang, audio_lang_pref);
                                        if let Some(p) = vs.player.as_ref() { p.set_audio_track(t.id); }
                                    }
                                    // No match → leave mpv default unchanged
                                }
                            }
                            g.set_sub_tracks(sub_model);
                            g.set_audio_tracks(audio_model);
                            g.set_video_tracks(video_model);
                            g.set_current_sub_id(cur_sub as i32);
                            g.set_current_audio_id(cur_audio as i32);
                            g.set_current_video_id(cur_video as i32);
                            vs.tracks_loaded = true;
                        }
                    }
                }

                vs.pos_tick = vs.pos_tick.wrapping_add(1);
                if vs.pos_tick % 30 == 0 {
                    if let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                        let pos = p.get_position();
                        let dur = p.get_duration();
                        let (buf_active, buf_pct) = p.get_buffering();
                        let buffered_pos = p.get_buffer_end_fraction();
                        // Done with p (releases immutable borrow on vs)
                        let _ = p;
                        // Also show buffering overlay during initial load: player alive but no
                        // video data yet after 500 ms grace period (covers HDD spin-up delays
                        // where paused-for-cache is false because playback hasn't started yet).
                        let initial_stall = vs.play_start
                            .map_or(false, |t| t.elapsed() >= Duration::from_millis(500))
                            && dur == 0.0;
                        let buf_active = buf_active || initial_stall;
                        if pos > 0.0 { vs.last_known_pos_ticks = (pos * 10_000_000.0) as i64; }
                        let g = AppState::get(&w);
                        // Suppress position updates while a committed seek is settling.
                        // seek_committed stores 3; each timer tick decrements until 0.
                        // This gives mpv ~1440 ms to update time-pos before we read it,
                        // preventing the bar from jumping back to the pre-seek position.
                        let suppressed = {
                            let n = seek_suppress.load(Ordering::Relaxed);
                            if n > 0 { seek_suppress.fetch_sub(1, Ordering::Relaxed); true }
                            else { false }
                        };
                        if !suppressed {
                            let ratio = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
                            g.set_playback_pos(ratio);
                            g.set_playback_time(fmt_secs(pos));
                            g.set_playback_ends_at(fmt_ends_at(dur - pos));
                            // Also drive music bar position when audio-only
                            if g.get_is_audio_playing() {
                                g.set_music_bar_pos(ratio);
                                g.set_music_bar_elapsed(fmt_secs(pos));
                            }
                        }
                        g.set_playback_total(fmt_secs(dur));
                        g.set_playback_total_secs(dur as f32);
                        if g.get_is_audio_playing() {
                            g.set_music_bar_total(fmt_secs(dur));
                        }
                        g.set_buffering_active(buf_active);
                        g.set_buffering_pct(buf_pct);
                        g.set_buffered_pos(buffered_pos);

                        // ── Lyrics active-line tracking ───────────────────────
                        if g.get_show_lyrics() && g.get_is_audio_playing() {
                            if let Some(lyrics) = vs.lyrics.as_ref() {
                                let pos_ms = (pos * 1000.0) as u64;
                                // Find last line whose start_ms ≤ current position.
                                let new_idx = lyrics.iter()
                                    .rposition(|(ms, _)| *ms > 0 && *ms <= pos_ms)
                                    .map(|i| i as i32)
                                    .unwrap_or(-1);
                                if g.get_lyrics_active_idx() != new_idx {
                                    g.set_lyrics_active_idx(new_idx);
                                }
                            }
                        }

                        // Report progress to Jellyfin every ~10 s.
                        if vs.pos_tick % 600 == 0 {
                            if let (Some(cli), Some(id)) = (vs.client.as_ref().map(Arc::clone), vs.item_id.clone()) {
                                let ticks  = (pos * 10_000_000.0) as i64;
                                let paused = g.get_is_paused();
                                rt_handle.spawn(async move {
                                    if let Err(e) = cli.report_playback_progress(&id, ticks, paused).await {
                                        warn!("report_playback_progress failed: {e}");
                                    }
                                });
                            }
                        }
                    }
                }

                // ── Stats poll every ~512 ms (CR2-7, CR2-8) ──────────────────
                // Full poll when overlay is visible; 1 read for passthrough only
                // when hidden so the volume-control guard stays current.
                if vs.pos_tick % 32 == 0 {
                    if let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                        if AppState::get(&w).get_stats_visible() {
                            let stats = p.poll_stats();
                            update_stats_window(&w, &stats);
                        } else {
                            AppState::get(&w).set_audio_passthrough_active(p.poll_passthrough());
                        }
                    }
                }

                // ── Periodic frame-drop log every 5 min ───────────────────────
                if vs.pos_tick > 0 && vs.pos_tick % 18750 == 0 {
                    if let Some(p) = vs.player.as_ref() {
                        let (drops, dec_drops) = p.get_drop_counts();
                        let pos = p.get_position();
                        info!("stats at {:.0}s: frame-drops={} decoder-drops={}", pos, drops, dec_drops);
                    }
                }

                // ── Skip segment prompt (Intro / Recap / Preview / Commercial) ─
                // Determine active segment in priority order; dispatch by mode:
                //   always-skip → seek immediately, no overlay
                //   ask         → show single "Skip →" button
                //   ask-timed   → show two-button overlay + countdown; auto-seek on expiry
                //   never-skip  → do nothing
                if vs.player.is_some() {
                    let pos = vs.player.as_ref().unwrap().get_position();
                    let seg_in = |t: &Option<Segment>| t.as_ref().map_or(false, |s| pos >= s.start && pos < s.end);

                    // (label, end, key) — key used to look up mode/secs from AppState
                    let seg_info: Option<(&str, f64, &str)> =
                        if seg_in(&vs.intro_timestamps) {
                            vs.intro_timestamps.as_ref().map(|s| ("Skip Intro →", s.end, "intro"))
                        } else if seg_in(&vs.recap_timestamps) {
                            vs.recap_timestamps.as_ref().map(|s| ("Skip Recap →", s.end, "recap"))
                        } else if seg_in(&vs.preview_timestamps) {
                            vs.preview_timestamps.as_ref().map(|s| ("Skip Preview →", s.end, "preview"))
                        } else if seg_in(&vs.commercial_timestamps) {
                            vs.commercial_timestamps.as_ref().map(|s| ("Skip Commercial →", s.end, "commercial"))
                        } else {
                            None
                        };

                    if let Some((label, seg_end, seg_key)) = seg_info {
                        vs.skip_segment_end = Some(seg_end);

                        // Read mode + secs from AppState (timer runs on Slint event loop thread)
                        let (mode, prompt_secs) = if let Some(w) = window_timer.upgrade() {
                            let g = AppState::get(&w);
                            match seg_key {
                                "intro"      => (g.get_settings_skip_intro_mode().to_string(),      g.get_settings_skip_intro_secs() as u32),
                                "recap"      => (g.get_settings_skip_recap_mode().to_string(),      g.get_settings_skip_recap_secs() as u32),
                                "preview"    => (g.get_settings_skip_preview_mode().to_string(),    g.get_settings_skip_preview_secs() as u32),
                                "commercial" => (g.get_settings_skip_commercial_mode().to_string(), g.get_settings_skip_commercial_secs() as u32),
                                _            => ("ask".to_string(), 8u32),
                            }
                        } else {
                            ("ask".to_string(), 8u32)
                        };

                        if vs.skip_segment_handled {
                            // Already handled — ensure overlays are hidden
                            if let Some(w) = window_timer.upgrade() {
                                let g = AppState::get(&w);
                                if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                                if g.get_show_skip_timed()   { g.set_show_skip_timed(false); }
                            }
                        } else {
                            match mode.as_str() {
                                "always-skip" => {
                                    vs.skip_segment_handled = true;
                                    vs.player.as_ref().unwrap().seek_to(seg_end);
                                    info!("always-skip: seeking to {:.1}s", seg_end);
                                    if let Some(w) = window_timer.upgrade() {
                                        let g = AppState::get(&w);
                                        if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                                        if g.get_show_skip_timed()   { g.set_show_skip_timed(false); }
                                    }
                                }
                                "ask" => {
                                    if let Some(w) = window_timer.upgrade() {
                                        let g = AppState::get(&w);
                                        if g.get_show_skip_timed() {
                                            g.set_show_skip_timed(false);
                                            vs.skip_timed_shown_at = None;
                                        }
                                        if !g.get_show_skip_segment() {
                                            g.set_show_skip_segment(true);
                                            g.set_skip_segment_label(label.into());
                                        }
                                    }
                                }
                                "ask-timed" => {
                                    if let Some(w) = window_timer.upgrade() {
                                        let g = AppState::get(&w);
                                        if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                                        if vs.skip_timed_shown_at.is_none() {
                                            // First tick in segment: start countdown
                                            vs.skip_timed_shown_at    = Some(Instant::now());
                                            vs.skip_timed_prompt_secs = prompt_secs;
                                            g.set_skip_timed_label(label.into());
                                            g.set_skip_timed_secs(prompt_secs as i32);
                                            g.set_skip_timed_focused(0);
                                            g.set_show_skip_timed(true);
                                        } else {
                                            // Update countdown each tick
                                            let elapsed = vs.skip_timed_shown_at.unwrap().elapsed();
                                            let remaining = (vs.skip_timed_prompt_secs as f64 - elapsed.as_secs_f64())
                                                .max(0.0).ceil() as i32;
                                            if remaining != g.get_skip_timed_secs() {
                                                g.set_skip_timed_secs(remaining);
                                            }
                                            if remaining <= 0 {
                                                // Countdown expired — auto-seek
                                                g.set_show_skip_timed(false);
                                                vs.skip_timed_shown_at  = None;
                                                vs.skip_segment_handled = true;
                                                vs.player.as_ref().unwrap().seek_to(seg_end);
                                                info!("ask-timed auto-skip: seeking to {:.1}s", seg_end);
                                            }
                                        }
                                    }
                                }
                                _ => { // "never-skip" or unrecognized
                                    if let Some(w) = window_timer.upgrade() {
                                        let g = AppState::get(&w);
                                        if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                                        if g.get_show_skip_timed() {
                                            g.set_show_skip_timed(false);
                                            vs.skip_timed_shown_at = None;
                                        }
                                    }
                                }
                            }
                        }

                        vs.intro_skip_shown      = seg_key == "intro"      && mode == "ask" && !vs.skip_segment_handled;
                        vs.recap_skip_shown      = seg_key == "recap"      && mode == "ask" && !vs.skip_segment_handled;
                        vs.preview_skip_shown    = seg_key == "preview"    && mode == "ask" && !vs.skip_segment_handled;
                        vs.commercial_skip_shown = seg_key == "commercial" && mode == "ask" && !vs.skip_segment_handled;
                    } else {
                        // No segment active — clear all state
                        if let Some(w) = window_timer.upgrade() {
                            let g = AppState::get(&w);
                            if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                            if g.get_show_skip_timed()   { g.set_show_skip_timed(false); }
                        }
                        vs.intro_skip_shown      = false;
                        vs.recap_skip_shown      = false;
                        vs.preview_skip_shown    = false;
                        vs.commercial_skip_shown = false;
                        vs.skip_segment_end      = None;
                        vs.skip_timed_shown_at   = None;
                        vs.skip_segment_handled  = false;
                    }
                }

                // ── Up Next banner trigger ────────────────────────────────────
                // Fire once per episode when position reaches credits_start or
                // falls within the last 30 s of the runtime (fallback when the
                // Intro Skipper Credits endpoint is unavailable).
                // Respects skip_credits_mode: always-skip → immediate auto-advance,
                // ask → show banner with countdown, never-skip → no trigger.
                if !vs.next_ep_banner_shown && vs.playing_series_id.is_some() {
                    if let Some(p) = vs.player.as_ref() {
                        let pos = p.get_position();
                        let dur = p.get_duration();
                        let credits_fire = vs.credits_start.map_or(false, |c| c > 0.0 && pos >= c);
                        // Require dur >= 60 s so the banner doesn't fire instantly on short clips.
                        let fallback_fire = dur >= 60.0 && pos > 0.0 && dur - pos <= 30.0;
                        if credits_fire || fallback_fire {
                            let (credits_mode, credits_secs) = window_timer.upgrade()
                                .map(|w| {
                                    let g = AppState::get(&w);
                                    (g.get_settings_skip_credits_mode().to_string(),
                                     g.get_settings_skip_credits_secs() as u32)
                                })
                                .unwrap_or_else(|| ("ask".to_string(), 30u32));
                            if credits_mode != "never-skip" {
                                vs.next_ep_banner_shown = true;
                                // always-skip: secs=0 (countdown loop is empty), no banner shown
                                let (secs, show_banner) = if credits_mode == "always-skip" {
                                    (0u32, false)
                                } else {
                                    (credits_secs, true)
                                };
                                banner_trigger = Some((
                                    vs.playing_series_id.clone().unwrap(),
                                    vs.client.as_ref().map(Arc::clone),
                                    secs,
                                    show_banner,
                                ));
                            }
                        }
                    }
                }

                // Seek accumulation debounce (~480 ms = 30 × 16 ms)
                if vs.seek_pending_ticks > 0 {
                    vs.seek_pending_ticks -= 1;
                    if vs.seek_pending_ticks == 0 {
                        let pending = vs.seek_pending_secs;
                        vs.seek_pending_secs = 0.0;
                        if pending.abs() > 0.001 {
                            if let Some(p) = vs.player.as_ref() {
                                if pending > 0.0 { p.seek_forward(pending); }
                                else             { p.seek_backward(-pending); }
                            }
                        }
                        if let Some(w) = window_timer.upgrade() {
                            let g = AppState::get(&w);
                            g.set_seek_osd_visible(false);
                            g.set_seek_bar_pos(0.0);
                            g.set_seek_bar_time("".into());
                            g.set_seek_delta_text("".into());
                        }
                    }
                }

                if controls_show.swap(false, Ordering::Relaxed) {
                    vs.controls_idle_ticks = 0;
                } else {
                    vs.controls_idle_ticks = vs.controls_idle_ticks.saturating_add(1);
                }
                if vs.controls_idle_ticks == 187 {
                    if let Some(w) = window_timer.upgrade() {
                        let g = AppState::get(&w);
                        g.set_controls_visible(false);
                        // Force Slint to re-evaluate the cursor at the last-known position.
                        // Slint only calls set_cursor_visible() during mouse event processing;
                        // dispatching PointerMoved at the same coordinates triggers that path
                        // without changing mouse-x/y (so show-controls won't fire).
                        let cx = g.get_player_cursor_x();
                        let cy = g.get_player_cursor_y();
                        w.window().dispatch_event(WindowEvent::PointerMoved {
                            position: LogicalPosition::new(cx as f32, cy as f32),
                        });
                    }
                }
            }

            let finished = if let Some(player) = vs.player.as_mut() {
                matches!(player.poll(), PollResult::Finished)
            } else {
                false
            };

            (finished, banner_trigger)
        };

        // ── Spawn Up Next countdown task when trigger fired ───────────────────
        if let Some((series_id, Some(cli), credits_secs, show_banner)) = banner_trigger {
            let state2  = Arc::clone(&state_timer);
            let video2  = Arc::clone(&video_timer);
            let ww2     = window_timer.clone();
            let rt2     = rt_handle.clone();
            // Capture generation so rapid episode skips cancel the old task immediately
            // instead of waiting up to 1 s for the next loop tick (CR2-10).
            let my_gen          = video_timer.lock().unwrap().playback_generation;
            let current_item_id = video_timer.lock().unwrap().item_id.clone();
            rt_handle.spawn(async move {
                let Ok(Some(mut next)) = cli.get_next_up_for_series(&series_id).await else { return; };
                // Banner fires ~30 s before end. Jellyfin hasn't received a stop/played report
                // yet, so it still considers the current episode unplayed and returns it as
                // "next up". Resolve the true next episode READ-ONLY from the series episode
                // list (CR10-13) — the old approach marked the current episode played here,
                // which stuck even when the user cancelled the banner and stopped watching.
                if current_item_id.as_deref() == Some(next.id.as_str()) {
                    let Ok(eps) = cli.get_series_episodes(&series_id).await else { return; };
                    let Some(pos) = eps.iter().position(|e| e.id == next.id) else { return; };
                    let Some(real_next) = eps.into_iter().nth(pos + 1) else { return; };
                    next = real_next;
                }
                info!("up-next: queued {} (secs={} banner={})", next.id, credits_secs, show_banner);

                // Check generation and set next_ep_pending in one lock scope — holding the lock
                // across both prevents start_playback from incrementing the generation and
                // clearing next_ep_pending between the guard and the write.
                {
                    let mut vs = video2.lock().unwrap();
                    if vs.player.is_none() || vs.playback_generation != my_gen { return; }
                    vs.next_ep_pending = Some(next.clone());
                }

                if show_banner {
                    let title_str = next.display_name();
                    let t = SharedString::from(title_str.as_str());
                    let next_ep_secs = next.run_time_ticks.unwrap_or(0) as f64 / 10_000_000.0;
                    let ends_at = fmt_ends_at(next_ep_secs);
                    let _ = slint::invoke_from_event_loop({
                        let ww = ww2.clone();
                        move || {
                            if let Some(w) = ww.upgrade() {
                                let g = AppState::get(&w);
                                g.set_next_ep_title(t);
                                g.set_next_ep_ends_at(ends_at);
                                g.set_next_ep_secs(credits_secs as i32);
                                g.set_next_ep_banner_focused(0);
                                g.set_show_next_ep_banner(true);
                            }
                        }
                    });
                }

                // Count down credits_secs → 1, checking each second for cancellation.
                // When credits_secs == 0 (always-skip mode), loop body never executes.
                for remaining in (1i32..=credits_secs as i32).rev() {
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    let (still_playing, pending_ok, gen_ok) = {
                        let vs = video2.lock().unwrap();
                        (vs.player.is_some(), vs.next_ep_pending.is_some(),
                         vs.playback_generation == my_gen)
                    };
                    if !still_playing || !pending_ok || !gen_ok {
                        // !still_playing: video ended naturally — let the natural-end path in
                        //   the 16 ms timer take() next_ep_pending and advance. Clearing it here
                        //   would race with that path and silently drop the episode advance.
                        // !gen_ok: start_playback already cleared next_ep_pending.
                        // !pending_ok: user pressed Skip/cancel, already cleared.
                        return;
                    }
                    if show_banner {
                        let _ = slint::invoke_from_event_loop({
                            let ww = ww2.clone();
                            move || {
                                if let Some(w) = ww.upgrade() {
                                    AppState::get(&w).set_next_ep_secs(remaining);
                                }
                            }
                        });
                    }
                }

                // Countdown reached 0 (or was 0 for always-skip) — play next now.
                let next = video2.lock().unwrap().next_ep_pending.take();
                let Some(next) = next else { return; };

                let config = state2.lock().unwrap().player_config();
                let cli2   = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                let Some(cli2) = cli2 else { return; };

                let url        = cli2.direct_play_url(&next.id);
                let title      = next.display_name();
                let ep_id      = next.id.clone();
                let series_id2 = next.series_id.clone();
                info!("up-next countdown expired, starting {}", ep_id);

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        AppState::get(&w).set_show_next_ep_banner(false);
                        start_playback(url, ep_id, "Episode", title, config, cli2,
                                       series_id2, None, &video2, &ww2, &rt2);
                    }
                });
            });
        }

        if finished {
            let (dropped, dec_dropped) = video_timer.lock().unwrap().player.as_ref()
                .map(|p| p.get_drop_counts()).unwrap_or((0, 0));
            info!("playback finished: frame-drops={} decoder-drops={}", dropped, dec_dropped);
            let (had_series, advance_series_id) = {
                let vs = video_timer.lock().unwrap();
                (vs.playing_series_id.is_some(), vs.playing_series_id.clone())
            };
            let (item_id, client, ss_cookie, final_ticks) = {
                let mut vs = video_timer.lock().unwrap();
                vs.playing_series_id = None;
                tear_down_player(&mut vs)
            };
            uninhibit_screensaver(ss_cookie);

            if let Some(w) = window_timer.upgrade() { reset_playback_ui(&w); }

            // Stop report then home refresh, sequenced so Jellyfin has processed the stop
            // before we fetch continue-watching.
            if let (Some(id), Some(cli)) = (item_id, client) {
                let ww_home  = window_timer.clone();
                let rth_home = rt_handle.clone();
                rt_handle.spawn(async move {
                    if let Err(e) = cli.report_playback_stopped(&id, final_ticks).await {
                        warn!("report_playback_stopped (natural end) failed: {e}");
                    }
                    let home_data = fetch_home_data(&cli).await;
                    let sections  = home_data_sections(&home_data);
                    let ww2       = ww_home.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() { push_home_data(&w, &home_data); }
                    });
                    spawn_poster_loading(cli, sections, ww_home, rth_home);
                });
            }

            // Playlist/queue advance when not a series item.
            if !had_series {
                let next_item: Option<QueueItem> = {
                    let mut vs = video_timer.lock().unwrap();
                    if !vs.playlist.is_empty() {
                        // Playlist mode (album/artist): advance with repeat/shuffle logic.
                        let len      = vs.playlist.len();
                        let next_idx = match vs.repeat_mode {
                            RepeatMode::One => Some(vs.playlist_index), // restart same track
                            RepeatMode::Off | RepeatMode::All => {
                                if vs.shuffle && !vs.shuffle_order.is_empty() {
                                    let cur_pos = vs.shuffle_order.iter()
                                        .position(|&i| i == vs.playlist_index)
                                        .unwrap_or(0);
                                    let next_pos = cur_pos + 1;
                                    match vs.repeat_mode {
                                        RepeatMode::Off => vs.shuffle_order.get(next_pos).copied(),
                                        RepeatMode::All => Some(vs.shuffle_order[next_pos % len]),
                                        RepeatMode::One => unreachable!(),
                                    }
                                } else {
                                    let next = vs.playlist_index + 1;
                                    match vs.repeat_mode {
                                        RepeatMode::Off => if next < len { Some(next) } else { None },
                                        RepeatMode::All => Some(next % len),
                                        RepeatMode::One => unreachable!(),
                                    }
                                }
                            }
                        };
                        if let Some(idx) = next_idx {
                            vs.playlist_index = idx;
                            vs.keep_playlist  = true; // playlist advance — start_playback must not wipe it
                            Some(vs.playlist[idx].clone())
                        } else {
                            None
                        }
                    } else if !vs.queue.is_empty() {
                        // Context-menu queue: pop from front.
                        vs.keep_playlist = true; // keep the remaining queue across start_playback
                        Some(vs.queue.remove(0))
                    } else {
                        None
                    }
                };

                if let Some(q) = next_item {
                    let config = state_timer.lock().unwrap().player_config();
                    let cli    = state_timer.lock().unwrap().client.as_ref().map(Arc::clone);
                    if let Some(cli) = cli {
                        let remaining = upcoming_count(&video_timer.lock().unwrap());
                        let audio_m  = q.audio_meta.clone();
                        let url      = cli.direct_play_url(&q.id);
                        let ww_q     = window_timer.clone();
                        let vid_q    = Arc::clone(&video_timer);
                        let rt_q     = rt_handle.clone();
                        info!("playlist/queue advance: starting {} ({} remaining)", q.id, remaining);
                        let vid_rq = Arc::clone(&vid_q);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww_q.upgrade() {
                                let g = AppState::get(&w);
                                crate::push_queue_display(&vid_rq.lock().unwrap(), &g);
                                start_playback(url, q.id, &q.item_type, q.title, config, cli,
                                               q.series_id, audio_m, &vid_q, &ww_q, &rt_q);
                            }
                        });
                    }
                }
            }

            if had_series {
                let next = video_timer.lock().unwrap().next_ep_pending.take();
                if let Some(next) = next {
                    let config = state_timer.lock().unwrap().player_config();
                    let cli    = state_timer.lock().unwrap().client.as_ref().map(Arc::clone);
                    if let Some(cli) = cli {
                        let url        = cli.direct_play_url(&next.id);
                        let title      = next.display_name();
                        let ep_id      = next.id.clone();
                        let series_id  = next.series_id.clone();
                        info!("natural end with pending next-ep, starting {}", ep_id);
                        if let Some(w) = window_timer.upgrade() {
                            AppState::get(&w).set_show_next_ep_banner(false);
                        }
                        start_playback(url, ep_id, "Episode", title, config, cli,
                                       series_id, None, &video_timer, &window_timer, &rt_handle);
                    }
                } else if let Some(sid) = advance_series_id {
                    // EOF arrived before the background next-up fetch completed.
                    // The countdown task bails when player.is_none(), so next_ep_pending was
                    // never set. Spawn a fresh fetch as a fallback — but only when the credits
                    // mode actually wants an advance (never-skip means stop here).
                    let skip_mode = state_timer.lock().unwrap().config.skip_credits_mode.clone();
                    if skip_mode != "never-skip" {
                        let end_gen = video_timer.lock().unwrap().playback_generation;
                        let video2  = Arc::clone(&video_timer);
                        let state2  = Arc::clone(&state_timer);
                        let ww2     = window_timer.clone();
                        let rt2     = rt_handle.clone();
                        rt_handle.spawn(async move {
                            let cli = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                            let Some(cli) = cli else { return; };
                            let Ok(Some(next)) = cli.get_next_up_for_series(&sid).await else { return; };
                            // Bail if the user started watching something else.
                            if video2.lock().unwrap().playback_generation != end_gen { return; }
                            let config = state2.lock().unwrap().player_config();
                            let cli2   = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                            let Some(cli2) = cli2 else { return; };
                            let url   = cli2.direct_play_url(&next.id);
                            let title = next.display_name();
                            let ep_id = next.id.clone();
                            let sid2  = next.series_id.clone();
                            info!("natural-end fallback advance: starting {}", ep_id);
                            let _ = slint::invoke_from_event_loop(move || {
                                if ww2.upgrade().is_some() {
                                    start_playback(url, ep_id, "Episode", title, config, cli2,
                                                   sid2, None, &video2, &ww2, &rt2);
                                }
                            });
                        });
                    }
                }
            }
        }
    });
    timer
}
