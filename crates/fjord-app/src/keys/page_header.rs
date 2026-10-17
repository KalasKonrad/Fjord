// ── fjord-app · keys/page_header.rs ──────────────────────────────────────────
//   MusicPage           the album/playlist page and the artist page share one header — ← Back,
//                       then ▶ Play All + ♥ (btn 0/1), then the bio (btn 2, only when there is one),
//                       then the page's own list (tracks / album grid) — as one table of their
//                       AppState fields and actions
//   header_keys         keys while the header has the focus; None = the list has it
//   list_up_to_header   Up from the list's first row: the bio, else ▶ Play All
// ─────────────────────────────────────────────────────────────────────────────
use super::Action;
use crate::AppState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MusicPage {
    Album,
    Artist,
}

impl MusicPage {
    fn back_focused(self, g: &AppState) -> bool {
        match self {
            MusicPage::Album => g.get_album_back_focused(),
            MusicPage::Artist => g.get_artist_back_focused(),
        }
    }

    fn set_back_focused(self, g: &AppState, on: bool) {
        match self {
            MusicPage::Album => g.set_album_back_focused(on),
            MusicPage::Artist => g.set_artist_back_focused(on),
        }
    }

    /// The focused header button: 0 ▶ Play All, 1 ♥, 2 the bio; -1 = the list.
    fn btn(self, g: &AppState) -> i32 {
        match self {
            MusicPage::Album => g.get_album_btn_focused(),
            MusicPage::Artist => g.get_artist_btn_focused(),
        }
    }

    fn set_btn(self, g: &AppState, btn: i32) {
        match self {
            MusicPage::Album => g.set_album_btn_focused(btn),
            MusicPage::Artist => g.set_artist_btn_focused(btn),
        }
    }

    fn has_bio(self, g: &AppState) -> bool {
        match self {
            MusicPage::Album => !g.get_album_overview().is_empty(),
            MusicPage::Artist => !g.get_artist_overview().is_empty(),
        }
    }

    fn toggle_bio(self, g: &AppState) {
        match self {
            MusicPage::Album => g.set_album_overview_expanded(!g.get_album_overview_expanded()),
            MusicPage::Artist => g.set_artist_overview_expanded(!g.get_artist_overview_expanded()),
        }
    }

    fn play_all(self, g: &AppState) {
        match self {
            MusicPage::Album => g.invoke_play_album_all(),
            MusicPage::Artist => g.invoke_play_artist_all(),
        }
    }

    fn toggle_fav(self, g: &AppState) {
        match self {
            MusicPage::Album => g.invoke_toggle_album_fav(),
            MusicPage::Artist => g.invoke_toggle_artist_fav(),
        }
    }

    fn close(self, g: &AppState) {
        match self {
            MusicPage::Album => g.set_show_album(false),
            MusicPage::Artist => g.invoke_close_artist(),
        }
    }

    /// Down out of the header into the list: the album starts at its first track; the artist
    /// grid keeps the card it had — or, with no albums, the artist's Tracks start at the first.
    fn enter_list(self, g: &AppState) {
        match self {
            MusicPage::Album => g.set_album_focused_track(0),
            MusicPage::Artist => {
                use slint::Model;
                if g.get_artist_albums().row_count() == 0 && g.get_artist_tracks().row_count() > 0 {
                    g.set_artist_in_tracks(true);
                    g.set_artist_track_focused(0);
                }
            }
        }
    }
}

/// The header's keys (← Back, ▶ Play All / ♥, the bio). `None` while the list has the focus —
/// the page's own handler takes the key then.
pub(crate) fn header_keys(page: MusicPage, action: &Action, g: &AppState) -> Option<bool> {
    if page.back_focused(g) {
        return Some(match action {
            Action::Confirm | Action::Back => {
                page.close(g);
                true
            }
            Action::Down => {
                page.set_back_focused(g, false);
                page.set_btn(g, 0);
                true
            }
            Action::Up => false, // let focus_bar_on_up have it
            _ => true,
        });
    }
    let btn = page.btn(g);
    if btn < 0 {
        return None;
    }
    Some(match action {
        Action::Left => {
            if btn == 1 {
                page.set_btn(g, 0);
            }
            true
        }
        Action::Right => {
            if btn == 0 {
                page.set_btn(g, 1);
            }
            true
        }
        Action::Confirm => {
            match btn {
                0 => page.play_all(g),
                1 => page.toggle_fav(g),
                _ => page.toggle_bio(g),
            }
            true
        }
        Action::Up => {
            if btn == 2 {
                page.set_btn(g, 0); // bio → ▶ Play All
            } else {
                page.set_btn(g, -1);
                page.set_back_focused(g, true);
            }
            true
        }
        Action::Down => {
            if btn <= 1 && page.has_bio(g) {
                page.set_btn(g, 2); // buttons → bio
            } else {
                page.set_btn(g, -1);
                page.enter_list(g);
            }
            true
        }
        Action::Back => {
            page.set_btn(g, -1);
            page.close(g);
            true
        }
        _ => true,
    })
}

/// Up from the list's first row: the bio (it sits between the header and the list), else ▶ Play All.
pub(crate) fn list_up_to_header(page: MusicPage, g: &AppState) {
    page.set_btn(g, if page.has_bio(g) { 2 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Global;

    #[test]
    fn album_and_artist_headers_move_the_same_way() {
        i_slint_backend_testing::init_no_event_loop();
        let w = crate::MainWindow::new().unwrap();
        let g = crate::AppState::get(&w);
        for page in [MusicPage::Album, MusicPage::Artist] {
            // Back → Down → ▶ → Right → ♥ → Down: no bio → the list.
            page.set_back_focused(&g, true);
            assert_eq!(header_keys(page, &Action::Down, &g), Some(true));
            assert_eq!((page.back_focused(&g), page.btn(&g)), (false, 0));
            header_keys(page, &Action::Right, &g);
            assert_eq!(page.btn(&g), 1);
            header_keys(page, &Action::Right, &g);
            assert_eq!(page.btn(&g), 1, "♥ is the last button");
            header_keys(page, &Action::Down, &g);
            assert_eq!(page.btn(&g), -1);
            assert_eq!(
                header_keys(page, &Action::Down, &g),
                None,
                "the list has the keys"
            );
            // Up from the list's first row → ▶ (no bio); Up → Back.
            list_up_to_header(page, &g);
            assert_eq!(page.btn(&g), 0);
            header_keys(page, &Action::Up, &g);
            assert!(page.back_focused(&g));
            assert_eq!(
                header_keys(page, &Action::Up, &g),
                Some(false),
                "Up from Back: the bar"
            );
            page.set_back_focused(&g, false);
        }
        // With a bio: ▶ → Down → bio → Down → the list; the list's Up → the bio.
        g.set_album_overview("A bio".into());
        g.set_album_btn_focused(0);
        header_keys(MusicPage::Album, &Action::Down, &g);
        assert_eq!(g.get_album_btn_focused(), 2);
        g.set_album_focused_track(5);
        header_keys(MusicPage::Album, &Action::Down, &g);
        assert_eq!(
            (g.get_album_btn_focused(), g.get_album_focused_track()),
            (-1, 0)
        );
        list_up_to_header(MusicPage::Album, &g);
        assert_eq!(g.get_album_btn_focused(), 2);
    }
}
