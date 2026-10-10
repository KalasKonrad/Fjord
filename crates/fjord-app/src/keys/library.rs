// ── fjord-app · keys/library.rs ──────────────────────────────────────────────
//   dispatch_library   keyboard nav for the library grid (4 focus states: grid → search → sort → back)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Library grid dispatch ─────────────────────────────────────────────────────

pub(crate) fn dispatch_library(action: &Action, g: &crate::AppState) -> bool {
    // ── Back button focused (top bar) ─────────────────────────────────────────
    if g.get_library_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_library_back_focused(false);
                g.set_show_library(false);
                g.set_library_header_focused(false);
                g.set_library_sort_focused(false);
                g.set_library_scrubber_focused(false);
                g.invoke_library_search_clear();
                true
            }
            Action::Down => {
                g.set_library_back_focused(false);
                g.set_library_sort_focused(true);
                g.set_library_sort_cursor(sort_bar_init_cursor(g));
                true
            }
            Action::Up => false, // let focus_bar_on_up handle mini-player
            _ => true,
        };
    }

    // ── Sort bar navigation ───────────────────────────────────────────────────
    if g.get_library_sort_focused() {
        match action {
            Action::Left => {
                let c = g.get_library_sort_cursor();
                if c > 0 {
                    g.set_library_sort_cursor(c - 1);
                }
                return true;
            }
            Action::Right => {
                let c = g.get_library_sort_cursor();
                let nav = g.get_active_nav();
                // Music: cursor 0-2=view, 3-7=sort, 8=Favorites. Others: 0-4=sort, 5-6=filters or 0-4.
                let max = if nav == 4 {
                    8
                } else if g.get_library_has_filters() {
                    6
                } else {
                    4
                };
                if c < max {
                    g.set_library_sort_cursor(c + 1);
                } else if g.get_library_sort() == 0 && g.get_library_query().is_empty() {
                    // Right past last pill when sorted A-Z: enter the alphabet scrubber.
                    g.set_library_sort_focused(false);
                    g.set_library_scrubber_focused(true);
                    g.set_library_scrubber_cursor(0);
                }
                return true;
            }
            Action::Confirm => {
                let c = g.get_library_sort_cursor();
                let nav = g.get_active_nav();
                let sort = g.get_library_sort();
                let fw = g.get_library_filter_unwatched();
                let ff = g.get_library_filter_favorites();
                if nav == 4 {
                    // Music: 0=Artists, 1=Albums, 2=Playlists, 3-7=sort(c-3), 8=Favorites
                    match c {
                        0 => {
                            g.invoke_library_music_view_changed(0);
                            g.set_library_sort_focused(false);
                        }
                        1 => {
                            g.invoke_library_music_view_changed(1);
                            g.set_library_sort_focused(false);
                        }
                        2 => {
                            g.invoke_library_music_view_changed(2);
                            g.set_library_sort_focused(false);
                        }
                        3..=7 => {
                            g.invoke_library_sort_apply(c - 3, fw, ff);
                            g.set_library_sort_focused(false);
                        }
                        _ => g.invoke_library_sort_apply(sort, fw, !ff), // 8=Favorites, stays open
                    }
                } else {
                    match c {
                        0..=4 => {
                            g.invoke_library_sort_apply(c, fw, ff);
                            g.set_library_sort_focused(false);
                        }
                        5 => g.invoke_library_sort_apply(sort, !fw, ff),
                        _ => g.invoke_library_sort_apply(sort, fw, !ff),
                    }
                }
                return true;
            }
            Action::Back => {
                g.set_library_sort_focused(false);
                g.set_library_sort_cursor(sort_bar_init_cursor(g));
                return true;
            }
            Action::Up => {
                g.set_library_sort_focused(false);
                g.set_library_back_focused(true);
                return true;
            }
            Action::Down => {
                g.set_library_sort_focused(false);
                g.set_library_header_focused(true);
                return true;
            }
            _ => return false,
        }
    }

    // ── Alphabet scrubber navigation ─────────────────────────────────────────
    if g.get_library_scrubber_focused() {
        match action {
            Action::Up => {
                let c = g.get_library_scrubber_cursor();
                if c > 0 {
                    g.set_library_scrubber_cursor(c - 1);
                }
                return true;
            }
            Action::Down => {
                let c = g.get_library_scrubber_cursor();
                if c < 26 {
                    g.set_library_scrubber_cursor(c + 1);
                }
                return true;
            }
            Action::Confirm => {
                let c = g.get_library_scrubber_cursor();
                let cols = g.get_library_cols();
                let offsets = g.get_library_alpha_offsets();
                if let Some(flat_idx) = offsets.row_data(c as usize)
                    && flat_idx >= 0
                {
                    g.set_library_focused(flat_idx);
                    g.set_library_focused_row(flat_idx / cols);
                }
                g.set_library_scrubber_focused(false);
                return true;
            }
            Action::Back | Action::Left => {
                g.set_library_scrubber_focused(false);
                g.set_library_sort_focused(true);
                g.set_library_sort_cursor(sort_bar_init_cursor(g));
                return true;
            }
            _ => return true, // swallow all other keys while scrubber is focused
        }
    }

    match action {
        Action::Back => {
            g.set_library_back_focused(false);
            g.set_show_library(false);
            g.set_library_header_focused(false);
            g.set_library_scrubber_focused(false);
            g.invoke_library_search_clear();
            true
        }
        Action::Left => {
            let f = g.get_library_focused();
            let cols = g.get_library_cols();
            if f % cols > 0 {
                g.set_library_focused(f - 1); // within row — no scroll
            } else if f > 0 {
                let nf = f - 1;
                g.set_library_focused(nf);
                g.set_library_focused_row(nf / cols); // wrap to prev row — scroll
            }
            true
        }
        Action::Right => {
            let f = g.get_library_focused();
            let cols = g.get_library_cols();
            let count = g.get_library_display().row_count() as i32;
            if f % cols < cols - 1 && f + 1 < count {
                g.set_library_focused(f + 1); // within row — no scroll
            } else if f + 1 < count {
                let nf = f + 1;
                g.set_library_focused(nf);
                g.set_library_focused_row(nf / cols); // wrap to next row — scroll
            }
            true
        }
        Action::Up => {
            let f = g.get_library_focused();
            let cols = g.get_library_cols();
            if f >= cols {
                let nf = f - cols;
                g.set_library_focused(nf);
                g.set_library_focused_row(nf / cols);
            } else {
                g.set_library_header_focused(true);
            }
            true
        }
        Action::Down => {
            let f = g.get_library_focused();
            let cols = g.get_library_cols();
            if f + cols < g.get_library_display().row_count() as i32 {
                let nf = f + cols;
                g.set_library_focused(nf);
                g.set_library_focused_row(nf / cols);
                true
            } else {
                false // at last row — let focus_bar_on_down handle it
            }
        }
        Action::Confirm => {
            let f = g.get_library_focused();
            if f < g.get_library_display().row_count() as i32 {
                let card = g.get_library_display().row_data(f as usize).unwrap();
                if g.get_active_nav() == 3 {
                    g.invoke_open_collection(card.id, card.title);
                } else {
                    g.invoke_open_detail(card.id, card.item_type);
                }
            }
            true
        }
        Action::OpenContextMenu => {
            let f = g.get_library_focused();
            if f < g.get_library_display().row_count() as i32 {
                let card = g.get_library_display().row_data(f as usize).unwrap();
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
            true
        }
        Action::SearchJump => {
            g.set_library_header_focused(true);
            g.set_library_focused(0);
            g.set_library_focused_row(0);
            true
        }
        _ => false,
    }
}
