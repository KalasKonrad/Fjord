// ── fjord-app · keys/dashboard.rs ────────────────────────────────────────────
//   handle_global_shortcuts  F/Ctrl+Q/B/1/2/3/S shortcuts shared between Dashboard and Settings
//   dispatch_dashboard  content grid nav + item actions
//   focus_bar_on_up / focus_bar_on_down  music-bar / mini-player-bar focus fallbacks
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Bar focus fallbacks ───────────────────────────────────────────────────────
// The video mini-bar and the music bar are both docked at the bottom: focus_bar_on_down
// (a screen's Down falling off the bottom) focuses whichever is visible; focus_bar_on_up is a
// no-op kept for its call sites.
pub(crate) fn focus_bar_on_up(_action: &Action, _window: &crate::MainWindow) -> bool {
    false
}

pub(crate) fn focus_bar_on_down(action: &Action, window: &crate::MainWindow) -> bool {
    if *action != Action::Down {
        return false;
    }
    let g = crate::AppState::get(window);
    if g.get_is_audio_playing() {
        g.set_music_bar_focused(1); // enter at play/pause; navigate Left to reach art/title
        true
    } else if g.get_has_background_player() && !g.get_is_playing() {
        g.set_float_card_focused(0);
        true
    } else {
        false
    }
}

// ── Global shortcuts ──────────────────────────────────────────────────────────
// Active from Dashboard and Settings; per-screen handlers (detail, series, player)
// intercept F/Q first where they need special handling.

pub(crate) fn handle_global_shortcuts(action: &Action, window: &crate::MainWindow) -> bool {
    match action {
        Action::Fullscreen => {
            crate::AppState::get(window).invoke_toggle_fullscreen();
            true
        }
        Action::Quit => {
            crate::AppState::get(window).invoke_quit();
            true
        }
        Action::NavHome => {
            nav_to(window, 0);
            true
        }
        Action::NavMovies => {
            nav_to(window, 2);
            true
        } // Movies is now nav=2
        Action::NavTV => {
            nav_to(window, 1);
            true
        } // TV Shows is now nav=1
        Action::NavSettings => {
            nav_to(window, 10);
            true
        }
        Action::OpenBrowse => {
            let g = crate::AppState::get(window);
            if g.get_active_nav() < 10 {
                g.set_show_library(false);
                g.set_library_scrubber_focused(false);
                g.set_settings_section("".into());
                g.set_settings_focused("".into());
                g.set_show_browse(true);
                g.invoke_browse_search_clear();
            }
            true
        }
        _ => false,
    }
}

// ── Dashboard dispatch ────────────────────────────────────────────────────────
// Handles: content grid nav and card item actions.
// Global shortcuts are pre-checked by the caller before this is reached.

pub(crate) fn dispatch_dashboard(
    action: &Action,
    repeat: bool,
    window: &crate::MainWindow,
) -> bool {
    if *action == Action::Back {
        let g = crate::AppState::get(window);
        if g.get_focused_section() >= 0 {
            g.set_focused_section(-1);
            return true;
        }
        return false;
    }

    if *action == Action::Up || *action == Action::Down {
        let g = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if *action == Action::Down {
            if fs < 0 {
                sidebar_nav(&g, 1);
                return true;
            }
            let n = g.invoke_find_next_section(fs);
            if n != fs {
                g.set_focused_section(n);
                g.set_focused_card(0);
                return true;
            }
            return false; // at bottom of content — let focus_bar_on_down handle it
        }
        // Up
        if fs < 0 {
            sidebar_nav(&g, -1);
            return true;
        }
        let p = g.invoke_find_prev_section(fs);
        if p >= 0 {
            g.set_focused_section(p);
            g.set_focused_card(0);
            return true;
        }
        return false; // at top of content grid — let focus_bar_on_up handle it
    }

    if *action == Action::Left {
        let g = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let fc = g.get_focused_card();
            if fc > 0 {
                g.set_focused_card(fc - 1);
            } else if !repeat {
                g.set_focused_section(-1);
            }
            return true;
        }
    }

    if *action == Action::Right {
        let g = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs < 0 && g.get_active_nav() == 7 {
            // The Profile row (nav 7) has no content section: open its quick-menu like the mouse
            // does (layout.slint), and leave focused_section alone (entering "content" here
            // stranded the keys).
            g.invoke_open_sidebar_profile_menu();
        } else if fs < 0 && g.get_active_nav() < 10 {
            g.set_focused_section(g.invoke_find_first_section());
            g.set_focused_card(0);
        } else if fs >= 0 {
            let fc = g.get_focused_card();
            if fc < g.invoke_section_len(fs) - 1 {
                g.set_focused_card(fc + 1);
            }
        }
        return true;
    }

    // Watchlist/Coming Up rows mix Discover/TMDB cards into these Jellyfin dashboards: Enter,
    // I and C route them like the mouse does (SectionRow's item-play branches on the type) —
    // otherwise a TMDB id went to Jellyfin (400). Same routing as the Recommended/Other
    // Work/Missing rows.
    if *action == Action::OpenDetail {
        let g = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let card = g.invoke_section_card_item(fs, g.get_focused_card());
            if card.item_type.as_str().starts_with("Discover") {
                let media_type = if card.item_type.as_str() == "DiscoverMovie" {
                    "movie"
                } else {
                    "tv"
                };
                g.invoke_open_discover_item(media_type.into(), card.id);
            } else {
                g.invoke_open_detail(card.id, card.item_type);
            }
            return true;
        }
    }

    if *action == Action::OpenContextMenu {
        let g = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let card = g.invoke_section_card_item(fs, g.get_focused_card());
            if card.item_type.as_str().starts_with("Discover") {
                g.invoke_open_context_menu_discover(card);
            } else {
                g.set_context_menu_title(card.title.clone());
                g.invoke_open_context_menu(
                    card.id,
                    card.has_played,
                    card.is_favorite,
                    card.resume_pct,
                    card.item_type,
                    card.series_id,
                );
            }
            return true;
        }
    }

    if *action == Action::Confirm {
        let g = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let card = g.invoke_section_card_item(fs, g.get_focused_card());
            if card.item_type.as_str().starts_with("Discover") {
                let media_type = if card.item_type.as_str() == "DiscoverMovie" {
                    "movie"
                } else {
                    "tv"
                };
                g.invoke_open_discover_item(media_type.into(), card.id);
            } else {
                g.invoke_item_play(card.id);
            }
            return true;
        }
        let nav = g.get_active_nav();
        if nav == 11 {
            g.invoke_quit();
            return true;
        }
        if nav == 7 {
            // Same fix, same reasoning as the Right arm just above — nav==7
            // (Profile row) has no content section for the generic fallback
            // below to enter; open the quick-menu instead, matching mouse.
            g.invoke_open_sidebar_profile_menu();
            return true;
        }
        if nav < 10 {
            if nav == 5 {
                // Browse All
                if g.get_media_items().row_count() > 0 {
                    g.set_current_item(0);
                }
            } else if nav == 1 || nav == 2 || nav == 3 || nav == 4 {
                g.set_show_library(true);
                g.set_library_focused(0);
                g.set_library_focused_row(0);
                g.set_library_header_focused(false);
                g.invoke_open_library(nav);
            } else {
                g.set_focused_section(g.invoke_find_first_section());
                g.set_focused_card(0);
            }
            return true;
        }
        return false;
    }

    false
}
