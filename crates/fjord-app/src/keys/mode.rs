// ── fjord-app · keys/mode.rs ─────────────────────────────────────────────────
//   AppMode            active UI mode; priority: ContextMenu > QueuePanel > NowPlaying > the screen
//                      stack's top page (Person, Detail, Season, Series, Artist, Collection, Album,
//                      RequestOptions, RequestDetail, Calendar(DayPopup), Blocklist — the most
//                      recently opened one shown) > BonfireAdmin > Player > Library > Browse >
//                      Discover > Settings > Dashboard
//   StackScreen        the stacked pages as one table: show flag, open-order z (AppState *-z), mode
//   open_screen        show a page on top of the stack (also when it was open underneath)
//   top_screen         the stack page that gets the keys (highest z shown; ties: StackScreen::ALL)
//   active_mode        derive AppMode from AppState flags (single source of screen priority)
// ─────────────────────────────────────────────────────────────────────────────
use tracing::debug;

// ── AppMode ───────────────────────────────────────────────────────────────────

/// The active UI mode — computed by `active_mode()` from `AppState` flags.
/// Sub-modes (season row, player panel) are resolved inside their arm's handler.
/// `LibrarySearch`/`BrowseSearch` bypass key-lookup and are handled before `active_mode`.
/// `Login` is guarded before `active_mode` is called and never appears as a mode value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    ContextMenu,
    QueuePanel,
    NowPlaying,
    Person,
    Season,
    Series,
    Detail,
    Artist,
    Collection,
    Album,
    RequestOptions,
    RequestDetail,
    CalendarDayPopup,
    Calendar,
    Blocklist,
    BonfireAdmin,
    Player,
    Library,
    Browse,
    Discover,
    Settings,
    Dashboard,
}

// ── Screen stack ──────────────────────────────────────────────────────────────

/// A full-screen page of main.slint's `stack` container: its show flag, its place in the open
/// order (AppState `*-z`) and the mode that gets the keys while it's on top — one table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StackScreen {
    Person,
    Detail,
    Season,
    Series,
    Artist,
    Collection,
    Album,
    RequestOptions,
    RequestDetail,
    Calendar,
    Blocklist,
}

impl StackScreen {
    /// Every page, in the old fixed priority — the tie-break when two were never raised.
    pub(crate) const ALL: [StackScreen; 11] = [
        StackScreen::Person,
        StackScreen::Detail,
        StackScreen::Season,
        StackScreen::Series,
        StackScreen::Artist,
        StackScreen::Collection,
        StackScreen::Album,
        StackScreen::RequestOptions,
        StackScreen::RequestDetail,
        StackScreen::Calendar,
        StackScreen::Blocklist,
    ];

    pub(crate) fn set_shown(self, g: &crate::AppState, on: bool) {
        match self {
            StackScreen::Person => g.set_show_person(on),
            StackScreen::Detail => g.set_show_detail(on),
            StackScreen::Season => g.set_show_season(on),
            StackScreen::Series => g.set_show_series(on),
            StackScreen::Artist => g.set_show_artist(on),
            StackScreen::Collection => g.set_show_collection(on),
            StackScreen::Album => g.set_show_album(on),
            StackScreen::RequestOptions => g.set_show_request_options(on),
            StackScreen::RequestDetail => g.set_show_request_detail(on),
            StackScreen::Calendar => g.set_show_calendar(on),
            StackScreen::Blocklist => g.set_show_blocklist(on),
        }
    }

    /// Shown (the calendar counts while only its day popup is up).
    fn shown(self, g: &crate::AppState) -> bool {
        match self {
            StackScreen::Person => g.get_show_person(),
            StackScreen::Detail => g.get_show_detail(),
            StackScreen::Season => g.get_show_season(),
            StackScreen::Series => g.get_show_series(),
            StackScreen::Artist => g.get_show_artist(),
            StackScreen::Collection => g.get_show_collection(),
            StackScreen::Album => g.get_show_album(),
            StackScreen::RequestOptions => g.get_show_request_options(),
            StackScreen::RequestDetail => g.get_show_request_detail(),
            StackScreen::Calendar => g.get_show_calendar() || g.get_show_calendar_day_popup(),
            StackScreen::Blocklist => g.get_show_blocklist(),
        }
    }

    fn z(self, g: &crate::AppState) -> i32 {
        match self {
            StackScreen::Person => g.get_person_z(),
            StackScreen::Detail => g.get_detail_z(),
            StackScreen::Season => g.get_season_z(),
            StackScreen::Series => g.get_series_z(),
            StackScreen::Artist => g.get_artist_z(),
            StackScreen::Collection => g.get_collection_z(),
            StackScreen::Album => g.get_album_z(),
            StackScreen::RequestOptions => g.get_request_options_z(),
            StackScreen::RequestDetail => g.get_request_detail_z(),
            StackScreen::Calendar => g.get_calendar_z(),
            StackScreen::Blocklist => g.get_blocklist_z(),
        }
    }

    pub(crate) fn set_z(self, g: &crate::AppState, z: i32) {
        match self {
            StackScreen::Person => g.set_person_z(z),
            StackScreen::Detail => g.set_detail_z(z),
            StackScreen::Season => g.set_season_z(z),
            StackScreen::Series => g.set_series_z(z),
            StackScreen::Artist => g.set_artist_z(z),
            StackScreen::Collection => g.set_collection_z(z),
            StackScreen::Album => g.set_album_z(z),
            StackScreen::RequestOptions => g.set_request_options_z(z),
            StackScreen::RequestDetail => g.set_request_detail_z(z),
            StackScreen::Calendar => g.set_calendar_z(z),
            StackScreen::Blocklist => g.set_blocklist_z(z),
        }
    }

    /// The mode while this page is on top (the day popup and Request Options take all input).
    fn mode(self, g: &crate::AppState) -> AppMode {
        match self {
            StackScreen::Person => AppMode::Person,
            StackScreen::Detail => AppMode::Detail,
            StackScreen::Season => AppMode::Season,
            StackScreen::Series => AppMode::Series,
            StackScreen::Artist => AppMode::Artist,
            StackScreen::Collection => AppMode::Collection,
            StackScreen::Album => AppMode::Album,
            StackScreen::RequestOptions => AppMode::RequestOptions,
            StackScreen::RequestDetail => AppMode::RequestDetail,
            StackScreen::Calendar if g.get_show_calendar_day_popup() => AppMode::CalendarDayPopup,
            StackScreen::Calendar => AppMode::Calendar,
            StackScreen::Blocklist => AppMode::Blocklist,
        }
    }
}

/// Opens a page on top of the stack: its show flag + the next place in the open order — also
/// when it was already open underneath (Detail → Person → another film's Detail). A page
/// restored after playback only gets its flag back (it keeps its place).
pub(crate) fn open_screen(g: &crate::AppState, screen: StackScreen) {
    screen.set_shown(g, true);
    let z = g.get_screen_seq().saturating_add(1);
    g.set_screen_seq(z);
    screen.set_z(g, z);
    debug!("screen stack: {screen:?} on top (z={z})");
}

/// The page that gets the keys: the most recently opened one shown (main.slint draws it on
/// top); ties (never raised) go to the earlier one in `StackScreen::ALL`.
fn top_screen(g: &crate::AppState) -> Option<AppMode> {
    if g.get_is_playing() {
        return None;
    }
    let mut top: Option<(i32, StackScreen)> = None;
    for screen in StackScreen::ALL {
        let z = screen.z(g);
        if screen.shown(g) && top.is_none_or(|(best, _)| z > best) {
            top = Some((z, screen));
        }
    }
    top.map(|(_, screen)| screen.mode(g))
}

pub(crate) fn active_mode(g: &crate::AppState) -> AppMode {
    if g.get_show_context_menu() {
        AppMode::ContextMenu
    } else if g.get_show_queue_panel() {
        AppMode::QueuePanel
    } else if g.get_show_now_playing() && g.get_is_audio_playing() {
        AppMode::NowPlaying
    } else if let Some(top) = top_screen(g) {
        top
    }
    // Bonfire Admin (Phase 6, admin actions, 2026-09-04) — same tier and
    // !is_playing guard as Blocklist above, for the identical reason.
    else if g.get_show_bonfire_admin() && !g.get_is_playing() {
        AppMode::BonfireAdmin
    } else if g.get_is_playing() {
        AppMode::Player
    } else if g.get_show_library() {
        AppMode::Library
    } else if g.get_show_browse() {
        AppMode::Browse
    } else if g.get_active_nav() == 6 {
        AppMode::Discover
    } else if g.get_active_nav() == 10 {
        AppMode::Settings
    } else {
        AppMode::Dashboard
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Global;

    #[test]
    fn the_last_opened_page_gets_the_keys() {
        i_slint_backend_testing::init_no_event_loop();
        let w = crate::MainWindow::new().unwrap();
        let g = crate::AppState::get(&w);
        g.set_show_login(false);

        // An album opened from an artist page (the artist stays open underneath).
        open_screen(&g, StackScreen::Artist);
        assert_eq!(active_mode(&g), AppMode::Artist);
        open_screen(&g, StackScreen::Album);
        assert_eq!(active_mode(&g), AppMode::Album);
        // Back from the album reveals the artist page.
        g.set_show_album(false);
        assert_eq!(active_mode(&g), AppMode::Artist);
        g.set_show_artist(false);

        // Detail → Person (cast) → a film's Detail again: the detail page was open underneath.
        open_screen(&g, StackScreen::Detail);
        open_screen(&g, StackScreen::Person);
        assert_eq!(active_mode(&g), AppMode::Person);
        open_screen(&g, StackScreen::Detail);
        assert_eq!(active_mode(&g), AppMode::Detail);
        assert!(g.get_detail_z() > g.get_person_z());

        // Nothing raised yet (e.g. both restored after playback with z 0): the old fixed order.
        g.set_show_detail(false);
        g.set_show_person(false);
        g.set_person_z(0);
        g.set_season_z(0);
        g.set_show_season(true);
        g.set_show_person(true);
        assert_eq!(active_mode(&g), AppMode::Person);

        // The player hides the stack.
        g.set_is_playing(true);
        assert_eq!(active_mode(&g), AppMode::Player);
    }
}
