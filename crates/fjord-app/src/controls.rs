// ── fjord-app · controls.rs ──────────────────────────────────────────────────
//   wire_controls  registers all AppState player callbacks on the window
//     playback     pause_play_toggle, seek_*, stop_playback
//     seek / intro seek_to (throttled ≤10/s), seek_drag_started (pause during scrub, queries mpv directly),
//                  seek_committed (seek + resume + optimistic playback-pos),
//                  skip_segment (ask mode), dismiss_skip_timed (ask-timed mode), update-seek-hover
//     track panels select_sub/audio/video, commit_panel_selection (panels 1-4); panel 4 = chapter jump
//     volume / misc volume_up/down, show_controls, resume_player, mute, stats, minimize
//     chapters     chapter_prev/chapter_next: step ±1, compute OSD name, set chapter-osd for ~2 s
//                  chapter_jump(idx): seek to vs.chapters[idx].0; also called from commit_panel (panel=4)
//     delays       sub_delay_inc/dec (z/Z ±100 ms), audio_delay_inc/dec (x/X ±100 ms);
//                  set delay-osd-text + delay-osd-visible for ~2 s; also update sub/audio-delay-ms (Sync panel)
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use slint::{ComponentHandle, Global, Model};
use tracing::{debug, info};

use crate::AppState;
use crate::playback::{VideoState, do_stop_playback, fmt_secs};
use crate::MainWindow;

// Given the chapter list and current playback position, return the OSD name
// for the chapter we'll land on after stepping by `delta` (±1).
// Uses the same logic as mpv: stepping back when >5 s into a chapter first
// seeks to the current chapter start before going to the previous one.
fn chapter_osd_name(chapters: &[(f64, String)], pos: f64, delta: i64) -> String {
    if chapters.is_empty() { return String::new(); }
    let cur_idx = chapters.iter().enumerate().rev()
        .find(|(_, (t, _))| *t <= pos)
        .map(|(i, _)| i);
    let target_idx = match (delta.cmp(&0), cur_idx) {
        (std::cmp::Ordering::Greater, Some(idx)) => {
            let next = idx.saturating_add(delta as usize);
            if next < chapters.len() { Some(next) } else { None }
        }
        (std::cmp::Ordering::Less, Some(idx)) => {
            let cur_start = chapters[idx].0;
            if pos - cur_start > 5.0 { Some(idx) }
            else if idx > 0 { Some(idx - 1) }
            else { None }
        }
        _ => None,
    };
    if let Some(tidx) = target_idx {
        let name = &chapters[tidx].1;
        if name.is_empty() { format!("Chapter {}", tidx + 1) }
        else { name.clone() }
    } else {
        String::new()
    }
}

fn fmt_delay_ms(label: &str, delay_secs: f64) -> slint::SharedString {
    let ms = (delay_secs * 1000.0).round() as i64;
    if ms >= 0 {
        format!("{label}: +{ms} ms").into()
    } else {
        format!("{label}: {ms} ms").into()
    }
}

fn fmt_seek_delta(secs: f64) -> slint::SharedString {
    let sign = if secs >= 0.0 { "+" } else { "−" };
    let abs  = secs.abs() as u64;
    if abs < 60 {
        format!("{sign}{abs}s").into()
    } else {
        format!("{sign}{}:{:02}", abs / 60, abs % 60).into()
    }
}

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
                let g = AppState::get(&w);
                g.set_is_paused(now_paused);
                g.set_music_bar_paused(now_paused);
                // Mouse-click resume: clear the minimal pause bar if it was showing.
                if !now_paused { g.set_pause_bar_visible(false); }
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
        // Keyboard seek accumulation: +secs = forward, -secs = backward.
        // Accumulates into VideoState.seek_pending_secs and resets the debounce counter.
        // The 16ms timer in wire_mpv_timer executes the seek after ~480ms of inactivity.
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_seek_acc(move |delta| {
            let delta = delta as f64;
            let mut vs = video.lock().unwrap();
            vs.seek_pending_secs += delta;
            vs.seek_pending_ticks = 30; // ~480 ms at 16 ms/tick
            let accumulated = vs.seek_pending_secs;
            let pos = vs.player.as_ref().map(|p| p.get_position()).unwrap_or(0.0);
            let dur = vs.player.as_ref().map(|p| p.get_duration()).unwrap_or(0.0);
            drop(vs);

            let Some(w) = ww.upgrade() else { return };
            let target_secs = (pos + accumulated).clamp(0.0, if dur > 0.0 { dur } else { f64::MAX });
            let target_ratio = if dur > 0.0 { (target_secs / dur) as f32 } else { 0.0 };
            let target_time  = crate::playback::fmt_secs(target_secs);
            let delta_text   = fmt_seek_delta(accumulated);
            let g = AppState::get(&w);
            g.set_controls_visible(false);
            g.set_seek_bar_pos(target_ratio);
            g.set_seek_bar_time(target_time);
            g.set_seek_delta_text(delta_text);
            g.set_seek_osd_visible(true);
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
                    4 => {
                        if let Some((t, _)) = vs.chapters.get(cursor) {
                            debug!("chapter jump: cursor={} → {:.1}s", cursor, t);
                            p.seek_to(*t);
                        }
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
                g.set_float_card_focused(-1);
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
            let g = AppState::get(&w);
            let from_detail = g.get_playback_from_detail();
            let behind = g.get_settings_video_behind();
            g.set_is_playing(false);
            g.set_has_background_player(true);
            g.set_video_behind_ui(behind);
            g.set_stats_visible(false);
            let from_series = g.get_playback_from_series();
            if from_detail {
                // Return to detail page. Video continues in background.
                // playback-from-detail stays true so stop also restores detail.
                g.set_show_detail(true);
                g.set_detail_focused_btn(0);
                w.invoke_grab_keyboard_focus();
            } else if from_series {
                // Return to series/season screen. Video continues in background.
                // playback_from_series stays true so stop also restores the screen.
                g.set_show_series(true);
                if g.get_playback_from_season() {
                    g.set_show_season(true);
                }
                w.invoke_grab_keyboard_focus();
            } else {
                g.set_show_detail(false);
                g.set_playback_from_detail(false);
            }
        });
    }
    // ── chapter navigation ────────────────────────────────────────────────────
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_chapter_prev(move || {
            let name = {
                let vs = video.lock().unwrap();
                let Some(p) = vs.player.as_ref() else { return };
                let pos = p.get_position();
                p.chapter_step(-1);
                chapter_osd_name(&vs.chapters, pos, -1)
            };
            if !name.is_empty() {
                video.lock().unwrap().chapter_osd_ticks = 125;
                if let Some(w) = ww.upgrade() {
                    let g = AppState::get(&w);
                    g.set_chapter_osd_text(name.into());
                    g.set_chapter_osd_visible(true);
                }
            }
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_chapter_next(move || {
            let name = {
                let vs = video.lock().unwrap();
                let Some(p) = vs.player.as_ref() else { return };
                let pos = p.get_position();
                p.chapter_step(1);
                chapter_osd_name(&vs.chapters, pos, 1)
            };
            if !name.is_empty() {
                video.lock().unwrap().chapter_osd_ticks = 125;
                if let Some(w) = ww.upgrade() {
                    let g = AppState::get(&w);
                    g.set_chapter_osd_text(name.into());
                    g.set_chapter_osd_visible(true);
                }
            }
        });
    }
    // ── sub / audio delay ─────────────────────────────────────────────────────
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_sub_delay_inc(move || {
            let delay = {
                let vs = video.lock().unwrap();
                let Some(p) = vs.player.as_ref() else { return };
                p.adjust_sub_delay(100)
            };
            info!("sub-delay → {:.3}s", delay);
            video.lock().unwrap().delay_osd_ticks = 125;
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_delay_osd_text(fmt_delay_ms("Sub delay", delay));
                g.set_delay_osd_visible(true);
                g.set_sub_delay_ms((delay * 1000.0).round() as i32);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_sub_delay_dec(move || {
            let delay = {
                let vs = video.lock().unwrap();
                let Some(p) = vs.player.as_ref() else { return };
                p.adjust_sub_delay(-100)
            };
            info!("sub-delay → {:.3}s", delay);
            video.lock().unwrap().delay_osd_ticks = 125;
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_delay_osd_text(fmt_delay_ms("Sub delay", delay));
                g.set_delay_osd_visible(true);
                g.set_sub_delay_ms((delay * 1000.0).round() as i32);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_audio_delay_inc(move || {
            let delay = {
                let vs = video.lock().unwrap();
                let Some(p) = vs.player.as_ref() else { return };
                p.adjust_audio_delay(100)
            };
            info!("audio-delay → {:.3}s", delay);
            video.lock().unwrap().delay_osd_ticks = 125;
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_delay_osd_text(fmt_delay_ms("Audio delay", delay));
                g.set_delay_osd_visible(true);
                g.set_audio_delay_ms((delay * 1000.0).round() as i32);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        AppState::get(window).on_audio_delay_dec(move || {
            let delay = {
                let vs = video.lock().unwrap();
                let Some(p) = vs.player.as_ref() else { return };
                p.adjust_audio_delay(-100)
            };
            info!("audio-delay → {:.3}s", delay);
            video.lock().unwrap().delay_osd_ticks = 125;
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_delay_osd_text(fmt_delay_ms("Audio delay", delay));
                g.set_delay_osd_visible(true);
                g.set_audio_delay_ms((delay * 1000.0).round() as i32);
            }
        });
    }
    // ── chapter jump (Chapters panel click / kbd Enter) ───────────────────────
    {
        let video = Arc::clone(&video);
        AppState::get(window).on_chapter_jump(move |idx| {
            let vs = video.lock().unwrap();
            let Some(p) = vs.player.as_ref() else { return };
            if let Some((t, _)) = vs.chapters.get(idx as usize) {
                p.seek_to(*t);
            }
        });
    }
}
