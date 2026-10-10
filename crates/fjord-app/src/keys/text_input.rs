// ── fjord-app · keys/text_input.rs ───────────────────────────────────────────
//   open_onscreen_keyboard  the one Rust way to open the on-screen keyboard; false (nothing
//                      changed) when Settings → UI has it off — the caller then does its own Enter
//                      action. Slint twin: AppState.open-onscreen-keyboard
//   onscreen_keyboard_move_row  Up/Down across QwertyKeyboard's centered rows of different
//                      lengths: nearest key by on-screen position
//   handle_library_search / handle_browse_search  raw-key pre-dispatch for the drawn search fields
//                      (typing, caret keys, Enter opens the on-screen keyboard)
//   handle_discover_search  same for Discover's search field (bypasses the Action/KeyMap
//                      lookup): typing/Backspace, caret keys (Left/Right/Home/End, Delete);
//                      Up/Down enter the filter bar; Enter opens the on-screen keyboard; Left on
//                      an empty query → sidebar (like Escape)
//   handle_playlist_picker  Add-to-playlist picker (raw keys — naming mode needs text input; the
//                      first Enter opens naming + the on-screen keyboard, Right creates)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// Opens the on-screen keyboard for `target` — unless Settings → UI has it turned
/// off: then nothing changes and `false` comes back, so the caller keeps its own
/// Enter behaviour (an opened-but-undrawn keyboard used to swallow focus, 2026-10-10).
/// Slint's twin: `AppState.open-onscreen-keyboard`.
pub(crate) fn open_onscreen_keyboard(g: &crate::AppState, target: &str) -> bool {
    if !g.get_settings_onscreen_keyboard_enabled() {
        debug!("onscreen-kb: not opened for {target} (turned off in Settings)");
        return false;
    }
    g.set_onscreen_keyboard_target(target.into());
    g.set_onscreen_keyboard_cursor(g.get_onscreen_keyboard_done_cursor());
    g.set_show_onscreen_keyboard(true);
    true
}

// ── On-screen alphanumeric keyboard: cursor math ─────────────────────────────
// Up/Down land on the key geometrically closest to the current one across QwertyKeyboard's
// irregular row widths (onscreen-keyboard-row-lens). A no-op at the top/bottom row (the
// bottom row has its own Done key; nothing to hand off to).
//
// Rows are centered (`alignment: center`, widgets.slint), not left-aligned, so a row's left
// offset is `(max_len - len) * half_pitch` and a key's center is `offset + col * pitch +
// pitch/2`. Equal centers solve to `col' = col + (row_lens[new_row] - row_lens[row]) / 2` —
// no pixel constants. (Mapping by fractional position landed on the wrong keys.)
pub(crate) fn onscreen_keyboard_move_row(row_lens: &[i32], cursor: i32, dir: i32) -> i32 {
    let starts: Vec<i32> = row_lens
        .iter()
        .scan(0, |acc, &l| {
            let s = *acc;
            *acc += l;
            Some(s)
        })
        .collect();
    let Some(row) = starts.iter().rposition(|&s| s <= cursor) else {
        return cursor;
    };
    let col = cursor - starts[row];
    let new_row = row as i32 + dir;
    if new_row < 0 || new_row as usize >= row_lens.len() {
        return cursor;
    }
    let new_row = new_row as usize;
    let delta = (row_lens[new_row] - row_lens[row]) as f32 / 2.0;
    let target_col = (col as f32 + delta).round() as i32;
    starts[new_row] + target_col.clamp(0, row_lens[new_row] - 1)
}

// ── Library search text input ─────────────────────────────────────────────────

pub(crate) fn handle_library_search(key: &str, ctrl: bool, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    if ctrl {
        return true;
    }
    match key {
        k if k == key::ESCAPE => {
            g.invoke_library_search_clear();
            g.set_library_header_focused(false);
            g.set_library_focused(0);
            g.set_library_focused_row(0);
            true
        }
        k if k == key::DOWN => {
            g.set_library_header_focused(false);
            g.set_library_focused(0);
            g.set_library_focused_row(0);
            true
        }
        // Enter opens the on-screen keyboard (Down moves into the grid). The field is a hand-drawn
        // Text, so there's no native focus to release.
        k if k == key::RETURN => {
            open_onscreen_keyboard(&g, "library-search");
            true
        }
        k if k == key::BACKSPACE => {
            if !g.get_library_query().is_empty() {
                g.invoke_library_search_backspace();
            }
            true
        }
        k if k == key::UP => {
            g.set_library_header_focused(false);
            g.set_library_sort_focused(true);
            g.set_library_sort_cursor(sort_bar_init_cursor(&g));
            true
        }
        // Caret keys (2026-10-05) — Left/Right were swallowed before.
        k if caret_key(&crate::text_field::LIBRARY_SEARCH, k, &g) => true,
        k if k == key::DELETE => {
            g.invoke_library_search_delete();
            true
        }
        k if is_navigation_key(k) => true,
        k if is_printable(k) => {
            g.invoke_library_search_append(k.into());
            true
        }
        _ => true,
    }
}

// ── Browse search text input ──────────────────────────────────────────────────

pub(crate) fn handle_browse_search(key: &str, ctrl: bool, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    if ctrl {
        return true;
    }
    match key {
        k if k == key::ESCAPE => {
            g.invoke_browse_search_clear();
            g.set_browse_header_focused(false);
            true
        }
        k if k == key::DOWN => {
            g.set_browse_header_focused(false);
            if g.get_media_items().row_count() > 0 {
                g.set_current_item(0);
            }
            true
        }
        // Enter opens the on-screen keyboard (Down moves into the list); hand-drawn field, no
        // refocus needed.
        k if k == key::RETURN => {
            open_onscreen_keyboard(&g, "browse-search");
            true
        }
        k if k == key::BACKSPACE => {
            if !g.get_browse_query().is_empty() {
                g.invoke_browse_search_backspace();
            }
            true
        }
        // Caret keys (2026-10-05) — Left/Right were swallowed before.
        k if caret_key(&crate::text_field::BROWSE_SEARCH, k, &g) => true,
        k if k == key::DELETE => {
            g.invoke_browse_search_delete();
            true
        }
        k if is_navigation_key(k) => true,
        k if is_printable(k) => {
            g.invoke_browse_search_append(k.into());
            true
        }
        _ => true,
    }
}

pub(crate) fn handle_discover_search(key: &str, ctrl: bool, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    if ctrl {
        return true;
    }
    match key {
        k if k == key::ESCAPE => {
            g.invoke_discover_search_clear();
            g.set_discover_header_focused(false);
            g.set_focused_section(-1);
            true
        }
        // Down enters the filter bar, which sits between the search field and the content (Up
        // enters it too).
        k if k == key::DOWN => {
            g.set_discover_header_focused(false);
            g.set_discover_filter_bar_active(true);
            true
        }
        // Enter opens the on-screen keyboard (it used to jump to the top result; Down still reaches
        // the grid through the filter bar). Hand-drawn field, no refocus needed.
        k if k == key::RETURN => {
            open_onscreen_keyboard(&g, "discover-search");
            true
        }
        k if k == key::BACKSPACE => {
            if !g.get_discover_query().is_empty() {
                g.invoke_discover_search_backspace();
            }
            true
        }
        // Up always enters the filter bar (always visible, like the Library grid's sort bar).
        k if k == key::UP => {
            g.set_discover_header_focused(false);
            g.set_discover_filter_bar_active(true);
            true
        }
        // Left on an empty query goes straight to the sidebar (fs = -1, like Escape) — not into the
        // empty-grid state Up-from-the-grid comes from. With text, Left moves the caret (below).
        // (Backspace is taken for deleting, so there's no Back key at this layer.)
        k if k == key::LEFT && g.get_discover_query().is_empty() => {
            g.set_discover_header_focused(false);
            g.set_focused_section(-1);
            true
        }
        // Caret keys with text in the field. Left at the very start stays put (too easy to leave
        // mid-edit by accident); Escape/Up/Down still leave.
        k if caret_key(&crate::text_field::DISCOVER_SEARCH, k, &g) => true,
        k if k == key::DELETE => {
            g.invoke_discover_search_delete();
            true
        }
        k if is_navigation_key(k) => true,
        k if is_printable(k) => {
            g.invoke_discover_search_append(k.into());
            true
        }
        _ => true,
    }
}

// ── Add-to-playlist picker (raw keys — naming mode needs text input) ──────────

pub(crate) fn handle_playlist_picker(key: &str, ctrl: bool, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    if ctrl {
        if key == "q" || key == "Q" {
            g.invoke_quit();
        }
        return true;
    }
    if g.get_playlist_picker_naming() {
        return match key {
            k if k == key::ESCAPE => {
                g.set_playlist_picker_naming(false);
                true
            }
            // Enter opens the on-screen keyboard; Right creates the playlist (Done always just
            // closes the keyboard).
            k if k == key::RETURN => {
                open_onscreen_keyboard(&g, "playlist-picker-name");
                true
            }
            // Right still means "create" — but only with the caret at the
            // end of the name (where typing leaves it); earlier in the name
            // it moves the caret (2026-10-05).
            k if k == key::RIGHT && crate::text_field::PLAYLIST_NAME.caret_at_end(&g) => {
                g.invoke_playlist_picker_create();
                true
            }
            k if caret_key(&crate::text_field::PLAYLIST_NAME, k, &g) => true,
            k if k == key::DELETE => {
                crate::text_field::PLAYLIST_NAME.delete(&g);
                true
            }
            // Routed through the new playlist-picker-name-append/-backspace
            // callbacks (grapheme-cluster-correct) instead of a direct
            // property mutation, unifying this with the on-screen
            // keyboard's own path so the two can't drift apart.
            k if k == key::BACKSPACE => {
                if !g.get_playlist_picker_name().is_empty() {
                    g.invoke_playlist_picker_name_backspace();
                }
                true
            }
            k if is_navigation_key(k) => true,
            k if is_printable(k) => {
                g.invoke_playlist_picker_name_append(k.into());
                true
            }
            _ => true,
        };
    }
    let count = g.get_playlist_picker_items().row_count() as i32;
    match key {
        k if k == key::ESCAPE || k == key::BACKSPACE => {
            g.set_show_playlist_picker(false);
            true
        }
        k if k == key::UP => {
            let c = g.get_playlist_picker_cursor();
            if c > 0 {
                g.set_playlist_picker_cursor(c - 1);
            }
            true
        }
        k if k == key::DOWN => {
            let c = g.get_playlist_picker_cursor();
            if c < count {
                g.set_playlist_picker_cursor(c + 1);
            }
            true
        }
        k if k == key::RETURN => {
            let c = g.get_playlist_picker_cursor();
            if c == 0 {
                // Naming mode and the on-screen keyboard open together on the first Enter (no
                // second press).
                g.set_playlist_picker_name("".into());
                g.set_playlist_picker_naming(true);
                open_onscreen_keyboard(&g, "playlist-picker-name");
            } else {
                g.invoke_playlist_picker_select(c - 1);
            }
            true
        }
        _ => true, // swallow everything else while the picker is open
    }
}
