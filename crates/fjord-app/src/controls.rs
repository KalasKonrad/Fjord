// ── fjord-app · controls.rs ──────────────────────────────────────────────────
//   wire_controls  registers all AppState player callbacks on the window
//     playback     pause_play_toggle, seek_*, stop_playback
//     seek / intro seek_to (throttled ≤10/s), seek_drag_started (pause during scrub, queries mpv directly),
//                  seek_committed (seek + resume + optimistic playback-pos),
//                  skip_segment (ask mode), dismiss_skip_timed (ask-timed mode), update-seek-hover
//     track panels select_sub/audio/video, commit_panel_selection
//     volume / misc volume_up/down, show_controls, resume_player, mute, stats, minimize
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Global, Model};
use tracing::{debug, info};

use crate::AppState;
use crate::playback::{VideoState, do_stop_playback, fmt_secs};
use crate::MainWindow;

pub(crate) fn wire_controls(
    window:        &MainWindow,
    video:         Arc<Mutex<VideoState>>,
    controls_show: Arc<AtomicBool>,
    seek_suppress: Arc<AtomicU32>,
    rt_handle:     tokio::runtime::Handle,
) {
    // ── playback ──────────────────────────────────────────────────────────────
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_pause_play_toggle(move || {
            let vs = video.lock().unwrap();
            let now_paused = if let Some(p) = vs.player.as_ref() {
                p.toggle_pause();
                // Query mpv directly so we stay in sync even if mpv self-paused (CR-4).
                p.is_paused()
            } else {
                return;
            };
            drop(vs);
            if let Some(w) = ww.upgrade() {
                debug!("pause_play_toggle → {}", if now_paused { "paused" } else { "playing" });
                AppState::get(&w).set_is_paused(now_paused);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_seek_backward(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("seek_backward 10s");
                p.seek_backward(10.0);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_seek_forward(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("seek_forward 10s");
                p.seek_forward(10.0);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_seek_backward_long(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("seek_backward 30s");
                p.seek_backward(30.0);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_seek_forward_long(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("seek_forward 30s");
                p.seek_forward(30.0);
            }
        });
    }
    {
        let video  = Arc::clone(&video);
        let ww     = window.as_weak();
        let rth    = rt_handle.clone();
        AppState::get(window).on_stop_playback(move || {
            info!("stop_playback requested");
            do_stop_playback(&video, &ww, &rth);
        });
    }
    {
        let video     = Arc::clone(&video);
        // Throttle drag seeks to at most one per 100 ms — rapid seeks can cause
        // libmpv to abort internally. Initialised 200 ms in the past so the very
        // first drag seek always goes through.
        let last_seek = Arc::new(Mutex::new(
            Instant::now() - Duration::from_millis(200),
        ));
        AppState::get(window).on_seek_to(move |ratio| {
            let mut last = last_seek.lock().unwrap();
            if last.elapsed() < Duration::from_millis(100) {
                return;
            }
            *last = Instant::now();
            drop(last);
            let vs = video.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                let dur = p.get_duration();
                if dur > 0.0 {
                    let secs = ratio as f64 * dur;
                    debug!("seek_to: ratio={:.3} → {:.1}s / {:.1}s", ratio, secs, dur);
                    p.seek_to(secs);
                }
            }
        });
    }
    // seek_was_playing: true while the user is scrubbing a playing video.
    // Set in seek_drag_started, cleared and consumed in seek_committed.
    let seek_was_playing = Arc::new(AtomicBool::new(false));
    {
        let video = Arc::clone(&video);
        let swp   = Arc::clone(&seek_was_playing);
        AppState::get(window).on_seek_drag_started(move || {
            // Query mpv directly so we stay in sync even if mpv self-paused on a
            // cache underrun (CR2-1) — same fix as on_pause_play_toggle (CR-4).
            let vs = video.lock().unwrap();
            let Some(p) = vs.player.as_ref() else { return };
            let was_playing = !p.is_paused();
            swp.store(was_playing, Ordering::Relaxed);
            if was_playing {
                debug!("seek_drag_started: pausing mpv during scrub");
                p.set_paused(true);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        let swp   = Arc::clone(&seek_was_playing);
        let ss    = Arc::clone(&seek_suppress);
        AppState::get(window).on_seek_committed(move |ratio| {
            let mut did_seek = false;
            {
                let vs = video.lock().unwrap();
                if let Some(p) = vs.player.as_ref() {
                    let dur = p.get_duration();
                    // Consume seek_was_playing BEFORE the dur guard so mpv is
                    // always resumed if it was paused — even when duration is
                    // transiently 0 (early playback before mpv has parsed the file).
                    let should_resume = swp.swap(false, Ordering::Relaxed);
                    if dur > 0.0 {
                        let secs = ratio as f64 * dur;
                        debug!("seek_committed: ratio={:.3} → {:.1}s / {:.1}s", ratio, secs, dur);
                        p.seek_to(secs);
                        did_seek = true;
                    }
                    if should_resume {
                        debug!("seek_committed: resuming mpv after scrub");
                        p.set_paused(false);
                    }
                }
            } // release video lock before touching AppState
            if let Some(w) = ww.upgrade() {
                if did_seek {
                    // Suppress the next 3 timer position-update ticks (~1440 ms)
                    // so they don't overwrite the optimistic playback-pos before
                    // mpv's reported time-pos has caught up with the committed seek.
                    ss.store(3, Ordering::Relaxed);
                    AppState::get(&w).set_playback_pos(ratio);
                }
                // Safety path: clear seek-dragging even if compositor stole the
                // pointer-up event so Space/K/P are never permanently blocked.
                AppState::get(&w).set_seek_dragging(false);
            }
        });
    }
    // ── seek hover tooltip ────────────────────────────────────────────────────
    {
        let ww = window.as_weak();
        AppState::get(window).on_update_seek_hover(move |fraction| {
            let Some(w) = ww.upgrade() else { return };
            let g     = AppState::get(&w);
            let total = g.get_playback_total_secs() as f64;
            let secs  = (fraction as f64 * total).max(0.0);
            g.set_seek_hover_time(fmt_secs(secs));
        });
    }
    // ── seek / skip segment ───────────────────────────────────────────────────
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_skip_segment(move || {
            let vs = video.lock().unwrap();
            if let (Some(end), Some(p)) = (vs.skip_segment_end, vs.player.as_ref()) {
                info!("skip segment: seeking to {:.1}s", end);
                p.seek_to(end);
            }
        });
    }
    {
        // dismiss_skip_timed: user pressed "Don't Skip" in the ask-timed overlay.
        // Mark the segment handled so it won't re-show, and hide the overlay immediately.
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_dismiss_skip_timed(move || {
            {
                let mut vs = video.lock().unwrap();
                vs.skip_segment_handled = true;
                vs.skip_timed_shown_at  = None;
            }
            if let Some(w) = ww.upgrade() {
                AppState::get(&w).set_show_skip_timed(false);
            }
        });
    }
    // ── track panels ──────────────────────────────────────────────────────────
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_select_sub(move |id| {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("select subtitle track id={}", id);
                p.set_sub_track(id as i64);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_select_audio(move |id| {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("select audio track id={}", id);
                p.set_audio_track(id as i64);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_commit_panel_selection(move || {
            let Some(w) = ww.upgrade() else { return };
            let g      = AppState::get(&w);
            let panel  = g.get_player_open_panel();
            let cursor = g.get_player_panel_cursor() as usize;
            let vs = video.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                match panel {
                    1 => {
                        let id = if cursor == 0 {
                            0i32
                        } else {
                            AppState::get(&w).get_sub_tracks().row_data(cursor - 1).map(|t| t.id).unwrap_or(0)
                        };
                        debug!("commit sub: cursor={} → id={}", cursor, id);
                        p.set_sub_track(id as i64);
                        AppState::get(&w).set_current_sub_id(id);
                    }
                    2 => {
                        let id = AppState::get(&w).get_audio_tracks().row_data(cursor).map(|t| t.id).unwrap_or(1);
                        debug!("commit audio: cursor={} → id={}", cursor, id);
                        p.set_audio_track(id as i64);
                        AppState::get(&w).set_current_audio_id(id);
                    }
                    3 => {
                        let id = AppState::get(&w).get_video_tracks().row_data(cursor).map(|t| t.id).unwrap_or(1);
                        debug!("commit video: cursor={} → id={}", cursor, id);
                        p.set_video_track(id as i64);
                        AppState::get(&w).set_current_video_id(id);
                    }
                    _ => {}
                }
            }
        });
    }
    // ── volume + overlay ──────────────────────────────────────────────────────
    // Generation counter: only the latest volume-change task hides the overlay.
    let volume_gen = Arc::new(AtomicU32::new(0));
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        let vgen  = Arc::clone(&volume_gen);
        AppState::get(window).on_volume_up(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let passthrough = g.get_audio_passthrough_active();
            {
                let vs = video.lock().unwrap();
                if vs.player.is_none() { return; }
                if !passthrough {
                    let vol = vs.player.as_ref().unwrap().adjust_volume(5.0);
                    g.set_volume_level(vol.round() as i32);
                }
            }
            g.set_show_volume_overlay(true);
            let gen  = vgen.fetch_add(1, Ordering::Relaxed) + 1;
            let vg2  = Arc::clone(&vgen);
            let ww2  = ww.clone();
            rt.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                if vg2.load(Ordering::Relaxed) == gen {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() {
                            AppState::get(&w).set_show_volume_overlay(false);
                        }
                    });
                }
            });
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        let rt    = rt_handle.clone();
        let vgen  = Arc::clone(&volume_gen);
        AppState::get(window).on_volume_down(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let passthrough = g.get_audio_passthrough_active();
            {
                let vs = video.lock().unwrap();
                if vs.player.is_none() { return; }
                if !passthrough {
                    let vol = vs.player.as_ref().unwrap().adjust_volume(-5.0);
                    g.set_volume_level(vol.round() as i32);
                }
            }
            g.set_show_volume_overlay(true);
            let gen  = vgen.fetch_add(1, Ordering::Relaxed) + 1;
            let vg2  = Arc::clone(&vgen);
            let ww2  = ww.clone();
            rt.spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(1500)).await;
                if vg2.load(Ordering::Relaxed) == gen {
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() {
                            AppState::get(&w).set_show_volume_overlay(false);
                        }
                    });
                }
            });
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_show_controls(move || {
            if let Some(w) = ww.upgrade() { AppState::get(&w).set_controls_visible(true); }
            // Signal the mpv timer to reset the idle counter on its next tick.
            // Avoids taking video.lock() here — the GL rendering notifier holds
            // that same lock during mpv_render_context_render, and changed mouse-x
            // fires for every pixel, so blocking here froze the Slint event loop.
            controls_show.store(true, Ordering::Relaxed);
        });
    }
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_select_video(move |id| {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("select video track id={}", id);
                p.set_video_track(id as i64);
            }
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_resume_player(move || {
            let Some(w) = ww.upgrade() else { return };
            if AppState::get(&w).get_has_background_player() {
                info!("resuming player to fullscreen");
                let g = AppState::get(&w);
                g.set_is_playing(true);
                g.set_has_background_player(false);
                g.set_video_behind_ui(false);
                g.set_controls_visible(true);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_mute_toggle(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                p.toggle_mute();
            }
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_toggle_stats(move || {
            let Some(w) = ww.upgrade() else { return };
            let vis = AppState::get(&w).get_stats_visible();
            AppState::get(&w).set_stats_visible(!vis);
        });
    }
    {
        let ww = window.as_weak();
        AppState::get(window).on_minimize_player(move || {
            let Some(w) = ww.upgrade() else { return };
            let behind = AppState::get(&w).get_settings_video_behind();
            let g = AppState::get(&w);
            g.set_is_playing(false);
            g.set_has_background_player(true);
            g.set_video_behind_ui(behind);
            g.set_stats_visible(false);
        });
    }
}
