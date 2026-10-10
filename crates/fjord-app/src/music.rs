// ── fjord-app · music.rs ─────────────────────────────────────────────────────
//   Music bar, Now Playing, queue controls/panel and lyrics callbacks (moved from main(), 0.5.0 step 3)
//   wire_music_bar         music bar, Now Playing, queue panel open
//   wire_queue             queue prev/next/shuffle/repeat, queue panel, lyrics
// ─────────────────────────────────────────────────────────────────────────────

// ── wire_music_bar (moved from main(), 0.5.0 step 3) ─────────────────────
/// Wires music bar, Now Playing, queue panel open: music_bar_play_pause, music_bar_stop, music_bar_seek, music_bar_seek_rel, music_bar_open_album, open_now_playing, open_queue_panel.
pub(crate) fn wire_music_bar(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    video: &std::sync::Arc<std::sync::Mutex<crate::playback::VideoState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    let video = std::sync::Arc::clone(video);
    // ── Music bar callbacks ───────────────────────────────────────────────────
    {
        let ww_mb = window.as_weak();
        AppState::get(&window).on_music_bar_play_pause(move || {
            // Delegate to pause_play_toggle which also updates is_paused + music_bar_paused.
            if let Some(w) = ww_mb.upgrade() {
                AppState::get(&w).invoke_pause_play_toggle();
            }
        });
    }
    {
        let video_ms = Arc::clone(&video);
        let ww_ms = window.as_weak();
        let rt_ms = rt.handle().clone();
        let state_ms = Arc::clone(&state);
        AppState::get(&window).on_music_bar_stop(move || {
            crate::playback::do_stop_playback(&video_ms, &ww_ms, &rt_ms, &state_ms);
        });
    }
    {
        let video_msk = Arc::clone(&video);
        AppState::get(&window).on_music_bar_seek(move |ratio| {
            let vs = video_msk.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                let dur = p.get_duration();
                if dur > 0.0 {
                    p.seek_to(ratio as f64 * dur);
                }
            }
        });
    }
    {
        let video_msr = Arc::clone(&video);
        AppState::get(&window).on_music_bar_seek_rel(move |secs| {
            let vs = video_msr.lock().unwrap();
            if let Some(p) = vs.player.as_ref() {
                if secs >= 0.0 {
                    p.seek_forward(secs as f64);
                } else {
                    p.seek_backward(-secs as f64);
                }
            }
        });
    }
    {
        let state_mo = Arc::clone(&state);
        let ww_mo = window.as_weak();
        let rt_mo = rt.handle().clone();
        AppState::get(&window).on_music_bar_open_album(move || {
            let Some(w) = ww_mo.upgrade() else { return };
            let g = AppState::get(&w);
            let id = g.get_music_bar_album_id().to_string();
            if id.is_empty() {
                return;
            }
            let title = "".to_string(); // open_album_screen fetches the real title
            album::open_album_screen(
                id,
                title,
                Arc::clone(&state_mo),
                ww_mo.clone(),
                rt_mo.clone(),
            );
        });
    }
    {
        let ww_np = window.as_weak();
        AppState::get(&window).on_open_now_playing(move || {
            let Some(w) = ww_np.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_now_playing_back_focused(false);
            g.set_now_playing_in_strip(false);
            g.set_now_playing_ctrl_focused(2); // play/pause
            g.set_now_playing_strip_focused(0);
            // Refreshes queue-items AND kicks off art loading (on_refresh_queue_display
            // → spawn_queue_poster_loading) — the Up Next strip reads the same model
            // the Queue Panel does, but nothing else triggers the art fetch for it.
            g.invoke_refresh_queue_display();
            g.set_show_now_playing(true);
            // The three entry paths (mouse click, m key, idle auto-open) can each
            // land here with the global FocusScope having lost focus in between —
            // most reliably on idle auto-open, since nothing guarantees fs still
            // holds focus after a stretch of mouse-only interaction. Re-grab
            // unconditionally, matching the pattern used by every other
            // screen-open site (season/person/detail/series/auth).
            w.invoke_grab_keyboard_focus();
        });
    }
    {
        let ww_qp = window.as_weak();
        // Mouse entry point for the queue panel (music-bar ⋮ button). Mirrors the
        // 'q' keyboard path in keys.rs, which doesn't need a focus re-grab since a
        // keypress reaching handle_key already proves fs has focus — a mouse click
        // after a stretch of mouse-only interaction can't assume that (CR11-6).
        AppState::get(&window).on_open_queue_panel(move || {
            let Some(w) = ww_qp.upgrade() else { return };
            let g = AppState::get(&w);
            g.invoke_refresh_queue_display();
            g.set_queue_panel_cursor(0);
            let items = g.get_queue_items();
            for i in 0..items.row_count() {
                if let Some(e) = items.row_data(i)
                    && e.is_current
                {
                    g.set_queue_panel_cursor(i as i32);
                    break;
                }
            }
            g.set_show_queue_panel(true);
            w.invoke_grab_keyboard_focus();
        });
    }
}

// ── wire_queue (moved from main(), 0.5.0 step 3) ─────────────────────────
/// Wires queue prev/next/shuffle/repeat, queue panel, lyrics: queue_prev_track, queue_next_track, toggle_shuffle, cycle_repeat, refresh_queue_display, queue_jump, queue_remove, queue_clear, toggle_lyrics.
pub(crate) fn wire_queue(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    video: &std::sync::Arc<std::sync::Mutex<crate::playback::VideoState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    let video = std::sync::Arc::clone(video);
    // ── queue prev / next / shuffle / repeat ──────────────────────────────────
    {
        let video_qp = Arc::clone(&video);
        let state_qp = Arc::clone(&state);
        let ww_qp = window.as_weak();
        let rt_qp = rt.handle().clone();
        AppState::get(&window).on_queue_prev_track(move || {
            let (item, should_seek_start) = {
                let mut vs = video_qp.lock().unwrap();
                let pos = vs.player.as_ref().map(|p| p.get_position()).unwrap_or(0.0);
                let qi = crate::playback::playlist_prev(&mut vs);
                // None means either seek-to-0 (pos >= 2s) or already at start
                (qi, pos >= 2.0)
            };
            match item {
                Some(qi) => {
                    let s = state_qp.lock().unwrap();
                    let Some(client) = s.client.as_ref().map(Arc::clone) else {
                        return;
                    };
                    let mut config = s.player_config();
                    config.start_position_secs = None;
                    drop(s);
                    let url = client.direct_play_url(&qi.id);
                    let am = qi.audio_meta.clone();
                    start_playback(
                        url,
                        qi.id.clone(),
                        &qi.item_type,
                        qi.title.clone(),
                        config,
                        client,
                        qi.series_id.clone(),
                        am,
                        &video_qp,
                        &ww_qp,
                        &rt_qp,
                        &state_qp,
                        None,
                    );
                }
                None if should_seek_start => {
                    // pos >= 2s and no prev: restart current track from 0
                    if let Some(p) = video_qp.lock().unwrap().player.as_ref() {
                        p.seek_to(0.0)
                    }
                }
                None => {} // already at start, nothing to do
            }
        });
    }
    {
        let video_qn = Arc::clone(&video);
        let state_qn = Arc::clone(&state);
        let ww_qn = window.as_weak();
        let rt_qn = rt.handle().clone();
        AppState::get(&window).on_queue_next_track(move || {
            let item = {
                let mut vs = video_qn.lock().unwrap();
                crate::playback::playlist_next(&mut vs)
            };
            if let Some(qi) = item {
                let s = state_qn.lock().unwrap();
                let Some(client) = s.client.as_ref().map(Arc::clone) else {
                    return;
                };
                let mut config = s.player_config();
                config.start_position_secs = None;
                drop(s);
                let url = client.direct_play_url(&qi.id);
                let am = qi.audio_meta.clone();
                start_playback(
                    url,
                    qi.id.clone(),
                    &qi.item_type,
                    qi.title.clone(),
                    config,
                    client,
                    qi.series_id.clone(),
                    am,
                    &video_qn,
                    &ww_qn,
                    &rt_qn,
                    &state_qn,
                    None,
                );
            }
        });
    }
    {
        let video_ts = Arc::clone(&video);
        let ww_ts = window.as_weak();
        AppState::get(&window).on_toggle_shuffle(move || {
            let shuffled = {
                let mut vs = video_ts.lock().unwrap();
                crate::playback::toggle_shuffle(&mut vs);
                vs.shuffle
            };
            if let Some(w) = ww_ts.upgrade() {
                let g = AppState::get(&w);
                g.set_queue_shuffle(shuffled);
                push_queue_display(&video_ts.lock().unwrap(), &g);
            }
        });
    }
    {
        let video_cr = Arc::clone(&video);
        let ww_cr = window.as_weak();
        AppState::get(&window).on_cycle_repeat(move || {
            use crate::playback::RepeatMode;
            let next_mode = {
                let mut vs = video_cr.lock().unwrap();
                crate::playback::invalidate_preload(&mut vs);
                vs.repeat_mode = match vs.repeat_mode {
                    RepeatMode::Off => RepeatMode::All,
                    RepeatMode::All => RepeatMode::One,
                    RepeatMode::One => RepeatMode::Off,
                };
                info!("repeat mode -> {:?}", vs.repeat_mode);
                vs.repeat_mode as i32
            };
            if let Some(w) = ww_cr.upgrade() {
                AppState::get(&w).set_queue_repeat_mode(next_mode);
            }
        });
    }

    // ── queue panel: refresh / jump / remove / clear ──────────────────────────
    {
        let video_rq = Arc::clone(&video);
        let state_rq = Arc::clone(&state);
        let ww_rq = window.as_weak();
        let rt_rq = rt.handle().clone();
        AppState::get(&window).on_refresh_queue_display(move || {
            let Some(w) = ww_rq.upgrade() else { return };
            push_queue_display(&video_rq.lock().unwrap(), &AppState::get(&w));
            // Spawn poster loading for the freshly-built model
            let client = state_rq.lock().unwrap().client.as_ref().map(Arc::clone);
            if let Some(cli) = client {
                spawn_queue_poster_loading(cli, ww_rq.clone(), rt_rq.clone());
            }
        });
    }
    {
        let video_qj = Arc::clone(&video);
        let state_qj = Arc::clone(&state);
        let ww_qj = window.as_weak();
        let rt_qj = rt.handle().clone();
        AppState::get(&window).on_queue_jump(move |idx| {
            // idx is QueueEntry.index: the UNDERLYING position — 0..playlist.len()
            // are playlist tracks, after that context-menu queue items (CR10-6);
            // -1 is the synthetic now-playing row (already playing — nothing to do).
            if idx < 0 {
                return;
            }
            let item = {
                let mut vs = video_qj.lock().unwrap();
                let idx = idx as usize;
                if idx < vs.playlist.len() {
                    vs.playlist_index = idx;
                    vs.playlist[idx].clone()
                } else {
                    let qidx = idx - vs.playlist.len();
                    if qidx >= vs.queue.len() {
                        return;
                    }
                    vs.queue.remove(qidx)
                }
            };
            let s = state_qj.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else {
                return;
            };
            let mut config = s.player_config();
            config.start_position_secs = None;
            drop(s);
            let url = client.direct_play_url(&item.id);
            let am = item.audio_meta.clone();
            if let Some(w) = ww_qj.upgrade() {
                AppState::get(&w).set_show_queue_panel(false);
            }
            start_playback(
                url,
                item.id.clone(),
                &item.item_type,
                item.title.clone(),
                config,
                client,
                item.series_id.clone(),
                am,
                &video_qj,
                &ww_qj,
                &rt_qj,
                &state_qj,
                None,
            );
        });
    }
    {
        let video_qr = Arc::clone(&video);
        let ww_qr = window.as_weak();
        AppState::get(&window).on_queue_remove(move |idx| {
            if idx < 0 {
                return;
            } // synthetic now-playing row
            let Some(w) = ww_qr.upgrade() else { return };
            let g = AppState::get(&w);
            {
                let mut vs = video_qr.lock().unwrap();
                let idx = idx as usize;
                // The currently-playing row can't be removed — the track keeps
                // playing regardless, and removing it shifted the is-current
                // highlight onto the wrong row (CR10-17).
                if !vs.playlist.is_empty() && idx == vs.playlist_index {
                    return;
                }
                if idx < vs.playlist.len() {
                    vs.playlist.remove(idx);
                    // Keep playlist_index valid after removal
                    if vs.playlist_index > idx && vs.playlist_index > 0 {
                        vs.playlist_index -= 1;
                    } else if vs.playlist_index >= vs.playlist.len() && !vs.playlist.is_empty() {
                        vs.playlist_index = vs.playlist.len() - 1;
                    }
                    // Rebuild shuffle_order from scratch (indices shifted)
                    crate::playback::rebuild_shuffle_order(&mut vs);
                } else {
                    // Context-menu queue row (CR10-6)
                    let qidx = idx - vs.playlist.len();
                    if qidx >= vs.queue.len() {
                        return;
                    }
                    vs.queue.remove(qidx);
                }
                crate::playback::invalidate_preload(&mut vs);
                push_queue_display(&vs, &g);
            }
            // Snap cursor if it's past the new end
            let len = g.get_queue_items().row_count() as i32;
            let c = g.get_queue_panel_cursor();
            if c >= len && len > 0 {
                g.set_queue_panel_cursor(len - 1);
            }
            if len == 0 {
                g.set_show_queue_panel(false);
            }
        });
    }
    {
        let video_qc = Arc::clone(&video);
        let ww_qc = window.as_weak();
        AppState::get(&window).on_queue_clear(move || {
            let Some(w) = ww_qc.upgrade() else { return };
            let g = AppState::get(&w);
            {
                let mut vs = video_qc.lock().unwrap();
                vs.playlist.clear();
                vs.playlist_index = 0;
                vs.queue.clear();
                vs.shuffle_order.clear();
                crate::playback::invalidate_preload(&mut vs);
                push_queue_display(&vs, &g); // also zeroes queue-count (CR10-6)
            }
            g.set_show_queue_panel(false);
        });
    }

    // ── lyrics toggle ─────────────────────────────────────────────────────────
    {
        let ww_lyr = window.as_weak();
        AppState::get(&window).on_toggle_lyrics(move || {
            let Some(w) = ww_lyr.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_lyrics_available() {
                g.set_show_lyrics(!g.get_show_lyrics());
            }
        });
    }
}
