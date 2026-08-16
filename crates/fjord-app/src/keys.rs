// ── fjord-app · keys.rs ───────────────────────────────────────────────────────
//   Action             semantic action enum (~42 variants, incl. ToggleLyrics)
//   KeyCombo           key text (Slint event.text) + shift/ctrl/alt bools; key is always
//                      lower-cased (KeyCombo::new, the one normalizing constructor every
//                      KeyCombo must go through — Caps Lock has no modifier flag, so only
//                      lower-casing makes "N"/"n" the same combo regardless of it); TryFrom
//                      migrates a pre-KeyCombo::new file's bare-uppercase letters (shift
//                      encoded via case alone) into shift+<lowercase>, restricted to no
//                      ctrl/alt held (see its own doc comment)
//                      serialises/deserialises as a human-readable string ("ctrl+shift+f")
//   ActionMap          Normal or Player — which KeyMap an action lives in
//   deserialize_keymap custom KeyMap Deserialize — detects two raw strings colliding to the
//                      same KeyCombo (the migration above makes this reachable) and resolves
//                      it deliberately instead of a silent last-insert-wins
//   Keybindings        normal + player KeyMaps (via deserialize_keymap); user JSON replaces
//                      defaults on load
//   PendingKeybindRebind  stashed (row, combo) while the rebind-collision confirm dialog is open
//   apply_rebind       the ONLY place that mutates `keybindings` — called directly (no
//                      collision) or from the collision-confirm callbacks in main.rs
//   AppMode            active UI mode — 20 variants; priority: ContextMenu > QueuePanel > NowPlaying >
//                      Person > Detail > Season > Series > Artist > Collection > Album > RequestOptions >
//                      RequestDetail > CalendarDayPopup > Calendar (Seerr) > Blocklist (Seerr, 2026-08-06,
//                      Manage Blocklist) > Player > Library > Browse > Discover (Seerr) > Settings > Dashboard
//   active_mode        derive AppMode from AppState flags (single source of screen priority)
//   default_keybindings  hardcoded defaults; user keybindings.json replaces on load
//   remappable_actions   ordered list of (Action, label, ActionMap) for the settings UI
//   key_display_name   human-readable label for a Slint key string
//   action_key_labels  all KeyCombos for an Action joined into a display string
//   push_keybinding_rows  build + push keybinding model to AppState
//   handle_key         router: show-login bypass → startup connectivity gate (show-connecting
//                        swallows all keys; show-offline: Enter → retry-connection, OfflineScreen's
//                        only interactive element has no native widget focus) → show-profile-picker
//                        / show-account-picker raw-key tiers (both pre-AppMode, same reason
//                        show-login is — can show before any session/AppMode-relevant state
//                        exists yet; 2026-08-14, 2-tier account/profile redesign) → search
//                        bypasses → loading-guard (app-content-loading) → rebind capture →
//                        key lookup → active_mode() → match per-screen arm
//   show-account-picker tier  Left/Right move the tile cursor (count == "+ Add Account" tile's
//                        own cursor value); Enter on a real tile → account-picker-select,
//                        on the trailing tile → account-picker-add-account; Escape/Backspace
//                        closes only when account-picker-cancelable (the startup-gate open has
//                        nothing to cancel back to)
//   show-profile-picker tier  same shape one tier down, always account-scoped; Escape/Backspace
//                        checks profile-picker-show-back-to-accounts FIRST (→
//                        profile-picker-back-to-accounts) before falling back to the plain
//                        cancelable-close behavior — a 2+-account install's profile picker is
//                        never a dead end, it always has somewhere to go back to
//   dispatch_player    ask-timed overlay; ask overlay; Up Next banner; panel nav; player controls;
//                      chapter-prev/next (,/.); sub/audio delay (z/Z/x/X)
//   dispatch_library   keyboard nav for the library grid (4 focus states: grid → search → sort → back)
//   handle_global_shortcuts  F/Ctrl+Q/B/1/2/3/S shortcuts shared between Dashboard and Settings
//   dispatch_dashboard  content grid nav + item actions
//   Settings dispatch → crate::settings (dispatch_settings, settings_row_action)
//   Per-screen key handlers live in their own modules:
//     context_menu::handle_key, series::handle_key, season::handle_key,
//     detail::handle_key, browse::handle_key,
//     discover::handle_key (Discover grid), discover::handle_key_request_detail (Seerr detail/Request)
//   handle_discover_search  raw-key pre-dispatch for Discover's search field (typing/backspace/
//                      escape), mirrors handle_browse_search — bypasses the Action/KeyMap lookup;
//                      Up and Down both unconditionally enter the filter bar (Down fixed
//                      2026-07-18 — previously skipped straight into content, asymmetric
//                      with Up); Enter still jumps straight to the top search result
//                      (unchanged, a different well-established convention); Left on an
//                      empty query still exits to the sidebar (fs=-1), same destination
//                      Escape targets — real bug fixed 2026-07-18: this function had no Up
//                      handler at all (unlike handle_library_search), so Escape was the
//                      ONLY way out of an empty/cleared search field
//   ── Keyboard-navigation fixes (2026-07-18, see discover.rs's own header block for
//      the full investigation this came from) ── AppMode::RequestDetail/RequestOptions
//      added to 3 global pre-dispatch exclusion lists (ResumePlayer, music-bar-focused,
//      mini-player-bar-focused) that already excluded their peer group
//      (Person/Detail/Season/.../Album) but were missing these two — real bug: 'r' could
//      yank the user into the fullscreen player mid-request-flow, and a stale
//      music-bar-focused/float-card-focused left over from earlier keyboard nav could
//      hijack these screens' own arrow keys after a mouse-driven screen switch.
//      active_mode()'s RequestOptions arm also gained the same !is_playing guard every
//      sibling overlay already had (real bug: the modal could get stuck rendered on top
//      of a resumed fullscreen video).
//   ── Watchlist + Release Calendar (2026-07-18, see discover.rs's own header block) ──
//      AppMode::Calendar/CalendarDayPopup added to active_mode() (own !is_playing guard,
//      same as every sibling overlay) and to the same 3 global exclusion lists
//      (ResumePlayer, music-bar-focused, mini-player-bar-focused) RequestDetail/
//      RequestOptions were added to above — CalendarScreen/its day popup dispatch to
//      discover::handle_key_calendar/handle_key_calendar_day_popup.
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use slint::{Global, Model, ModelRc, SharedString, VecModel};
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use crate::config::FjordState;

// ── Slint key string constants ────────────────────────────────────────────────
// Slint encodes named keys as Unicode Private Use Area (PUA) codepoints.
// These match i-slint-common/key_codes.rs exactly.
pub mod key {
    pub const BACKSPACE:  &str = "\u{0008}";
    pub const RETURN:     &str = "\u{000a}";
    pub const ESCAPE:     &str = "\u{001b}";
    pub const UP:         &str = "\u{F700}";
    pub const DOWN:       &str = "\u{F701}";
    pub const LEFT:       &str = "\u{F702}";
    pub const RIGHT:      &str = "\u{F703}";
    pub const F11:        &str = "\u{F70E}";
}

// ── Action ────────────────────────────────────────────────────────────────────

/// All distinct user-visible actions Fjord can perform.
///
/// Keys map to `Action`s; the dispatch function interprets each `Action`
/// in the context of the current [`AppMode`].  The two-map design (`normal`
/// vs `player`) means the same physical key (e.g. "1") can map to different
/// actions depending on whether the player is open.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Action {
    // ── Universal navigation ─────────────────────────────────────────────────
    Confirm,          // Return — confirm / play / activate
    Back,             // Escape / Backspace — go back / close
    Up,               // UpArrow
    Down,             // DownArrow
    Left,             // LeftArrow
    Right,            // RightArrow
    SearchJump,       // / — focus the search field

    // ── Player-only ──────────────────────────────────────────────────────────
    MinimizePlayer,   // Backspace (player) — close panel or minimize; Escape stops instead

    // ── Global tab / screen shortcuts ────────────────────────────────────────
    NavHome,          // 1
    NavMovies,        // 2
    NavTV,            // 3
    NavSettings,      // S (when not in player)
    OpenBrowse,       // B
    Fullscreen,       // F / F11
    Quit,             // Ctrl+Q (plain q/Q opens the queue panel)

    // ── Card / item actions ──────────────────────────────────────────────────
    OpenDetail,       // I — open detail or series screen
    OpenContextMenu,  // C — context menu on focused card / episode
    ResumePlayer,     // R — resume the background player
    FocusFloatCard,   // N — focus the mini-player bar from any screen

    // ── Player controls (active in player map) ───────────────────────────────
    PausePlay,        // Space / K / P
    SeekBackward,     // Left  (player)
    SeekForward,      // Right (player)
    SeekBackwardLong, // Shift+Left
    SeekForwardLong,  // Shift+Right
    VolumeUp,         // Up    (player)
    VolumeDown,       // Down  (player)
    Mute,             // M
    ToggleStats,      // I (player — shadows OpenDetail)
    PanelSubtitles,   // S (player — shadows NavSettings)
    PanelAudio,       // A
    PanelVideo,       // V
    SeekToPercent(u8), // 0–9 → seek to 0%, 10%, …, 90% (player only)
    NextChapter,       // .
    PrevChapter,       // ,
    SubDelayIncrease,  // z  (+100 ms, matching mpv default)
    SubDelayDecrease,  // Z  (−100 ms, matching mpv default)
    AudioDelayIncrease, // x (+100 ms)
    AudioDelayDecrease, // X (−100 ms)

    // ── Playlist controls ────────────────────────────────────────────────────
    PrevTrack,       // [ — prev track or restart current (music bar / player)
    NextTrack,       // ] — next track (music bar / player)
    ToggleShuffle,   // remappable — flip shuffle on/off
    CycleRepeat,     // remappable — cycle Off → All → One → Off
    OpenQueuePanel,  // q — open/close queue panel (audio playing, queue non-empty, or video player)
    DeleteItem,      // Delete — remove focused item from playlist in queue panel
    ToggleLyrics,    // L — show/hide lyrics overlay (only when lyrics-available)
    ToggleNowPlaying, // m — open/close fullscreen Now Playing screen (audio playing only)
}

// ── KeyCombo ──────────────────────────────────────────────────────────────────

/// A key combination: the Slint `event.text` string plus modifier booleans.
///
/// Serialises as a human-readable string so that `~/.config/fjord/keybindings.json`
/// is directly editable:
///   `"f"`, `"shift+Left"`, `"ctrl+shift+f"`, `"Space"`, `"F11"`, etc.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub key:   String,
    pub shift: bool,
    pub ctrl:  bool,
    pub alt:   bool,
}

/// A captured rebind that collided with another action's existing binding,
/// stashed in `FjordState` while the user is shown a confirm/cancel dialog
/// (see `rebind_action`/`dispatch_keybinding_nav`'s own doc comments).
#[derive(Debug, Clone)]
pub(crate) struct PendingKeybindRebind {
    pub fi:    i32,
    pub combo: KeyCombo,
}

impl KeyCombo {
    /// The single normalizing constructor — every KeyCombo in this app should
    /// be built through this (or `plain`/`shifted`, which just call it), never
    /// a raw struct literal. Lower-cases `key` so captured/looked-up/stored
    /// combos are keyed on the physical Shift-key state alone, never on the
    /// resulting glyph's case. Slint reports the *effective* character after
    /// both Shift and Caps Lock are applied (`event.text`), but Caps Lock has
    /// no modifier flag at all — plain "n" with Caps Lock on arrives as
    /// `{key: "N", shift: false}`, indistinguishable at face value from an
    /// actual attempt to bind capital "N", and different from the SAME
    /// physical key with Caps Lock off. Lower-casing collapses all four
    /// (Shift × Caps Lock) states of a letter down to the only two that
    /// should ever matter for a binding — Shift held, or not — which is
    /// also then the only source of truth for it (no more hand-registering
    /// both "f" and "F" as separate defaults to cover Caps Lock, and no more
    /// need for a shift-strip retry in lookup_action — see its own history
    /// in this file's git log before this comment). No-op for digits/
    /// symbols/named keys — `to_lowercase()` only changes actual uppercase
    /// letters.
    pub fn new(key: impl Into<String>, shift: bool, ctrl: bool, alt: bool) -> Self {
        Self { key: key.into().to_lowercase(), shift, ctrl, alt }
    }
    pub fn plain(key: impl Into<String>) -> Self {
        Self::new(key, false, false, false)
    }
    pub fn shifted(key: impl Into<String>) -> Self {
        Self::new(key, true, false, false)
    }
}

// ── KeyCombo ↔ string serialisation ──────────────────────────────────────────

impl std::fmt::Display for KeyCombo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl  { write!(f, "ctrl+")?;  }
        if self.alt   { write!(f, "alt+")?;   }
        if self.shift { write!(f, "shift+")?; }
        let name = match self.key.as_str() {
            k if k == key::BACKSPACE => "Backspace",
            k if k == key::RETURN    => "Return",
            k if k == key::ESCAPE    => "Escape",
            k if k == key::UP        => "Up",
            k if k == key::DOWN      => "Down",
            k if k == key::LEFT      => "Left",
            k if k == key::RIGHT     => "Right",
            k if k == key::F11       => "F11",
            " "                      => "Space",
            k                        => k,
        };
        write!(f, "{}", name)
    }
}

impl TryFrom<String> for KeyCombo {
    type Error = String;
    fn try_from(s: String) -> Result<Self, Self::Error> {
        let parts: Vec<&str> = s.split('+').collect();
        let (mods, key_parts) = parts.split_at(parts.len().saturating_sub(1));
        let key_name = key_parts.first().copied().unwrap_or("");
        let mut shift = mods.contains(&"shift");
        let ctrl  = mods.contains(&"ctrl");
        let alt   = mods.contains(&"alt");
        let key = match key_name {
            "Backspace"          => key::BACKSPACE.to_string(),
            "Return" | "Enter"   => key::RETURN.to_string(),
            "Escape" | "Esc"     => key::ESCAPE.to_string(),
            "Up"                 => key::UP.to_string(),
            "Down"               => key::DOWN.to_string(),
            "Left"               => key::LEFT.to_string(),
            "Right"              => key::RIGHT.to_string(),
            "F11"                => key::F11.to_string(),
            "Space"              => " ".to_string(),
            k if k.chars().count() == 1 => {
                // Migration (2026-08-08): an old-format single uppercase
                // letter with no explicit "shift+" prefix — e.g. a bare
                // "Z" — encoded Shift entirely via the character's OWN
                // case, the pre-KeyCombo::new convention this file used
                // to rely on. Blindly lower-casing that (KeyCombo::new's
                // job below) without also recovering the shift it implied
                // would silently collide it with plain "z": for a
                // shift-SENSITIVE pair (z/Z sub-delay, x/X audio-delay —
                // see default_player_map's own doc comment) that's real
                // data loss, not a harmless dedupe — confirmed live, one
                // of the two colliding actions ends up completely
                // unbound (shows "—") depending on HashMap deserialization
                // order. Reconstruct the original intent instead: an
                // uppercase letter with no already-explicit shift is
                // shift+<lowercase letter>, matching what physically
                // produced it. Harmless when the two entries actually
                // pointed at the SAME action (the old redundant-pair
                // shape, e.g. "f"/"F" both -> Fullscreen) — that just
                // leaves a redundant-but-correct second entry rather than
                // colliding, which the next rebind of that action
                // naturally prunes away (rebind_action retains only the
                // one freshly-captured combo).
                //
                // Restricted to ctrl==false && alt==false (code review,
                // 2026-08-08): the legacy case-encodes-shift convention
                // only ever applied to a BARE letter with no modifier
                // prefix at all — the old Display always wrote an explicit
                // "shift+"/"ctrl+"/"alt+" prefix whenever that modifier was
                // actually held, so a string like "ctrl+Z" never meant
                // "Ctrl+Shift+z"; it means Ctrl+z captured with Caps Lock
                // on, shift genuinely not held. Reconstructing shift there
                // too would wrongly turn a real Ctrl+z binding into
                // Ctrl+Shift+z.
                if !shift && !ctrl && !alt {
                    if let Some(ch) = k.chars().next() {
                        if ch.is_uppercase() { shift = true; }
                    }
                }
                k.to_string()
            }
            k => return Err(format!("unknown key: {k}")),
        };
        Ok(KeyCombo::new(key, shift, ctrl, alt))
    }
}

impl serde::Serialize for KeyCombo {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_string())
    }
}

impl<'de> serde::Deserialize<'de> for KeyCombo {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        KeyCombo::try_from(s).map_err(serde::de::Error::custom)
    }
}

// ── KeyMap / Keybindings ──────────────────────────────────────────────────────

pub type KeyMap = HashMap<KeyCombo, Action>;

/// Custom `Deserialize` for `KeyMap` (code review, 2026-08-08). A plain
/// derived `HashMap<KeyCombo, Action>` deserialize just calls `.insert()`
/// per JSON entry in file order — if two DIFFERENT raw strings normalize to
/// the SAME `KeyCombo` (the Caps-Lock/case migration makes this reachable:
/// a pre-existing bare-uppercase legacy default like `"Z"` and a genuine
/// user rebind stored as `"shift+Z"` both resolve to `{z, shift:true}`),
/// the later one in the file silently overwrites the earlier one with no
/// trace — one binding just disappears. This visits the map as raw
/// `(String, Action)` pairs so a collision can be detected and resolved
/// deliberately instead of by accident: prefer whichever raw string carries
/// an explicit modifier prefix (`"shift+"`/`"ctrl+"`/`"alt+"`) over a bare
/// one, since the bare-uppercase form is exactly the redundant legacy
/// encoding this project's own default-keymap cleanup already prunes going
/// forward, while an explicit prefix only ever comes from deliberate intent
/// (either a real rebind, or the current Display format for a shift-bound
/// default). Logs a warning either way, so a resolved collision is visible
/// in `fjord.log` rather than fully silent.
fn deserialize_keymap<'de, D>(d: D) -> Result<KeyMap, D::Error>
where
    D: serde::Deserializer<'de>,
{
    struct KeyMapVisitor;

    impl<'de> serde::de::Visitor<'de> for KeyMapVisitor {
        type Value = KeyMap;

        fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "a map of key-combo strings to actions")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: serde::de::MapAccess<'de>,
        {
            let mut out: KeyMap = HashMap::new();
            let mut raw_by_combo: HashMap<KeyCombo, String> = HashMap::new();

            while let Some((raw_key, action)) = map.next_entry::<String, Action>()? {
                let combo = KeyCombo::try_from(raw_key.clone())
                    .map_err(|e| serde::de::Error::custom(format!("invalid key combo {raw_key:?}: {e}")))?;
                match raw_by_combo.get(&combo).cloned() {
                    Some(existing_raw) => {
                        let keep_new = raw_key.contains('+') && !existing_raw.contains('+');
                        warn!(
                            "keybindings.json: {raw_key:?} and {existing_raw:?} both resolve to \
                             {combo:?} — keeping {:?}",
                            if keep_new { &raw_key } else { &existing_raw }
                        );
                        if keep_new {
                            raw_by_combo.insert(combo.clone(), raw_key);
                            out.insert(combo, action);
                        }
                    }
                    None => {
                        raw_by_combo.insert(combo.clone(), raw_key);
                        out.insert(combo, action);
                    }
                }
            }
            Ok(out)
        }
    }

    d.deserialize_map(KeyMapVisitor)
}

/// The full binding configuration.
///
/// `normal` is checked in every non-player mode.
/// `player` is checked first when the player is open; any key not found there
/// falls through to `normal`, so global shortcuts (F, Q, Escape) always work.
///
/// The full effective keybindings are saved to `~/.config/fjord/keybindings.json`
/// on any rebind.  On next launch, the file is loaded directly (no default merge)
/// so explicit removals persist.  Missing file → compiled-in defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Keybindings {
    #[serde(default, deserialize_with = "deserialize_keymap")]
    pub normal: KeyMap,
    #[serde(default, deserialize_with = "deserialize_keymap")]
    pub player: KeyMap,
}

// ── ActionMap ─────────────────────────────────────────────────────────────────

/// Which KeyMap an action belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionMap { Normal, Player }

// ── AppMode ───────────────────────────────────────────────────────────────────

/// The active UI mode — computed by `active_mode()` from `AppState` flags.
/// Sub-modes (season row, player panel) are resolved inside their arm's handler.
/// `LibrarySearch`/`BrowseSearch` bypass key-lookup and are handled before `active_mode`.
/// `Login` is guarded before `active_mode` is called and never appears as a mode value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMode {
    ContextMenu, QueuePanel, NowPlaying, Person, Season, Series, Detail, Artist, Collection, Album,
    RequestOptions, RequestDetail, CalendarDayPopup, Calendar, Blocklist, Player, Library, Browse, Discover, Settings, Dashboard,
}

fn active_mode(g: &crate::AppState) -> AppMode {
    if g.get_show_context_menu()                                    { AppMode::ContextMenu }
    else if g.get_show_queue_panel()                                { AppMode::QueuePanel }
    else if g.get_show_now_playing() && g.get_is_audio_playing()    { AppMode::NowPlaying }
    else if g.get_show_person()     && !g.get_is_playing()         { AppMode::Person }
    else if g.get_show_detail()     && !g.get_is_playing()         { AppMode::Detail }
    else if g.get_show_season()     && !g.get_is_playing()         { AppMode::Season }
    else if g.get_show_series()     && !g.get_is_playing()         { AppMode::Series }
    else if g.get_show_artist()     && !g.get_is_playing()         { AppMode::Artist }
    else if g.get_show_collection() && !g.get_is_playing()         { AppMode::Collection }
    else if g.get_show_album()      && !g.get_is_playing()         { AppMode::Album }
    // Checked ahead of RequestDetail so the modal captures all input while
    // open — show-request-options can only ever be true while already on
    // that screen, so there's no ordering conflict with it taking priority.
    // !is_playing mirrors every other overlay-style mode above (real bug,
    // 2026-07-18: this was the one Seerr overlay missing it — resuming a
    // backgrounded player via 'r' while the modal was open left it stuck
    // rendered on top of the fullscreen video, still eating all keyboard
    // input meant for playback).
    else if g.get_show_request_options() && !g.get_is_playing()     { AppMode::RequestOptions }
    else if g.get_show_request_detail() && !g.get_is_playing()     { AppMode::RequestDetail }
    // Calendar (2026-07-18, Watchlist + Release Calendar) — same tier and
    // !is_playing guard as RequestOptions/RequestDetail above, for the
    // identical reason (see that fix's own comment). DayPopup checked
    // first so it captures all input while open, same nesting shape as
    // RequestOptions-over-RequestDetail.
    else if g.get_show_calendar_day_popup() && !g.get_is_playing()  { AppMode::CalendarDayPopup }
    else if g.get_show_calendar() && !g.get_is_playing()            { AppMode::Calendar }
    // Manage Blocklist (2026-08-06, Seerr Blocklist support) — same tier
    // and !is_playing guard as Calendar above, for the identical reason.
    else if g.get_show_blocklist() && !g.get_is_playing()           { AppMode::Blocklist }
    else if g.get_is_playing()                                      { AppMode::Player }
    else if g.get_show_library()                                    { AppMode::Library }
    else if g.get_show_browse()                                     { AppMode::Browse }
    else if g.get_active_nav() == 6                                 { AppMode::Discover }
    else if g.get_active_nav() == 10                                { AppMode::Settings }
    else                                                            { AppMode::Dashboard }
}

// ── Default keybindings ───────────────────────────────────────────────────────

pub fn default_keybindings() -> Keybindings {
    Keybindings {
        normal: default_normal_map(),
        player: default_player_map(),
    }
}

fn default_normal_map() -> KeyMap {
    let mut m = KeyMap::new();

    m.insert(KeyCombo::plain(key::ESCAPE),    Action::Back);
    m.insert(KeyCombo::plain(key::BACKSPACE),  Action::Back);
    m.insert(KeyCombo::plain(key::RETURN),     Action::Confirm);
    m.insert(KeyCombo::plain(key::UP),         Action::Up);
    m.insert(KeyCombo::plain(key::DOWN),       Action::Down);
    m.insert(KeyCombo::plain(key::LEFT),       Action::Left);
    m.insert(KeyCombo::plain(key::RIGHT),      Action::Right);
    m.insert(KeyCombo::plain("/"),             Action::SearchJump);

    // Single, shift-insensitive entry per letter (2026-08-08) — used to be
    // two ("f" and "F") to cover Shift/Caps Lock, which also meant the Key
    // Bindings screen showed both as separate labels for the same action.
    // lookup_action's own shift-and-retry-unshifted fallback now makes the
    // second entry unnecessary: pressing the key with Shift held (or with
    // Caps Lock on, which KeyCombo::new's lower-casing makes indistinguishable
    // from not holding Shift at all) still resolves to this one entry.
    m.insert(KeyCombo::plain("f"),             Action::Fullscreen);
    m.insert(KeyCombo::plain(key::F11),        Action::Fullscreen);
    // Ctrl+Q quits. Plain q belongs to OpenQueuePanel (Phase 51) — before
    // CR10-4, plain-q Quit entries here were silently overwritten by the
    // queue-panel inserts below, leaving Quit with no binding at all.
    m.insert(KeyCombo::new("q", false, true, false), Action::Quit);
    m.insert(KeyCombo::plain("b"),             Action::OpenBrowse);
    m.insert(KeyCombo::plain("1"),             Action::NavHome);
    m.insert(KeyCombo::plain("2"),             Action::NavMovies);
    m.insert(KeyCombo::plain("3"),             Action::NavTV);
    m.insert(KeyCombo::plain("s"),             Action::NavSettings);

    m.insert(KeyCombo::plain("i"),             Action::OpenDetail);
    m.insert(KeyCombo::plain("c"),             Action::OpenContextMenu);
    m.insert(KeyCombo::plain("r"),             Action::ResumePlayer);
    m.insert(KeyCombo::plain("n"),             Action::FocusFloatCard);

    m.insert(KeyCombo::plain("["),             Action::PrevTrack);
    m.insert(KeyCombo::plain("]"),             Action::NextTrack);
    m.insert(KeyCombo::plain("q"),             Action::OpenQueuePanel);
    m.insert(KeyCombo::plain("\u{007f}"),      Action::DeleteItem); // Delete key
    m.insert(KeyCombo::plain("l"),             Action::ToggleLyrics);
    m.insert(KeyCombo::plain("m"),             Action::ToggleNowPlaying);

    m
}

fn default_player_map() -> KeyMap {
    let mut m = KeyMap::new();

    m.insert(KeyCombo::plain(key::BACKSPACE),  Action::MinimizePlayer);

    m.insert(KeyCombo::plain(key::LEFT),       Action::SeekBackward);
    m.insert(KeyCombo::plain(key::RIGHT),      Action::SeekForward);
    m.insert(KeyCombo::shifted(key::LEFT),     Action::SeekBackwardLong);
    m.insert(KeyCombo::shifted(key::RIGHT),    Action::SeekForwardLong);
    m.insert(KeyCombo::plain(key::UP),         Action::VolumeUp);
    m.insert(KeyCombo::plain(key::DOWN),       Action::VolumeDown);

    m.insert(KeyCombo::plain(" "),             Action::PausePlay);
    m.insert(KeyCombo::plain("k"),             Action::PausePlay);
    m.insert(KeyCombo::plain("p"),             Action::PausePlay);
    m.insert(KeyCombo::plain("m"),             Action::Mute);

    m.insert(KeyCombo::plain("i"),             Action::ToggleStats);
    m.insert(KeyCombo::plain("s"),             Action::PanelSubtitles);
    m.insert(KeyCombo::plain("a"),             Action::PanelAudio);
    m.insert(KeyCombo::plain("v"),             Action::PanelVideo);

    m.insert(KeyCombo::plain("."),             Action::NextChapter);
    m.insert(KeyCombo::plain(","),             Action::PrevChapter);

    // Genuinely shift-SENSITIVE, unlike every plain letter above (matches
    // mpv's own convention: z/x increase, Shift+z/Shift+x decrease) — used
    // to be registered as two unshifted entries ("z" and "Z", relying on
    // "Z" only ever being reachable by literally typing a capital Z) rather
    // than an explicit shift:true combo, which happened to work before this
    // file's own Caps-Lock/case-normalization fix but was never really
    // correct: Caps Lock alone (no Shift held) would have produced the same
    // "Z" text and wrongly fired Decrease instead of Increase.
    // KeyCombo::shifted expresses the real intent directly.
    m.insert(KeyCombo::plain("z"),             Action::SubDelayIncrease);
    m.insert(KeyCombo::shifted("z"),           Action::SubDelayDecrease);
    m.insert(KeyCombo::plain("x"),             Action::AudioDelayIncrease);
    m.insert(KeyCombo::shifted("x"),           Action::AudioDelayDecrease);

    m.insert(KeyCombo::plain("["),             Action::PrevTrack);
    m.insert(KeyCombo::plain("]"),             Action::NextTrack);
    m.insert(KeyCombo::plain("q"),             Action::OpenQueuePanel);
    m.insert(KeyCombo::plain("l"),             Action::ToggleLyrics);

    m.insert(KeyCombo::plain("0"),             Action::SeekToPercent(0));
    m.insert(KeyCombo::plain("1"),             Action::SeekToPercent(10));
    m.insert(KeyCombo::plain("2"),             Action::SeekToPercent(20));
    m.insert(KeyCombo::plain("3"),             Action::SeekToPercent(30));
    m.insert(KeyCombo::plain("4"),             Action::SeekToPercent(40));
    m.insert(KeyCombo::plain("5"),             Action::SeekToPercent(50));
    m.insert(KeyCombo::plain("6"),             Action::SeekToPercent(60));
    m.insert(KeyCombo::plain("7"),             Action::SeekToPercent(70));
    m.insert(KeyCombo::plain("8"),             Action::SeekToPercent(80));
    m.insert(KeyCombo::plain("9"),             Action::SeekToPercent(90));

    m
}

// ── Remappable actions ────────────────────────────────────────────────────────

/// Ordered list of actions exposed in the key-binding settings UI.
/// `SeekToPercent` is excluded (parameterised; best edited in JSON directly).
/// Normal-map actions come first (indices 0..16), player-map actions follow
/// (indices 17..28).  `keybinding-focused` in AppState uses these same indices.
pub fn remappable_actions() -> Vec<(Action, &'static str, ActionMap)> {
    use ActionMap::*;
    vec![
        // Normal map — navigation
        (Action::Confirm,          "Confirm",           Normal),
        (Action::Back,             "Back",              Normal),
        (Action::Up,               "Up",                Normal),
        (Action::Down,             "Down",              Normal),
        (Action::Left,             "Left",              Normal),
        (Action::Right,            "Right",             Normal),
        (Action::SearchJump,       "Jump to Search",    Normal),
        // Normal map — global shortcuts
        (Action::NavHome,          "Nav: Home",         Normal),
        (Action::NavMovies,        "Nav: Movies",       Normal),
        (Action::NavTV,            "Nav: TV",           Normal),
        (Action::NavSettings,      "Nav: Settings",     Normal),
        (Action::OpenBrowse,       "Open Browse",       Normal),
        (Action::Fullscreen,       "Toggle Fullscreen", Normal),
        (Action::Quit,             "Quit",              Normal),
        // Normal map — item actions
        (Action::OpenDetail,       "Open Detail",       Normal),
        (Action::OpenContextMenu,  "Context Menu",      Normal),
        (Action::ResumePlayer,     "Resume Player",     Normal),
        (Action::FocusFloatCard,   "Focus Mini Player", Normal),
        // Player map
        (Action::PausePlay,        "Pause / Play",      Player),
        (Action::SeekBackward,     "Seek Back",         Player),
        (Action::SeekForward,      "Seek Fwd",          Player),
        (Action::SeekBackwardLong, "Seek Back (Long)",  Player),
        (Action::SeekForwardLong,  "Seek Fwd (Long)",   Player),
        (Action::VolumeUp,         "Volume Up",         Player),
        (Action::VolumeDown,       "Volume Down",       Player),
        (Action::Mute,             "Mute",              Player),
        (Action::ToggleStats,      "Toggle Stats",      Player),
        (Action::PanelSubtitles,   "Subtitles Panel",   Player),
        (Action::PanelAudio,       "Audio Panel",       Player),
        (Action::PanelVideo,       "Video Panel",       Player),
        (Action::MinimizePlayer,   "Minimize Player",   Player),
        (Action::NextChapter,       "Next Chapter",       Player),
        (Action::PrevChapter,       "Prev Chapter",       Player),
        (Action::SubDelayIncrease,  "Sub Delay +100ms",   Player),
        (Action::SubDelayDecrease,  "Sub Delay −100ms",   Player),
        (Action::AudioDelayIncrease,"Audio Delay +100ms", Player),
        (Action::AudioDelayDecrease,"Audio Delay −100ms", Player),
        // Playlist controls (normal map — active when music is playing)
        (Action::PrevTrack,         "Prev Track",         Normal),
        (Action::NextTrack,         "Next Track",         Normal),
        (Action::ToggleShuffle,     "Toggle Shuffle",     Normal),
        (Action::CycleRepeat,       "Cycle Repeat",       Normal),
    ]
}

// ── Key display helpers ───────────────────────────────────────────────────────

/// Human-readable label for a Slint key string (PUA codepoints → symbol names).
pub fn key_display_name(key: &str) -> String {
    match key {
        k if k == key::BACKSPACE => "Bksp".into(),
        k if k == key::RETURN    => "Enter".into(),
        k if k == key::ESCAPE    => "Esc".into(),
        k if k == key::UP        => "↑".into(),
        k if k == key::DOWN      => "↓".into(),
        k if k == key::LEFT      => "←".into(),
        k if k == key::RIGHT     => "→".into(),
        k if k == key::F11       => "F11".into(),
        " "                      => "Space".into(),
        k                        => k.into(),
    }
}

fn format_combo(combo: &KeyCombo) -> String {
    let key_name = key_display_name(&combo.key);
    let mut mods: Vec<&str> = vec![];
    if combo.ctrl  { mods.push("Ctrl"); }
    if combo.alt   { mods.push("Alt");  }
    if combo.shift { mods.push("Shift");}
    if mods.is_empty() { key_name }
    else { format!("{}+{}", mods.join("+"), key_name) }
}

/// All KeyCombos in `map` that resolve to `action`, formatted and joined with "  ".
/// Returns "—" if the action has no binding.
pub fn action_key_labels(action: &Action, map: &KeyMap) -> String {
    let mut labels: Vec<String> = map.iter()
        .filter(|(_, v)| *v == action)
        .map(|(k, _)| format_combo(k))
        .collect();
    if labels.is_empty() { return "—".into(); }
    labels.sort();
    labels.dedup();
    labels.join("  ")
}

// ── Keybinding row model ──────────────────────────────────────────────────────

fn build_keybinding_entries(kb: &Keybindings)
    -> (Vec<crate::KeyBindingEntry>, Vec<crate::KeyBindingEntry>)
{
    let mut normal_rows = vec![];
    let mut player_rows = vec![];

    for (action, label, map) in remappable_actions() {
        let the_map = match map { ActionMap::Normal => &kb.normal, ActionMap::Player => &kb.player };
        let key_str = action_key_labels(&action, the_map);
        let entry = crate::KeyBindingEntry {
            action: SharedString::from(label),
            key:    SharedString::from(key_str.as_str()),
        };
        match map {
            ActionMap::Normal => normal_rows.push(entry),
            ActionMap::Player => player_rows.push(entry),
        }
    }

    (normal_rows, player_rows)
}

pub(crate) fn push_keybinding_rows(window: &crate::MainWindow, state: &Arc<Mutex<FjordState>>) {
    let (normal_rows, player_rows) = {
        let st = state.lock().unwrap();
        build_keybinding_entries(&st.keybindings)
    };
    let g = crate::AppState::get(window);
    g.set_keybinding_normal(ModelRc::new(VecModel::from(normal_rows)));
    g.set_keybinding_player(ModelRc::new(VecModel::from(player_rows)));
}

// ── Rebind an action ──────────────────────────────────────────────────────────

/// Actually applies a rebind — the ONLY place that mutates `keybindings`,
/// shared by the direct (no-collision) path below and
/// `on_keybinding_collision_confirmed` (main.rs), which calls this once the
/// user has confirmed overwriting another action's binding.
pub(crate) fn apply_rebind(
    fi:        i32,
    new_combo: KeyCombo,
    state:     &Arc<Mutex<FjordState>>,
    window:    &crate::MainWindow,
) {
    let actions = remappable_actions();
    let Some((action, _, map)) = actions.get(fi as usize) else {
        debug!("keybindings: apply_rebind fi={fi} out of range ({} actions), ignoring", actions.len());
        return;
    };
    debug!("keybindings: rebinding {action:?} ({map:?}) -> {new_combo:?}");

    {
        let mut st = state.lock().unwrap();
        match map {
            ActionMap::Normal => {
                st.keybindings.normal.retain(|_, v| v != action);
                st.keybindings.normal.insert(new_combo, action.clone());
            }
            ActionMap::Player => {
                st.keybindings.player.retain(|_, v| v != action);
                st.keybindings.player.insert(new_combo, action.clone());
            }
        }
        crate::config::save_keybindings(&st.keybindings);
    }

    push_keybinding_rows(window, state);
}

/// Captures one rebind keypress. `KeyCombo::new` lower-cases `key` — a
/// rebind captured while Caps Lock happens to be on (or off) always lands
/// on the same stored combo, and `shift` alone (the physical Shift key,
/// unaffected by Caps Lock) decides whether it's a shift-sensitive binding.
///
/// If the captured combo already belongs to a DIFFERENT action, this does
/// NOT apply it — `HashMap::insert` would otherwise silently steal that
/// other action's binding with no warning at all. Instead it stashes the
/// pending rebind in `FjordState.pending_keybind_rebind` and shows a
/// confirm dialog (`show-keybinding-collision-confirm`); the actual apply
/// happens in `on_keybinding_collision_confirmed` (main.rs) via
/// `apply_rebind` above, once the user has explicitly said to overwrite it.
fn rebind_action(
    fi:     i32,
    key:    &str,
    shift:  bool,
    ctrl:   bool,
    state:  &Arc<Mutex<FjordState>>,
    window: &crate::MainWindow,
) {
    let actions = remappable_actions();
    let g = crate::AppState::get(window);
    // Either applied directly below, or handed off to the collision dialog
    // — either way, capture mode itself is over the moment a key lands.
    g.set_keybinding_rebinding(false);

    if fi < 0 || fi as usize >= actions.len() {
        debug!("keybindings: rebind_action fi={fi} out of range ({} actions), ignoring", actions.len());
        return;
    }

    let new_combo = KeyCombo::new(key, shift, ctrl, false);
    let (action, _, map) = &actions[fi as usize];
    debug!("keybindings: rebind capture {new_combo:?} for row {fi} ({action:?})");

    // Code review, 2026-08-08: this used to look the OTHER action up in
    // `remappable_actions()` (the settings-screen row list) and treat a miss
    // as "no collision" — but several real, bound actions have no row there
    // at all (OpenQueuePanel/q, DeleteItem/Delete, ToggleLyrics/l,
    // ToggleNowPlaying/m, SeekToPercent/0-9), so rebinding onto any of THEIR
    // keys silently stole the binding with no dialog — defeating the whole
    // "block and require confirmation" feature for exactly the bindings a
    // user is least likely to expect losing. Fall back to the action's own
    // Debug label when it isn't a settings-screen row, rather than treating
    // "no row" as "no collision".
    let collision: Option<String> = {
        let st = state.lock().unwrap();
        let existing_map = match map {
            ActionMap::Normal => &st.keybindings.normal,
            ActionMap::Player => &st.keybindings.player,
        };
        existing_map.get(&new_combo)
            .filter(|other| *other != action)
            .map(|other_action| {
                actions.iter()
                    .find(|(a, _, _)| a == other_action)
                    .map(|(_, label, _)| label.to_string())
                    .unwrap_or_else(|| format!("{other_action:?}"))
            })
    };

    if let Some(other_label) = collision {
        let message = format!("{new_combo} is already bound to {other_label} — reassign it?");
        debug!("keybindings: collision — {message}");
        state.lock().unwrap().pending_keybind_rebind = Some(PendingKeybindRebind { fi, combo: new_combo });
        g.set_keybinding_collision_message(message.into());
        g.set_keybinding_collision_confirm_focused(0);
        g.set_show_keybinding_collision_confirm(true);
        return;
    }

    apply_rebind(fi, new_combo, state, window);
}

// ── Dispatch ──────────────────────────────────────────────────────────────────

/// Look up `combo` in `map`. Most single-letter bindings ("f" → Fullscreen,
/// "b" → OpenBrowse, ...) are deliberately shift-insensitive — they're
/// registered once, unshifted, and meant to fire whether or not Shift was
/// held. A few (z/Z for sub-delay, x/X for audio-delay, matching mpv's own
/// convention) are deliberately shift-*sensitive* and register both an
/// unshifted and an explicit `KeyCombo::shifted(...)` entry for two
/// different actions. This function has to serve both: try the exact combo
/// first (so a shift-sensitive pair's own shifted entry always wins over
/// falling through to its unshifted sibling), then, only on a miss with
/// shift held, retry unshifted (so a shift-insensitive binding's letter
/// still fires when actually typed with Shift held, since it only ever
/// registered the unshifted form). Named keys (arrows etc., PUA codepoints)
/// never get the retry, so Shift+Left stays distinct from Left rather than
/// silently falling back to plain seeking. `KeyCombo::new` already
/// lower-cases `key` for both `combo` and everything in `map`, so this
/// never needs to reason about letter case itself — only about whether an
/// exact (key, shift) match exists.
fn lookup_action(map: &KeyMap, combo: &KeyCombo) -> Option<Action> {
    if let Some(a) = map.get(combo) { return Some(a.clone()); }
    if combo.shift && is_printable(&combo.key) {
        let unshifted = KeyCombo { shift: false, ..combo.clone() };
        return map.get(&unshifted).cloned();
    }
    None
}

pub(crate) fn handle_key(
    key:    &str,
    shift:  bool,
    ctrl:   bool,
    repeat: bool,
    state:  &Arc<Mutex<FjordState>>,
    window: &crate::MainWindow,
    _rt:    &tokio::runtime::Handle,
) -> bool {
    let g = crate::AppState::get(window);

    if key.is_empty() { return false; }

    // LoginScreen: return false below to let LineEdit handle normal typing/
    // tabbing, but Ctrl+Q must be carved out first — it would otherwise never
    // reach the global Quit pre-dispatch further down, same class of bug just
    // fixed for the connectivity-gate screens below.
    if g.get_show_login() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        return false;
    }

    // ProfilePickerScreen (Bonfire Phase 1, step 6, 2026-08-09) — same tier
    // as show-login above (checked before active_mode() ever runs, never
    // appears as an AppMode value). Raw-key handling, same shape as
    // OfflineScreen below: no native widget focus path, so Left/Right/Enter
    // are matched directly rather than going through the Action/KeyMap
    // layer. PIN entry is a layered sub-state that captures all input first
    // when open — mirrors VirtualKeyboard's own 12-key row-major layout
    // (widgets.slint) exactly, so keyboard and mouse activation always
    // agree on what "cursor N" means.
    if g.get_show_profile_picker() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if g.get_show_profile_pin_entry() {
            if key == key::RETURN {
                g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
            }
            const PIN_VALS: [&str; 12] = ["1","2","3","4","5","6","7","8","9","backspace","0","confirm"];
            match key {
                key::LEFT   => g.set_profile_pin_cursor((g.get_profile_pin_cursor() - 1).max(0)),
                key::RIGHT  => g.set_profile_pin_cursor((g.get_profile_pin_cursor() + 1).min(11)),
                key::UP     => g.set_profile_pin_cursor((g.get_profile_pin_cursor() - 3).max(0)),
                key::DOWN   => g.set_profile_pin_cursor((g.get_profile_pin_cursor() + 3).min(11)),
                key::RETURN => {
                    if let Some(v) = PIN_VALS.get(g.get_profile_pin_cursor() as usize) {
                        g.invoke_profile_pin_key((*v).into());
                    }
                }
                key::ESCAPE | key::BACKSPACE => {
                    g.set_show_profile_pin_entry(false);
                    g.set_profile_pin_error("".into());
                }
                _ => {}
            }
            return true;
        }
        // 2026-08-16, direct follow-up to the Back-button fix immediately
        // below ("quit it not also reacheble by keybord navigation"): the
        // on-screen Quit button had the identical gap — Ctrl+Q already
        // quits from any screen, but there was no keyboard CURSOR path
        // onto the button itself. Down from the tile row (below) sets
        // this — always reachable, unlike the conditional Back button;
        // Up returns to the tile row, Enter activates, Escape/Backspace
        // un-focuses it without quitting (quitting is a terminal action,
        // not something Escape should trigger as a side effect).
        if g.get_profile_picker_quit_focused() {
            match key {
                key::UP => g.set_profile_picker_quit_focused(false),
                key::RETURN => {
                    g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
                    g.set_profile_picker_quit_focused(false);
                    g.invoke_quit();
                }
                key::ESCAPE | key::BACKSPACE => g.set_profile_picker_quit_focused(false),
                _ => {}
            }
            return true;
        }
        // 2026-08-16, real bug ("the button shows but i cant navigate to
        // it with keybord and press enter"): the "← Back to Accounts"
        // button was mouse-only — visible and clickable, but with no
        // keyboard CURSOR path onto it at all; only the Escape/Backspace
        // shortcut below reached the same action. Handled as its own
        // focus state, mirroring the "Back button focused" convention
        // every other content-style screen in this app already
        // establishes (Detail/Season/Collection/Album/Artist: Up from the
        // top of content focuses Back, Down returns to content, Enter
        // activates) — Up from the tile row below sets this when the
        // button exists; here, Down returns to the tile row and
        // Enter/Escape/Backspace all activate it, same destination the
        // pre-existing shortcut already reaches.
        if g.get_profile_picker_back_focused() {
            match key {
                key::DOWN => g.set_profile_picker_back_focused(false),
                key::RETURN => {
                    g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
                    g.set_profile_picker_back_focused(false);
                    g.invoke_profile_picker_back_to_accounts();
                }
                key::ESCAPE | key::BACKSPACE => {
                    g.set_profile_picker_back_focused(false);
                    g.invoke_profile_picker_back_to_accounts();
                }
                _ => {}
            }
            return true;
        }
        // 2026-08-14, the 2-tier redesign: Escape/Backspace goes back ONE
        // level at a time. If there's an account tier to return to
        // (profile-picker-show-back-to-accounts — true whenever 2+
        // accounts exist at all, independent of whether it was actually
        // shown on the way here), that's always where Back goes, even for
        // a non-cancelable startup-gate picker (going back to the account
        // tier isn't "cancel the whole flow," there was never a live
        // session to cancel back to in the first place at the account
        // tier either). Only when there's NO account tier at all does
        // profile-picker-cancelable decide whether Escape does anything —
        // the original startup gate (single account, no live session)
        // still has no Back/Escape handling, unchanged.
        if key == key::ESCAPE || key == key::BACKSPACE {
            if g.get_profile_picker_show_back_to_accounts() {
                g.invoke_profile_picker_back_to_accounts();
                return true;
            }
            if g.get_profile_picker_cancelable() {
                g.set_show_profile_picker(false);
                window.invoke_grab_keyboard_focus();
                return true;
            }
        }
        // No trailing "+ Add Account" cursor slot anymore (2026-08-14) —
        // this screen is always scoped to one account's own profiles, and
        // adding a brand-new account lives on the account tier instead.
        let count = g.get_profile_picker_profiles().row_count() as i32;
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            key::UP if g.get_profile_picker_show_back_to_accounts() => {
                g.set_profile_picker_back_focused(true);
            }
            key::DOWN  => g.set_profile_picker_quit_focused(true),
            key::LEFT  => g.set_profile_picker_cursor((g.get_profile_picker_cursor() - 1).max(0)),
            key::RIGHT => g.set_profile_picker_cursor((g.get_profile_picker_cursor() + 1).min((count - 1).max(0))),
            key::RETURN => {
                let cursor = g.get_profile_picker_cursor();
                if let Some(t) = g.get_profile_picker_profiles().row_data(cursor as usize) {
                    g.invoke_profile_picker_select(t.user_id);
                }
            }
            _ => {}
        }
        return true;
    }

    // Account picker (2026-08-14, the 2-tier account/profile redesign) —
    // same tier and shape as ProfilePickerScreen just above (checked
    // before active_mode() ever runs); no PIN sub-state at this tier at
    // all (accounts aren't PIN-protected, only profiles within them are —
    // picking a single-profile account either switches directly or opens
    // ProfilePickerScreen's own PIN modal, never one here).
    if g.get_show_account_picker() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        // 2026-08-16, same Quit-keyboard-reachability fix as the profile
        // picker's own quit-focused branch just above — see that block's
        // doc comment for the full reasoning.
        if g.get_account_picker_quit_focused() {
            match key {
                key::UP => g.set_account_picker_quit_focused(false),
                key::RETURN => {
                    g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
                    g.set_account_picker_quit_focused(false);
                    g.invoke_quit();
                }
                key::ESCAPE | key::BACKSPACE => g.set_account_picker_quit_focused(false),
                _ => {}
            }
            return true;
        }
        if (key == key::ESCAPE || key == key::BACKSPACE) && g.get_account_picker_cancelable() {
            g.set_show_account_picker(false);
            window.invoke_grab_keyboard_focus();
            return true;
        }
        let count = g.get_account_picker_accounts().row_count() as i32; // == "+ Add Account" tile's cursor value
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            key::DOWN  => g.set_account_picker_quit_focused(true),
            key::LEFT  => g.set_account_picker_cursor((g.get_account_picker_cursor() - 1).max(0)),
            key::RIGHT => g.set_account_picker_cursor((g.get_account_picker_cursor() + 1).min(count)),
            key::RETURN => {
                let cursor = g.get_account_picker_cursor();
                if cursor == count {
                    g.invoke_account_picker_add_account();
                } else if let Some(t) = g.get_account_picker_accounts().row_data(cursor as usize) {
                    g.invoke_account_picker_select(t.root_id);
                }
            }
            _ => {}
        }
        return true;
    }

    // Sidebar profile quick-menu (2026-08-14) — dim-backdrop overlay opened
    // from the sidebar's own profile row; same raw-key-tier shape as the
    // profile picker just above (checked before active_mode() ever runs).
    if g.get_show_sidebar_profile_menu() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        let count = g.get_sidebar_profile_menu_rows().row_count() as i32;
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            key::UP     => g.set_sidebar_profile_menu_focused((g.get_sidebar_profile_menu_focused() - 1).max(0)),
            key::DOWN   => g.set_sidebar_profile_menu_focused((g.get_sidebar_profile_menu_focused() + 1).min(count - 1)),
            key::RETURN => g.invoke_sidebar_profile_menu_action(g.get_sidebar_profile_menu_focused()),
            key::ESCAPE | key::BACKSPACE => {
                g.set_show_sidebar_profile_menu(false);
                window.invoke_grab_keyboard_focus();
            }
            _ => {}
        }
        return true;
    }

    // ConnectSeerrScreen: same native-LineEdit-focus shape as LoginScreen —
    // let typing/tabbing pass through untouched. Still bump the centralized
    // press-pulse counter on Enter (Phase 105's PressPulse-driven buttons
    // don't flash on keyboard Enter otherwise — the same class of gap fixed
    // for OfflineScreen/PlaylistPicker, see CLAUDE.md) and handle Escape to
    // close, since a LineEdit-focused form has no other "cancel" key.
    if g.get_show_connect_seerr() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        if key == key::ESCAPE {
            g.set_show_connect_seerr(false);
            window.invoke_grab_keyboard_focus();
            return true;
        }
        return false;
    }

    // ManageProfilesScreen / ProfileEditScreen (Bonfire Phase 2, 2026-08-09)
    // — same minimal shape as ConnectSeerrScreen above: mouse (+ physical
    // keyboard for ProfileEditScreen's LineEdits) is the primary input
    // method for these two screens (see app_state.slint's own doc comment
    // on profile-edit-* for why), so Rust only needs Escape-to-close +
    // Ctrl+Q + the press-pulse bump, not a full D-pad dispatch.
    if g.get_show_manage_profiles() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if key == key::ESCAPE {
            g.set_show_manage_profiles(false);
            window.invoke_grab_keyboard_focus();
            return true;
        }
        // Real bug, code-review 2026-08-16: this screen previously had no
        // keyboard navigation at all beyond Escape/Ctrl+Q — a dead end for
        // a D-pad/remote user. Mirrors AccountPickerScreen's own tile-row +
        // trailing "+" tile dispatch exactly (Left/Right cursor, Enter
        // activates); AppState.manage-profiles-cursor was already declared
        // for exactly this, just never wired.
        let list_count = g.get_manage_profiles_list().row_count() as i32;
        let add_shown  = list_count < 5;
        let max_cursor = if add_shown { list_count } else { (list_count - 1).max(0) };
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            key::LEFT  => g.set_manage_profiles_cursor((g.get_manage_profiles_cursor() - 1).max(0)),
            key::RIGHT => g.set_manage_profiles_cursor((g.get_manage_profiles_cursor() + 1).min(max_cursor)),
            key::RETURN => {
                let cursor = g.get_manage_profiles_cursor();
                if add_shown && cursor == list_count {
                    g.invoke_manage_profiles_add();
                } else if let Some(t) = g.get_manage_profiles_list().row_data(cursor as usize) {
                    g.invoke_manage_profiles_select(t.user_id);
                }
            }
            _ => {}
        }
        return true;
    }
    if g.get_show_profile_edit() {
        if ctrl && (key == "q" || key == "Q") {
            g.invoke_quit();
            return true;
        }
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        if key == key::ESCAPE {
            g.invoke_profile_edit_cancel();
            return true;
        }
        return false;
    }

    // Startup connectivity gate: ConnectingScreen has nothing to focus; on
    // OfflineScreen Left/Right cycle the 3 buttons (0=Retry 1=Change Server
    // 2=Quit — a permanent failure needs a way out besides retrying forever)
    // and Enter activates the focused one, since neither screen has a native
    // widget focus path the way LoginScreen's LineEdits do. Ctrl+Q still
    // quits directly regardless of focus, matching the global Quit shortcut
    // used everywhere else — both branches below return unconditionally, so
    // without this the later global Quit pre-dispatch would never run here.
    if (g.get_show_connecting() || g.get_show_offline()) && ctrl && (key == "q" || key == "Q") {
        g.invoke_quit();
        return true;
    }
    if g.get_show_connecting() { return true; }
    if g.get_show_offline() {
        // Bump the same central press-pulse counter as the main RETURN handler
        // below (which this block returns before ever reaching) — otherwise
        // OfflineScreen's Retry/Change Server/Quit FjordButtons, which DO react
        // to kb-activate-pulse via their own built-in PressPulse, never flash on
        // keyboard Enter, only on mouse click.
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        match key {
            key::LEFT  => g.set_offline_focused((g.get_offline_focused() + 2) % 3),
            key::RIGHT => g.set_offline_focused((g.get_offline_focused() + 1) % 3),
            key::RETURN => match g.get_offline_focused() {
                0 => g.invoke_retry_connection(),
                1 => g.invoke_sign_out(),
                _ => g.invoke_quit(),
            },
            _ => {}
        }
        return true;
    }

    // Search field text-input modes bypass the KeyMap
    if g.get_show_library() && g.get_library_header_focused() {
        return handle_library_search(key, ctrl, window);
    }
    if g.get_show_browse() && g.get_browse_header_focused() {
        return handle_browse_search(key, ctrl, window);
    }
    if g.get_active_nav() == 6 && !g.get_show_request_detail() && g.get_discover_header_focused() {
        return handle_discover_search(key, ctrl, window);
    }
    if g.get_show_playlist_picker() {
        // Same reason as the show_offline block above: this returns before the
        // main RETURN handler's kb-activate-pulse bump, so PlaylistPicker's own
        // PressPulse-driven rows (new-pulse/p-pulse in context_menu.slint) never
        // flashed on keyboard Enter.
        if key == key::RETURN {
            g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
        }
        return handle_playlist_picker(key, ctrl, window);
    }

    // While a detail/series page is loading (app-content-loading), block all keys except
    // Back/Escape (cancel the pending load) and Quit.
    if g.get_app_content_loading() {
        let cancel = key == key::ESCAPE || key == key::BACKSPACE;
        let quit   = ctrl && (key == "q" || key == "Q");
        if cancel || quit {
            g.set_app_content_loading(false);
            g.set_app_loading_progress(0.0);
            // Clear both IDs so any still-running fetch tasks see a stale check and exit.
            g.set_detail_id("".into());
            g.set_series_id("".into());
            if quit { g.invoke_quit(); }
        }
        return true; // swallow all keys during loading
    }

    // Keybinding rebind capture
    if g.get_keybinding_rebinding() {
        if key == key::ESCAPE {
            debug!("keybindings: rebind cancelled (Escape)");
            g.set_keybinding_rebinding(false);
        } else {
            let fi = g.get_keybinding_focused();
            debug!("keybindings: rebind capture key={key:?} shift={shift} ctrl={ctrl} for row {fi}");
            drop(g);
            rebind_action(fi, key, shift, ctrl, state, window);
        }
        return true;
    }

    // Tab in library grid mode: toggle sort bar focus
    if key == "\t" && g.get_show_library() && !g.get_library_header_focused() {
        let focused = g.get_library_sort_focused();
        g.set_library_sort_focused(!focused);
        if !focused { g.set_library_sort_cursor(sort_bar_init_cursor(&g)); }
        return true;
    }

    // Key → Action lookup. KeyCombo::new lower-cases key, so Caps Lock never
    // affects which binding this resolves to — only the physical Shift key
    // state (shift, reported separately by Slint) does.
    let combo     = KeyCombo::new(key, shift, ctrl, false);
    let in_player = g.get_is_playing();
    let action: Option<Action> = {
        let s = state.lock().unwrap();
        if in_player {
            lookup_action(&s.keybindings.player, &combo)
                .or_else(|| lookup_action(&s.keybindings.normal, &combo))
        } else {
            lookup_action(&s.keybindings.normal, &combo)
        }
    };
    let mode = active_mode(&g);
    // AppState.sidebar-kb-active (app_state.slint) is a pure Slint expression
    // mirroring this same active_mode()==Dashboard condition, not something
    // pushed from here — it needs to stay correct for mouse-driven screen
    // changes too, not just keyboard ones.
    // Every focusable widgets.slint::PressPulse instance plays a brief
    // border-flash "press" cue when this bumps, gated on its own already-
    // existing focus/selection expression — this is the single centralized
    // hook for keyboard press feedback, mirroring how active_mode() itself
    // centralizes screen-priority logic instead of scattering it. Mouse press
    // feedback needs no Rust involvement (TouchArea.pressed is used directly).
    if key == key::RETURN {
        g.set_kb_activate_pulse(g.get_kb_activate_pulse().wrapping_add(1));
    }
    drop(g);

    // Ctrl+Q quits from ANY mode. Quit has no per-screen meaning, so it is
    // handled here once instead of per-mode — the old per-screen arms only
    // covered dashboard/settings/series/season/detail/person, which is why
    // q/Q never worked in the library grid, browse, player, or the music
    // screens (CR10-4 follow-up).
    if action == Some(Action::Quit) {
        crate::AppState::get(window).invoke_quit();
        return true;
    }

    // F / F11 toggles fullscreen from any non-player mode. Several focus states
    // (album/artist/collection button rows, queue panel, context menu) swallowed
    // it with catch-all arms; like Quit it has no per-screen meaning. The Player
    // arm keeps its own handling so the controls-reveal behaviour is unchanged.
    if action == Some(Action::Fullscreen) && mode != AppMode::Player {
        crate::AppState::get(window).invoke_toggle_fullscreen();
        return true;
    }

    // Global R: resume background player from any non-fullscreen, non-detail, non-overlay mode.
    // RequestDetail/RequestOptions added 2026-07-18 (real bug) — they're the
    // same class of detail/overlay screen as Person/Detail/.../Album above but
    // were missing from this list, so 'r' could yank the user into the
    // fullscreen player mid-request-flow.
    if action == Some(Action::ResumePlayer)
        && !matches!(mode, AppMode::Player | AppMode::Person | AppMode::Season | AppMode::Detail | AppMode::Artist | AppMode::Collection | AppMode::Album | AppMode::ContextMenu | AppMode::QueuePanel | AppMode::NowPlaying | AppMode::RequestDetail | AppMode::RequestOptions | AppMode::Calendar | AppMode::CalendarDayPopup | AppMode::Blocklist)
    {
        let g = crate::AppState::get(window);
        if g.get_has_background_player() { g.invoke_resume_player(); return true; }
    }

    // N: focus the mini-player bar from any non-player screen.
    if action == Some(Action::FocusFloatCard) && mode != AppMode::Player && mode != AppMode::ContextMenu {
        let g = crate::AppState::get(window);
        if g.get_has_background_player() && !g.get_is_playing() {
            g.set_float_card_focused(0);
            return true;
        }
    }

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
                if let Some(e) = items.row_data(i) {
                    if e.is_current { g.set_queue_panel_cursor(i as i32); break; }
                }
            }
            g.set_show_queue_panel(true);
        } else {
            crate::show_toast(slint::ComponentHandle::as_weak(window), "Queue is empty".to_string());
        }
        return true;
    }

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
        return true;
    }

    // Global playlist controls when audio is playing (fire from any mode except ContextMenu).
    if mode != AppMode::ContextMenu {
        let g = crate::AppState::get(window);
        if g.get_is_audio_playing() {
            if let Some(ref a) = action {
                match a {
                    Action::PrevTrack      => { g.invoke_queue_prev_track();   return true; }
                    Action::NextTrack      => { g.invoke_queue_next_track();   return true; }
                    Action::ToggleShuffle  => { g.invoke_toggle_shuffle();     return true; }
                    Action::CycleRepeat    => { g.invoke_cycle_repeat();       return true; }
                    Action::ToggleLyrics   => { g.invoke_toggle_lyrics();      return true; }
                    _ => {}
                }
            }
        }
    }

    // Music bar keyboard focus: intercept nav keys when a button is focused.
    // Layout: [art (0)] | [⏸/▶ (1)] [⏹ (2)] | [timeline (3)] | [⏮ (4)] [⏭ (5)] [⇌ (6)] [↺ (7)] [⋮ (8)] [♪ (9)] [🔉 (10)] [🔊 (11)]
    //         Left zone   Centre zone            Below buttons   Right zone (9 only when lyrics-available; 10/11 always)
    // Left/Right: 0↔1↔2 → 4↔5↔6↔7↔8↔(9)↔10↔11 (skip over 3, and over 9 when lyrics unavailable); Down from any button→3; Up from 3→1.
    // RequestDetail/RequestOptions added 2026-07-18 (real bug, same class as
    // the QueuePanel/NowPlaying exclusions already here) — a stale
    // music-bar-focused >= 0 left over from earlier keyboard navigation
    // survives a mouse-driven screen switch (mouse clicks bypass handle_key
    // entirely) and would otherwise hijack this screen's own arrow keys/Enter.
    if !matches!(mode, AppMode::Player | AppMode::ContextMenu | AppMode::QueuePanel | AppMode::NowPlaying | AppMode::RequestDetail | AppMode::RequestOptions | AppMode::Calendar | AppMode::CalendarDayPopup | AppMode::Blocklist) {
        let mf = crate::AppState::get(window).get_music_bar_focused();
        if mf >= 0 {
            let g = crate::AppState::get(window);
            if g.get_is_audio_playing() {
                let Some(ref action) = action else { return false; };
                match action {
                    Action::Left => {
                        match mf {
                            3 => { g.invoke_music_bar_seek_rel(-10.0); }
                            4 => { g.set_music_bar_focused(2); }  // ⏮ ← ⏹ (skip timeline)
                            // Slot 10 (🔉) steps back to 9 (♪) only when lyrics are available.
                            10 => {
                                g.set_music_bar_focused(if g.get_lyrics_available() { 9 } else { 8 });
                            }
                            1 | 2 | 5 | 6 | 7 | 8 | 9 | 11 => { g.set_music_bar_focused(mf - 1); }
                            _ => {} // 0: absorbed
                        }
                        return true;
                    }
                    Action::Right => {
                        match mf {
                            3 => { g.invoke_music_bar_seek_rel(10.0); }
                            2 => { g.set_music_bar_focused(4); }  // ⏹ → ⏮ (skip timeline)
                            // Slot 8 (⋮) advances to 9 (♪) when available, else straight to 10 (🔉).
                            8 => {
                                g.set_music_bar_focused(if g.get_lyrics_available() { 9 } else { 10 });
                            }
                            0 | 1 | 4 | 5 | 6 | 7 | 9 | 10 => { g.set_music_bar_focused(mf + 1); }
                            _ => {} // 11: absorbed
                        }
                        return true;
                    }
                    Action::Down => {
                        if mf != 3 { g.set_music_bar_focused(3); }
                        return true;
                    }
                    Action::Up => {
                        if mf == 3 { g.set_music_bar_focused(1); }
                        else { g.set_music_bar_focused(-1); }
                        return true;
                    }
                    Action::Confirm => {
                        match mf {
                            0 => { g.invoke_open_now_playing(); }
                            2 => { g.set_music_bar_focused(-1); g.invoke_music_bar_stop(); }
                            4 => { g.invoke_queue_prev_track(); }
                            5 => { g.invoke_queue_next_track(); }
                            6 => { g.invoke_toggle_shuffle(); }
                            7 => { g.invoke_cycle_repeat(); }
                            8 => {
                                // ⋮ Queue button: open queue panel
                                g.invoke_refresh_queue_display();
                                let items = g.get_queue_items();
                                for i in 0..items.row_count() {
                                    if let Some(e) = items.row_data(i) {
                                        if e.is_current { g.set_queue_panel_cursor(i as i32); break; }
                                    }
                                }
                                g.set_show_queue_panel(true);
                            }
                            9 => { g.invoke_toggle_lyrics(); } // ♪ Lyrics
                            10 => { g.invoke_volume_down(); }  // 🔉
                            11 => { g.invoke_volume_up(); }    // 🔊
                            _ => { g.invoke_music_bar_play_pause(); } // 1 or 3
                        }
                        return true;
                    }
                    Action::Back => {
                        g.set_music_bar_focused(-1);
                        return true;
                    }
                    _ => {}
                }
            } else {
                crate::AppState::get(window).set_music_bar_focused(-1);
            }
        }
    }

    // Mini-player bar focused: intercept nav keys before the underlying screen sees them.
    // RequestDetail/RequestOptions added 2026-07-18 — same stale-focus-survives-
    // a-mouse-click reasoning as the music-bar block above.
    if !matches!(mode, AppMode::Player | AppMode::ContextMenu | AppMode::NowPlaying | AppMode::QueuePanel | AppMode::RequestDetail | AppMode::RequestOptions | AppMode::Calendar | AppMode::CalendarDayPopup | AppMode::Blocklist) {
        let fc = crate::AppState::get(window).get_float_card_focused();
        if fc >= 0 {
            let g = crate::AppState::get(window);
            if g.get_has_background_player() && !g.get_is_playing() {
                let Some(ref action) = action else { return false; };
                match action {
                    Action::Left | Action::Right => {
                        g.set_float_card_focused(1 - fc);
                        return true;
                    }
                    Action::Confirm => {
                        g.set_float_card_focused(-1);
                        if fc == 0 { g.invoke_resume_player(); } else { g.invoke_stop_playback(); }
                        return true;
                    }
                    Action::Up | Action::Back => {
                        g.set_float_card_focused(-1);
                        return true;
                    }
                    Action::Down => {
                        return true; // already at bottom, absorb
                    }
                    _ => {}
                }
            } else {
                crate::AppState::get(window).set_float_card_focused(-1);
            }
        }
    }

    // Music bar: Space/K/P pause/play during audio-only from any non-player mode.
    // PausePlay lives in the player map; look it up directly when is-audio-playing.
    if !matches!(mode, AppMode::Player | AppMode::ContextMenu) {
        let g = crate::AppState::get(window);
        if g.get_is_audio_playing() {
            let player_action = lookup_action(&state.lock().unwrap().keybindings.player, &combo);
            if let Some(Action::PausePlay) = player_action { g.invoke_music_bar_play_pause(); return true; }
        }
    }

    // ── Per-screen dispatch ───────────────────────────────────────────────────
    // Priority is encoded once in active_mode(); each arm is exhaustive.
    match mode {
        AppMode::ContextMenu => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return true; }; // swallow unknown keys
            crate::context_menu::handle_key(&action, &g)
        }

        AppMode::QueuePanel => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return true; }; // swallow unknown keys
            handle_key_queue_panel(&action, &g)
        }

        AppMode::NowPlaying => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return true; }; // swallow unknown keys
            handle_key_now_playing(&action, &g)
        }

        AppMode::Person => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::person::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Season => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::season::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Series => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::series::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        // show-detail stays true during playback (hidden by !is-playing in main.slint);
        // active_mode() already routes is-playing → Player, so this arm is safe.
        AppMode::Detail => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::detail::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Artist => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::artist::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Collection => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::collection::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Album => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::album::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::RequestOptions => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return true; }; // swallow unknown keys, same as ContextMenu/QueuePanel
            crate::discover::handle_key_request_options(&action, &g)
        }

        AppMode::RequestDetail => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::discover::handle_key_request_detail(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Calendar => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::discover::handle_key_calendar(&action, &g)
        }

        AppMode::CalendarDayPopup => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return true; }; // swallow unknown keys, same as ContextMenu/QueuePanel/RequestOptions
            crate::discover::handle_key_calendar_day_popup(&action, &g)
        }

        AppMode::Blocklist => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return true; }; // swallow unknown keys, same as Calendar's own sibling modes
            crate::blocklist::handle_key(&action, &g)
        }

        AppMode::Discover => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::discover::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Player => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            // ToggleStats and PausePlay must not reveal the full controls bar.
            // Seek actions use seek accumulation + minimal bar (no full controls).
            // Confirm (Enter) activates skip/banner/panel overlays — should not reveal controls.
            let shows_controls = !matches!(action,
                Action::ToggleStats
                | Action::PausePlay
                | Action::SeekBackward | Action::SeekForward
                | Action::SeekBackwardLong | Action::SeekForwardLong
                | Action::NextChapter | Action::PrevChapter
                | Action::SubDelayIncrease | Action::SubDelayDecrease
                | Action::AudioDelayIncrease | Action::AudioDelayDecrease
                | Action::PrevTrack | Action::NextTrack
                | Action::Confirm
            );
            if shows_controls { g.invoke_show_controls(); }
            drop(g);
            dispatch_player(action, window)
        }

        AppMode::Library => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            dispatch_library(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Browse => {
            let g = crate::AppState::get(window);
            let Some(action) = action else { return false; };
            crate::browse::handle_key(&action, &g) || focus_bar_on_up(&action, window) || focus_bar_on_down(&action, window)
        }

        AppMode::Settings => {
            let Some(action) = action else { return false; };
            {
                let g = crate::AppState::get(window);
                if g.get_keybinding_focused() >= 0 {
                    return dispatch_keybinding_nav(action, &g);
                }
            }
            {
                let g = crate::AppState::get(window);
                if let Some(handled) = crate::settings::dispatch_settings(&action, &g) {
                    return handled;
                }
            }
            // dispatch_settings returned None: settings-section == "" (sidebar mode).
            // Let sidebar Up/Down and global shortcuts through so nav remains functional.
            dispatch_dashboard(&action, repeat, window)
                || handle_global_shortcuts(&action, window)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }

        AppMode::Dashboard => {
            let Some(action) = action else { return false; };
            if handle_global_shortcuts(&action, window) { return true; }
            dispatch_dashboard(&action, repeat, window)
                || focus_bar_on_up(&action, window)
                || focus_bar_on_down(&action, window)
        }
    }
}

// ── Bar focus fallbacks ───────────────────────────────────────────────────────
// Both the video mini-bar and the music bar are docked at the bottom of the
// window (Phase 49+). focus_bar_on_down is called when a screen's Down handler
// falls off the bottom; it focuses whichever bar is currently visible.
// focus_bar_on_up is kept as a no-op so call sites compile without change.
fn focus_bar_on_up(_action: &Action, _window: &crate::MainWindow) -> bool { false }

fn focus_bar_on_down(action: &Action, window: &crate::MainWindow) -> bool {
    if *action != Action::Down { return false; }
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

// ── Library grid dispatch ─────────────────────────────────────────────────────

fn dispatch_library(action: &Action, g: &crate::AppState) -> bool {
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
                if c > 0 { g.set_library_sort_cursor(c - 1); }
                return true;
            }
            Action::Right => {
                let c   = g.get_library_sort_cursor();
                let nav = g.get_active_nav();
                // Music: cursor 0-2=view, 3-7=sort, 8=Favorites. Others: 0-4=sort, 5-6=filters or 0-4.
                let max = if nav == 4 { 8 } else if g.get_library_has_filters() { 6 } else { 4 };
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
                let c    = g.get_library_sort_cursor();
                let nav  = g.get_active_nav();
                let sort = g.get_library_sort();
                let fw   = g.get_library_filter_unwatched();
                let ff   = g.get_library_filter_favorites();
                if nav == 4 {
                    // Music: 0=Artists, 1=Albums, 2=Playlists, 3-7=sort(c-3), 8=Favorites
                    match c {
                        0 => { g.invoke_library_music_view_changed(0); g.set_library_sort_focused(false); }
                        1 => { g.invoke_library_music_view_changed(1); g.set_library_sort_focused(false); }
                        2 => { g.invoke_library_music_view_changed(2); g.set_library_sort_focused(false); }
                        3..=7 => { g.invoke_library_sort_apply(c - 3, fw, ff); g.set_library_sort_focused(false); }
                        _ => g.invoke_library_sort_apply(sort, fw, !ff), // 8=Favorites, stays open
                    }
                } else {
                    match c {
                        0..=4 => { g.invoke_library_sort_apply(c, fw, ff); g.set_library_sort_focused(false); }
                        5     => g.invoke_library_sort_apply(sort, !fw, ff),
                        _     => g.invoke_library_sort_apply(sort, fw, !ff),
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
                if c > 0 { g.set_library_scrubber_cursor(c - 1); }
                return true;
            }
            Action::Down => {
                let c = g.get_library_scrubber_cursor();
                if c < 26 { g.set_library_scrubber_cursor(c + 1); }
                return true;
            }
            Action::Confirm => {
                let c       = g.get_library_scrubber_cursor();
                let cols    = g.get_library_cols();
                let offsets = g.get_library_alpha_offsets();
                if let Some(flat_idx) = offsets.row_data(c as usize) {
                    if flat_idx >= 0 {
                        g.set_library_focused(flat_idx);
                        g.set_library_focused_row(flat_idx / cols);
                    }
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
            let f    = g.get_library_focused();
            let cols = g.get_library_cols();
            if f % cols > 0 {
                g.set_library_focused(f - 1);                   // within row — no scroll
            } else if f > 0 {
                let nf = f - 1;
                g.set_library_focused(nf);
                g.set_library_focused_row(nf / cols);           // wrap to prev row — scroll
            }
            true
        }
        Action::Right => {
            let f     = g.get_library_focused();
            let cols  = g.get_library_cols();
            let count = g.get_library_display().row_count() as i32;
            if f % cols < cols - 1 && f + 1 < count {
                g.set_library_focused(f + 1);                   // within row — no scroll
            } else if f + 1 < count {
                let nf = f + 1;
                g.set_library_focused(nf);
                g.set_library_focused_row(nf / cols);           // wrap to next row — scroll
            }
            true
        }
        Action::Up => {
            let f    = g.get_library_focused();
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
            let f    = g.get_library_focused();
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
                g.invoke_open_context_menu(card.id, card.has_played, card.is_favorite,
                    card.resume_pct, card.item_type, card.series_id);
            }
            true
        }
        Action::SearchJump => {
            g.set_library_header_focused(true);
            g.set_library_focused(0);
            g.set_library_focused_row(0);
            true
        }
        _ => false
    }
}

// ── Player dispatch ───────────────────────────────────────────────────────────

fn dispatch_player(action: Action, window: &crate::MainWindow) -> bool {
    let g     = crate::AppState::get(window);
    let panel = g.get_player_open_panel();

    // Ask-timed overlay: Left/Right toggle focus; Enter activates; Back/Esc dismisses
    if g.get_show_skip_timed() {
        match action {
            Action::Left | Action::Right | Action::SeekBackward | Action::SeekForward => {
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
                if c > 0 { g.set_player_panel_cursor(c - 1); }
                return true;
            }
            Action::Down | Action::VolumeDown => {
                let c   = g.get_player_panel_cursor();
                let max = match panel {
                    1 => g.get_sub_tracks().row_count() as i32,
                    2 => (g.get_audio_tracks().row_count() as i32 - 1).max(0),
                    3 => (g.get_video_tracks().row_count() as i32 - 1).max(0),
                    _ => (g.get_chapter_entries().row_count() as i32 - 1).max(0),
                };
                if c < max { g.set_player_panel_cursor(c + 1); }
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
        Action::PausePlay if g.get_seek_dragging() => { true }
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
        Action::SeekBackward     => { g.invoke_seek_acc(-(g.get_settings_seek_step_secs() as f32)); true }
        Action::SeekForward      => { g.invoke_seek_acc(  g.get_settings_seek_step_secs() as f32);  true }
        Action::SeekBackwardLong => { g.invoke_seek_acc(-(g.get_settings_seek_step_long_secs() as f32)); true }
        Action::SeekForwardLong  => { g.invoke_seek_acc(  g.get_settings_seek_step_long_secs() as f32);  true }
        Action::VolumeUp         => { g.invoke_volume_up(); true }
        Action::VolumeDown       => { g.invoke_volume_down(); true }
        Action::Mute             => { g.invoke_mute_toggle(); true }
        Action::ToggleStats      => { g.invoke_toggle_stats(); true }
        Action::Fullscreen       => { g.invoke_toggle_fullscreen(); true }
        Action::PanelSubtitles   => {
            g.set_player_open_panel(if panel == 1 { 0 } else { 1 });
            g.set_player_panel_cursor(0); true
        }
        Action::PanelAudio => {
            g.set_player_open_panel(if panel == 2 { 0 } else { 2 });
            g.set_player_panel_cursor(0); true
        }
        Action::PanelVideo => {
            g.set_player_open_panel(if panel == 3 { 0 } else { 3 });
            g.set_player_panel_cursor(0); true
        }
        Action::SeekToPercent(p) => { g.invoke_seek_to(p as f32 / 100.0); true }
        Action::NextChapter         => { g.invoke_chapter_next();      true }
        Action::PrevChapter         => { g.invoke_chapter_prev();      true }
        Action::SubDelayIncrease    => { g.invoke_sub_delay_inc();     true }
        Action::SubDelayDecrease    => { g.invoke_sub_delay_dec();     true }
        Action::AudioDelayIncrease  => { g.invoke_audio_delay_inc();   true }
        Action::AudioDelayDecrease  => { g.invoke_audio_delay_dec();   true }
        // Playlist prev/next fire in player mode too (e.g. audio queued into video player).
        Action::PrevTrack           => { g.invoke_queue_prev_track();  true }
        Action::NextTrack           => { g.invoke_queue_next_track();  true }
        Action::OpenQueuePanel => {
            if g.get_show_queue_panel() {
                g.set_show_queue_panel(false);
            } else {
                g.invoke_refresh_queue_display();
                let items = g.get_queue_items();
                for i in 0..items.row_count() {
                    if let Some(e) = items.row_data(i) {
                        if e.is_current { g.set_queue_panel_cursor(i as i32); break; }
                    }
                }
                g.set_show_queue_panel(true);
            }
            true
        }
        _ => false
    }
}

// ── Queue panel dispatch ──────────────────────────────────────────────────────

// Cursor -1 = Clear All button in the header; 0.. = list rows.
fn handle_key_queue_panel(action: &Action, g: &crate::AppState) -> bool {
    use slint::Model;
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
            let c   = g.get_queue_panel_cursor();
            let max = (g.get_queue_items().row_count() as i32 - 1).max(0);
            if c < max { g.set_queue_panel_cursor(c + 1); }
            true
        }
        Action::Confirm => {
            let c = g.get_queue_panel_cursor();
            if c < 0 {
                g.invoke_queue_clear(); // Clear All focused
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
            if c < 0 { return true; }
            if let Some(row) = g.get_queue_items().row_data(c as usize) {
                g.invoke_queue_remove(row.index);
            }
            true
        }
        _ => true // absorb all other keys while panel is open
    }
}

// Cursor split: !now-playing-in-strip = transport row (0=Album 1=Prev 2=Play/
// Pause 3=Next 4=Shuffle 5=Repeat); in-strip = index into queue-items. Global
// pre-dispatch already handles PrevTrack/NextTrack/ToggleShuffle/CycleRepeat/
// ToggleLyrics/Space-pause before this runs, so only navigation reaches here.
fn handle_key_now_playing(action: &Action, g: &crate::AppState) -> bool {
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
                if c > 0 { g.set_now_playing_strip_focused(c - 1); }
            } else {
                let c = g.get_now_playing_ctrl_focused();
                if c > 0 { g.set_now_playing_ctrl_focused(c - 1); }
            }
            true
        }
        Action::Right => {
            if g.get_now_playing_in_strip() {
                let c   = g.get_now_playing_strip_focused();
                let max = g.get_queue_items().row_count() as i32 - 1;
                if c < max { g.set_now_playing_strip_focused(c + 1); }
            } else {
                let c = g.get_now_playing_ctrl_focused();
                if c < 7 { g.set_now_playing_ctrl_focused(c + 1); }
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
                    0 => { g.invoke_music_bar_open_album(); g.set_show_now_playing(false); }
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

// ── Library search text input ─────────────────────────────────────────────────

fn handle_library_search(key: &str, ctrl: bool, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    if ctrl { return true; }
    match key {
        k if k == key::ESCAPE => {
            g.invoke_library_search_clear();
            g.set_library_header_focused(false);
            g.set_library_focused(0);
            g.set_library_focused_row(0);
            true
        }
        k if k == key::DOWN || k == key::RETURN => {
            g.set_library_header_focused(false);
            g.set_library_focused(0);
            g.set_library_focused_row(0);
            true
        }
        k if k == key::BACKSPACE => {
            if !g.get_library_query().is_empty() { g.invoke_library_search_backspace(); }
            true
        }
        k if k == key::UP => {
            g.set_library_header_focused(false);
            g.set_library_sort_focused(true);
            g.set_library_sort_cursor(sort_bar_init_cursor(&g));
            true
        }
        k if is_navigation_key(k) => true,
        k if is_printable(k) => { g.invoke_library_search_append(k.into()); true }
        _ => true
    }
}

// ── Browse search text input ──────────────────────────────────────────────────

fn handle_browse_search(key: &str, ctrl: bool, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    if ctrl { return true; }
    match key {
        k if k == key::ESCAPE => {
            g.invoke_browse_search_clear();
            g.set_browse_header_focused(false);
            true
        }
        k if k == key::DOWN || k == key::RETURN => {
            g.set_browse_header_focused(false);
            if g.get_media_items().row_count() > 0 { g.set_current_item(0); }
            true
        }
        k if k == key::BACKSPACE => {
            if !g.get_browse_query().is_empty() { g.invoke_browse_search_backspace(); }
            true
        }
        k if is_navigation_key(k) => true,
        k if is_printable(k) => { g.invoke_browse_search_append(k.into()); true }
        _ => true
    }
}

fn handle_discover_search(key: &str, ctrl: bool, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    if ctrl { return true; }
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
        // field and content in real visual layout order. Enter keeps its own
        // "jump straight to the top result" behavior — a different, well-
        // established search-field convention, not touched here.
        k if k == key::DOWN => {
            g.set_discover_header_focused(false);
            g.set_discover_filter_bar_active(true);
            true
        }
        k if k == key::RETURN => {
            if g.get_discover_results().row_count() > 0 {
                g.set_discover_header_focused(false);
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
            }
            true
        }
        k if k == key::BACKSPACE => {
            if !g.get_discover_query().is_empty() { g.invoke_discover_search_backspace(); }
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
        k if is_navigation_key(k) => true,
        k if is_printable(k) => { g.invoke_discover_search_append(k.into()); true }
        _ => true
    }
}

// ── Add-to-playlist picker (raw keys — naming mode needs text input) ──────────

fn handle_playlist_picker(key: &str, ctrl: bool, window: &crate::MainWindow) -> bool {
    let g = crate::AppState::get(window);
    if ctrl {
        if key == "q" || key == "Q" { g.invoke_quit(); }
        return true;
    }
    if g.get_playlist_picker_naming() {
        return match key {
            k if k == key::ESCAPE => { g.set_playlist_picker_naming(false); true }
            k if k == key::RETURN => { g.invoke_playlist_picker_create(); true }
            k if k == key::BACKSPACE => {
                let name = g.get_playlist_picker_name().to_string();
                if !name.is_empty() {
                    let mut cs: Vec<char> = name.chars().collect();
                    cs.pop();
                    g.set_playlist_picker_name(cs.into_iter().collect::<String>().into());
                }
                true
            }
            k if is_navigation_key(k) => true,
            k if is_printable(k) => {
                let mut name = g.get_playlist_picker_name().to_string();
                name.push_str(k);
                g.set_playlist_picker_name(name.into());
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
            if c > 0 { g.set_playlist_picker_cursor(c - 1); }
            true
        }
        k if k == key::DOWN => {
            let c = g.get_playlist_picker_cursor();
            if c < count { g.set_playlist_picker_cursor(c + 1); }
            true
        }
        k if k == key::RETURN => {
            let c = g.get_playlist_picker_cursor();
            if c == 0 {
                g.set_playlist_picker_name("".into());
                g.set_playlist_picker_naming(true);
            } else {
                g.invoke_playlist_picker_select(c - 1);
            }
            true
        }
        _ => true, // swallow everything else while the picker is open
    }
}

// ── Keybinding section navigation ────────────────────────────────────────────

fn dispatch_keybinding_nav(action: Action, g: &crate::AppState<'_>) -> bool {
    // Reset-to-defaults confirmation (2026-08-07) — ConfirmDialog itself is
    // keyboard-dumb (see its own doc comment in widgets.slint), so this
    // screen owns Left/Right/Confirm/Back for it, same shape as every other
    // ConfirmDialog/zone-based overlay in this app. Reachable whether the
    // dialog was opened by keyboard (Confirm on the Reset row below) or
    // mouse (settings.slint's FjordButton.clicked, which also sets
    // keybinding-focused to the Reset-button position before opening this)
    // — both converge on the same state, so this one gate handles both.
    if g.get_show_keybinding_reset_confirm() {
        let focused = g.get_keybinding_reset_confirm_focused();
        match action {
            Action::Left => {
                debug!("keybindings: reset-confirm focus -> Cancel");
                g.set_keybinding_reset_confirm_focused(0);
            }
            Action::Right => {
                debug!("keybindings: reset-confirm focus -> Confirm");
                g.set_keybinding_reset_confirm_focused(1);
            }
            Action::Confirm => {
                if focused == 1 {
                    debug!("keybindings: reset CONFIRMED — resetting to defaults");
                    g.invoke_keybinding_reset_defaults();
                } else {
                    debug!("keybindings: reset cancelled (Cancel focused)");
                }
                g.set_show_keybinding_reset_confirm(false);
            }
            Action::Back => {
                debug!("keybindings: reset cancelled (Back)");
                g.set_show_keybinding_reset_confirm(false);
            }
            _ => {}
        }
        return true;
    }

    // Rebind-collision confirmation (2026-08-08) — same keyboard-dumb-
    // ConfirmDialog shape as the reset dialog above. This function only
    // has AppState, not FjordState/the window, so the actual apply/discard
    // (which needs both, via keys::apply_rebind) lives in two AppState
    // callbacks registered in main.rs — invoked here for keyboard, and
    // directly from settings.slint's ConfirmDialog for mouse, so both
    // paths always go through the exact same Rust logic.
    if g.get_show_keybinding_collision_confirm() {
        let focused = g.get_keybinding_collision_confirm_focused();
        match action {
            Action::Left => {
                debug!("keybindings: collision-confirm focus -> Cancel");
                g.set_keybinding_collision_confirm_focused(0);
            }
            Action::Right => {
                debug!("keybindings: collision-confirm focus -> Confirm");
                g.set_keybinding_collision_confirm_focused(1);
            }
            Action::Confirm => {
                if focused == 1 {
                    g.invoke_keybinding_collision_confirmed();
                } else {
                    g.invoke_keybinding_collision_cancelled();
                }
            }
            Action::Back => {
                g.invoke_keybinding_collision_cancelled();
            }
            _ => {}
        }
        return true;
    }

    let fi    = g.get_keybinding_focused();
    let total = g.get_keybinding_normal().row_count() as i32
              + g.get_keybinding_player().row_count() as i32;
    debug!("keybindings: dispatch action={action:?} fi={fi} total={total}");

    match action {
        Action::Up => {
            if fi > 0 {
                debug!("keybindings: focused row {fi} -> {}", fi - 1);
                g.set_keybinding_focused(fi - 1);
            } else {
                // Return to Key Bindings section in left pane
                debug!("keybindings: exit to Key Bindings section (left pane)");
                g.set_keybinding_focused(-1);
                g.set_settings_section(crate::settings::SECTION_KEYBINDINGS.into());
                g.set_settings_focused("".into());
            }
            true
        }
        Action::Down => {
            if fi < total {
                debug!("keybindings: focused row {fi} -> {}", fi + 1);
                g.set_keybinding_focused(fi + 1);
            }
            true
        }
        Action::Back => {
            // Exit keybindings → back to Key Bindings section in left pane
            debug!("keybindings: back — exit to Key Bindings section (left pane)");
            g.set_keybinding_focused(-1);
            g.set_keybinding_rebinding(false);
            g.set_settings_section(crate::settings::SECTION_KEYBINDINGS.into());
            g.set_settings_focused("".into());
            true
        }
        Action::Confirm => {
            if fi < total {
                debug!("keybindings: start rebinding row {fi}");
                g.set_keybinding_rebinding(true);
            } else {
                // Reset button — open confirm dialog rather than resetting
                // immediately (live-tested feedback: no way to back out of
                // an accidental Confirm here before).
                debug!("keybindings: Reset button activated -> showing confirm dialog");
                g.set_keybinding_reset_confirm_focused(0);
                g.set_show_keybinding_reset_confirm(true);
            }
            true
        }
        _ => false
    }
}

// ── Global shortcuts ──────────────────────────────────────────────────────────
// Active from Dashboard and Settings; per-screen handlers (detail, series, player)
// intercept F/Q first where they need special handling.

fn handle_global_shortcuts(action: &Action, window: &crate::MainWindow) -> bool {
    match action {
        Action::Fullscreen  => { crate::AppState::get(window).invoke_toggle_fullscreen(); true }
        Action::Quit        => { crate::AppState::get(window).invoke_quit(); true }
        Action::NavHome     => { nav_to(window, 0);  true }
        Action::NavMovies   => { nav_to(window, 2);  true }  // Movies is now nav=2
        Action::NavTV       => { nav_to(window, 1);  true }  // TV Shows is now nav=1
        Action::NavSettings => { nav_to(window, 10); true }
        Action::OpenBrowse  => {
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
        _ => false
    }
}

// ── Dashboard dispatch ────────────────────────────────────────────────────────
// Handles: content grid nav and card item actions.
// Global shortcuts are pre-checked by the caller before this is reached.

fn dispatch_dashboard(action: &Action, repeat: bool, window: &crate::MainWindow) -> bool {
    if *action == Action::Back {
        let g = crate::AppState::get(window);
        if g.get_focused_section() >= 0 { g.set_focused_section(-1); return true; }
        return false;
    }

    if *action == Action::Up || *action == Action::Down {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if *action == Action::Down {
            if fs < 0 { sidebar_nav(&g, 1); return true; }
            let n = g.invoke_find_next_section(fs);
            if n != fs { g.set_focused_section(n); g.set_focused_card(0); return true; }
            return false; // at bottom of content — let focus_bar_on_down handle it
        }
        // Up
        if fs < 0 {
            sidebar_nav(&g, -1);
            return true;
        }
        let p = g.invoke_find_prev_section(fs);
        if p >= 0 { g.set_focused_section(p); g.set_focused_card(0); return true; }
        return false; // at top of content grid — let focus_bar_on_up handle it
    }

    if *action == Action::Left {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let fc = g.get_focused_card();
            if fc > 0 { g.set_focused_card(fc - 1); }
            else if !repeat { g.set_focused_section(-1); }
            return true;
        }
    }

    if *action == Action::Right {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs < 0 && g.get_active_nav() == 7 {
            // Real bug, live-reported 2026-08-14: the Profile sidebar row
            // (nav==7) has no content section at all — falling through to
            // the generic "enter content" branch below set focused_section
            // to whatever invoke_find_first_section() happened to return
            // for a nav value that was never meant to have one, leaving
            // keyboard nav stuck (Up/Down/Left/Right routed through the
            // content-navigation arms instead of sidebar ones) until Back
            // reset focused_section back to -1. Mouse already worked
            // because its own clicked handler calls open-sidebar-profile-
            // menu() directly (layout.slint) — mirror that here instead of
            // touching focused_section at all.
            g.invoke_open_sidebar_profile_menu();
        } else if fs < 0 && g.get_active_nav() < 10 {
            g.set_focused_section(g.invoke_find_first_section());
            g.set_focused_card(0);
        } else if fs >= 0 {
            let fc = g.get_focused_card();
            if fc < g.invoke_section_len(fs) - 1 { g.set_focused_card(fc + 1); }
        }
        return true;
    }

    if *action == Action::OpenDetail {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let card = g.invoke_section_card_item(fs, g.get_focused_card());
            g.invoke_open_detail(card.id, card.item_type);
            return true;
        }
    }

    if *action == Action::OpenContextMenu {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let card = g.invoke_section_card_item(fs, g.get_focused_card());
            g.set_context_menu_title(card.title.clone());
            g.invoke_open_context_menu(card.id, card.has_played, card.is_favorite,
                card.resume_pct, card.item_type, card.series_id);
            return true;
        }
    }

    if *action == Action::Confirm {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            g.invoke_item_play(g.invoke_section_card_id(fs, g.get_focused_card()));
            return true;
        }
        let nav = g.get_active_nav();
        if nav == 11 { g.invoke_quit(); return true; }
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
                if g.get_media_items().row_count() > 0 { g.set_current_item(0); }
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

// ── Helpers ───────────────────────────────────────────────────────────────────

// Returns the sort-bar cursor position that lands on the currently active sort pill.
// For Music (nav=4) view pills occupy cursor 0-1, so sort pills start at offset 2.
fn sort_bar_init_cursor(g: &crate::AppState) -> i32 {
    let sort = g.get_library_sort();
    if g.get_active_nav() == 4 { sort + 3 } else { sort }
}

fn nav_to(window: &crate::MainWindow, nav: i32) {
    let g = crate::AppState::get(window);
    g.set_show_browse(false);
    g.set_show_library(false);
    g.set_library_header_focused(false);
    g.set_library_scrubber_focused(false);
    g.set_focused_section(-1);
    g.set_settings_section("".into());
    g.set_settings_focused("".into());
    g.set_settings_dropdown_open(false);
    g.set_keybinding_focused(-1);
    g.set_active_nav(nav);
    g.invoke_nav_selected(nav);
}

fn sidebar_nav(g: &crate::AppState<'_>, dir: i32) {
    crate::browse::sidebar_nav(g, dir);
}

fn is_navigation_key(key: &str) -> bool {
    let Some(ch) = key.chars().next() else { return true; };
    (ch as u32) >= 0xE000 || ch.is_control()
}

fn is_printable(key: &str) -> bool {
    let Some(ch) = key.chars().next() else { return false; };
    if key.chars().count() != 1 { return false; }
    (ch as u32) < 0xE000 && !ch.is_control()
}
