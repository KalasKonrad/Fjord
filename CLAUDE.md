# Fjord — Claude Code Context

Fjord is a Jellyfin media frontend built in Rust with Slint as the GUI toolkit and libmpv for video playback. It is built by KalasKonrad as a personal project, partly as a learning exercise in Rust and partly to solve a real problem: every existing Flutter-based Jellyfin frontend (Fladder, Jellyflix) uses media_kit which embeds mpv into a Flutter texture. That path never calls `mpv_render_context_report_swap()`, so mpv has no vsync feedback and playback is choppy on NVIDIA legacy drivers on Wayland. Fjord fixes this by using the mpv render API so mpv renders into an OpenGL FBO that Slint composites, with `report_swap()` called after every frame.

## Project structure

```
Fjord/
├── Cargo.toml                  workspace root
├── PLAN.md                     development roadmap
├── JELLYFIN.md                 Jellyfin API reference (endpoints, params, WebSocket events, caveats)
├── SLINT.md                    Slint best practices and gotchas for Fjord
├── README.md                   public-facing project description
├── crates/
│   ├── fjord-api/              Jellyfin REST API client
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── auth.rs         authenticate() — POST /Users/AuthenticateByName
│   │       ├── client.rs       JellyfinClient struct, all API calls
│   │       └── models/         serde types for Jellyfin responses
│   │           ├── mod.rs      re-exports all model types
│   │           ├── auth.rs     AuthResponse, UserDto
│   │           ├── intro.rs    Segment, EpisodeTimestamps (Intro Skipper plugin)
│   │           └── media.rs    MediaItem, UserData, StudioInfo, ItemsResponse
│   ├── fjord-player/           libmpv wrapper
│   │   └── src/
│   │       ├── lib.rs
│   │       └── mpv.rs          Player struct, MpvRenderCtx, FBO rendering
│   └── fjord-app/              Slint UI + main binary
│       ├── build.rs            compiles .slint files
│       ├── src/
│       │   ├── main.rs         entry point: apply saved config, wire modules, window.run()
│       │   ├── config.rs       Config (persisted JSON, all settings+auth), FjordState (holds config: Config + runtime state + movie_collections map), path helpers, load/save
│       │   ├── home.rs         HomeData, fetch_home_data, push_home_data, home cache, fetch_movie_collections
│       │   ├── poster.rs       fetch_poster_cached, decode_poster_buffer, spawn_poster_loading
│       │   ├── movies.rs       spawn_movies_poster_loading, movie library grid logic
│       │   ├── series.rs       EpisodeRaw, open_series_screen, spawn_episode_thumb_loading
│       │   ├── detail.rs       open_detail, detail page fetch + metadata + cast photos + collection row + similar row
│       │   ├── playback.rs     VideoState, fmt_secs, fmt_ends_at, build_track_model, GL FBO helpers
│       │   ├── stats.rs        update_stats_window, all stats formatting helpers; sets audio-passthrough-active
│       │   ├── browse.rs       update_library_filter, populate_browse_async (off-thread), browse list + library search callback wiring
│       │   ├── auth.rs         do_login, initial library fetch after authentication
│       │   ├── controls.rs     wire_controls: all player control callback registrations
│       │   ├── context_menu.rs wire_context_menu, update_card_in_all_models
│       │   ├── keys.rs         Action enum, KeyCombo, Keybindings, AppMode, active_mode(),
│       │   │                   handle_key (main dispatcher), handle_global_shortcuts,
│       │   │                   dispatch_player, dispatch_library, dispatch_dashboard
│       │   ├── settings.rs     dispatch_settings, apply_dropdown_selection; section/row index constants
│       │   └── pipewire_fix.rs is_pipewire_device, apply_alsa_irq_scheduling (WirePlumber config)
│       └── ui/
│           ├── main.slint      MainWindow: keyboard handler, sync-layout, export { AppState }
│           ├── app_state.slint global AppState singleton — all shared UI state + callbacks
│           ├── theme.slint     color palette, spacing tokens, HomeItem / CardItem structs
│           ├── layout.slint    AppShell: sidebar (random logo, nav items) + content area
│           ├── home.slint      HomeDashboard, DashboardScreen, LibraryGrid components
│           ├── detail.slint    DetailPage component (BackdropHero, PosterBlock, MetaLine, CastRow atoms; tagline, director, writer, studio; SectionRow for "More Like This")
│           ├── series.slint    SeriesScreen component
│           ├── player.slint    PlayerOverlay component
│           ├── settings.slint  SettingsPage two-pane layout (section list + rows)
│           ├── browse.slint    BrowseScreen component
│           ├── login.slint     LoginScreen component
│           ├── context_menu.slint ContextMenu overlay component
│           └── widgets.slint   FjordButton, NavItem, BrowseItem, MediaCard, LoadingSpinner, ToggleSwitch,
│                               SectionHeader, SettingsDropdown, SettingsRow, StatRow,
│                               BackdropHero, PosterBlock, MetaLine (shared detail-page atoms)
```

### `fjord-app/src/` module responsibilities

Each module owns one concern. `main.rs` wires all modules together: rendering
notifier, mpv event-poll timer, not-watched refresh timer, applying saved config
on startup, and `AppState::get(&window).on_*()` callback registrations.
`slint::include_modules!()` must stay in `main.rs` — it generates `MainWindow`
and `AppState` (the Slint global) as `crate::MainWindow` and `crate::AppState`.
Every module that accesses the global imports `use slint::Global;` and uses
`AppState::get(&window).set_X()` / `.get_X()` / `.on_X()`.

| Module | Owns |
|---|---|
| `config.rs` | `Config` (persisted JSON: auth + all settings, including `sub_enabled`, `sub_lang`, `sub_lang2`), `FjordState` (runtime app state: `config: Config` is the canonical settings copy + auth; client, library vecs, keybindings, `movie_collections: HashMap<movie_id,(boxset_id,boxset_name)>`, `series_episode_cache: HashMap<season_id, Vec<MediaItem>>` (in-memory cache, cleared on series switch), `series_season_generation: u64` (stale-fetch guard for rapid season tab navigation)), XDG path helpers, `load/save_config`, `ensure_device_id`, `load/save_keybindings`. Adding a setting: add to `Config` only — `FjordState.config` is the single copy, saved directly in `on_settings_changed`. |
| `home.rs` | `HomeData`, home/movies/series cache, `fetch_home_data`, `push_home_data`, `home_data_sections`, `load/save_movies_cache`, `load/save_series_cache`, `fetch_movie_collections` (background BoxSet membership map) |
| `poster.rs` | `fetch_poster_cached`, `fetch_backdrop_cached`, `decode_poster_buffer`, `spawn_poster_loading`, `spawn_series_poster_loading` |
| `movies.rs` | `spawn_movies_poster_loading`, future movie-specific logic |
| `series.rs` | `EpisodeRaw`, `make_episode_raw`, `raw_to_entry`, `spawn_episode_thumb_loading`; `SeriesCtx` (shared context for background fetch task) + `spawn_main` (fetches seasons, first-season episodes, poster, backdrop in parallel); `open_series_screen` (resets AppState, builds `SeriesCtx`, calls `spawn_main`); `handle_key` (series screen keyboard dispatch) |
| `detail.rs` | Detail page: fetch item, build cast with portraits, load backdrop/poster, format metadata (director, writer, tagline, studio); collection `SectionRow` (BoxSet siblings via `movie_collections` map); "More Like This" `SectionRow`; populates `detail-series-id` for Episodes |
| `playback.rs` | `VideoState`, `start_playback`, `fmt_secs`, `fmt_ends_at` (local wall-clock "ends at"), `build_track_model` (title→lang→codec label order; `external_filename` fallback for external subs), GL FBO helpers, `wire_rendering_notifier`, `wire_mpv_timer`, `reset_playback_ui` (shared stop/natural-end UI reset) |
| `stats.rs` | `update_stats_window` and all stats string formatting; also sets `audio-passthrough-active` (checked every 500 ms via `audio-out-params/format`) |
| `browse.rs` | `update_library_filter`, `populate_browse_async` (snapshots data on UI thread, filters off-thread via Tokio, pushes back via `invoke_from_event_loop`; `AtomicU64` gen counter drops stale results), browse list + library search callback wiring |
| `auth.rs` | Login flow: authenticate, persist config, fetch initial library + home data |
| `controls.rs` | `wire_controls`: registers all player control `AppState::get(window).on_*()` callbacks |
| `context_menu.rs` | `wire_context_menu`: open-context-menu / browse / series-ep callbacks, context-mark-played, context-toggle-fav, context-play-from-start; `update_card_in_all_models` patches played/fav across every Slint model |
| `keys.rs` | `Action` enum (~35 semantic actions), `KeyCombo`, `Keybindings` (normal + player maps); `AppMode` (8 variants: ContextMenu/Series/Detail/Player/Library/Browse/Settings/Dashboard); `active_mode()` derives `AppMode` from `AppState` flags — encodes screen priority in one place; `handle_key()` main dispatcher: calls `active_mode()`, pre-match `ResumePlayer` check, then `match mode` routes to per-module handlers; `handle_global_shortcuts` (F/Q/B/1/2/3/S shared by Dashboard+Settings); `dispatch_player`, `dispatch_library`, `dispatch_dashboard`; `default_keybindings`, `remappable_actions`, `key_display_name`, `action_key_labels` |
| `settings.rs` | `dispatch_settings`, `apply_dropdown_selection`; section constants (`SECTION_GENERAL/VIDEO/AUDIO/PLAYER_CFG/KEYBINDINGS`) and per-section row index constants (`GEN_*`, `VID_*`, `AUD_*`, `PLY_*`) |
| `pipewire_fix.rs` | `is_pipewire_device` (true for `""` / `pipewire` / `pipewire/*`), `apply_alsa_irq_scheduling` (writes/deletes `~/.config/wireplumber/wireplumber.conf.d/fjord-alsa-irq.conf` and restarts WirePlumber) |
| `home.rs` (timer) | `wire_nw_timer`: 30 s not-watched refresh poll |

## Key design decisions

### mpv render API (not X11 embedding)
mpv uses `vo=libmpv` and `mpv_render_context`. It never opens its own window. Each frame:
1. Slint's `BeforeRendering` notifier fires on the GL thread
2. mpv renders into the back FBO (`mpv_render_context_render`)
3. The FBO texture is exposed to Slint as a `BorrowedOpenGLTexture`
4. Slint's `AfterRendering` notifier calls `report_swap()` for vsync feedback

**Double-buffer FBO:** Two GL texture/FBO pairs alternate each frame. Single-buffer caused Slint to skip re-renders because the texture ID was unchanged (Slint's change detection). Alternating IDs force a re-render every frame.

**Drop ordering:** `MpvRenderCtx` must be dropped before `Player`. This is enforced in `VideoState` and in the rendering teardown path.

### Three playback modes
1. **Fullscreen player** (`is-playing = true`): covers the full window, shows controls bar + inline stats overlay.
2. **Video behind UI** (`has-background-player + video-behind-ui = true`): video fills the window at 88% opacity, UI overlays it.
3. **Mini card** (`has-background-player + video-behind-ui = false`): sidebar shows a live thumbnail card with a Resume button.

The "Video in background" setting (persisted) controls whether Back during playback enters mode 2 or mode 3.

### Dashboards and library grid

There are three dashboard screens (horizontal `SectionRow` card rows) and one library grid:

- **Home dashboard** (`HomeDashboard`, `active-nav == 0`, 4 rows): Continue Watching, Next Up, Recently Added Shows (`Series`), Recently Added Movies. Shows both movies and series.
- **Movies dashboard** (`DashboardScreen`, `active-nav == 1`, 3 rows): Continue Watching Movies, Recently Added Movies, Not Watched Movies.
- **Series dashboard** (`DashboardScreen`, `active-nav == 2`, 4 rows): Continue Watching TV, Next Up, Recently Added Shows (`Series`), Not Watched Shows (`Series`).
- **Library grid** (`LibraryGrid`, `show-library == true`): full poster grid of every item in a category. Opened by pressing Enter from the Movies or TV sidebar tab. Separate concept from the dashboards — do not call this a "dashboard."

Episode cards in dashboard rows display the series poster (`series_id` used as the fetch key), not the episode thumbnail. `spawn_poster_loading` carries a `poster_id` field alongside `item_id` in its metadata tuple for exactly this reason.

Not Watched rows use `SortBy=Random` so each fetch returns a different selection. A 30-second polling timer (`timer_nw`) refreshes the Not Watched row when the relevant tab is visible, no playback is active, and 10 minutes have elapsed since the last refresh. Timestamps `last_nw_mov_refresh` / `last_nw_tv_refresh` in `AppState` track this independently per tab.

Poster images are cached to `~/.cache/fjord/posters/` and decoded off the UI thread — JPEG decode runs on a Tokio worker producing `SharedPixelBuffer<Rgba8Pixel>` (which is `Send`), then `Image::from_rgba8` is called inside `invoke_from_event_loop` because `slint::Image` is `!Send`.

`HomeItem` (defined in `theme.slint`) carries `has-played: bool`, `resume-pct: float`, `unplayed-count: int`, and `is-favorite: bool` — populated from `UserData`. `MediaCard` renders:
- **✓ badge** (top-right, accent circle, bold) when `has-played`
- **progress bar** (bottom) when `resume-pct > 0 && !has-played`
- **unplayed-count pill** (top-right, accent circle, bold) when `unplayed-count > 0 && !has-played` (series posters only)
- **♥ badge** (top-left, accent circle) when `is-favorite`

Card dimensions are computed by breakpoint pure functions (`dash-card-w`, `dash-card-h`, `grid-cols`) that live on `MainWindow` because they reference `self.width`. A `sync-layout()` function pushes the results to `AppState.dash-cw`, `AppState.dash-ch`, and `AppState.library-cols` on `init` and `changed width` so all screens see the current sizes.

### Disk caches
- `~/.cache/fjord/home.json` — home row data. Shown from cache immediately on warm start, always refreshed in the background.
- `~/.cache/fjord/movies.json` — full movie list (`Vec<MediaItem>`). Populated after first network fetch; on warm start loaded immediately so Browse All and the Movies grid are instant. Refreshed once per session on grid open (`movies_fetched` flag guards re-fetch).
- `~/.cache/fjord/series.json` — full series list. Populated at login/auto-login and on every background refresh. Loaded on warm start so Browse All and the TV grid are instant.
- `~/.cache/fjord/posters/<id>` — raw poster bytes, one file per item.

On a warm start (valid saved session + fresh cache) the window opens in the logged-in state with content visible on the first frame — no loading flash.

### Keyboard navigation
A global zero-size `FocusScope` (`fs`) captures all keyboard input. `invoke_grab_keyboard_focus()` is called from Rust at startup **and after every login** (manual + auto-login) to give `fs` focus — without the post-login call, all keyboard navigation is dead until restart.

All keyboard input flows into `keys::handle_key()` in Rust (called from Slint's `key-pressed` handler). `active_mode()` derives the current `AppMode` (8 variants) from `AppState` flags — screen priority is encoded once in this function, not scattered across conditionals. A `match mode { ... }` then routes to per-module handlers: `context_menu::handle_key`, `series::handle_key`, `detail::handle_key`, `dispatch_player`, `dispatch_library`, `browse::handle_key`, `dispatch_settings`, `dispatch_dashboard`. A pre-match check for `ResumePlayer` fires globally for all modes except Player/Detail/ContextMenu. `handle_global_shortcuts` (F/Q/B/1/2/3/S) is called as a fallback from both Dashboard and Settings arms. The contract is uniform: **Enter/Right enter**, **Backspace/Escape go back**, **Up/Down navigate rows/items**, **Left/Right navigate within a row or cycle a combobox**.

All keyboard state lives in the `AppState` global singleton. Key nav state:
- **`-1` = sidebar**: Up/Down cycle nav tabs (0 Home → 1 Movies → 2 TV → 3 Browse All → **4 Now Playing** (only when `has-background-player`) → 10 Settings → 11 Quit → wrap); arrowing to nav=3 opens `show-browse` immediately; arrowing to nav=4 focuses the Now Playing mini card (`mini-card-focused` resets to 0=Resume); Right/Enter enters the content grid or library; `settings-focused` is reset to -1 when `active-nav` changes and also when `B` opens browse.
- **`≥ 0` = content grid**: focused-section is the row index, `focused-card` is the column. Up/Down move between rows (Up at row 0 stays in content); Left/Right move between cards; Enter plays; I opens detail/series screen.
- **Browse list** (`show-browse = true`, `active-nav == 3`): opens in sidebar mode (`current-item = -1`). Up/Down navigate the sidebar; Right or Enter enters the list (`current-item = 0`). In list mode: Up/Down navigate items; Up at item 0 focuses the search bar (`browse-header-focused = true`); Left returns to sidebar; `/` also jumps to search. Search bar focused (`browse-header-focused = true`): typing filters client-side; Backspace deletes (empty → back to list); Down/Enter moves into results; Escape clears query and unfocuses. Backspace/Escape in list/sidebar mode closes browse and resets `active-nav = 0` when exiting via the Browse All sidebar entry. `B` shortcut also opens browse without changing `active-nav`.
- **Library grid** (`show-library = true`): 2D arrow nav across the poster grid; Enter opens detail; Backspace/Escape closes. An always-visible search field sits below the top bar and shows the active query at all times. Two states tracked by `library-header-focused`: (1) **grid mode** (`library-header-focused = false`) — arrow keys navigate posters, Up at row 0 focuses the search field, `/` also jumps to the search field; (2) **search field focused** (`library-header-focused = true`) — letters type into the query immediately, Backspace deletes (empty → back to grid), Down/Enter moves into the results grid (keeps query), Escape clears the query and returns to grid.
- **Series screen** (`show-series = true`): Left/Right navigate season tabs; Enter/Down enters episode list from season row; Up/Down navigate episodes; Up at episode 0 jumps back to season row; Enter/Space plays focused episode; Backspace/Escape closes.
- **Detail page** (`show-detail = true`): Up/Down scroll the overview; Left/Right cycle the focused button: Play (0) → Resume (1, only if `detail-can-resume`) → Series (2, only if `detail-series-id` non-empty, Episodes only); Enter/Space activates the focused button — Play calls `play-detail`, Resume calls `resume-detail`, Series closes the detail page and opens the series screen via `open-series(detail-series-id)`; R resumes (if available); Backspace/Escape or the Back button closes and resets `detail-scroll`. **Important:** Rust code that closes the detail page (e.g. `on_play_detail`, `on_resume_detail`) must also reset `detail-scroll = 0` before calling `set_show_detail(false)`; otherwise the next detail open starts scrolled.
- **Settings** (`active-nav == 10`): two-pane layout. `settings-section: int` (-1 = app sidebar, ≥0 = selected section in the left pane). `settings-focused: int` (-1 = left pane focus, ≥0 = focused row in right pane). Left pane: Up/Down navigate sections (General=0, Video=1, Audio=2, Player=3, Key Bindings=4); Right/Enter enters the right pane (`settings-focused = 0`). Right pane: Up/Down move through rows; Left/Right cycle combobox values; **Enter on a dropdown row opens a popup list** (`settings-dropdown-open = true`, cursor set to current value's index) — Up/Down move the cursor, Enter confirms selection, Escape/Left closes without change; Enter on a toggle row toggles it; Left/Backspace returns to left pane (`settings-focused = -1`); Backspace/Escape from left pane exits settings (`settings-section = -1`). Row hover highlighting uses `SettingsRow` component (TouchArea lower z, `@children` higher z). `SettingsDropdown` has `kb-open` / `kb-cursor` properties; the popup highlights the cursor item and scrolls to keep it centred (max height 320px with Flickable). Section rows: **Video** hwdec(0), vf(1), deinterlace(2), video-sync(3), interpolation(4), tscale(5 virtual, hidden when interpolation off), target-colorspace(6), tone-mapping(7 virtual, hidden when HDR passthrough on), opengl-early-flush(8), video-latency-hacks(9 virtual, hidden when video-sync≠display-resample). **Audio** audio-device(0), SPDIF(1), AC3(2 hidden when SPDIF off), EAC3(3 hidden when SPDIF off), DTS(4 hidden when SPDIF off), DTS-HD(5 hidden when SPDIF off), TrueHD(6 hidden when SPDIF off), alsa-irq-scheduling(7 virtual, hidden when SPDIF off OR non-PipeWire device), audio-lang(8). The audio-device dropdown is dynamic (populated at startup via `mpv --no-config --audio-device=help`) and uses a special path in `dispatch_settings` / `apply_dropdown_selection` — `AppState.audio-device-selected(desc)` callback maps description → mpv name via `FjordState.audio_devices`. `AppState.settings-device-is-pipewire` (bool, set by Rust) gates the IRQ row; `pipewire_fix.rs` implements `is_pipewire_device()` and `apply_alsa_irq_scheduling(bool)` (writes/deletes `~/.config/wireplumber/wireplumber.conf.d/fjord-alsa-irq.conf` and restarts WirePlumber via `systemctl --user restart wireplumber`; config persists after Fjord exits and is only changed when the toggle changes state). **Player** sub-enabled(0), sub-lang(1, hidden + indented when sub-enabled is off), sub-lang2(2, hidden + indented when sub-enabled is off), cache-mb(3); **INTRO SKIPPER** intro-mode(4), intro-secs(5 virtual, shown when intro=ask-timed), recap-mode(6), recap-secs(7 virtual), preview-mode(8), preview-secs(9 virtual), commercial-mode(10), commercial-secs(11 virtual); **CREDITS** credits-mode(12), credits-secs(13 virtual, shown when credits=ask). Skip modes for Intro/Recap/Preview/Commercial: `always-skip` (immediate seek, no overlay), `ask` (single "Skip →" button), `ask-timed` (two-button overlay "Skip" + "Don't Skip" + per-segment countdown — auto-skips when timer runs out; Back/Esc dismisses), `never-skip` (no overlay). Credits modes: `always-skip` (auto-advance immediately), `ask` (show Up Next banner with configurable countdown), `never-skip` (no banner). Cross-section conflict "⚠ passthrough + display-resample" shown in both Video (below video-sync) and Audio (below SPDIF rows); only shown when master SPDIF toggle is on and at least one format is enabled. (GPU API row removed — had no effect with `vo=libmpv` + OpenGL context.)
- **Now Playing mini card** (`active-nav == 4`, only when `has-background-player`): Left/Right toggle `mini-card-focused` between 0 (Resume) and 1 (Stop); Enter activates the focused button. Up exits to Browse All (3); Down exits to Settings (10). `reset_playback_ui` resets `active-nav` from 4 → 0 so the card disappearing doesn't leave the UI stuck — called by both `do_stop_playback` and the natural-end path in `wire_mpv_timer`.
- **Player** (`is-playing = true`): `dispatch_player` checks overlays in priority order — **(1) ask-timed overlay** (`show-skip-timed`): Left/Right toggle `skip-timed-focused` (0=Skip, 1=Don't Skip), Enter activates focused button, Back/Esc dismisses (sets `skip_segment_handled = true`, hides overlay); **(2) ask-mode skip segment** (`show-skip-segment`): Enter skips; **(3) Up Next banner** (`show-next-ep-banner`): Left/Right toggle `next-ep-banner-focused` (0=Play Now, 1=Skip), Enter activates. All three take priority over all other player keys. Space/K/P pause (blocked while seek bar is held — `seek-dragging` is true during drag, `dispatch_player` eats the event so mpv isn't toggled while the bar shows the frozen drag position); Left/Right seek ±10s (Shift ±30s); Up/Down volume; S/A/V open track panels; Up/Down in panel navigates tracks; Enter commits selection; M mute; I toggles stats overlay only (does **not** show the controls bar — the player-mode key handler skips `invoke_show_controls()` for `Action::ToggleStats`); F/F11 fullscreen; 0–9 seek to %; Backspace minimizes (or closes open panel first); Escape stops (or closes open panel first). Volume Up/Down shows a top-center toast overlay (~1.5 s, auto-hides); when SPDIF passthrough is active (`audio-passthrough-active`) the overlay shows "Vol · passthrough" and `adjust_volume` is skipped. The controls bar shows title, seek track, `HH:MM:SS / HH:MM:SS` elapsed/total, and **"Ends HH:MM"** (`playback-ends-at`, local wall-clock time, updated every ~500 ms, cleared on stop). Track panels are `min(parent.width - 32px, 400px)` wide with `wrap: word-wrap`; labels are ordered **title → lang → codec** (external subtitle files fall back to base filename as title). The stats overlay (`stats-visible`) is a 420 px panel top-right with three sections: **VIDEO** (IN/OUT/COLOR/HWDEC), **AUDIO** (IN/OUT), **SYNC** (DISPLAY/VSYNC/A/V/SPEED/DROP/BITRATE/CACHE). Values use `wrap: word-wrap` so long codec/format strings never elide.

**Hold vs tap Left:** At `focused-card == 0`, a single tap Left exits to the sidebar; this uses `!event.repeat` as a best-effort guard. `event.repeat` is unreliable in Slint (see Slint gotchas), so this distinction may not always hold — but the worst case is landing in the sidebar, which is harmless.

Shortcuts active at dashboard/browse level: `1`/`2`/`3` jump to Home/Movies/TV (also resets `settings-focused`); `S` to Settings; `B` opens the browse list; `F`/`F11` toggles fullscreen; `Q` quits; `R` resumes background player.

### Context menu
Triggered by `C` key on any focused card or right-click on any `MediaCard`. State lives in `AppState`:
- `context-menu-item-id`, `context-menu-item-type`, `context-menu-has-played`, `context-menu-is-favorite`, `context-menu-resume-pct`, `context-menu-focused: int`

Menu rows (in order): **Resume** (row 0, conditional: `resume-pct > 0 && !has-played`), **Play from Start** (row 1), **Mark Played/Unplayed** (row 2), **Add/Remove Favourite** (row 3), **View Details** (row 4). Initial focus lands on row 0 when Resume is available, otherwise row 1. Up/Down loop — pressing Up from the top row wraps to row 4; Down from row 4 wraps to the top row. The min row for looping is 0 when Resume is shown, 1 otherwise.

`wire_context_menu` in `context_menu.rs` registers `on_open_context_menu` (from card data), `on_open_context_menu_browse` (resolves browse index → `filtered_items`), and `on_open_context_menu_series_ep` (episode C-key). All three set `context-menu-focused` to 0 or 1 depending on resume availability. `on_context_play_from_start` checks `all_series`: for series it calls `get_next_up_for_series` (falling back to series screen); for movies/episodes it plays from position 0. `update_card_in_all_models` patches `has-played` / `is-favorite` across every `CardItem` and `EpisodeEntry` model after a successful API toggle.

### Sidebar logo
The sidebar header shows a randomly selected icon from the kept pool: `fjord_01`, `fjord_02`, `fjord_04`, `fjord_05`, `fjord_09`, `fjord_10`. The index is picked at startup via `LOGOS[subsec_nanos % LOGOS.len()]` (array `[1,2,4,5,9,10]`) and stored in `AppState.app-logo-idx`. All 6 SVGs are embedded at compile time via a `@image-url()` ternary chain in `layout.slint`. Icon 01 has a transparent background with white FJORD text (evenodd fill-rule for O/D/R letter holes). Icons 02/04/10 have intentional dark rounded-square backgrounds — white corner-fill paths were removed and a `<clipPath>` with a rounded `<rect>` (rx=234/222/176 respectively) wraps all content to make the corners transparent. Icons 05/09 have fully transparent backgrounds. The random selection stays until a permanent icon is chosen.

### Subtitle auto-select
At playback start, if `settings-sub-enabled` is false → force track 0 (off). If a language preference is set, `sub_lang_code()` maps display names ("English" → "en") and tries `sub_lang` then `sub_lang2` by `lang.starts_with(code)`. If no match, mpv's default selection is left unchanged. External subtitle tracks use `track-list/N/external-filename` (base filename) as the label fallback when `title` is empty.

### Fullscreen
`window.window().set_fullscreen(bool)` / `is_fullscreen()` used directly. Toggle is wired to `on_toggle_fullscreen` callback (called by `F`/`F11` key). The "Launch in fullscreen" setting applies the flag before `window.run()` and also immediately when the checkbox is toggled.

### Session identity (DeviceId)

`JellyfinClient` carries a `device_id: String` field used in the `Authorization` header (`DeviceId="…"`). The internal `reqwest::Client` is built with a **30-second request timeout** so a server that accepts the TCP connection but stops responding never hangs the auto-login task or API calls indefinitely. On first run, `ensure_device_id()` reads `/proc/sys/kernel/random/uuid`, saves it to `~/.config/fjord/config.json`, and uses it for the lifetime of the install. This is critical: if two machines share the same DeviceId, Jellyfin invalidates one machine's token when the other authenticates, causing 401 errors on all API calls.

On startup, after loading a saved session, `check_auth()` does a cheap `GET /Users/{id}/Items?Limit=0&Recursive=true` probe. On 401 the login screen is shown; any other error is ignored and the app proceeds (transient network issue). Passwords are never stored — Jellyfin tokens don't expire under normal use.

### Workspace crates
- `fjord-api`: no UI, no mpv. Pure async HTTP + JSON. Testable in isolation.
- `fjord-player`: no UI, no HTTP. Just libmpv bindings + render context.
- `fjord-app`: thin wiring layer. Imports the other two, drives the Slint event loop.

### Episode auto-advance
Behaviour depends on `Config.skip_credits_mode`:
- **`never-skip`**: no banner, no auto-advance — `banner_trigger` is never fired.
- **`always-skip`**: auto-advance immediately when credits position is reached (no banner shown); `start_playback` called directly from the timer via `invoke_from_event_loop`.
- **`ask`** (default): Up Next banner fires *during* playback at `VideoState.credits_start` (from the Intro Skipper `/Timestamps` response — `credits.start`) or when `duration >= 60 s AND duration - position <= 30 s` (fallback). `next_ep_banner_shown` flag prevents it firing more than once per episode. A configurable countdown (`Config.skip_credits_secs`, default 30 s, stored as `banner_trigger.2`) counts down; the banner auto-advances when it reaches zero. "Play Now" calls `on_play_next_ep`; "Skip" calls `cancel-auto-advance` which sets `next_ep_pending = None` and exits the countdown task without playing. Keyboard: `next-ep-banner-focused` (0=Play Now, 1=Skip), Left/Right toggle, Enter activates.

`banner_trigger` type is `Option<(String, Option<Arc<JellyfinClient>>, u32, bool)>` — the `u32` is countdown seconds (0 for always-skip, so loop body never executes), `bool` is `show_banner` (false for always-skip suppresses UI updates).

`next_ep_pending` lives in `VideoState` (not `FjordState`) so it is cleared atomically when a new video starts, preventing stale pending state bleeding across sessions.

Every `start_playback` call site must pass `series_id` so auto-advance works for plays from any screen.

### Intro Skipper plugin
When starting playback of an Episode, `start_playback` spawns one background task:
- **All segments**: `client.get_episode_timestamps(item_id)` (`GET /Episode/{id}/Timestamps`). Returns `EpisodeTimestamps { introduction, credits, recap, preview, commercial }` where each is a `Segment { start: f64, end: f64 }`. Valid when `end > 0.0`. On success the matching `VideoState` fields are populated: `intro_timestamps`, `recap_timestamps`, `preview_timestamps`, `commercial_timestamps`, and `credits_start = Some(ts.credits.start)`.

Returns `None` gracefully when the plugin is absent (404).

The 16 ms timer checks the current playback position against each segment in priority order (Intro → Recap → Preview → Commercial). At most one segment is active at a time. Behaviour per segment depends on the configured mode (`settings-skip-*-mode` from `AppState`):
- **`always-skip`**: seek to segment end immediately, set `skip_segment_handled = true`, hide overlays.
- **`ask`**: show `show-skip-segment` (single "Skip →" button). Enter in `dispatch_player` calls `invoke_skip_segment()`.
- **`ask-timed`**: on first tick set `skip_timed_shown_at = Some(Instant::now())`. Each tick compute `remaining = prompt_secs - elapsed`. Set `show-skip-timed = true`, update countdown label. When remaining ≤ 0: seek to segment end, set `skip_segment_handled = true`, hide overlays. User can "Don't Skip" via the overlay button or Esc (calls `invoke_dismiss_skip_timed()`, sets `skip_segment_handled = true`, hides overlay — suppresses re-show while still in segment).
- **`never-skip`**: hide all overlays, do nothing.

`VideoState` fields: `skip_segment_handled: bool` — set after seek or dismiss, reset to false when position exits the segment; `skip_timed_shown_at: Option<Instant>` — when the timed overlay first appeared; `skip_timed_prompt_secs: u32` — snapshot of per-segment secs at overlay start. `show-skip-timed`, `skip-timed-label`, `skip-timed-secs`, `skip-timed-focused` in `AppState` drive the UI.

**Stale-response guard:** `VideoState.playback_generation` is a `u64` counter incremented at the top of every `start_playback` call. Each spawned task captures the current generation and discards its result if `vs.playback_generation` no longer matches when the response arrives. This prevents a slow network response for episode A from overwriting episode B's `intro_timestamps` or `credits_start` after a fast episode skip.

### Async strategy
Tokio for all async. The Slint event loop runs on the main thread. Background tasks (API calls, poster fetching) use `tokio::spawn`. Communication back to the UI uses `slint::invoke_from_event_loop` or channels.

## Build

```bash
cargo build                     # debug build
cargo build --release           # release
cargo run -p fjord-app          # run the app
```

Requires `mpv` and `libmpv` to be installed (`pacman -S mpv`).

## Dependencies (key ones)

| Crate | Purpose |
|-------|---------|
| `slint` | GUI framework |
| `slint-build` | build.rs compiler for .slint files |
| `libmpv2` | libmpv bindings |
| `reqwest` | HTTP client for Jellyfin API |
| `serde` / `serde_json` | JSON serialization |
| `tokio` | async runtime |
| `image` | JPEG/PNG decode for poster thumbnails |
| `gl` / `euclid` | OpenGL FBO management for mpv render API |
| `anyhow` / `thiserror` | error handling |

## What is Jellyfin

Jellyfin is an open-source media server. It exposes a REST API for browsing libraries (movies, TV shows, music) and getting playback URLs. Auth is username+password → returns an API token that goes in every subsequent request header as `X-Emby-Token` (Jellyfin kept the Emby header name).

Key API endpoints used:
- `POST /Users/AuthenticateByName` — login
- `GET /Users/{userId}/Items` — browse/search items
- `GET /Users/{userId}/Items/{itemId}` — item detail (overview, cast, backdrop tags, etc.)
- `GET /Items/{itemId}/Images/Primary` — poster image
- `GET /Items/{itemId}/Images/Backdrop/0` — backdrop image
- `GET /Users/{userId}/Items?Filters=IsResumable` — continue watching
- `GET /Shows/NextUp` — next unwatched episode per series (home row)
- `GET /Shows/NextUp?SeriesId=…` — next episode for a specific series (auto-advance)
- `GET /Shows/{seriesId}/Seasons` — season list
- `GET /Shows/{seriesId}/Episodes?seasonId=…` — episode list for a season
- `GET /Videos/{itemId}/stream?static=true&api_key=…` — direct-play URL
- `POST /Sessions/Playing` — report playback started
- `POST /Sessions/Playing/Progress` — report position
- `POST /Sessions/Playing/Stopped` — report stopped
- `GET /Episode/{itemId}/Timestamps` — all skippable segments (Introduction, Recap, Preview, Commercial, Credits) in one call (Intro Skipper plugin v2+, optional)
- `GET /Items/{itemId}/Similar?userId=…&Limit=12&Fields=…` — similar items (same type, movies or series)
- `GET /Users/{userId}/Items?IncludeItemTypes=BoxSet&Recursive=true&Fields=Id,Name` — all BoxSets (collection map build)
- `GET /Users/{userId}/Items?ParentId={boxsetId}&Fields=ProductionYear,UserData` — items in a BoxSet (collection row)

## Development workflow

1. **Implement** the feature or fix.
2. **Update PLAN.md** — check off completed items, add any new ones discovered.
3. **Update TOC headers** in every modified `.rs` / `.slint` file — symbols added/removed *and* behaviour changes.
4. **Commit and push** — always push immediately after committing (`git push`). The HTPC only sees what's on GitHub, so an unpushed commit is the same as no commit from the HTPC's perspective.
5. **Test on HTPC** — SSH in and run `makepkg -si` in the repo root. The PKGBUILD pulls from GitHub and does a native `cargo build --release --locked`.

## Testing setup

Two machines:
- **Dev machine** (this repo): AMD GPU, Wayland, Vulkan. Used for development.
- **HTPC**: NVIDIA legacy GPU, Wayland/EGL. The primary target. Logs land in `/home/htpc/.cache/fjord/fjord.log`.

Deploy workflow: push to GitHub → on the HTPC run `makepkg -si` with the `PKGBUILD` at the repo root. The PKGBUILD pulls from `https://github.com/KalasKonrad/Fjord.git` and does a native `cargo build --release --locked`, installing the binary to `/usr/bin/fjord`.

The HTPC is the harder target — it is what motivated the render API design in the first place.

## Known platform issues

### NVIDIA legacy Wayland: NVDEC stride corruption
**Symptom:** Diagonal stripe artifact (raw YUV scan lines) when using hardware decoding (`nvdec`, `nvdec-copy`). Software decoding is clean.

**Root cause:** NVDEC aligns decoded frame rows to 256-byte boundaries (e.g., a 1920-pixel-wide video gets a 2048-byte stride). mpv uploads via `glTexSubImage2D` with `GL_UNPACK_ROW_LENGTH=2048`. The NVIDIA legacy EGL driver silently ignores `GL_UNPACK_ROW_LENGTH`, so GL reads each row 128 bytes too tight — each successive row is offset from the previous, producing the diagonal slant.

**Fix:** Set `vf=format=yuv420p` in Settings → Video. This adds a software format conversion step after NVDEC decodes the frame, producing tight-packed yuv420p output so `GL_UNPACK_ROW_LENGTH` is never needed. For 10-bit HDR use `format=yuv420p10le`. The `auto` option detects the active hwdec and bit depth at runtime and picks the right format. `hwdec-image-format` was tried first but has no effect on NVIDIA legacy EGL.

**AMD Vulkan:** `vulkan-copy` works correctly with no stride workaround needed.

### PlayerConfig fields (fjord-player/src/mpv.rs)
All fields are logged at playback start so the log shows exactly what options were active. Key fields:
- `hwdec` — decoder selection (`auto`, `nvdec-copy`, `vulkan-copy`, etc.)
- `vf` — video filter string. Use `format=yuv420p` (or `auto`) for NVIDIA legacy stride fix.
- `video_sync` — `audio` (default), `display-resample` (locks to display refresh via `report_swap()`), `display-vdrop`, `display-adrop`, or `desync` (no A/V correction — debug option for isolating #39 passthrough dropout).
- `opengl_early_flush` — flush GL after each frame; may help with EGL pipeline ordering on NVIDIA.
- `video_latency_hacks` — compensates for imprecise Wayland vsync timestamps on NVIDIA 5xx legacy.

## Known Slint gotchas

These have each caused real bugs in this codebase:

**`Flickable` is the only reliable keyboard-scrollable container.** `ScrollView` ignores declarative `viewport-y` bindings (it manages its own scroll internally). `ListView` also writes to `viewport-y` from its own scroll handler, silently overwriting any binding you set. The correct pattern for any keyboard-driven scrollable list is `Flickable { viewport-height: ...; VerticalLayout { for ... } }` with `viewport-y` bound to a `clamp(...)` expression that tracks the focused index.

**Do not self-reference a `Flickable`'s own layout properties in its `viewport-y` binding.** Writing `viewport-y: clamp(... flk.height ... flk.viewport-height ...)` creates a binding whose dependencies Slint may not reliably track — `flk.height` and `flk.viewport-height` are layout-managed and may not trigger binding re-evaluation when `player-panel-cursor` changes. Instead, reference `parent.height` (the outer Rectangle's height) and the content layout's `preferred-height` directly: `clamp(-(cursor * 34px) + parent.height / 2 - 17px, min(0px, parent.height - list.preferred-height), 0px)`. This is what fixed the track panel scroll bug (#22).

**A `viewport-y` binding on a `Flickable` blocks native mouse-wheel scrolling.** When `viewport-y` is bound to an expression, the Flickable's internal scroll handler can't write to it (the binding overrides any assignment on the next frame), so mouse-wheel does nothing. When both keyboard nav and mouse-wheel scroll are needed: remove the `viewport-y` binding; on the outer Rectangle declare `property <length> kb-y: clamp(...)` and `changed kb-y => { fl.viewport-y = kb-y; }` for keyboard nav; the Flickable then handles mouse-wheel natively. Also fix any `fl.height` / `self.viewport-height` self-references in the old binding expression — use the outer container's `self.height` and the content layout's `preferred-height` instead. This is the pattern applied to all scrollable Flickables in the codebase (player panels, detail, series, home/movies/TV dashboards, library grid, settings right pane, browse list). Note: the browse list previously used `interactive: false` which blocks mouse-wheel regardless of bindings; changing it to `interactive: true` re-enables native scroll while child `TouchArea` clicks still fire normally (Slint distinguishes drag from click).

**Plain `Rectangle` children are horizontally centred by default.** If you need a fill bar or overlay anchored to the left edge, you must set `x: 0` explicitly. Omitting it centres the element and produces the "progress bar starts from the middle" bug.

**`KeyEvent.repeat` is unreliable — never use it to guard state transitions.** In practice `repeat` can be `false` for auto-repeated key events (confirmed on desktop Wayland, not just wireless keyboards). A guard like `if !event.repeat { close_screen() }` will fire on every spurious non-repeat event during a hold, chaining through screens unexpectedly. The correct pattern is to let the state machine be the guard: once the transition fires (e.g. `show-browse = false`), the outer `if AppState.show-browse` condition stops subsequent events from re-firing it. For search fields specifically: Backspace should only delete characters; use Escape as the dedicated "exit search" key. Never use `!event.repeat` to gate a backspace-exits-search path — a held Backspace will empty the query and then bleed into the close-screen handler.

**Slint ternary short-circuits dependency tracking.** If a property binding uses `condition ? A : B` and `B` contains a reactive property (e.g. `has-hover`), Slint only tracks `B`'s dependencies when the else-branch is actually evaluated. If the condition is initially true, `has-hover` is never read and hover changes never trigger a repaint. Fix: read the property unconditionally first using a block expression — `background: { let hov = ta.has-hover; cond ? Theme.accent : (hov ? Theme.surface : transparent) };`. This was the root cause of settings left-pane hover not working.

**`invoke_from_event_loop` closures must be `'static + Send`.** Capture owned values (`String`, `Arc<…>`) not references. This is the correct pattern for communicating from Tokio tasks back to Slint UI state.

**`TouchArea.moved` fires only during drag (button held), not plain cursor movement.** To react to mouse movement without a button press, use `changed mouse-x => { ... }` and `changed mouse-y => { ... }` callbacks. This is how the player controls overlay auto-show is implemented.

**`opacity: 0` elements remain fully hit-testable.** Setting `opacity: 0` makes an element invisible but it still participates in hit-testing and determines the mouse cursor shape — only `visible: false` removes it from event handling. The player controls bar fades via `opacity`, so its child `TouchArea`s were silently overriding `mouse-cursor: none` on the element beneath them. The fix is a full-size cursor-hider `TouchArea` declared last (highest z-order) with `enabled: !root.controls-visible` and `mouse-cursor: MouseCursor.none`. When `enabled: false`, a `TouchArea` passes events through to elements below it.

## Style

- Standard Rust formatting (`cargo fmt`)
- Errors: use `anyhow::Result` at the top level, `thiserror` for library error types
- No `unwrap()` in library code — propagate errors
- Keep `fjord-api` and `fjord-player` free of Slint imports
- Every `.rs` and `.slint` source file opens with a `// ── <crate> · <filename> ──` header block listing its major symbols/sections (one line each). Longer files additionally carry `// ──` inline section markers immediately before major functions and visual blocks. The header is the first thing in the file, before any `use` statements or declarations. Update the header whenever symbols are added, removed, or their behaviour changes — not just when the name changes.
