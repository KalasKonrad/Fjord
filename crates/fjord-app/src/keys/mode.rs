// ── fjord-app · keys/mode.rs ─────────────────────────────────────────────────
//   AppMode            active UI mode — 20 variants; priority: ContextMenu > QueuePanel > NowPlaying >
//                      Person > Detail > Season > Series > Artist > Collection > Album > RequestOptions >
//                      RequestDetail > CalendarDayPopup > Calendar (Seerr) > Blocklist (Seerr, 2026-08-06,
//                      Manage Blocklist) > Player > Library > Browse > Discover (Seerr) > Settings > Dashboard
//   active_mode        derive AppMode from AppState flags (single source of screen priority)
// ─────────────────────────────────────────────────────────────────────────────
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

pub(crate) fn active_mode(g: &crate::AppState) -> AppMode {
    if g.get_show_context_menu() {
        AppMode::ContextMenu
    } else if g.get_show_queue_panel() {
        AppMode::QueuePanel
    } else if g.get_show_now_playing() && g.get_is_audio_playing() {
        AppMode::NowPlaying
    } else if g.get_show_person() && !g.get_is_playing() {
        AppMode::Person
    } else if g.get_show_detail() && !g.get_is_playing() {
        AppMode::Detail
    } else if g.get_show_season() && !g.get_is_playing() {
        AppMode::Season
    } else if g.get_show_series() && !g.get_is_playing() {
        AppMode::Series
    } else if g.get_show_artist() && !g.get_is_playing() {
        AppMode::Artist
    } else if g.get_show_collection() && !g.get_is_playing() {
        AppMode::Collection
    } else if g.get_show_album() && !g.get_is_playing() {
        AppMode::Album
    }
    // Checked ahead of RequestDetail so the modal captures all input while
    // open — show-request-options can only ever be true while already on
    // that screen, so there's no ordering conflict with it taking priority.
    // !is_playing mirrors every other overlay-style mode above (real bug,
    // 2026-07-18: this was the one Seerr overlay missing it — resuming a
    // backgrounded player via 'r' while the modal was open left it stuck
    // rendered on top of the fullscreen video, still eating all keyboard
    // input meant for playback).
    else if g.get_show_request_options() && !g.get_is_playing() {
        AppMode::RequestOptions
    } else if g.get_show_request_detail() && !g.get_is_playing() {
        AppMode::RequestDetail
    }
    // Calendar (2026-07-18, Watchlist + Release Calendar) — same tier and
    // !is_playing guard as RequestOptions/RequestDetail above, for the
    // identical reason (see that fix's own comment). DayPopup checked
    // first so it captures all input while open, same nesting shape as
    // RequestOptions-over-RequestDetail.
    else if g.get_show_calendar_day_popup() && !g.get_is_playing() {
        AppMode::CalendarDayPopup
    } else if g.get_show_calendar() && !g.get_is_playing() {
        AppMode::Calendar
    }
    // Manage Blocklist (2026-08-06, Seerr Blocklist support) — same tier
    // and !is_playing guard as Calendar above, for the identical reason.
    else if g.get_show_blocklist() && !g.get_is_playing() {
        AppMode::Blocklist
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
