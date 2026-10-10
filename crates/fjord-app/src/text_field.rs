// ── fjord-app · text_field.rs ─────────────────────────────────────────────
//   Caret editing for the hand-drawn text fields (2026-10-05 — live-reported:
//   fixing one letter meant deleting everything after it). Those fields are a
//   Text + drawn caret, not a LineEdit: their text lives in an AppState string,
//   the caret in a matching `<field>-cursor` int (grapheme index; -1 or past
//   the end = at the end, so a clear/restore elsewhere never needs to touch it).
//   The drawn text comes from AppState.caret-text(text, cursor, glyph), a pure
//   Rust callback, so it can't go stale.
//
//   DrawnField           one field's getters/setters + its on-screen-keyboard
//                        target name; caret(), insert(), backspace(), delete(),
//                        move_by(), home(), end()
//   DISCOVER_SEARCH, BROWSE_SEARCH, LIBRARY_SEARCH, PLAYLIST_NAME, JOIN_CODE
//                        the five fields
//   by_target            on-screen-keyboard target name → field (◀ ▶ keys)
//   wire                 registers caret-text, onscreen-keyboard-edit (LineEdit
//                        targets) and text-field-caret (◀ ▶ on drawn targets)
//   osk_edit             pure: one on-screen-keyboard key applied at a caret
//                        (unit-tested) — what onscreen-keyboard-edit returns
// ───────────────────────────────────────────────────────────────────────────

use slint::{ComponentHandle, Global, SharedString};

use crate::{AppState, MainWindow, OskEdit};

type GetText = for<'a, 'b> fn(&'b AppState<'a>) -> SharedString;
type SetText = for<'a, 'b> fn(&'b AppState<'a>, SharedString);
type GetCursor = for<'a, 'b> fn(&'b AppState<'a>) -> i32;
type SetCursor = for<'a, 'b> fn(&'b AppState<'a>, i32);

pub(crate) struct DrawnField {
    /// The on-screen keyboard's target id for this field.
    pub target: &'static str,
    get: GetText,
    set: SetText,
    get_cursor: GetCursor,
    set_cursor: SetCursor,
}

pub(crate) const DISCOVER_SEARCH: DrawnField = DrawnField {
    target: "discover-search",
    get: |g| g.get_discover_query(),
    set: |g, s| g.set_discover_query(s),
    get_cursor: |g| g.get_discover_query_cursor(),
    set_cursor: |g, c| g.set_discover_query_cursor(c),
};
pub(crate) const BROWSE_SEARCH: DrawnField = DrawnField {
    target: "browse-search",
    get: |g| g.get_browse_query(),
    set: |g, s| g.set_browse_query(s),
    get_cursor: |g| g.get_browse_query_cursor(),
    set_cursor: |g, c| g.set_browse_query_cursor(c),
};
pub(crate) const LIBRARY_SEARCH: DrawnField = DrawnField {
    target: "library-search",
    get: |g| g.get_library_query(),
    set: |g, s| g.set_library_query(s),
    get_cursor: |g| g.get_library_query_cursor(),
    set_cursor: |g, c| g.set_library_query_cursor(c),
};
pub(crate) const PLAYLIST_NAME: DrawnField = DrawnField {
    target: "playlist-picker-name",
    get: |g| g.get_playlist_picker_name(),
    set: |g, s| g.set_playlist_picker_name(s),
    get_cursor: |g| g.get_playlist_picker_name_cursor(),
    set_cursor: |g, c| g.set_playlist_picker_name_cursor(c),
};
pub(crate) const JOIN_CODE: DrawnField = DrawnField {
    target: "bonfire-group-join-code",
    get: |g| g.get_bonfire_group_join_code(),
    set: |g, s| g.set_bonfire_group_join_code(s),
    get_cursor: |g| g.get_bonfire_group_join_code_cursor(),
    set_cursor: |g, c| g.set_bonfire_group_join_code_cursor(c),
};

const ALL: [&DrawnField; 5] = [
    &DISCOVER_SEARCH,
    &BROWSE_SEARCH,
    &LIBRARY_SEARCH,
    &PLAYLIST_NAME,
    &JOIN_CODE,
];

pub(crate) fn by_target(target: &str) -> Option<&'static DrawnField> {
    ALL.into_iter().find(|f| f.target == target)
}

impl DrawnField {
    pub(crate) fn text(&self, g: &AppState) -> String {
        (self.get)(g).to_string()
    }

    /// Caret as a grapheme index, clamped to the text (-1/past the end → end).
    pub(crate) fn caret(&self, g: &AppState) -> usize {
        let n = crate::grapheme_count(&self.text(g));
        let c = (self.get_cursor)(g);
        if c < 0 { n } else { (c as usize).min(n) }
    }

    pub(crate) fn caret_at_end(&self, g: &AppState) -> bool {
        self.caret(g) == crate::grapheme_count(&self.text(g))
    }

    fn store(&self, g: &AppState, text: &str, caret: usize) {
        (self.set)(g, text.into());
        (self.set_cursor)(g, caret as i32);
    }

    /// Types `s` at the caret; returns the new text.
    pub(crate) fn insert(&self, g: &AppState, s: &str) -> String {
        let (text, caret) = crate::insert_at_grapheme(&self.text(g), self.caret(g), s);
        self.store(g, &text, caret);
        text
    }

    /// Backspace; Some(new text) if anything was removed.
    pub(crate) fn backspace(&self, g: &AppState) -> Option<String> {
        let old = self.text(g);
        let (text, caret) = crate::delete_before_grapheme(&old, self.caret(g));
        if text == old {
            return None;
        }
        self.store(g, &text, caret);
        Some(text)
    }

    /// Delete key; Some(new text) if anything was removed.
    pub(crate) fn delete(&self, g: &AppState) -> Option<String> {
        let old = self.text(g);
        let caret = self.caret(g);
        let text = crate::delete_at_grapheme(&old, caret);
        if text == old {
            return None;
        }
        self.store(g, &text, caret);
        Some(text)
    }

    pub(crate) fn move_by(&self, g: &AppState, delta: i32) {
        let n = crate::grapheme_count(&self.text(g)) as i64;
        let c = (self.caret(g) as i64 + delta as i64).clamp(0, n);
        (self.set_cursor)(g, c as i32);
    }

    pub(crate) fn home(&self, g: &AppState) {
        (self.set_cursor)(g, 0);
    }
    pub(crate) fn end(&self, g: &AppState) {
        (self.set_cursor)(g, -1);
    }
}

/// One on-screen-keyboard key applied to `text` with the caret at `caret`
/// (grapheme index, -1/past the end = end): "backspace", "left", "right", or
/// a literal string to insert. Returns (text, caret, caret's byte offset —
/// what LineEdit.set-selection-offsets takes).
pub(crate) fn osk_edit(text: &str, caret: i32, key: &str) -> (String, usize, usize) {
    use unicode_segmentation::UnicodeSegmentation;
    let n = crate::grapheme_count(text);
    let c = if caret < 0 {
        n
    } else {
        (caret as usize).min(n)
    };
    let (out, c) = match key {
        "backspace" => crate::delete_before_grapheme(text, c),
        "left" => (text.to_string(), c.saturating_sub(1)),
        "right" => (text.to_string(), (c + 1).min(n)),
        s => crate::insert_at_grapheme(text, c, s),
    };
    let byte = out.graphemes(true).take(c).map(str::len).sum();
    (out, c, byte)
}

pub(crate) fn wire(window: &MainWindow) {
    let g = AppState::get(window);
    g.on_caret_text(|text, cursor, glyph| {
        let c = if cursor < 0 {
            usize::MAX
        } else {
            cursor as usize
        };
        crate::with_caret(&text, c, &glyph).into()
    });
    g.on_onscreen_keyboard_edit(|text, caret, key| {
        let (text, caret, byte) = osk_edit(&text, caret, &key);
        OskEdit {
            text: text.into(),
            caret: caret as i32,
            byte: byte as i32,
        }
    });
    let ww = window.as_weak();
    g.on_text_field_caret(move |target, delta| {
        let Some(w) = ww.upgrade() else { return };
        if let Some(field) = by_target(&target) {
            field.move_by(&AppState::get(&w), delta);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn osk_keys_edit_at_the_caret() {
        assert_eq!(osk_edit("helo", 3, "l"), ("hello".to_string(), 4, 4));
        assert_eq!(osk_edit("hello", -1, "!"), ("hello!".to_string(), 6, 6));
        assert_eq!(
            osk_edit("hexllo", 3, "backspace"),
            ("hello".to_string(), 2, 2)
        );
        assert_eq!(osk_edit("abc", 0, "left"), ("abc".to_string(), 0, 0));
        assert_eq!(osk_edit("abc", -1, "right"), ("abc".to_string(), 3, 3));
        assert_eq!(osk_edit("abc", 1, "right"), ("abc".to_string(), 2, 2));
        // Byte offset counts UTF-8 bytes, caret counts graphemes.
        assert_eq!(osk_edit("åäö", 1, "right"), ("åäö".to_string(), 2, 4));
    }
}
