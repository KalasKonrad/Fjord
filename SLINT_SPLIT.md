# Slint UI Split Plan — `fjord-app/ui/main.slint`

`main.slint` is currently 3273 lines. The goal is to split it into focused
files so adding a new screen or changing the layout means touching one file,
not navigating a 3000-line monolith.

---

## Guiding rules

- **No behaviour change.** Every step is a pure move/restructure.
- **Global over property-threading.** A `global AppState` lets any component
  read/write shared state without long `in-out property` parameter lists.
  Adding a new screen = create one file + two lines in `app_state.slint`.
- **One step at a time.** After each step `cargo build` must succeed before
  continuing. The Slint compiler runs in `build.rs`, so Slint errors are
  caught by the normal build.
- **Pure functions and helpers live where they belong.** Width-dependent layout
  calculations (`grid-cols`, `dash-card-w/h`) stay in MainWindow because they
  reference `root.width`; MainWindow writes them into AppState once.

---

## Architecture: global AppState

`global AppState` in `app_state.slint` holds all shared state:

```
app_state.slint          global AppState — all properties, all callbacks, all
                         pure nav helpers (section-len, section-card-id, etc.)

main.slint               MainWindow shell:
                           • window sizing / title / background
                           • `grab-keyboard-focus` callback (references `fs` by name)
                           • width-dependent pure functions (grid-cols, dash-card-w/h)
                           • `changed width` handler that writes layout props to AppState
                           • FocusScope `fs` + keyboard handler (reads/writes AppState)
                           • `if` conditionals that instantiate the screen components
```

Every Slint file that needs shared state imports AppState:
```slint
import { AppState } from "app_state.slint";
import { Theme }    from "theme.slint";
```

---

## Target file map

```
ui/
├── theme.slint          Theme global + all struct types (already exists, unchanged)
├── app_state.slint      global AppState — all properties, callbacks, nav helpers
├── widgets.slint        FjordButton, NavItem, BrowseItem, MediaCard,
│                        LoadingSpinner, StatRow
├── home.slint           SectionRow, LibraryGrid, HomeScreen, DashboardScreen
├── browse.slint         BrowseScreen
├── settings.slint       SettingsScreen
├── login.slint          LoginScreen  (extracted from MainWindow)
├── player.slint         PlayerScreen (extracted from MainWindow)
├── series.slint         SeriesScreen (extracted from MainWindow)
├── detail.slint         DetailPage   (extracted from MainWindow)
└── main.slint           MainWindow shell + FocusScope + keyboard handler
```

Import graph (→ = imports):
```
theme.slint           (no imports)
app_state.slint       → theme.slint
widgets.slint         → theme.slint
home.slint            → theme.slint, app_state.slint, widgets.slint
browse.slint          → theme.slint, app_state.slint, widgets.slint
settings.slint        → theme.slint, app_state.slint, widgets.slint
login.slint           → theme.slint, app_state.slint, widgets.slint
player.slint          → theme.slint, app_state.slint, widgets.slint
series.slint          → theme.slint, app_state.slint, widgets.slint
detail.slint          → theme.slint, app_state.slint, widgets.slint
main.slint            → all of the above
```

---

## global AppState — full property / callback inventory

### Screen routing flags
```slint
in-out property <bool> show-login:            true;
in-out property <bool> show-browse:           false;
in-out property <bool> show-library:          false;
in-out property <bool> show-series:           false;
in-out property <bool> show-detail:           false;
in-out property <bool> is-playing:            false;
in-out property <bool> has-background-player: false;
in-out property <bool> video-behind-ui:       false;
```

### Navigation state
```slint
in-out property <int>    active-nav:        0;
in-out property <int>    focused-section:  -1;
in-out property <int>    focused-card:      0;
in-out property <string> server-url:        "";
property        <int>    settings-focused: -1;  // private — only keyboard handler writes it
```

### Library grid state
```slint
in-out property <bool>       library-searching:      false;
in-out property <bool>       library-header-focused: false;
in-out property <int>        library-focused:        0;
in-out property <[CardItem]> library-display:        [];
in-out property <string>     library-query:          "";
in-out property <int>        library-cols:           6;     // written by MainWindow on resize
in-out property <length>     dash-cw:                140px; // written by MainWindow on resize
in-out property <length>     dash-ch:                224px; // written by MainWindow on resize
```

### Browse list state
```slint
in     property <string>                  status:       "";
in     property <[StandardListViewItem]>  media-items:  [];
in-out property <int>                     current-item: -1;
```

### Home / dashboard data rows
```slint
in property <[CardItem]> continue-watching:          [];
in property <[CardItem]> next-up:                    [];
in property <[CardItem]> recently-added:             [];
in property <[CardItem]> continue-watching-movies:   [];
in property <[CardItem]> recently-added-movies:      [];
in property <[CardItem]> not-watched-movies:         [];
in property <[CardItem]> continue-watching-tv:       [];
in property <[CardItem]> recently-added-tv:          [];
in property <[CardItem]> not-watched-tv:             [];
in property <[CardItem]> all-movies:                 [];
in property <[CardItem]> all-series:                 [];
```

### Auto-advance banner
```slint
in-out property <bool>   show-next-ep-banner: false;
in-out property <string> next-ep-title:       "";
in-out property <int>    next-ep-secs:         5;
```

### Intro skip prompt
```slint
in-out property <bool> show-skip-intro: false;
```

### Player state
```slint
in     property <image>   video-frame;
in-out property <bool>    is-paused:            false;
in-out property <string>  playing-title:        "";
in-out property <bool>    stats-visible:        false;
in-out property <float>   playback-pos:         0;
in-out property <string>  playback-time:        "0:00";
in-out property <string>  playback-total:       "0:00";
in-out property <bool>    controls-visible:     true;
in-out property <int>     player-open-panel:    0;
in-out property <int>     player-panel-cursor:  0;
in-out property <[TrackEntry]> sub-tracks;
in-out property <[TrackEntry]> audio-tracks;
in-out property <[TrackEntry]> video-tracks;
in-out property <int>     current-sub-id:   0;
in-out property <int>     current-audio-id: 1;
in-out property <int>     current-video-id: 1;

in property <string> stat-vid-in:  "—";
in property <string> stat-vid-out: "—";
in property <string> stat-color:   "—";
in property <string> stat-hwdec:   "—";
in property <string> stat-aud-in:  "—";
in property <string> stat-aud-out: "—";
in property <string> stat-display: "—";
in property <string> stat-vsync:   "—";
in property <string> stat-avsync:  "—";
in property <string> stat-drop:    "—";
in property <string> stat-bitrate: "—";
in property <string> stat-cache:   "—";
```

### Detail page state
```slint
in-out property <string>        detail-id:           "";
in-out property <bool>          detail-loading:      false;
in-out property <string>        detail-title:        "";
in-out property <string>        detail-series-label: "";
in-out property <string>        detail-meta:         "";
in-out property <string>        detail-genres:       "";
in-out property <string>        detail-overview:     "";
in-out property <string>        detail-rating-label: "";
in-out property <image>         detail-poster;
in-out property <bool>          detail-has-poster:   false;
in-out property <image>         detail-backdrop;
in-out property <bool>          detail-has-backdrop: false;
in-out property <bool>          detail-can-resume:   false;
in-out property <string>        detail-resume-label: "";
in-out property <[CastMember]>  detail-cast;
in-out property <length>        detail-scroll:       0px;
```

### Series drill-down state
```slint
in-out property <string>          series-id:            "";
in-out property <bool>            series-loading:       true;
in-out property <string>          series-title:         "";
in-out property <string>          series-overview:      "";
in-out property <image>           series-poster;
in-out property <bool>            series-has-poster:    false;
in-out property <image>           series-backdrop;
in-out property <bool>            series-has-backdrop:  false;
in-out property <[SeasonEntry]>   series-seasons;
in-out property <[EpisodeEntry]>  series-episodes;
in-out property <int>             series-season-idx:    0;
in-out property <int>             series-focused-ep:    0;
in-out property <bool>            series-in-season-row: false;
```

### Settings state
```slint
in-out property <bool>   settings-launch-fullscreen:   false;
in-out property <bool>   settings-audio-spdif:         false;
in-out property <string> settings-hwdec:               "auto";
in-out property <string> settings-hwdec-image-format:  "";
in-out property <string> settings-vf:                  "";
in-out property <string> settings-vo:                   "gpu-next";
in-out property <string> settings-gpu-api:              "auto";
in-out property <string> settings-video-sync:           "audio";
in-out property <bool>   settings-opengl-early-flush:   false;
in-out property <bool>   settings-video-latency-hacks:  false;
in-out property <bool>   settings-interpolation:        false;
in-out property <string> settings-tscale:               "oversample";
in-out property <string> settings-tone-mapping:         "auto";
in-out property <bool>   settings-target-colorspace-hint: false;
in-out property <bool>   settings-deinterlace:          false;
in-out property <int>    settings-cache-mb:             0;
in-out property <bool>   settings-video-behind:         false;
```

### Callbacks (wired from Rust)
```slint
// Auth / nav
callback do-login(string, string, string);
callback sign-out;
callback nav-selected(int);
callback toggle-fullscreen;
callback quit;

// Browse
callback play-item(int);              // browse list play (index into media-items)
callback filter-changed(string);      // browse search text changed
callback library-search-append(string);
callback library-search-backspace();
callback library-search-clear();

// Item play / detail / series
callback item-play(string);           // play by item id (from dashboard cards)
callback open-detail(string);
callback play-detail;
callback resume-detail;
callback close-detail;
callback open-series(string);
callback series-select-season(int);
callback play-series-episode(string);
callback close-series;

// Player controls
callback pause-play-toggle;
callback seek-backward;
callback seek-forward;
callback seek-backward-long;
callback seek-forward-long;
callback stop-playback;
callback seek-to(float);
callback select-sub(int);
callback select-audio(int);
callback select-video(int);
callback commit-panel-selection;
callback mute-toggle;
callback volume-up;
callback volume-down;
callback show-controls;
callback toggle-stats;
callback minimize-player;
callback resume-player;

// Misc overlays
callback cancel-auto-advance;
callback skip-intro;
callback settings-changed;
```

### Pure nav helpers (moved from MainWindow)
```slint
// section-len, find-first-section, find-next-section, find-prev-section,
// section-card-id, max-sections
// (reference self.active-nav, self.continue-watching, etc.)
```

---

## MainWindow after the split

```slint
export component MainWindow inherits Window {
    title: "Fjord";  background: Theme.bg;
    min-width: 900px;  min-height: 520px;
    preferred-width: 1280px;  preferred-height: 720px;

    // Only things that cannot live in AppState:
    callback grab-keyboard-focus;
    grab-keyboard-focus => { fs.focus(); }

    // Width-dependent helpers — reference root.width, cannot be in AppState
    pure function grid-cols()  -> int    { ... }
    pure function dash-card-w() -> length { ... }
    pure function dash-card-h() -> length { ... }

    // Write layout values to AppState on every resize
    init => { self.sync-layout(); }
    changed width => { self.sync-layout(); }
    function sync-layout() {
        AppState.library-cols = self.grid-cols();
        AppState.dash-cw      = self.dash-card-w();
        AppState.dash-ch      = self.dash-card-h();
    }

    // Auto-advance banner (overlay — sits above all content, lives in MainWindow)
    if AppState.show-next-ep-banner: AutoAdvanceBanner { }

    // FocusScope — the global key handler
    fs := FocusScope {
        width: 0; height: 0;
        key-pressed(event) => {
            // unchanged logic, but all `root.X` references become `AppState.X`
        }
    }

    // Video-behind-UI background layer
    if AppState.video-behind-ui && !AppState.is-playing: Rectangle { ... }

    // Screen routing — thin instantiations only
    LoginScreen  { visible: AppState.show-login; }
    if !AppState.show-login: AppShell { }   // sidebar + content
    PlayerScreen { visible: AppState.is-playing; }
    SeriesScreen { visible: AppState.show-series; }
    DetailPage   { visible: AppState.show-detail; }
}
```

`AppShell` can be an inline component or a named component in `main.slint` that
holds the sidebar + content area (the `if !show-login: HorizontalLayout { ... }` block).

---

## Component responsibilities after the split

### `login.slint` — `LoginScreen`
- No `in` properties — reads `AppState.show-login`, `AppState.status`
- Invokes `AppState.do-login(...)` directly

### `player.slint` — `PlayerScreen`
- No `in` properties — reads everything from AppState (`video-frame`, `is-paused`, etc.)
- Controls bar, seek bar, stats overlay, track panels all inline
- All `root.X` callbacks become `AppState.X()`

### `series.slint` — `SeriesScreen`
- Reads `AppState.series-*` properties
- Invokes `AppState.series-select-season`, `AppState.play-series-episode`,
  `AppState.close-series`

### `detail.slint` — `DetailPage`
- Reads `AppState.detail-*` properties
- Invokes `AppState.play-detail`, `AppState.resume-detail`, `AppState.close-detail`

### `home.slint` — `SectionRow`, `LibraryGrid`, `HomeScreen`, `DashboardScreen`
- `HomeScreen` and `DashboardScreen` read card rows and `AppState.focused-section`,
  `AppState.focused-card`, `AppState.dash-cw/ch` directly; no `in` properties for
  nav state
- `LibraryGrid` reads `AppState.library-display`, `AppState.library-focused`, etc.
- `SectionRow` and `MediaCard` keep their existing `in` properties (purely presentational)

### `browse.slint` — `BrowseScreen`
- Reads `AppState.media-items`, `AppState.current-item`, `AppState.status`
- Invokes `AppState.filter-changed`, `AppState.play-item` directly

### `settings.slint` — `SettingsScreen`
- Reads/writes `AppState.settings-*` via `<=>` or direct write
- Invokes `AppState.settings-changed`, `AppState.sign-out` directly; no callbacks
  passed from MainWindow

### `widgets.slint` — `FjordButton`, `NavItem`, `BrowseItem`, `MediaCard`,
  `LoadingSpinner`, `StatRow`
- Pure presentational components — `in` properties + `callback` only, no AppState import

---

## Impact on Rust modules

Every `window.set_X()` / `window.get_X()` / `window.on_X()` / `window.invoke_X()`
call that corresponds to a property or callback now in AppState changes to the
`AppState::get(&window)` accessor:

```rust
// Before
window.set_is_playing(true);
window.on_item_play(move |id| { ... });

// After
AppState::get(&window).set_is_playing(true);
AppState::get(&window).on_item_play(move |id| { ... });
```

The `crate::AppState` type is generated by `slint::include_modules!()` in `main.rs`
and is available in all modules via `use crate::AppState;`.

`window.invoke_grab_keyboard_focus()` stays unchanged (still on MainWindow).
Fullscreen calls (`window.window().set_fullscreen(...)`) stay unchanged (use
`ComponentHandle::window()` from the Slint API, unrelated to AppState).

Estimated changes by module:

| Module      | Approximate call sites to update |
|-------------|----------------------------------|
| main.rs     | ~30 (startup apply + wiring)     |
| auth.rs     | ~10                              |
| home.rs     | ~15                              |
| playback.rs | ~30                              |
| stats.rs    | ~12                              |
| series.rs   | ~10                              |
| detail.rs   | ~10                              |
| browse.rs   | ~6                               |
| controls.rs | ~0 (uses VideoState, not window) |
| movies.rs   | ~5                               |
| poster.rs   | ~0                               |

---

## Step-by-step execution order

Do one step at a time. `cargo build` must succeed before the next step.

### Step 1 — Create `app_state.slint`
Create `ui/app_state.slint` with `export global AppState { ... }` containing all
properties and callbacks from the inventory above. Add the import to `main.slint`
but do NOT yet change any `root.X` references in the keyboard handler. Build.

### Step 2 — Create `widgets.slint`
Move `FjordButton`, `NavItem`, `BrowseItem`, `MediaCard`, `LoadingSpinner`, `StatRow`
(lines 5–1111 minus the screen components) into `widgets.slint`. Add
`import { FjordButton, NavItem, ... } from "widgets.slint";` to `main.slint`.
Remove the moved definitions from `main.slint`. Build.

### Step 3 — Create `home.slint`
Move `SectionRow`, `LibraryGrid`, `HomeScreen`, `DashboardScreen` into `home.slint`.
Convert `HomeScreen` and `DashboardScreen` to read nav state directly from AppState
(remove `kbd-section`, `kbd-card`, `card-w`, `card-h` `in` properties; read
`AppState.focused-section`, `AppState.dash-cw` etc. directly).
Keep `SectionRow.item-play` callback and `MediaCard` in properties unchanged
(purely presentational). Update `main.slint` import and instantiations. Build.

### Step 4 — Create `browse.slint`
Move `BrowseScreen` into `browse.slint`. Remove its `in` properties for
`status`, `media-items`, `current-item`; read from AppState directly. Remove the
`filter-changed`, `play-item`, `back` callbacks; invoke AppState directly.
Update `main.slint`. Build.

### Step 5 — Create `settings.slint`
Move `SettingsScreen` into `settings.slint`. Remove all `in-out` property
bindings — read/write AppState directly. Remove `sign-out` and `settings-changed`
callbacks; invoke AppState. Update `main.slint`. Build.

### Step 6 — Extract `login.slint`
Extract the `if show-login: Rectangle { ... }` block (lines 2099–2169) into
`export component LoginScreen`. No in properties — reads AppState directly.
Add import and replace the inline block with `LoginScreen { }`. Build.

### Step 7 — Extract `player.slint`
Extract the `if is-playing: Rectangle { ... }` block (lines 2459–2863) into
`export component PlayerScreen`. All state comes from AppState; no in properties.
Add import and replace the inline block with `PlayerScreen { }`. Build.

### Step 8 — Extract `series.slint`
Extract the `if show-series: Rectangle { ... }` block (lines 2865–3127) into
`export component SeriesScreen`. No in properties. Build.

### Step 9 — Extract `detail.slint`
Extract the `if show-detail: Rectangle { ... }` block (lines 3129–3271) into
`export component DetailPage`. No in properties. Build.

### Step 10 — Switch keyboard handler to use AppState
In `main.slint`, change every `root.X` reference inside `fs.key-pressed` to
`AppState.X`. Move the pure nav helper functions (`section-len`, `find-first-section`,
etc.) from MainWindow into `app_state.slint` — they become `pure function` members
of `global AppState`, referencing `self.active-nav`, `self.continue-watching`, etc.
Add `sync-layout()` to MainWindow. Build.

### Step 11 — Remove now-redundant MainWindow properties
Delete all the `in property`, `in-out property`, and `callback` declarations from
MainWindow that now live in AppState. This step will break the Rust build.
Do not run `cargo build` yet — proceed directly to step 12.

### Step 12 — Update all Rust modules
Systematically replace every `window.set_X()` / `window.get_X()` / `window.on_X()`
that maps to AppState with `AppState::get(&window).set_X()` etc. Add
`use crate::AppState;` to each module that needs it. Then `cargo build --fix`
won't help here — do it module by module: main.rs, auth.rs, home.rs,
playback.rs, stats.rs, series.rs, detail.rs, browse.rs, movies.rs.
Keep iterating until `cargo build` is clean.

### Step 13 — `cargo build --release`
Release build must be clean.

### Step 14 — Smoke-test
Run the app. Log in, browse all tabs, open detail, play series, test player
controls, check stats overlay. The app must behave identically to before.

### Step 15 — Commit and push

---

## What NOT to do

- Do not put `pure function grid-cols()` in AppState — it references `root.width`.
  Keep it in MainWindow and push the result to `AppState.library-cols` on resize.
- Do not remove callbacks from AppState just to thread them through components as
  `callback` properties — that reintroduces the threading problem we're solving.
- Do not split `theme.slint` — structs and the Theme global all live there cleanly.
- Do not add AppState import to `widgets.slint` — widgets are purely presentational
  and should remain reusable without coupling to app state.

---

## Line count targets (approximate)

| File              | Lines (estimate) |
|-------------------|-----------------|
| app_state.slint   | ~130            |
| widgets.slint     | ~430            |
| home.slint        | ~330            |
| browse.slint      | ~100            |
| settings.slint    | ~360            |
| login.slint       | ~75             |
| player.slint      | ~415            |
| series.slint      | ~270            |
| detail.slint      | ~150            |
| main.slint        | ~180            |
| **Total**         | **~2440**       |

(Down from 3273 + 81 = 3354 because the property-threading boilerplate disappears.)
