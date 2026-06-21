// ── fjord-app · keys.rs ───────────────────────────────────────────────────────
//   Action             semantic action enum (~35 variants)
//   KeyCombo           key text (Slint event.text) + shift/ctrl/alt bools
//                      serialises/deserialises as a human-readable string ("ctrl+shift+f")
//   ActionMap          Normal or Player — which KeyMap an action lives in
//   Keybindings        normal + player KeyMaps; user JSON replaces defaults on load
//   AppMode            active UI mode — derived from AppState flags at call site
//   default_keybindings  hardcoded defaults; user keybindings.json replaces on load
//   remappable_actions   ordered list of (Action, label, ActionMap) for the settings UI
//   key_display_name   human-readable label for a Slint key string
//   action_key_labels  all KeyCombos for an Action joined into a display string
//   push_keybinding_rows  build + push keybinding model to AppState
//   handle_key         router: search bypasses → rebind capture → key lookup → per-screen dispatch
//   dispatch_player    ask-timed overlay: L/R focus, Enter activate, Back/Esc dismiss;
//                      ask overlay: Enter skip; Up Next banner: L/R/Enter; MinimizePlayer/Back: panel → minimize/stop
//   dispatch_library   keyboard nav for the library grid (private)
//   Settings dispatch → crate::settings (dispatch_settings, settings_row_action)
//   Per-screen key handlers live in their own modules:
//     context_menu::handle_key, series::handle_key, detail::handle_key, browse::handle_key
// ─────────────────────────────────────────────────────────────────────────────

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use slint::{Global, Model, ModelRc, SharedString, VecModel};
use serde::{Deserialize, Serialize};

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
    Quit,             // Q

    // ── Card / item actions ──────────────────────────────────────────────────
    OpenDetail,       // I — open detail or series screen
    OpenContextMenu,  // C — context menu on focused card / episode
    ResumePlayer,     // R — resume the background / mini-card player

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

impl KeyCombo {
    pub fn plain(key: impl Into<String>) -> Self {
        Self { key: key.into(), shift: false, ctrl: false, alt: false }
    }
    pub fn shifted(key: impl Into<String>) -> Self {
        Self { key: key.into(), shift: true, ctrl: false, alt: false }
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
        let shift = mods.contains(&"shift");
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
            k if k.chars().count() == 1 => k.to_string(),
            k => return Err(format!("unknown key: {k}")),
        };
        Ok(KeyCombo { key, shift, ctrl, alt })
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
    #[serde(default)]
    pub normal: KeyMap,
    #[serde(default)]
    pub player: KeyMap,
}

// ── ActionMap ─────────────────────────────────────────────────────────────────

/// Which KeyMap an action belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionMap { Normal, Player }

// ── AppMode ───────────────────────────────────────────────────────────────────

/// The active UI mode — derived from `AppState` flags at the Rust call site.
#[derive(Debug, Clone, PartialEq, Eq)]
#[allow(dead_code)]
pub enum AppMode {
    Login, ContextMenu, Series, SeriesSeasonRow, Detail,
    PlayerPanel, Player, LibrarySearch, Library,
    BrowseSearch, Browse, Settings, Dashboard,
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

    m.insert(KeyCombo::plain("f"),             Action::Fullscreen);
    m.insert(KeyCombo::plain("F"),             Action::Fullscreen);
    m.insert(KeyCombo::plain(key::F11),        Action::Fullscreen);
    m.insert(KeyCombo::plain("q"),             Action::Quit);
    m.insert(KeyCombo::plain("Q"),             Action::Quit);
    m.insert(KeyCombo::plain("b"),             Action::OpenBrowse);
    m.insert(KeyCombo::plain("B"),             Action::OpenBrowse);
    m.insert(KeyCombo::plain("1"),             Action::NavHome);
    m.insert(KeyCombo::plain("2"),             Action::NavMovies);
    m.insert(KeyCombo::plain("3"),             Action::NavTV);
    m.insert(KeyCombo::plain("s"),             Action::NavSettings);
    m.insert(KeyCombo::plain("S"),             Action::NavSettings);

    m.insert(KeyCombo::plain("i"),             Action::OpenDetail);
    m.insert(KeyCombo::plain("I"),             Action::OpenDetail);
    m.insert(KeyCombo::plain("c"),             Action::OpenContextMenu);
    m.insert(KeyCombo::plain("C"),             Action::OpenContextMenu);
    m.insert(KeyCombo::plain("r"),             Action::ResumePlayer);
    m.insert(KeyCombo::plain("R"),             Action::ResumePlayer);

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
    m.insert(KeyCombo::plain("K"),             Action::PausePlay);
    m.insert(KeyCombo::plain("p"),             Action::PausePlay);
    m.insert(KeyCombo::plain("P"),             Action::PausePlay);
    m.insert(KeyCombo::plain("m"),             Action::Mute);
    m.insert(KeyCombo::plain("M"),             Action::Mute);

    m.insert(KeyCombo::plain("i"),             Action::ToggleStats);
    m.insert(KeyCombo::plain("I"),             Action::ToggleStats);
    m.insert(KeyCombo::plain("s"),             Action::PanelSubtitles);
    m.insert(KeyCombo::plain("S"),             Action::PanelSubtitles);
    m.insert(KeyCombo::plain("a"),             Action::PanelAudio);
    m.insert(KeyCombo::plain("A"),             Action::PanelAudio);
    m.insert(KeyCombo::plain("v"),             Action::PanelVideo);
    m.insert(KeyCombo::plain("V"),             Action::PanelVideo);

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
        // Player map
        (Action::PausePlay,        "Pause / Play",      Player),
        (Action::SeekBackward,     "Seek Back 10s",     Player),
        (Action::SeekForward,      "Seek Fwd 10s",      Player),
        (Action::SeekBackwardLong, "Seek Back 30s",     Player),
        (Action::SeekForwardLong,  "Seek Fwd 30s",      Player),
        (Action::VolumeUp,         "Volume Up",         Player),
        (Action::VolumeDown,       "Volume Down",       Player),
        (Action::Mute,             "Mute",              Player),
        (Action::ToggleStats,      "Toggle Stats",      Player),
        (Action::PanelSubtitles,   "Subtitles Panel",   Player),
        (Action::PanelAudio,       "Audio Panel",       Player),
        (Action::PanelVideo,       "Video Panel",       Player),
        (Action::MinimizePlayer,   "Minimize Player",   Player),
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

fn rebind_action(
    fi:     i32,
    key:    &str,
    shift:  bool,
    ctrl:   bool,
    state:  &Arc<Mutex<FjordState>>,
    window: &crate::MainWindow,
) {
    let actions = remappable_actions();
    if fi < 0 || fi as usize >= actions.len() { return; }

    let new_combo = KeyCombo { key: key.to_string(), shift, ctrl, alt: false };
    let (action, _, map) = &actions[fi as usize];

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

// ── Dispatch ──────────────────────────────────────────────────────────────────

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

    if key.is_empty() || g.get_show_login() { return false; }

    // Search field text-input modes bypass the KeyMap
    if g.get_show_library() && g.get_library_header_focused() {
        return handle_library_search(key, ctrl, window);
    }
    if g.get_show_browse() && g.get_browse_header_focused() {
        return handle_browse_search(key, ctrl, window);
    }

    // Keybinding rebind capture
    if g.get_keybinding_rebinding() {
        if key == key::ESCAPE {
            g.set_keybinding_rebinding(false);
        } else {
            let fi = g.get_keybinding_focused();
            drop(g);
            rebind_action(fi, key, shift, ctrl, state, window);
        }
        return true;
    }

    // Key → Action lookup
    let combo     = KeyCombo { key: key.to_string(), shift, ctrl, alt: false };
    let in_player = g.get_is_playing();
    let action: Option<Action> = {
        let s = state.lock().unwrap();
        if in_player {
            s.keybindings.player.get(&combo)
                .or_else(|| s.keybindings.normal.get(&combo))
                .cloned()
        } else {
            s.keybindings.normal.get(&combo).cloned()
        }
    };
    drop(g);

    // ── Per-screen dispatchers ────────────────────────────────────────────────
    // Each block returns early when its screen is active, so `action` is not
    // consumed unless we actually enter that branch.

    {
        let g = crate::AppState::get(window);
        if g.get_show_context_menu() {
            let Some(action) = action else { return true; }; // swallow unknown keys
            return crate::context_menu::handle_key(&action, &g);
        }
    }

    {
        let g = crate::AppState::get(window);
        if g.get_show_series() {
            let Some(action) = action else { return false; };
            return crate::series::handle_key(&action, &g);
        }
    }

    // Guard matches main.slint: `show-detail && !is-playing`. During playback
    // from the detail page show_detail stays true (hidden by !is-playing), so we
    // must skip this handler while the player is active.
    {
        let g = crate::AppState::get(window);
        if g.get_show_detail() && !g.get_is_playing() {
            let Some(action) = action else { return false; };
            return crate::detail::handle_key(&action, &g);
        }
    }

    {
        let g = crate::AppState::get(window);
        if g.get_is_playing() {
            let Some(action) = action else { return false; };
            // I (ToggleStats) must not reveal the controls bar
            if action != Action::ToggleStats { g.invoke_show_controls(); }
            return dispatch_player(action, window);
        }
    }

    {
        let g = crate::AppState::get(window);
        if g.get_has_background_player() && !g.get_show_browse() {
            if action == Some(Action::ResumePlayer) {
                g.invoke_resume_player(); return true;
            }
        }
    }

    {
        let g = crate::AppState::get(window);
        if g.get_show_library() {
            let Some(action) = action else { return false; };
            return dispatch_library(&action, &g);
        }
    }

    {
        let g = crate::AppState::get(window);
        if g.get_show_browse() {
            let Some(action) = action else { return false; };
            return crate::browse::handle_key(&action, &g);
        }
    }

    // ── Now Playing mini card (active-nav == 4) ───────────────────────────────
    {
        let g = crate::AppState::get(window);
        if g.get_active_nav() == 4 && g.get_focused_section() < 0 {
            if !g.get_has_background_player() {
                g.set_active_nav(0);
                return false;
            }
            match &action {
                Some(Action::Left) | Some(Action::Right) => {
                    g.set_mini_card_focused(1 - g.get_mini_card_focused());
                    return true;
                }
                Some(Action::Confirm) => {
                    if g.get_mini_card_focused() == 0 { g.invoke_resume_player(); }
                    else { g.invoke_stop_playback(); }
                    return true;
                }
                _ => {} // Up/Down/Back fall through to sidebar_nav
            }
        }
    }

    // ── Global shortcuts and dashboard ────────────────────────────────────────
    let Some(action) = action else { return false; };

    match &action {
        Action::Fullscreen => { crate::AppState::get(window).invoke_toggle_fullscreen(); return true; }
        Action::Quit       => { crate::AppState::get(window).invoke_quit(); return true; }
        Action::OpenBrowse => {
            let g = crate::AppState::get(window);
            if g.get_active_nav() < 10 {
                g.set_show_library(false);
                g.set_settings_section(-1);
                g.set_settings_focused(-1);
                g.set_show_browse(true);
                g.invoke_browse_search_clear();
            }
            return true;
        }
        Action::NavHome     => { nav_to(window, 0);  return true; }
        Action::NavMovies   => { nav_to(window, 1);  return true; }
        Action::NavTV       => { nav_to(window, 2);  return true; }
        Action::NavSettings => { nav_to(window, 10); return true; }
        _ => {}
    }

    {
        let g = crate::AppState::get(window);
        if g.get_active_nav() == 10 && !g.get_show_browse() && !g.get_show_library()
           && g.get_keybinding_focused() >= 0
        {
            return dispatch_keybinding_nav(action, &g);
        }
    }

    {
        let g = crate::AppState::get(window);
        if g.get_active_nav() == 10 && !g.get_show_browse() && !g.get_show_library() {
            if let Some(handled) = crate::settings::dispatch_settings(&action, &g) {
                return handled;
            }
        }
    }

    if action == Action::Back {
        let g = crate::AppState::get(window);
        if g.get_focused_section() >= 0 { g.set_focused_section(-1); return true; }
        return false;
    }

    if action == Action::Up || action == Action::Down {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs < 0 {
            sidebar_nav(&g, if action == Action::Up { -1 } else { 1 });
        } else if action == Action::Up {
            let p = g.invoke_find_prev_section(fs);
            if p >= 0 { g.set_focused_section(p); g.set_focused_card(0); }
        } else {
            let n = g.invoke_find_next_section(fs);
            if n != fs { g.set_focused_section(n); g.set_focused_card(0); }
        }
        return true;
    }

    if action == Action::Left {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let fc = g.get_focused_card();
            if fc > 0 { g.set_focused_card(fc - 1); }
            else if !repeat { g.set_focused_section(-1); }
            return true;
        }
    }

    if action == Action::Right {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs < 0 && g.get_active_nav() < 10 {
            g.set_focused_section(g.invoke_find_first_section());
            g.set_focused_card(0);
        } else if fs >= 0 {
            let fc = g.get_focused_card();
            if fc < g.invoke_section_len(fs) - 1 { g.set_focused_card(fc + 1); }
        }
        return true;
    }

    if action == Action::OpenDetail {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let card = g.invoke_section_card_item(fs, g.get_focused_card());
            g.invoke_open_detail(card.id, card.item_type);
            return true;
        }
    }

    if action == Action::OpenContextMenu {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            let card = g.invoke_section_card_item(fs, g.get_focused_card());
            g.invoke_open_context_menu(card.id, card.has_played, card.is_favorite,
                card.resume_pct, card.item_type, card.series_id);
            return true;
        }
    }

    if action == Action::Confirm {
        let g  = crate::AppState::get(window);
        let fs = g.get_focused_section();
        if fs >= 0 {
            g.invoke_item_play(g.invoke_section_card_id(fs, g.get_focused_card()));
            return true;
        }
        let nav = g.get_active_nav();
        if nav == 11 { g.invoke_quit(); return true; }
        if nav < 10 {
            if nav == 3 {
                if g.get_media_items().row_count() > 0 { g.set_current_item(0); }
            } else if nav == 1 || nav == 2 {
                g.set_show_library(true);
                g.set_library_focused(0);
                g.invoke_library_search_clear();
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

// ── Library grid dispatch ─────────────────────────────────────────────────────

fn dispatch_library(action: &Action, g: &crate::AppState) -> bool {
    match action {
        Action::Back => {
            g.set_show_library(false);
            g.set_library_header_focused(false);
            g.invoke_library_search_clear();
            true
        }
        Action::Left => {
            let f = g.get_library_focused();
            if f > 0 { g.set_library_focused(f - 1); }
            true
        }
        Action::Right => {
            let f = g.get_library_focused();
            if f < g.get_library_display().row_count() as i32 - 1 {
                g.set_library_focused(f + 1);
            }
            true
        }
        Action::Up => {
            let f    = g.get_library_focused();
            let cols = g.get_library_cols();
            if f >= cols { g.set_library_focused(f - cols); }
            else { g.set_library_header_focused(true); }
            true
        }
        Action::Down => {
            let f    = g.get_library_focused();
            let cols = g.get_library_cols();
            if f + cols < g.get_library_display().row_count() as i32 {
                g.set_library_focused(f + cols);
            }
            true
        }
        Action::Confirm => {
            let f = g.get_library_focused();
            if f < g.get_library_display().row_count() as i32 {
                let card = g.get_library_display().row_data(f as usize).unwrap();
                g.invoke_open_detail(card.id, card.item_type);
            }
            true
        }
        Action::OpenContextMenu => {
            let f = g.get_library_focused();
            if f < g.get_library_display().row_count() as i32 {
                let card = g.get_library_display().row_data(f as usize).unwrap();
                g.invoke_open_context_menu(card.id, card.has_played, card.is_favorite,
                    card.resume_pct, card.item_type, card.series_id);
            }
            true
        }
        Action::SearchJump => {
            g.set_library_header_focused(true);
            g.set_library_focused(0);
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
            Action::Left | Action::Right => {
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
            Action::Left | Action::Right => {
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
                    _ => (g.get_video_tracks().row_count() as i32 - 1).max(0),
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
        // back to playing while the seek bar still shows the frozen drag position, which is
        // confusing. The drag will resume/stay-paused correctly when seek-committed fires.
        Action::PausePlay if g.get_seek_dragging() => { true }
        Action::PausePlay        => { g.invoke_pause_play_toggle(); true }
        Action::SeekBackward     => { g.invoke_seek_backward(); true }
        Action::SeekForward      => { g.invoke_seek_forward(); true }
        Action::SeekBackwardLong => { g.invoke_seek_backward_long(); true }
        Action::SeekForwardLong  => { g.invoke_seek_forward_long(); true }
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
        _ => false
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
            true
        }
        k if k == key::DOWN || k == key::RETURN => {
            g.set_library_header_focused(false);
            g.set_library_focused(0);
            true
        }
        k if k == key::BACKSPACE => {
            if !g.get_library_query().is_empty() { g.invoke_library_search_backspace(); }
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

// ── Keybinding section navigation ────────────────────────────────────────────

fn dispatch_keybinding_nav(action: Action, g: &crate::AppState<'_>) -> bool {
    let fi    = g.get_keybinding_focused();
    let total = g.get_keybinding_normal().row_count() as i32
              + g.get_keybinding_player().row_count() as i32;

    match action {
        Action::Up => {
            if fi > 0 {
                g.set_keybinding_focused(fi - 1);
            } else {
                // Return to Key Bindings section in left pane
                g.set_keybinding_focused(-1);
                g.set_settings_section(crate::settings::SECTION_KEYBINDINGS);
                g.set_settings_focused(-1);
            }
            true
        }
        Action::Down => {
            if fi < total { g.set_keybinding_focused(fi + 1); }
            true
        }
        Action::Back => {
            // Exit keybindings → back to Key Bindings section in left pane
            g.set_keybinding_focused(-1);
            g.set_keybinding_rebinding(false);
            g.set_settings_section(crate::settings::SECTION_KEYBINDINGS);
            g.set_settings_focused(-1);
            true
        }
        Action::Confirm => {
            if fi < total {
                g.set_keybinding_rebinding(true);
            } else {
                // Reset button
                g.invoke_keybinding_reset_defaults();
            }
            true
        }
        _ => false
    }
}

// ── Helpers ───────────────────────────────────────────────────────────────────

fn nav_to(window: &crate::MainWindow, nav: i32) {
    let g = crate::AppState::get(window);
    g.set_show_browse(false);
    g.set_show_library(false);
    g.set_library_header_focused(false);
    g.set_focused_section(-1);
    g.set_settings_section(-1);
    g.set_settings_focused(-1);
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
