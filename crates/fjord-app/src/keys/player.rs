// ── fjord-app · keys/player.rs ───────────────────────────────────────────────
//   dispatch_player    ask-timed overlay; ask overlay; Up Next banner; panel nav; player controls;
//                      chapter-prev/next (,/.); sub/audio delay (z/Z/x/X)
//   handle_key_queue_panel / handle_key_now_playing  queue panel and Now Playing screen keys
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Player dispatch ───────────────────────────────────────────────────────────

pub(crate) fn dispatch_player(action: Action, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    let panel = g.get_player_open_panel();

    // Ask-timed overlay: Left/Right toggle focus; Enter activates; Back/Esc dismisses
    if g.get_show_skip_timed() {
        match action {
            Action::Left | Action::Right | Action::SeekBackward | Action::SeekForward => {
                // Debug logging added 2026-08-28 — a real live report of
                // "left/right don't seek" turned out to have no way to
                // confirm from the log whether it was silently swallowed
                // here (this branch never logs, and neither does the
                // real seek path below it). If show-skip-timed is ever
                // left stuck true outside a genuine Intro Skipper prompt,
                // this line is what would reveal it.
                debug!(
                    "dispatch_player: {action:?} intercepted by show-skip-timed overlay, not seeking"
                );
                g.set_skip_timed_focused(1 - g.get_skip_timed_focused());
                return true;
            }
            Action::Confirm => {
                if g.get_skip_timed_focused() == 0 {
                    g.invoke_skip_segment();
                } else {
                    g.invoke_dismiss_skip_timed();
                }
                return true;
            }
            Action::Back | Action::MinimizePlayer => {
                g.invoke_dismiss_skip_timed();
                return true;
            }
            _ => {}
        }
    }

    // Ask-mode skip segment overlay: Enter skips
    if g.get_show_skip_segment() && action == Action::Confirm {
        g.invoke_skip_segment();
        return true;
    }

    // Up Next banner: Left/Right toggles focus, Enter activates focused button
    if g.get_show_next_ep_banner() {
        match action {
            Action::Left | Action::Right | Action::SeekBackward | Action::SeekForward => {
                // Debug logging added 2026-08-28 — same reasoning as the
                // show-skip-timed branch above.
                debug!(
                    "dispatch_player: {action:?} intercepted by show-next-ep-banner, not seeking"
                );
                g.set_next_ep_banner_focused(1 - g.get_next_ep_banner_focused());
                return true;
            }
            Action::Confirm => {
                if g.get_next_ep_banner_focused() == 0 {
                    g.invoke_play_next_ep();
                } else {
                    g.invoke_cancel_auto_advance();
                }
                return true;
            }
            _ => {}
        }
    }

    if action == Action::MinimizePlayer || action == Action::Back {
        if panel != 0 {
            g.set_player_open_panel(0);
            g.set_player_panel_cursor(0);
        } else if action == Action::MinimizePlayer {
            g.invoke_minimize_player();
        } else {
            g.invoke_stop_playback();
        }
        return true;
    }

    if panel != 0 {
        match action {
            // Up/Down are remapped to VolumeUp/VolumeDown in the player keymap,
            // so match both forms here to keep panel nav working.
            Action::Up | Action::VolumeUp => {
                let c = g.get_player_panel_cursor();
                if c > 0 {
                    g.set_player_panel_cursor(c - 1);
                }
                return true;
            }
            Action::Down | Action::VolumeDown => {
                let c = g.get_player_panel_cursor();
                let max = match panel {
                    1 => g.get_sub_tracks().row_count() as i32,
                    2 => (g.get_audio_tracks().row_count() as i32 - 1).max(0),
                    3 => (g.get_video_tracks().row_count() as i32 - 1).max(0),
                    _ => (g.get_chapter_entries().row_count() as i32 - 1).max(0),
                };
                if c < max {
                    g.set_player_panel_cursor(c + 1);
                }
                return true;
            }
            Action::Confirm => {
                g.invoke_commit_panel_selection();
                g.set_player_open_panel(0);
                g.set_player_panel_cursor(0);
                return true;
            }
            _ => {}
        }
    }

    match action {
        // Ignore PausePlay while the seek bar is held — Space during scrub would toggle mpv
        // back to playing while the seek bar still shows the frozen drag position.
        Action::PausePlay if g.get_seek_dragging() => true,
        Action::PausePlay => {
            if g.get_is_paused() {
                // Resuming: immediately hide everything, even if full controls were up from mouse.
                g.set_controls_visible(false);
                g.set_pause_bar_visible(false);
            } else {
                // Pausing: hide the full controls bar and show only the minimal pause bar.
                g.set_controls_visible(false);
                g.set_pause_bar_visible(true);
            }
            g.invoke_pause_play_toggle();
            true
        }
        Action::SeekBackward => {
            g.invoke_seek_acc(-(g.get_settings_seek_step_secs() as f32));
            true
        }
        Action::SeekForward => {
            g.invoke_seek_acc(g.get_settings_seek_step_secs() as f32);
            true
        }
        Action::SeekBackwardLong => {
            g.invoke_seek_acc(-(g.get_settings_seek_step_long_secs() as f32));
            true
        }
        Action::SeekForwardLong => {
            g.invoke_seek_acc(g.get_settings_seek_step_long_secs() as f32);
            true
        }
        Action::VolumeUp => {
            g.invoke_volume_up();
            true
        }
        Action::VolumeDown => {
            g.invoke_volume_down();
            true
        }
        Action::Mute => {
            g.invoke_mute_toggle();
            true
        }
        Action::ToggleStats => {
            g.invoke_toggle_stats();
            true
        }
        Action::Fullscreen => {
            g.invoke_toggle_fullscreen();
            true
        }
        Action::PanelSubtitles => {
            g.set_player_open_panel(if panel == 1 { 0 } else { 1 });
            g.set_player_panel_cursor(0);
            true
        }
        Action::PanelAudio => {
            g.set_player_open_panel(if panel == 2 { 0 } else { 2 });
            g.set_player_panel_cursor(0);
            true
        }
        Action::PanelVideo => {
            g.set_player_open_panel(if panel == 3 { 0 } else { 3 });
            g.set_player_panel_cursor(0);
            true
        }
        Action::SeekToPercent(p) => {
            g.invoke_seek_to(p as f32 / 100.0);
            true
        }
        Action::NextChapter => {
            g.invoke_chapter_next();
            true
        }
        Action::PrevChapter => {
            g.invoke_chapter_prev();
            true
        }
        Action::SubDelayIncrease => {
            g.invoke_sub_delay_inc();
            true
        }
        Action::SubDelayDecrease => {
            g.invoke_sub_delay_dec();
            true
        }
        Action::AudioDelayIncrease => {
            g.invoke_audio_delay_inc();
            true
        }
        Action::AudioDelayDecrease => {
            g.invoke_audio_delay_dec();
            true
        }
        // Playlist prev/next fire in player mode too (e.g. audio queued into video player).
        Action::PrevTrack => {
            g.invoke_queue_prev_track();
            true
        }
        Action::NextTrack => {
            g.invoke_queue_next_track();
            true
        }
        Action::OpenQueuePanel => {
            if g.get_show_queue_panel() {
                g.set_show_queue_panel(false);
            } else {
                g.invoke_refresh_queue_display();
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
            }
            true
        }
        _ => false,
    }
}

// ── Queue panel dispatch ──────────────────────────────────────────────────────

// Cursor -1 = Clear All button in the header; 0.. = list rows.
pub(crate) fn handle_key_queue_panel(action: &Action, g: &crate::AppState) -> bool {
    use slint::Model;

    // Clear-queue confirmation (2026-08-22, see show-queue-clear-confirm's
    // own doc comment in app_state.slint) — checked first, same shape as
    // every other ConfirmDialog gate in this app: intercepts all panel
    // input while open.
    if g.get_show_queue_clear_confirm() {
        match action {
            Action::Left => g.set_queue_clear_confirm_focused(0),
            Action::Right => g.set_queue_clear_confirm_focused(1),
            Action::Confirm => {
                if g.get_queue_clear_confirm_focused() == 1 {
                    g.invoke_queue_clear();
                }
                g.set_show_queue_clear_confirm(false);
            }
            Action::Back => g.set_show_queue_clear_confirm(false),
            _ => {}
        }
        return true;
    }

    match action {
        Action::Back | Action::OpenQueuePanel | Action::Left => {
            // Left closes too — the panel slides in from the right edge.
            g.set_show_queue_panel(false);
            true
        }
        Action::Up => {
            let c = g.get_queue_panel_cursor();
            if c > 0 {
                g.set_queue_panel_cursor(c - 1);
            } else if c == 0 {
                g.set_queue_panel_cursor(-1); // top row → Clear All button
            }
            true
        }
        Action::Down => {
            let c = g.get_queue_panel_cursor();
            let max = (g.get_queue_items().row_count() as i32 - 1).max(0);
            if c < max {
                g.set_queue_panel_cursor(c + 1);
            }
            true
        }
        Action::Confirm => {
            let c = g.get_queue_panel_cursor();
            if c < 0 {
                // Confirmation dialog, 2026-08-22 — see show-queue-clear-
                // confirm's own doc comment in app_state.slint.
                g.set_queue_clear_confirm_focused(0);
                g.set_show_queue_clear_confirm(true);
                return true;
            }
            // Rows carry their UNDERLYING index (played rows are hidden, so the
            // visual position no longer matches the playlist position).
            if let Some(row) = g.get_queue_items().row_data(c as usize) {
                g.invoke_queue_jump(row.index);
            }
            true
        }
        Action::DeleteItem => {
            let c = g.get_queue_panel_cursor();
            if c < 0 {
                return true;
            }
            if let Some(row) = g.get_queue_items().row_data(c as usize) {
                g.invoke_queue_remove(row.index);
            }
            true
        }
        _ => true, // absorb all other keys while panel is open
    }
}

// Cursor split: !now-playing-in-strip = transport row (0=Album 1=Prev 2=Play/
// Pause 3=Next 4=Shuffle 5=Repeat); in-strip = index into queue-items. Global
// pre-dispatch already handles PrevTrack/NextTrack/ToggleShuffle/CycleRepeat/
// ToggleLyrics/Space-pause before this runs, so only navigation reaches here.
pub(crate) fn handle_key_now_playing(action: &Action, g: &crate::AppState) -> bool {
    use slint::Model;

    // ── Back button focused (top-left, like every other detail screen) ───────
    if g.get_now_playing_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_now_playing(false);
                true
            }
            Action::Down => {
                g.set_now_playing_back_focused(false);
                true
            }
            _ => true,
        };
    }

    match action {
        Action::Back => {
            g.set_show_now_playing(false);
            true
        }
        Action::Up => {
            if g.get_now_playing_in_strip() {
                g.set_now_playing_in_strip(false); // strip → transport row
            } else {
                g.set_now_playing_back_focused(true); // transport row → Back
            }
            true
        }
        Action::Down => {
            if !g.get_now_playing_in_strip() && g.get_queue_items().row_count() > 0 {
                g.set_now_playing_in_strip(true);
            }
            true
        }
        Action::Left => {
            if g.get_now_playing_in_strip() {
                let c = g.get_now_playing_strip_focused();
                if c > 0 {
                    g.set_now_playing_strip_focused(c - 1);
                }
            } else {
                let c = g.get_now_playing_ctrl_focused();
                if c > 0 {
                    g.set_now_playing_ctrl_focused(c - 1);
                }
            }
            true
        }
        Action::Right => {
            if g.get_now_playing_in_strip() {
                let c = g.get_now_playing_strip_focused();
                let max = g.get_queue_items().row_count() as i32 - 1;
                if c < max {
                    g.set_now_playing_strip_focused(c + 1);
                }
            } else {
                let c = g.get_now_playing_ctrl_focused();
                if c < 7 {
                    g.set_now_playing_ctrl_focused(c + 1);
                }
            }
            true
        }
        Action::Confirm => {
            if g.get_now_playing_in_strip() {
                let c = g.get_now_playing_strip_focused();
                if let Some(row) = g.get_queue_items().row_data(c.max(0) as usize) {
                    g.invoke_queue_jump(row.index);
                }
            } else {
                match g.get_now_playing_ctrl_focused() {
                    0 => {
                        g.invoke_music_bar_open_album();
                        g.set_show_now_playing(false);
                    }
                    1 => g.invoke_queue_prev_track(),
                    2 => g.invoke_music_bar_play_pause(),
                    3 => g.invoke_queue_next_track(),
                    4 => g.invoke_toggle_shuffle(),
                    5 => g.invoke_cycle_repeat(),
                    6 => g.invoke_volume_down(),
                    _ => g.invoke_volume_up(),
                }
            }
            true
        }
        _ => true, // absorb all other keys while the screen is open
    }
}
