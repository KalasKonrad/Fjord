// ── fjord-app · keys/text_input.rs ───────────────────────────────────────────
//   onscreen_keyboard_move_row  proportional column mapping across QwertyKeyboard's irregular
//                      [10,9,9,5] row widths for Up/Down (Bonfire Phase 3, on-screen alphanumeric
//                      keyboard, 2026-08-22, rolled out to every text-entry surface as of
//                      2026-08-23 — see app_state.slint's own show-onscreen-keyboard doc
//                      comment for the full design)
//   open_onscreen_keyboard  the one Rust way to open the on-screen keyboard; false (nothing
//                      changed) when Settings → UI has it off — the caller then does its own Enter
//                      action (2026-10-10)
//   handle_library_search / handle_browse_search  raw-key pre-dispatch for the drawn search fields
//   handle_playlist_picker  Add-to-playlist picker (raw keys — naming mode needs text input)
//     discover::handle_key (Discover grid), discover::handle_key_request_detail (Seerr detail/Request)
//   handle_discover_search  raw-key pre-dispatch for Discover's search field (typing/backspace/
//                           2026-10-04: Left/Right/Home/End move the caret, Delete deletes after it
//                      escape), mirrors handle_browse_search — bypasses the Action/KeyMap lookup;
//                      Up and Down both unconditionally enter the filter bar (Down fixed
//                      2026-07-18 — previously skipped straight into content, asymmetric
//                      with Up); Enter opens the on-screen keyboard (2026-08-23, full rollout —
//                      was "jump straight to the top search result," a deliberate trade-off the
//                      user chose directly; Down still reaches the grid via the filter bar);
//                      Left on an empty query still exits to the sidebar (fs=-1), same
//                      destination Escape targets — real bug fixed 2026-07-18: this function had
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
// Nearest-column mapping across QwertyKeyboard's irregular row widths
// (10/9/9/3) — Up/Down land on whichever key sits geometrically closest,
// left-to-right, to the current one. No-op at the top/bottom row (returns
// the cursor unchanged) rather than handing off past the grid edge the way
// the numeric VirtualKeyboard's own PIN entry does, since row 3 already
// contains its own in-grid Done key — there's nothing left to hand off to
// below it, and nothing above row 0.
//
// Real bug, live-reported 2026-08-23 ("if you mov up from the abc you
// always land on z" / "if you move down and is raigt abowe the abc you get
// to the middelbutton isted"): the original formula mapped a column by
// FRACTIONAL POSITION (col / (row_len-1)), which assumes every row spans
// the same left-to-right width — wrong, since QwertyKeyboard's own per-row
// HorizontalLayout uses `alignment: center` (widgets.slint), so a shorter
// row is horizontally CENTERED under the widest one, not left-aligned to
// it. Solving for "same on-screen pixel position" instead of "same
// fraction" is what the user actually wants (and matches how every real
// text editor moves a cursor vertically — preserving x-position, not a
// proportional fraction of line length).
//
// Derivation: each row's own left offset in the shared coordinate space is
// `(max_row_len - row_len) * half-cell-pitch` (half of the pixel gap
// between it and the widest row, exactly what centering means); a cell's
// on-screen center is `offset + col * cell-pitch + cell-pitch/2`. Setting
// center(row, col) == center(new_row, col') and solving for col' — the
// pitch and half-pitch terms cancel cleanly regardless of the actual pixel
// size of a key, leaving a closed form with no pixel constants in it at
// all: `col' = col + (row_lens[new_row] - row_lens[row]) / 2`.
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
        // On-screen keyboard, 2026-08-25 — missed in the original rollout
        // (real gap, live-reported: "why dont library serche spawn the
        // keybord on enter?"); this field is a hand-drawn Text+caret, same
        // shape as Discover/Browse's own search fields, so this is a direct
        // extension of that exact pattern. Was merged with Down above
        // ("move into the grid") — splitting them apart costs nothing,
        // since Down alone still does the identical job Enter used to. No
        // AppState.refocus() call needed — this field never held native
        // Slint focus to release.
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
        // On-screen keyboard, 2026-08-23 — was merged with Down above
        // (both did the same "move into the list" thing); splitting them
        // apart costs nothing, since Down alone still does the identical
        // job Enter used to. No AppState.refocus() call needed — this
        // field never held native Slint focus to release.
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
        // Down enters the filter bar, not the content grid directly — real
        // bug fixed 2026-07-18: this was asymmetric with Up (which already
        // enters the filter bar) and with the filter bar's own Down (which
        // goes to content), since the filter bar sits between the search
        // field and content in real visual layout order. (Enter used to
        // jump straight to the top result — see the RETURN arm below,
        // repurposed 2026-08-23 to open the on-screen keyboard instead.)
        k if k == key::DOWN => {
            g.set_discover_header_focused(false);
            g.set_discover_filter_bar_active(true);
            true
        }
        // On-screen keyboard, 2026-08-23 (full rollout beyond Login) —
        // replaces the old "jump straight to the top result" behavior
        // (a deliberate, user-confirmed trade-off: Down still reaches the
        // grid via the filter bar, one extra step, not a dead end). No
        // AppState.refocus() call needed — this field never held native
        // Slint focus to release in the first place (it's a hand-drawn
        // Text, not a LineEdit).
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
        // Up now always enters the filter bar (2026-07-18, Discover
        // filters) — previously a no-op for a non-empty query (silently
        // swallowed by the is_navigation_key catch-all below, since unlike
        // handle_library_search this function had no explicit Up arm at
        // all) since there was nothing above the search field to focus.
        // Deliberately unconditional (not gated on query emptiness like
        // Left below) — the filter bar is always visible regardless of
        // query state, matching Library grid's own always-visible sort bar.
        k if k == key::UP => {
            g.set_discover_header_focused(false);
            g.set_discover_filter_bar_active(true);
            true
        }
        // Real bug, user-reported 2026-07-18: with an empty query (either
        // never typed anything, or typed then backspaced all the way back
        // to empty — same state either way, see this function's own
        // investigation notes), Escape was the ONLY way out — Up/Left were
        // both silently swallowed by the is_navigation_key catch-all below,
        // since (unlike handle_library_search, which has an explicit Up
        // arm) this function never had one. (There's no separate raw
        // "Back" key at this layer — Backspace and Escape both map to
        // Action::Back elsewhere via the KeyMap, and Backspace is already
        // claimed above for character deletion.) Go straight to the
        // sidebar (fs=-1), same destination Escape now also targets,
        // rather than landing in the zero-result-grid limbo state that
        // Up-from-the-grid enters this field FROM (discover.rs's own
        // `count == 0` branch) — that limbo state has nothing useful to
        // show when the query is empty, so bouncing through it first would
        // just trade one extra keypress for another. Left keeps this
        // query-emptiness gating (unlike Up above) — a non-empty query's
        // Left is unrelated to this fix and stays swallowed by
        // is_navigation_key, unchanged.
        k if k == key::LEFT && g.get_discover_query().is_empty() => {
            g.set_discover_header_focused(false);
            g.set_focused_section(-1);
            true
        }
        // Caret keys with text in the field (2026-10-04, live-reported:
        // fixing one letter meant deleting everything after it). Left at the
        // very start stays put — leaving the field mid-edit would be easy to
        // hit by accident; Escape/Up/Down still leave it.
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
            // On-screen keyboard, 2026-08-23 — Enter now opens it (was
            // "create the playlist directly," the closest analog to
            // Login's own password-submit conflict). Right takes over
            // create, since Done should keep meaning "just close the
            // keyboard" everywhere, consistent with every other screen.
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
                // Real gap, live-reported 2026-08-25 ("Needs to presses to
                // get the of enter to open the virtual keybord on add new
                // playlist") — same shape as ProfileEditScreen's identical
                // 2-Enter friction: entering naming mode and opening the
                // on-screen keyboard used to be two separate presses (this
                // one, then a second Enter caught by the naming-mode match
                // arm above). Collapsed into one — naming mode and the
                // keyboard now open together on the very first Enter.
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
