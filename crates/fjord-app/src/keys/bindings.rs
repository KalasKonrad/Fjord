// ── fjord-app · keys/bindings.rs ─────────────────────────────────────────────
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
//   default_keybindings  hardcoded defaults; user keybindings.json replaces on load
//   remappable_actions   ordered list of (Action, label, ActionMap) for the settings UI
//   key_display_name   human-readable label for a Slint key string
//   action_key_labels  all KeyCombos for an Action joined into a display string
//   push_keybinding_rows  build + push keybinding model to AppState
//   dispatch_keybinding_nav  Settings → Keybindings section navigation
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

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
    Confirm,    // Return — confirm / play / activate
    Back,       // Escape / Backspace — go back / close
    Up,         // UpArrow
    Down,       // DownArrow
    Left,       // LeftArrow
    Right,      // RightArrow
    SearchJump, // / — focus the search field

    // ── Player-only ──────────────────────────────────────────────────────────
    MinimizePlayer, // Backspace (player) — close panel or minimize; Escape stops instead

    // ── Global tab / screen shortcuts ────────────────────────────────────────
    NavHome,     // 1
    NavMovies,   // 2
    NavTV,       // 3
    NavSettings, // S (when not in player)
    OpenBrowse,  // B
    Fullscreen,  // F / F11
    Quit,        // Ctrl+Q (plain q/Q opens the queue panel)

    // ── Card / item actions ──────────────────────────────────────────────────
    OpenDetail,      // I — open detail or series screen
    OpenContextMenu, // C — context menu on focused card / episode
    ResumePlayer,    // R — resume the background player
    FocusFloatCard,  // N — focus the mini-player bar from any screen

    // ── Player controls (active in player map) ───────────────────────────────
    PausePlay,          // Space / K / P
    SeekBackward,       // Left  (player)
    SeekForward,        // Right (player)
    SeekBackwardLong,   // Shift+Left
    SeekForwardLong,    // Shift+Right
    VolumeUp,           // Up    (player)
    VolumeDown,         // Down  (player)
    Mute,               // M
    ToggleStats,        // I (player — shadows OpenDetail)
    PanelSubtitles,     // S (player — shadows NavSettings)
    PanelAudio,         // A
    PanelVideo,         // V
    SeekToPercent(u8),  // 0–9 → seek to 0%, 10%, …, 90% (player only)
    NextChapter,        // .
    PrevChapter,        // ,
    SubDelayIncrease,   // z  (+100 ms, matching mpv default)
    SubDelayDecrease,   // Z  (−100 ms, matching mpv default)
    AudioDelayIncrease, // x (+100 ms)
    AudioDelayDecrease, // X (−100 ms)

    // ── Playlist controls ────────────────────────────────────────────────────
    PrevTrack,        // [ — prev track or restart current (music bar / player)
    NextTrack,        // ] — next track (music bar / player)
    ToggleShuffle,    // remappable — flip shuffle on/off
    CycleRepeat,      // remappable — cycle Off → All → One → Off
    OpenQueuePanel, // q — open/close queue panel (audio playing, queue non-empty, or video player)
    DeleteItem,     // Delete — remove focused item from playlist in queue panel
    ToggleLyrics,   // L — show/hide lyrics overlay (only when lyrics-available)
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
    pub key: String,
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

/// A captured rebind that collided with another action's existing binding,
/// stashed in `FjordState` while the user is shown a confirm/cancel dialog
/// (see `rebind_action`/`dispatch_keybinding_nav`'s own doc comments).
#[derive(Debug, Clone)]
pub(crate) struct PendingKeybindRebind {
    pub fi: i32,
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
        Self {
            key: key.into().to_lowercase(),
            shift,
            ctrl,
            alt,
        }
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
        if self.ctrl {
            write!(f, "ctrl+")?;
        }
        if self.alt {
            write!(f, "alt+")?;
        }
        if self.shift {
            write!(f, "shift+")?;
        }
        let name = match self.key.as_str() {
            k if k == key::BACKSPACE => "Backspace",
            k if k == key::RETURN => "Return",
            k if k == key::ESCAPE => "Escape",
            k if k == key::UP => "Up",
            k if k == key::DOWN => "Down",
            k if k == key::LEFT => "Left",
            k if k == key::RIGHT => "Right",
            k if k == key::F11 => "F11",
            " " => "Space",
            k => k,
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
        let ctrl = mods.contains(&"ctrl");
        let alt = mods.contains(&"alt");
        let key = match key_name {
            "Backspace" => key::BACKSPACE.to_string(),
            "Return" | "Enter" => key::RETURN.to_string(),
            "Escape" | "Esc" => key::ESCAPE.to_string(),
            "Up" => key::UP.to_string(),
            "Down" => key::DOWN.to_string(),
            "Left" => key::LEFT.to_string(),
            "Right" => key::RIGHT.to_string(),
            "F11" => key::F11.to_string(),
            "Space" => " ".to_string(),
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
                if !shift
                    && !ctrl
                    && !alt
                    && let Some(ch) = k.chars().next()
                    && ch.is_uppercase()
                {
                    shift = true;
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
                let combo = KeyCombo::try_from(raw_key.clone()).map_err(|e| {
                    serde::de::Error::custom(format!("invalid key combo {raw_key:?}: {e}"))
                })?;
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
pub enum ActionMap {
    Normal,
    Player,
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

    m.insert(KeyCombo::plain(key::ESCAPE), Action::Back);
    m.insert(KeyCombo::plain(key::BACKSPACE), Action::Back);
    m.insert(KeyCombo::plain(key::RETURN), Action::Confirm);
    m.insert(KeyCombo::plain(key::UP), Action::Up);
    m.insert(KeyCombo::plain(key::DOWN), Action::Down);
    m.insert(KeyCombo::plain(key::LEFT), Action::Left);
    m.insert(KeyCombo::plain(key::RIGHT), Action::Right);
    m.insert(KeyCombo::plain("/"), Action::SearchJump);

    // Single, shift-insensitive entry per letter (2026-08-08) — used to be
    // two ("f" and "F") to cover Shift/Caps Lock, which also meant the Key
    // Bindings screen showed both as separate labels for the same action.
    // lookup_action's own shift-and-retry-unshifted fallback now makes the
    // second entry unnecessary: pressing the key with Shift held (or with
    // Caps Lock on, which KeyCombo::new's lower-casing makes indistinguishable
    // from not holding Shift at all) still resolves to this one entry.
    m.insert(KeyCombo::plain("f"), Action::Fullscreen);
    m.insert(KeyCombo::plain(key::F11), Action::Fullscreen);
    // Ctrl+Q quits. Plain q belongs to OpenQueuePanel (Phase 51) — before
    // CR10-4, plain-q Quit entries here were silently overwritten by the
    // queue-panel inserts below, leaving Quit with no binding at all.
    m.insert(KeyCombo::new("q", false, true, false), Action::Quit);
    m.insert(KeyCombo::plain("b"), Action::OpenBrowse);
    m.insert(KeyCombo::plain("1"), Action::NavHome);
    m.insert(KeyCombo::plain("2"), Action::NavMovies);
    m.insert(KeyCombo::plain("3"), Action::NavTV);
    m.insert(KeyCombo::plain("s"), Action::NavSettings);

    m.insert(KeyCombo::plain("i"), Action::OpenDetail);
    m.insert(KeyCombo::plain("c"), Action::OpenContextMenu);
    m.insert(KeyCombo::plain("r"), Action::ResumePlayer);
    m.insert(KeyCombo::plain("n"), Action::FocusFloatCard);

    m.insert(KeyCombo::plain("["), Action::PrevTrack);
    m.insert(KeyCombo::plain("]"), Action::NextTrack);
    m.insert(KeyCombo::plain("q"), Action::OpenQueuePanel);
    m.insert(KeyCombo::plain("\u{007f}"), Action::DeleteItem); // Delete key
    m.insert(KeyCombo::plain("l"), Action::ToggleLyrics);
    m.insert(KeyCombo::plain("m"), Action::ToggleNowPlaying);

    m
}

fn default_player_map() -> KeyMap {
    let mut m = KeyMap::new();

    m.insert(KeyCombo::plain(key::BACKSPACE), Action::MinimizePlayer);

    m.insert(KeyCombo::plain(key::LEFT), Action::SeekBackward);
    m.insert(KeyCombo::plain(key::RIGHT), Action::SeekForward);
    m.insert(KeyCombo::shifted(key::LEFT), Action::SeekBackwardLong);
    m.insert(KeyCombo::shifted(key::RIGHT), Action::SeekForwardLong);
    m.insert(KeyCombo::plain(key::UP), Action::VolumeUp);
    m.insert(KeyCombo::plain(key::DOWN), Action::VolumeDown);

    m.insert(KeyCombo::plain(" "), Action::PausePlay);
    m.insert(KeyCombo::plain("k"), Action::PausePlay);
    m.insert(KeyCombo::plain("p"), Action::PausePlay);
    m.insert(KeyCombo::plain("m"), Action::Mute);

    m.insert(KeyCombo::plain("i"), Action::ToggleStats);
    m.insert(KeyCombo::plain("s"), Action::PanelSubtitles);
    m.insert(KeyCombo::plain("a"), Action::PanelAudio);
    m.insert(KeyCombo::plain("v"), Action::PanelVideo);

    m.insert(KeyCombo::plain("."), Action::NextChapter);
    m.insert(KeyCombo::plain(","), Action::PrevChapter);

    // Genuinely shift-SENSITIVE, unlike every plain letter above (matches
    // mpv's own convention: z/x increase, Shift+z/Shift+x decrease) — used
    // to be registered as two unshifted entries ("z" and "Z", relying on
    // "Z" only ever being reachable by literally typing a capital Z) rather
    // than an explicit shift:true combo, which happened to work before this
    // file's own Caps-Lock/case-normalization fix but was never really
    // correct: Caps Lock alone (no Shift held) would have produced the same
    // "Z" text and wrongly fired Decrease instead of Increase.
    // KeyCombo::shifted expresses the real intent directly.
    m.insert(KeyCombo::plain("z"), Action::SubDelayIncrease);
    m.insert(KeyCombo::shifted("z"), Action::SubDelayDecrease);
    m.insert(KeyCombo::plain("x"), Action::AudioDelayIncrease);
    m.insert(KeyCombo::shifted("x"), Action::AudioDelayDecrease);

    m.insert(KeyCombo::plain("["), Action::PrevTrack);
    m.insert(KeyCombo::plain("]"), Action::NextTrack);
    m.insert(KeyCombo::plain("q"), Action::OpenQueuePanel);
    m.insert(KeyCombo::plain("l"), Action::ToggleLyrics);

    m.insert(KeyCombo::plain("0"), Action::SeekToPercent(0));
    m.insert(KeyCombo::plain("1"), Action::SeekToPercent(10));
    m.insert(KeyCombo::plain("2"), Action::SeekToPercent(20));
    m.insert(KeyCombo::plain("3"), Action::SeekToPercent(30));
    m.insert(KeyCombo::plain("4"), Action::SeekToPercent(40));
    m.insert(KeyCombo::plain("5"), Action::SeekToPercent(50));
    m.insert(KeyCombo::plain("6"), Action::SeekToPercent(60));
    m.insert(KeyCombo::plain("7"), Action::SeekToPercent(70));
    m.insert(KeyCombo::plain("8"), Action::SeekToPercent(80));
    m.insert(KeyCombo::plain("9"), Action::SeekToPercent(90));

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
        (Action::Confirm, "Confirm", Normal),
        (Action::Back, "Back", Normal),
        (Action::Up, "Up", Normal),
        (Action::Down, "Down", Normal),
        (Action::Left, "Left", Normal),
        (Action::Right, "Right", Normal),
        (Action::SearchJump, "Jump to Search", Normal),
        // Normal map — global shortcuts
        (Action::NavHome, "Nav: Home", Normal),
        (Action::NavMovies, "Nav: Movies", Normal),
        (Action::NavTV, "Nav: TV", Normal),
        (Action::NavSettings, "Nav: Settings", Normal),
        (Action::OpenBrowse, "Open Browse", Normal),
        (Action::Fullscreen, "Toggle Fullscreen", Normal),
        (Action::Quit, "Quit", Normal),
        // Normal map — item actions
        (Action::OpenDetail, "Open Detail", Normal),
        (Action::OpenContextMenu, "Context Menu", Normal),
        (Action::ResumePlayer, "Resume Player", Normal),
        (Action::FocusFloatCard, "Focus Mini Player", Normal),
        // Player map
        (Action::PausePlay, "Pause / Play", Player),
        (Action::SeekBackward, "Seek Back", Player),
        (Action::SeekForward, "Seek Fwd", Player),
        (Action::SeekBackwardLong, "Seek Back (Long)", Player),
        (Action::SeekForwardLong, "Seek Fwd (Long)", Player),
        (Action::VolumeUp, "Volume Up", Player),
        (Action::VolumeDown, "Volume Down", Player),
        (Action::Mute, "Mute", Player),
        (Action::ToggleStats, "Toggle Stats", Player),
        (Action::PanelSubtitles, "Subtitles Panel", Player),
        (Action::PanelAudio, "Audio Panel", Player),
        (Action::PanelVideo, "Video Panel", Player),
        (Action::MinimizePlayer, "Minimize Player", Player),
        (Action::NextChapter, "Next Chapter", Player),
        (Action::PrevChapter, "Prev Chapter", Player),
        (Action::SubDelayIncrease, "Sub Delay +100ms", Player),
        (Action::SubDelayDecrease, "Sub Delay −100ms", Player),
        (Action::AudioDelayIncrease, "Audio Delay +100ms", Player),
        (Action::AudioDelayDecrease, "Audio Delay −100ms", Player),
        // Playlist controls (normal map — active when music is playing)
        (Action::PrevTrack, "Prev Track", Normal),
        (Action::NextTrack, "Next Track", Normal),
        (Action::ToggleShuffle, "Toggle Shuffle", Normal),
        (Action::CycleRepeat, "Cycle Repeat", Normal),
    ]
}

// ── Key display helpers ───────────────────────────────────────────────────────

/// Human-readable label for a Slint key string (PUA codepoints → symbol names).
pub fn key_display_name(key: &str) -> String {
    match key {
        k if k == key::BACKSPACE => "Bksp".into(),
        k if k == key::RETURN => "Enter".into(),
        k if k == key::ESCAPE => "Esc".into(),
        k if k == key::UP => "↑".into(),
        k if k == key::DOWN => "↓".into(),
        k if k == key::LEFT => "←".into(),
        k if k == key::RIGHT => "→".into(),
        k if k == key::F11 => "F11".into(),
        " " => "Space".into(),
        k => k.into(),
    }
}

fn format_combo(combo: &KeyCombo) -> String {
    let key_name = key_display_name(&combo.key);
    let mut mods: Vec<&str> = vec![];
    if combo.ctrl {
        mods.push("Ctrl");
    }
    if combo.alt {
        mods.push("Alt");
    }
    if combo.shift {
        mods.push("Shift");
    }
    if mods.is_empty() {
        key_name
    } else {
        format!("{}+{}", mods.join("+"), key_name)
    }
}

/// All KeyCombos in `map` that resolve to `action`, formatted and joined with "  ".
/// Returns "—" if the action has no binding.
pub fn action_key_labels(action: &Action, map: &KeyMap) -> String {
    let mut labels: Vec<String> = map
        .iter()
        .filter(|(_, v)| *v == action)
        .map(|(k, _)| format_combo(k))
        .collect();
    if labels.is_empty() {
        return "—".into();
    }
    labels.sort();
    labels.dedup();
    labels.join("  ")
}

// ── Keybinding row model ──────────────────────────────────────────────────────

fn build_keybinding_entries(
    kb: &Keybindings,
) -> (Vec<crate::KeyBindingEntry>, Vec<crate::KeyBindingEntry>) {
    let mut normal_rows = vec![];
    let mut player_rows = vec![];

    for (action, label, map) in remappable_actions() {
        let the_map = match map {
            ActionMap::Normal => &kb.normal,
            ActionMap::Player => &kb.player,
        };
        let key_str = action_key_labels(&action, the_map);
        let entry = crate::KeyBindingEntry {
            action: SharedString::from(label),
            key: SharedString::from(key_str.as_str()),
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
    fi: i32,
    new_combo: KeyCombo,
    state: &Arc<Mutex<FjordState>>,
    window: &crate::MainWindow,
) {
    let actions = remappable_actions();
    let Some((action, _, map)) = actions.get(fi as usize) else {
        debug!(
            "keybindings: apply_rebind fi={fi} out of range ({} actions), ignoring",
            actions.len()
        );
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
pub(crate) fn rebind_action(
    fi: i32,
    key: &str,
    shift: bool,
    ctrl: bool,
    state: &Arc<Mutex<FjordState>>,
    window: &crate::MainWindow,
) {
    let actions = remappable_actions();
    let g = crate::AppState::get(window);
    // Either applied directly below, or handed off to the collision dialog
    // — either way, capture mode itself is over the moment a key lands.
    g.set_keybinding_rebinding(false);

    if fi < 0 || fi as usize >= actions.len() {
        debug!(
            "keybindings: rebind_action fi={fi} out of range ({} actions), ignoring",
            actions.len()
        );
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
        existing_map
            .get(&new_combo)
            .filter(|other| *other != action)
            .map(|other_action| {
                actions
                    .iter()
                    .find(|(a, _, _)| a == other_action)
                    .map(|(_, label, _)| label.to_string())
                    .unwrap_or_else(|| format!("{other_action:?}"))
            })
    };

    if let Some(other_label) = collision {
        let message = format!("{new_combo} is already bound to {other_label} — reassign it?");
        debug!("keybindings: collision — {message}");
        state.lock().unwrap().pending_keybind_rebind = Some(PendingKeybindRebind {
            fi,
            combo: new_combo,
        });
        g.set_keybinding_collision_message(message.into());
        g.set_keybinding_collision_confirm_focused(0);
        g.set_show_keybinding_collision_confirm(true);
        return;
    }

    apply_rebind(fi, new_combo, state, window);
}

// ── Keybinding section navigation ────────────────────────────────────────────

pub(crate) fn dispatch_keybinding_nav(action: Action, g: &crate::AppState<'_>) -> bool {
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

    let fi = g.get_keybinding_focused();
    let total =
        g.get_keybinding_normal().row_count() as i32 + g.get_keybinding_player().row_count() as i32;
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
        _ => false,
    }
}
