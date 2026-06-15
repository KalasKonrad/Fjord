# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux (initially) that plays video smoothly on NVIDIA legacy hardware using the mpv render API with real vsync feedback via `report_swap()`.

---

## Phase 1 — Foundation ✅
Slint window opens, libmpv links, basic app structure and logging.

## Phase 2 — Player ✅
mpv render API (`vo=libmpv` + double-buffer FBO), vsync feedback via `report_swap()`, audio passthrough, hardware decode, playback reporting.

## Phase 3 — Jellyfin API client ✅
Auth, library browse, continue watching / next up / recently added, direct-play URL, session persistence, auto-login.

## Phase 4 — UI ✅
Login, browse list, home/movies/TV dashboards, poster thumbnails, three playback modes, player controls overlay, settings screen, detail page, resume, seek bar.

## Phase 6 — Packaging ✅
PKGBUILD for Arch, desktop file, random SVG icon, per-machine DeviceId.

## Phase 7 — NVIDIA legacy fix ✅
Diagnosed NVDEC stride corruption; `vf=auto` applies tight-packed pixel format at runtime; expanded stats overlay.

## Phase 8 — Code organisation ✅
Split `main.rs` (2600 lines) and `main.slint` (3200 lines) into focused modules. `global AppState` singleton holds all shared UI state; keyboard handler stays in `main.slint`.

---

## Phase 5 — HTPC Polish

Core keyboard nav and player controls are complete. Open items:

**Resume position data freshness:**
- [x] Fresh item fetch before playback — call `GET /Users/{userId}/Items/{itemId}` immediately before `start_playback` and use the returned `UserData.PlaybackPositionTicks` as the start position instead of `media_raw`. Fixes stale seek position for all play paths. `media_raw` is up to 6 h stale; the Continue Watching row's progress bar comes from a fresh home fetch so the two can disagree.
- [x] Refresh home data after playback stops — call `fetch_home_data` in the background when `on_stop_playback` fires and push the result to the UI. Keeps the Continue Watching row progress bars accurate within a session without requiring an app restart.

**Startup & search architecture:**
- [x] Server-side search — replace the browse list's client-side filter over `media_raw` with `GET /Users/{userId}/Items?searchTerm=<query>&recursive=true`. Results come from the server, always fresh, no local library needed. Debounce keystrokes before firing.
- [x] Lazy-load the library grid — fetch the full item list only when the user opens the Movies or TV library grid, not at startup. Combined with server-side search, the full `get_all_items()` startup fetch and `items.json` cache become unnecessary, making cold starts as fast as warm starts.

**Keyboard navigation gaps:**
- [x] Detail page button navigation — Tab/Left/Right cycles focus between Play, Resume, and secondary action buttons so every detail-page action is reachable by keyboard.
- [x] Secondary actions keyboard access — context menu via `C` key (+ right-click) on any focused card. Rows (in order): Resume (if resume point exists), Play from Start, Mark Played/Unplayed, Favourite/Unfavourite, View Details. Up/Down navigate with looping; Enter confirms; Escape/Backspace closes. Works on dashboard rows, library grid, browse list, and series episode list. Play from Start on a series uses `get_next_up_for_series` (falls back to series screen); on movie/episode plays from position 0.
- [x] Card badges — bolder watched ✓ badge, unplayed count pill (accent background, bold), favourite ♥ badge (top-left, accent circle). `HomeItem.is-favorite` populated from `UserData.IsFavorite`.
- [x] Server name + version in settings left pane — fetched from `GET /System/Info/Public` in parallel with home data on login/auto-login; displayed instead of the raw server URL.
- [ ] Cast member photos on detail page — add `id` field to `CastMember`, fetch person portraits (`GET /Items/{personId}/Images/Primary`) using the same poster-loading pipeline, display above name/role in the cast row.
- [ ] Cast row keyboard navigation — Left/Right moves through cast members on detail page; Enter opens person detail screen (depends on person detail screen being built).

**Key binding cleanup:**
- [x] Remove Space as play shortcut from detail page and series screen — Enter already handles play; Space=pause belongs only in the player.

**Settings screen keyboard nav bugs:**
- [x] Focus highlight drifts when tscale row is hidden — rows 12–17 in `row-approx-y` assume tscale is always present; when `settings-interpolation` is off the highlight is ~70px below the actual row.
- [x] Focus highlight drifts when SPDIF conflict warning appears — the conditional warning text between rows 1 and 2 pushes rows 2–17 down ~35px, not reflected in `row-approx-y`.
- [x] Fix root cause: replace `row-approx-y` hardcoded lookup table with named row elements so scroll/highlight positioning reads actual layout `.y` values instead of approximations.

**Keyboard nav bugs — global shortcut blockage:**
- [x] Series screen: `return reject` at end of series block swallows F/Q — can't toggle fullscreen or quit while series screen is open.
- [x] Detail page: `return reject` at end of detail block swallows F/Q — same problem on detail page.
- [x] Library header-focused mode: `return reject` at end of header block swallows F/Q — can't toggle fullscreen while search header is focused.
- [x] Settings: Down at row 17 (Sign Out) falls through to the global Down handler — cycles the sidebar from Settings (nav=10) to Quit (nav=11) and resets settings-focused. Guard `settings-focused < 17` needs to return accept to stay put.

**Architecture / data model:**
- [x] Library list cache — `~/.cache/fjord/movies.json` and `~/.cache/fjord/series.json` (same pattern as `home.json`): cached list loaded instantly on warm start, grid populated before network returns. Posters for all three caches (home, movies, series) spawned immediately from disk cache on warm start.
- [x] Add `item-type: string` to `CardItem` (theme.slint) and populate it everywhere `CardItem` is built — `open_detail` now routes by `item_type == "Series"` instead of scanning `all_series`.
- [x] Single entity / canonical store — each item (movie, series, episode) must have exactly one copy of its user state (`played`, `is_favorite`, `resume_pct`) in `FjordState`. `FjordState::update_item_user_state(id, played, fav)` patches all canonical Rust vecs (`all_movies`, `all_series`, `filtered_items`) before the Slint model patch, so any subsequent model rebuild reads correct data.

**Configurable key bindings — Rust-side key handler rewrite:**

Replaces the 670-line Slint key handler with a single callback into Rust. Enables user-configurable bindings and makes adding new screens trivial. Roll-back tag: `pre-keybinding-refactor`. The nav behaviour to replicate is documented in CLAUDE.md (Keyboard navigation section).

- [x] Define `Action` enum in Rust (~35 variants) in `keys.rs`.
- [x] Define `KeyCombo` (key string + shift/ctrl/alt bools), `KeyMap` (HashMap<KeyCombo, Action>), `Keybindings` (normal + player maps) in `keys.rs`. Serialize to `~/.config/fjord/keybindings.json`; user overrides merge on top of defaults loaded in `config.rs`.
- [x] Define `AppMode` enum (14 variants) in `keys.rs`; mode derived inline in `handle_key` from `AppState` flags.
- [x] Implement Rust `handle_key(key, shift, ctrl, repeat) -> bool` in `keys.rs` — full dispatch for all 14 modes.
- [x] Replaced the 670-line Slint key handler with a single `handle-key(text, shift, ctrl, repeat) -> bool` callback in `app_state.slint`; wired in `main.rs`; Slint returns accept/reject based on bool.
- [x] Settings UI: KEY BINDINGS section at the bottom of Settings — lists all remappable actions with current key labels, Enter to capture next key press and rebind, Reset to Defaults button. Keybindings serialised to `~/.config/fjord/keybindings.json` as human-readable `"ctrl+shift+f"` strings.

**Settings refactor:**
- [x] Remove dead `settings-vo` row — "Video output (vo)" shows gpu-next/gpu but `vo` is always forced to `libmpv` in `Player::new()`; the row silently does nothing and the label is misleading. Delete row from `settings.slint`, remove `settings-vo` property from `app_state.slint`, remove match arm 2 from `settings_row_action` and renumber.
- [x] Remove dead `settings-layout-style` property from `app_state.slint` — defined but never read, set, or referenced anywhere.
- [x] Merge `Config` into `FjordState` — both structs carried the same 15 settings fields. Now `FjordState.config: Config` is the single authoritative copy; `apply_from_config` is gone; `on_settings_changed` just calls `save_config(&s.config)`. Adding a setting now requires only: Config field, player_config() builder, PlayerConfig struct, Slint property, UI row, ROW_* constant.
- [x] Replace magic integers in `settings_row_action` with named `ROW_*` constants in `settings.rs` so inserting a row doesn't require renumbering every subsequent match arm.
- [x] Extract `dispatch_settings` + `settings_row_action` out of `keys.rs` into a dedicated `settings.rs` module (consistent with the rest of the module pattern; `keys.rs` is 1245 lines and shouldn't own settings keyboard nav).
- [x] Remove dead `hwdec-image-format` setting — confirmed no effect on NVIDIA legacy EGL (tested); `vf=format=yuv420p` is the working fix. Removed from `PlayerConfig`, `Config`, `FjordState`, all sync functions, and the UI.
- [ ] **Settings page redesign** — two-pane layout (done through Step 2); remaining sections below.

  Planned sections (left pane):
  - **General** — launch in fullscreen, video in background, sign out, key bindings
  - **Player** — all mpv options: GPU API, hwdec, vf, deinterlace, SPDIF, video-sync, interpolation, tscale, tone mapping, target colorspace hint, opengl early flush, video latency hacks, cache size
  - **Playback** — intro skipper behaviour (always ask / always skip / never skip), auto-advance on/off (future)
  - **Appearance** — theme selection, layout variants (future)
  - **Dashboard** — show/hide individual rows (Continue Watching, Next Up, Recently Added, Not Watched), reorder them (future)
  - **Server** — link/embed to Jellyfin server admin dashboard (future)

  Build order:
  - [x] **Step 1 — Cleanup** (prerequisites): remove dead `settings-vo` row + `settings-layout-style` property; extract `dispatch_settings` + `settings_row_action` from `keys.rs` into `settings.rs`; replace magic row integers with named constants.
  - [x] **Step 2 — Two-pane shell**: left pane section list + right pane placeholder; keyboard nav wired in Rust (`settings-section: int` replaces the current flat `settings-focused`); General and Player sections populated with current rows.
  - [ ] **Step 3 — Playback section**: intro skipper mode (`always-ask` / `always-skip` / `never-skip`) stored in `Config` and `FjordState`; `playback.rs` reads the mode and either auto-skips, auto-ignores, or shows the skip-intro prompt; toggle in the Playback settings page.
  - [ ] **Step 4 — Appearance section**: theme tokens (`accent` colour, at minimum) selectable from a small palette; layout variants if needed.
  - [ ] **Step 5 — Dashboard section**: per-row visibility toggles and drag-to-reorder for the home/movies/TV dashboard rows; stored in `Config`.
  - [ ] **Step 6 — Server section**: open the Jellyfin server admin web UI (launch browser or embed WebView).

**Code review bug fixes (June 2026):**
- [ ] **[CRITICAL] Player settings wiped on re-login after sign-out** — `do_login` calls `load_config().unwrap_or_default()`, but sign-out deletes `config.json` first, so re-login produces `Config::default()` and `s.config = cfg` overwrites all saved player settings (hwdec, vf, gpu_api, etc.) with defaults. Fix: read existing player settings from the live in-memory `s.config`, then patch only the auth fields (`server_url`, `user_id`, `token`, `device_id`) onto it before saving. (`auth.rs:37,65`)
- [ ] **[HIGH] Sign-out does not stop active playback** — `on_sign_out` shows the login screen but never calls `invoke_stop_playback`, clears `is_playing`, or clears `has_background_player`. mpv keeps running behind the login screen with no UI path to stop it. (`main.rs`)
- [ ] **[HIGH] `item_type` never set in any of the three poster loaders** — `spawn_poster_loading`, `spawn_series_poster_loading`, and `spawn_movies_poster_loading` all build `CardItem` without populating `item_type`, overwriting the correctly-typed `items_to_model` result when posters arrive. Context-menu "View Details" on any dashboard-row or library-grid Series card calls `invoke_open_detail(id, "")` and opens a movie detail page instead of the series screen. (`poster.rs:122,191`, `movies.rs:59`)
- [ ] **[MEDIUM] Sign-out doesn't reset `settings_section`, `settings_focused`, or `keybinding_focused`** — stale values persist into the next login session, causing Settings keyboard nav to start mid-pane instead of at the left-pane entry point. (`main.rs on_sign_out`)
- [ ] **[MEDIUM] `video.lock()` inside `invoke_from_event_loop` can stall the UI thread** — the series play-from-start path (and movie/episode path + auto-advance) locks the video mutex on the Slint event-loop thread. If the GL rendering notifier holds the lock during `mpv_render_context_render`, the UI thread blocks until the render completes. (`context_menu.rs:239`, `playback.rs`)
- [ ] **[MEDIUM] "Reset to Defaults" button missing `AppState.refocus()`** — every other interactive element in `settings.slint` calls `refocus()` after its action; the keybinding-reset button is the only exception. Mouse click loses keyboard focus permanently until another action restores it. (`settings.slint:485`)
- [ ] **[LOW] `.ok()` swallows `get_item_detail` error in play-from-start** — if the network call fails, `item_type=""` and `series_id=None` are passed to `start_playback`: intro-skip timestamps are never fetched and auto-advance is silently disabled for that playback session. (`context_menu.rs:257`)
- [ ] **[DOCS] Stale comment on `context-menu-focused`** — inline comment still says `0=played 1=fav 2=play-from-start 3=resume 4=detail` (old order); actual order is `0=Resume 1=PlayFromStart 2=MarkPlayed 3=Favourite 4=ViewDetails`. (`app_state.slint:161`)

---

## Architecture notes

### mpv render API

mpv uses `vo=libmpv`. The render context (`mpv_render_context`) is created lazily inside Slint's `BeforeRendering` notifier (where the GL context is current). Two FBOs alternate each frame:

```
BeforeRendering:
  mpv_render_context_render(fbos[back])
  expose textures[back] as BorrowedOpenGLTexture → Slint draws it
  back = 1 - back

AfterRendering:
  if did_render: mpv_render_context_report_swap()   ← vsync feedback (only after a real render)
```

The update callback (`mpv_render_context_set_update_callback`) calls `invoke_from_event_loop(|| request_redraw())` to trigger continuous rendering when mpv has new frames.

### Disk cache strategy

```
~/.cache/fjord/home.json       home row data    always refresh in background
~/.cache/fjord/movies.json     full movie list  refresh once per session on grid open
~/.cache/fjord/series.json     full series list refresh once per session on grid open
~/.cache/fjord/posters/<id>    poster bytes     permanent (never expire)
```

On warm start: load all caches synchronously before `window.run()` so the window opens in the fully populated state on the first frame. Home, movie, and series posters are spawned immediately from the poster disk cache — no network fetch needed.

### Poster loading pipeline

```
Tokio worker thread:
  for each item in section:
    fetch bytes (disk cache or HTTP with 8-connection semaphore)
    decode JPEG → SharedPixelBuffer<Rgba8Pixel>   ← Send
invoke_from_event_loop:
    Image::from_rgba8(buffer)                     ← !Send, must be on UI thread
    push HomeItem with poster into VecModel
```

Sections are pushed the moment all their posters resolve (not one-by-one) to avoid mid-update flicker.

### Thread model

```
main thread       Slint event loop + GL rendering notifier
tokio runtime     API calls, poster fetch/decode, home data refresh
16 ms timer       mpv event poll (playback finished detection)
```

### Keyboard navigation state machine

```
focused-section == -1   →  sidebar mode  (Up/Down cycle tabs: 0↔1↔2↔3↔10↔11)
focused-section >= 0    →  content mode  (arrow keys navigate card grid)
show-browse == true     →  browse mode   (Up/Down navigate list; nav=3 or B shortcut)
show-library == true    →  library grid  (2D arrow nav, Enter opens item)
show-series == true     →  series screen (Up/Down episodes, Left/Right seasons)
```

Transitions:
- Sidebar: Right/Enter → content or library grid; Enter on nav=11 → quit
- Content: hold Left → stops at card 0; tap Left at card 0 → sidebar; Up at row 0 → stays in content; Backspace → sidebar
- Browse: Backspace/Escape → close browse
- Library grid: Backspace/Escape → close grid
- Series: Up at episode 0 → season row; Down → episode list; Backspace → close

---

## Deferred / future

- Gamepad / remote control — d-pad maps directly to arrow keys so keyboard nav already works; formal evdev/udev support deferred until needed
- `--htpc` / `--fullscreen` CLI flags — not needed while keyboard nav covers the use case
