use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Model};
use tracing::{debug, info};

use crate::playback::VideoState;
use crate::MainWindow;

pub(crate) fn wire_controls(window: &MainWindow, video: Arc<Mutex<VideoState>>) {
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        window.on_pause_play_toggle(move || {
            let vs = video.lock().unwrap();
            if let Some(p) = vs.player.as_ref() { p.toggle_pause(); }
            drop(vs);
            if let Some(w) = ww.upgrade() {
                let now_paused = !w.get_is_paused();
                debug!("pause_play_toggle → {}", if now_paused { "paused" } else { "playing" });
                w.set_is_paused(now_paused);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_seek_backward(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("seek_backward 10s");
                p.seek_backward(10.0);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_seek_forward(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("seek_forward 10s");
                p.seek_forward(10.0);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_seek_backward_long(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("seek_backward 30s");
                p.seek_backward(30.0);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_seek_forward_long(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("seek_forward 30s");
                p.seek_forward(30.0);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_stop_playback(move || {
            info!("stop_playback requested");
            let mut vs = video.lock().unwrap();
            vs.user_stopped = true;
            if let Some(p) = vs.player.as_ref() { p.stop(); }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_seek_to(move |ratio| {
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
    {
        let video = Arc::clone(&video);
        window.on_skip_intro(move || {
            let vs = video.lock().unwrap();
            if let (Some(ref ts), Some(p)) = (vs.intro_timestamps.as_ref(), vs.player.as_ref()) {
                info!("skip intro: seeking to {:.1}s", ts.intro_end);
                p.seek_to(ts.intro_end);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_select_sub(move |id| {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("select subtitle track id={}", id);
                p.set_sub_track(id as i64);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_select_audio(move |id| {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("select audio track id={}", id);
                p.set_audio_track(id as i64);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        window.on_commit_panel_selection(move || {
            let Some(w) = ww.upgrade() else { return };
            let panel  = w.get_player_open_panel();
            let cursor = w.get_player_panel_cursor() as usize;
            let vs = video.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                match panel {
                    1 => {
                        // Sub panel: cursor 0 = Off, 1+ = sub-tracks[cursor-1]
                        let id = if cursor == 0 {
                            0i32
                        } else {
                            w.get_sub_tracks().row_data(cursor - 1).map(|t| t.id).unwrap_or(0)
                        };
                        debug!("commit sub: cursor={} → id={}", cursor, id);
                        p.set_sub_track(id as i64);
                        w.set_current_sub_id(id);
                    }
                    2 => {
                        let id = w.get_audio_tracks().row_data(cursor).map(|t| t.id).unwrap_or(1);
                        debug!("commit audio: cursor={} → id={}", cursor, id);
                        p.set_audio_track(id as i64);
                        w.set_current_audio_id(id);
                    }
                    3 => {
                        let id = w.get_video_tracks().row_data(cursor).map(|t| t.id).unwrap_or(1);
                        debug!("commit video: cursor={} → id={}", cursor, id);
                        p.set_video_track(id as i64);
                        w.set_current_video_id(id);
                    }
                    _ => {}
                }
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_volume_up(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() { p.adjust_volume(5.0); }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_volume_down(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() { p.adjust_volume(-5.0); }
        });
    }
    {
        let video = Arc::clone(&video);
        let ww    = window.as_weak();
        window.on_show_controls(move || {
            if let Some(w) = ww.upgrade() { w.set_controls_visible(true); }
            video.lock().unwrap().controls_idle_ticks = 0;
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_select_video(move |id| {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                debug!("select video track id={}", id);
                p.set_video_track(id as i64);
            }
        });
    }
    {
        let ww = window.as_weak();
        window.on_resume_player(move || {
            let Some(w) = ww.upgrade() else { return };
            if w.get_has_background_player() {
                info!("resuming player to fullscreen");
                w.set_is_playing(true);
                w.set_has_background_player(false);
                w.set_video_behind_ui(false);
                w.set_controls_visible(true);
            }
        });
    }
    {
        let video = Arc::clone(&video);
        window.on_mute_toggle(move || {
            if let Some(p) = video.lock().unwrap().player.as_ref() {
                p.toggle_mute();
            }
        });
    }
    {
        let ww = window.as_weak();
        window.on_toggle_stats(move || {
            let Some(w) = ww.upgrade() else { return };
            w.set_stats_visible(!w.get_stats_visible());
        });
    }
    {
        let ww = window.as_weak();
        window.on_minimize_player(move || {
            let Some(w) = ww.upgrade() else { return };
            let behind = w.get_settings_video_behind();
            w.set_is_playing(false);
            w.set_has_background_player(true);
            w.set_video_behind_ui(behind);
            w.set_stats_visible(false);
        });
    }
}
