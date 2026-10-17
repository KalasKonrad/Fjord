// ── fjord-app · keys/global.rs ───────────────────────────────────────────────
//   Global shortcuts, run by handle_key after the key lookup and before the per-screen dispatch:
//   GLOBAL_SHORTCUTS, in order — the first that returns Some(handled) decides the key:
//   quit / fullscreen         Ctrl+Q from any mode; F/F11 from any non-player mode
//   resume_player             R: back to the background player (not from detail-like/overlay modes)
//   focus_float_card          N: focus the mini-player bar
//   open_queue_panel          q: the queue panel (audio playing or a non-empty queue)
//   toggle_now_playing        m: Now Playing while audio plays
//   music_controls            prev/next/shuffle/repeat/lyrics while audio plays
//   music_bar_keys            arrows/Enter on a focused music-bar button (while AppState.music-bar-active)
//   mini_player_keys          arrows/Enter on the focused mini-player bar (AppState.float-card-active)
//   music_play_pause          Space/K/P pause/play while audio plays (looked up in the player map)
//   KeyCtx                    what each shortcut sees (action, mode, combo, window, state)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// One key press after the key-map lookup — what each global shortcut decides on.
#[derive(Clone)]
pub(crate) struct KeyCtx<'a> {
    pub action: Option<Action>,
    pub mode: AppMode,
    pub combo: &'a KeyCombo,
    pub window: &'a crate::MainWindow,
    pub state: &'a Arc<Mutex<FjordState>>,
}

/// Checked in this order; `Some(handled)` ends handle_key with that result, `None` passes on.
pub(crate) const GLOBAL_SHORTCUTS: &[fn(&KeyCtx) -> Option<bool>] = &[
    quit,
    fullscreen,
    resume_player,
    focus_float_card,
    open_queue_panel,
    toggle_now_playing,
    music_controls,
    music_bar_keys,
    mini_player_keys,
    music_play_pause,
];

fn quit(c: &KeyCtx) -> Option<bool> {
    let KeyCtx { action, window, .. } = c.clone();
    // Ctrl+Q quits from ANY mode — handled once here, not per screen (per-mode arms missed
    // several screens).
    if action == Some(Action::Quit) {
        crate::AppState::get(window).invoke_quit();
        return Some(true);
    }
    None
}

fn fullscreen(c: &KeyCtx) -> Option<bool> {
    let KeyCtx {
        action,
        mode,
        window,
        ..
    } = c.clone();
    // F / F11 toggles fullscreen from any non-player mode. Several focus states
    // (album/artist/collection button rows, queue panel, context menu) swallowed
    // it with catch-all arms; like Quit it has no per-screen meaning. The Player
    // arm keeps its own handling so the controls-reveal behaviour is unchanged.
    if action == Some(Action::Fullscreen) && mode != AppMode::Player {
        crate::AppState::get(window).invoke_toggle_fullscreen();
        return Some(true);
    }
    None
}

fn resume_player(c: &KeyCtx) -> Option<bool> {
    let KeyCtx {
        action,
        mode,
        window,
        ..
    } = c.clone();
    // Global R: resume the background player from any non-fullscreen, non-detail, non-overlay
    // mode (RequestDetail/RequestOptions included in the exclusions — R must not jump to the
    // player mid-request).
    if action == Some(Action::ResumePlayer)
        && !matches!(
            mode,
            AppMode::Player
                | AppMode::Person
                | AppMode::Season
                | AppMode::Detail
                | AppMode::Artist
                | AppMode::Collection
                | AppMode::Album
                | AppMode::ContextMenu
                | AppMode::QueuePanel
                | AppMode::NowPlaying
                | AppMode::RequestDetail
                | AppMode::RequestOptions
                | AppMode::Calendar
                | AppMode::CalendarDayPopup
                | AppMode::Blocklist
                | AppMode::BonfireAdmin
        )
    {
        let g = crate::AppState::get(window);
        if g.get_has_background_player() {
            g.invoke_resume_player();
            return Some(true);
        }
    }
    None
}

fn focus_float_card(c: &KeyCtx) -> Option<bool> {
    let KeyCtx {
        action,
        mode,
        window,
        ..
    } = c.clone();
    // N: focus the mini-player bar from any non-player screen.
    if action == Some(Action::FocusFloatCard)
        && mode != AppMode::Player
        && mode != AppMode::ContextMenu
    {
        let g = crate::AppState::get(window);
        if g.get_has_background_player() && !g.get_is_playing() {
            g.set_float_card_focused(0);
            return Some(true);
        }
    }
    None
}

fn open_queue_panel(c: &KeyCtx) -> Option<bool> {
    let KeyCtx {
        action,
        mode,
        window,
        ..
    } = c.clone();
    // q: the queue panel opens from any non-ContextMenu mode whenever audio is
    // playing OR the queue has content — a queue built while idle stays reachable
    // (Phase 56). Player mode keeps its own arm in dispatch_player.
    if action == Some(Action::OpenQueuePanel)
        && !matches!(mode, AppMode::ContextMenu | AppMode::Player)
    {
        let g = crate::AppState::get(window);
        if g.get_show_queue_panel() {
            g.set_show_queue_panel(false);
        } else if g.get_is_audio_playing() || g.get_queue_count() > 0 {
            g.invoke_refresh_queue_display();
            // Cursor: current item when one is playing, else the first row.
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
        } else {
            crate::show_toast(
                slint::ComponentHandle::as_weak(window),
                "Queue is empty".to_string(),
            );
        }
        return Some(true);
    }
    None
}

fn toggle_now_playing(c: &KeyCtx) -> Option<bool> {
    let KeyCtx {
        action,
        mode,
        window,
        ..
    } = c.clone();
    // m: fullscreen Now Playing screen — toggles from any non-ContextMenu/Player
    // mode while audio is playing; opening resets focus to the transport row.
    if action == Some(Action::ToggleNowPlaying)
        && !matches!(mode, AppMode::ContextMenu | AppMode::Player)
    {
        let g = crate::AppState::get(window);
        if g.get_is_audio_playing() {
            if g.get_show_now_playing() {
                g.set_show_now_playing(false);
            } else {
                g.invoke_open_now_playing();
            }
        }
        return Some(true);
    }
    None
}

fn music_controls(c: &KeyCtx) -> Option<bool> {
    let KeyCtx {
        action,
        mode,
        window,
        ..
    } = c.clone();
    // Global playlist controls when audio is playing (fire from any mode except ContextMenu).
    if mode != AppMode::ContextMenu {
        let g = crate::AppState::get(window);
        if g.get_is_audio_playing()
            && let Some(ref a) = action
        {
            match a {
                Action::PrevTrack => {
                    g.invoke_queue_prev_track();
                    return Some(true);
                }
                Action::NextTrack => {
                    g.invoke_queue_next_track();
                    return Some(true);
                }
                Action::ToggleShuffle => {
                    g.invoke_toggle_shuffle();
                    return Some(true);
                }
                Action::CycleRepeat => {
                    g.invoke_cycle_repeat();
                    return Some(true);
                }
                Action::ToggleLyrics => {
                    g.invoke_toggle_lyrics();
                    return Some(true);
                }
                _ => {}
            }
        }
    }
    None
}

fn music_bar_keys(c: &KeyCtx) -> Option<bool> {
    let KeyCtx { action, window, .. } = c.clone();
    // Music bar keyboard focus: intercept nav keys when a button is focused.
    // Layout: [art (0)] | [⏸/▶ (1)] [⏹ (2)] | [timeline (3)] | [⏮ (4)] [⏭ (5)] [⇌ (6)] [↺ (7)] [⋮ (8)] [♪ (9)] [🔉 (10)] [🔊 (11)]
    //         Left zone   Centre zone            Below buttons   Right zone (9 only when lyrics-available; 10/11 always)
    // Left/Right: 0↔1↔2 → 4↔5↔6↔7↔8↔(9)↔10↔11 (skip over 3, and over 9 when lyrics unavailable); Down from any button→3; Up from 3→1.
    // Only while AppState.music-bar-active: not under an overlay that takes the bar's keys (a
    // stale music-bar-focused from earlier keyboard use survives a mouse-driven screen switch and
    // would hijack that screen's arrows/Enter) — the same property draws the bar's focus and hides
    // the page's (bar-focus), so keys and highlight agree.
    let g = crate::AppState::get(window);
    let mf = g.get_music_bar_focused();
    if mf >= 0 && !g.get_is_audio_playing() {
        g.set_music_bar_focused(-1);
    }
    if !g.get_music_bar_active() {
        return None;
    }
    let Some(ref action) = action else {
        return Some(false);
    };
    match action {
        Action::Left => {
            match mf {
                3 => {
                    g.invoke_music_bar_seek_rel(-10.0);
                }
                4 => {
                    g.set_music_bar_focused(2);
                } // ⏮ ← ⏹ (skip timeline)
                // Slot 10 (🔉) steps back to 9 (♪) only when lyrics are available.
                10 => {
                    g.set_music_bar_focused(if g.get_lyrics_available() { 9 } else { 8 });
                }
                1 | 2 | 5 | 6 | 7 | 8 | 9 | 11 => {
                    g.set_music_bar_focused(mf - 1);
                }
                _ => {} // 0: absorbed
            }
            return Some(true);
        }
        Action::Right => {
            match mf {
                3 => {
                    g.invoke_music_bar_seek_rel(10.0);
                }
                2 => {
                    g.set_music_bar_focused(4);
                } // ⏹ → ⏮ (skip timeline)
                // Slot 8 (⋮) advances to 9 (♪) when available, else straight to 10 (🔉).
                8 => {
                    g.set_music_bar_focused(if g.get_lyrics_available() { 9 } else { 10 });
                }
                0 | 1 | 4 | 5 | 6 | 7 | 9 | 10 => {
                    g.set_music_bar_focused(mf + 1);
                }
                _ => {} // 11: absorbed
            }
            return Some(true);
        }
        Action::Down => {
            if mf != 3 {
                g.set_music_bar_focused(3);
            }
            return Some(true);
        }
        Action::Up => {
            if mf == 3 {
                g.set_music_bar_focused(1);
            } else {
                g.set_music_bar_focused(-1);
            }
            return Some(true);
        }
        Action::Confirm => {
            match mf {
                0 => {
                    g.invoke_open_now_playing();
                }
                2 => {
                    g.set_music_bar_focused(-1);
                    g.invoke_music_bar_stop();
                }
                4 => {
                    g.invoke_queue_prev_track();
                }
                5 => {
                    g.invoke_queue_next_track();
                }
                6 => {
                    g.invoke_toggle_shuffle();
                }
                7 => {
                    g.invoke_cycle_repeat();
                }
                8 => {
                    // ⋮ Queue button: open queue panel
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
                9 => {
                    g.invoke_toggle_lyrics();
                } // ♪ Lyrics
                10 => {
                    g.invoke_volume_down();
                } // 🔉
                11 => {
                    g.invoke_volume_up();
                } // 🔊
                _ => {
                    g.invoke_music_bar_play_pause();
                } // 1 or 3
            }
            return Some(true);
        }
        Action::Back => {
            g.set_music_bar_focused(-1);
            return Some(true);
        }
        _ => {}
    }
    None
}

fn mini_player_keys(c: &KeyCtx) -> Option<bool> {
    let KeyCtx { action, window, .. } = c.clone();
    // Mini-player bar focused: intercept nav keys before the underlying screen sees them —
    // only while AppState.float-card-active (same overlays as the music bar above).
    let g = crate::AppState::get(window);
    let fc = g.get_float_card_focused();
    if fc >= 0 && !g.get_has_background_player() {
        g.set_float_card_focused(-1);
    }
    if !g.get_float_card_active() {
        return None;
    }
    let Some(ref action) = action else {
        return Some(false);
    };
    match action {
        Action::Left | Action::Right => {
            g.set_float_card_focused(1 - fc);
            return Some(true);
        }
        Action::Confirm => {
            g.set_float_card_focused(-1);
            if fc == 0 {
                g.invoke_resume_player();
            } else {
                g.invoke_stop_playback();
            }
            return Some(true);
        }
        Action::Up | Action::Back => {
            g.set_float_card_focused(-1);
            return Some(true);
        }
        Action::Down => {
            return Some(true); // already at bottom, absorb
        }
        _ => {}
    }
    None
}

fn music_play_pause(c: &KeyCtx) -> Option<bool> {
    let KeyCtx {
        mode,
        combo,
        window,
        state,
        ..
    } = c.clone();
    // Music bar: Space/K/P pause/play during audio-only from any non-player mode.
    // PausePlay lives in the player map; look it up directly when is-audio-playing.
    if !matches!(mode, AppMode::Player | AppMode::ContextMenu) {
        let g = crate::AppState::get(window);
        if g.get_is_audio_playing() {
            let player_action = lookup_action(&state.lock().unwrap().keybindings.player, combo);
            if let Some(Action::PausePlay) = player_action {
                g.invoke_music_bar_play_pause();
                return Some(true);
            }
        }
    }
    None
}
