# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux (initially) that plays video smoothly on NVIDIA legacy hardware using the mpv render API with real vsync feedback via `report_swap()`.

---

## Phase 1 — Foundation ✅

**Goal:** `cargo run -p fjord-app` opens a Slint window.

- [x] Verify `cargo build` succeeds with slint + libmpv2 dependencies
- [x] Slint hello-world window (`ui/main.slint` → MainWindow)
- [x] Confirm libmpv2 crate links against system libmpv correctly
- [x] Basic app structure: main loop, tracing/logging setup

---

## Phase 2 — Player ✅

**Goal:** Video plays smoothly inside the Slint window with real vsync feedback.

Implemented using **mpv render API** (`vo=libmpv` + `mpv_render_context`) — mpv never opens its own window. The render API gives mpv real vsync feedback via `report_swap()` called in Slint's `AfterRendering` notifier.

- [x] `Player` struct in `fjord-player` wrapping `libmpv2::Mpv` with `vo=libmpv`
- [x] `MpvRenderCtx` struct: render/report_swap/update callback, correct drop ordering
- [x] Double-buffered FBO: two GL texture/FBO pairs alternating each frame (forces Slint re-render by changing texture ID)
- [x] `BeforeRendering` notifier: lazy render ctx creation, FBO resize, render frame, expose as `BorrowedOpenGLTexture`
- [x] `AfterRendering` notifier: `report_swap()` for vsync feedback
- [x] 16 ms poll timer for mpv events (playback finished detection)
- [x] Audio passthrough (`audio-spdif`), hardware decode (`hwdec`), all player settings configurable
- [x] Playback reporting to Jellyfin (started / progress / stopped)

---

## Phase 3 — Jellyfin API client ✅

**Goal:** Authenticate and retrieve library content from a real Jellyfin server.

- [x] `JellyfinClient` struct with server URL + API token
- [x] `authenticate(server, username, password) → Client`
- [x] `get_all_items()` — movies + episodes, paginated parallel fetch, sorted
- [x] `direct_play_url()` — static stream URL with api_key
- [x] `get_continue_watching()`, `get_next_up()`, `get_recently_added()`, `get_unwatched()`
- [x] Playback reporting: started / progress / stopped
- [x] Persist session to `~/.config/fjord/config.json`
- [x] Auto-login on startup when saved session exists

---

## Phase 4 — UI ✅

**Goal:** Browse libraries, pick a movie, it plays.

- [x] Login screen: server URL + username + password
- [x] Searchable flat browse list (all movies + episodes)
- [x] Home screen with curated horizontal card rows (Continue Watching, Next Up, Recently Added)
- [x] Movies screen (Continue Watching, Recently Added, Not Watched rows)
- [x] TV Shows screen (Continue Watching, Next Up, Recently Added, Not Watched rows)
- [x] Poster thumbnails: fetched from Jellyfin, disk-cached, decoded off UI thread
- [x] Three playback modes: fullscreen, video-behind-UI, mini sidebar card
- [x] On-screen player controls overlay (controls bar + inline stats)
- [x] Settings screen: all mpv parameters, "Video in background", "Launch in fullscreen"
- [x] Fullscreen toggle: F / F11 hotkey + settings checkbox (applies immediately)
- [x] Sign out

**Still to do:**
- [x] Item detail page (overview, cast, runtime, etc.)
- [x] Resume from saved position (Jellyfin tracks `PlaybackPositionTicks`)
- [x] Seek bar / progress indicator in player controls (click/drag to seek, elapsed + total time)

---

## Phase 5 — HTPC Polish ✅

**Goal:** Comfortable to use from a couch with a keyboard or remote.

- [x] Basic keyboard navigation: arrow keys through card grid, Backspace to go back
- [x] Sidebar navigation: Up/Down cycle tabs, Right/Enter enters content grid
- [x] Number shortcuts: 1/2/3/S jump to tab, B opens browse list
- [x] Quit button in sidebar + Q shortcut
- [x] Quit reachable by keyboard: active-nav=11 in Up/Down cycle; Enter triggers quit
- [x] Keyboard nav polish: scroll-to-focused in browse list and card rows
- [x] Up in content grid stops at first row (only Left exits to sidebar)
- [x] Hold Left stops at card 0; single tap Left at card 0 exits to sidebar (uses KeyEvent.repeat)
- [x] Home dashboard keyboard scroll (ScrollView → Flickable + viewport-y binding)
- [x] Episode list keyboard scroll (ListView → Flickable + viewport-y binding)
- [x] TV show → season list → episode list drill-down (series keyboard nav fix: find-first-section checked sections 0–4)
- [x] Season tab strip scrolls with keyboard Left/Right (viewport-x bound to selected season)
- [x] Movies/TV library grid view: Enter from sidebar opens full poster grid; double-click also opens it
- [x] Library grid: dynamic column count based on window width (3–10 cols, card-fit exact)
- [x] Library grid posters: spawn_movies_poster_loading mirrors series poster pipeline
- [x] Dashboard rows dynamic card scaling: SectionRow card size adapts to window width (115×184 → 190×304px)
- [x] Watched markers on library cards: ✓ badge (fully watched) and progress bar (in-progress) on every MediaCard
- [x] Episode auto-advance (5 s countdown banner, cancellable, uses Jellyfin NextUp API)
- [x] Intro skip prompt (Intro Skipper plugin — `GET /Episode/{id}/IntroTimestamps`)
- [x] Subtitle track selection (list tracks, switch mid-playback)
- [x] Audio track selection
- [x] Video track selection (multiple video streams / angles)
- [x] Full player keyboard navigation (Space/K=pause, arrows=seek±10s, Shift+arrows=±30s, S/A/V=track panels, M=mute, I=stats, 0–9=jump%, Up/Down=volume)
- [x] Controls auto-hide after 3 s idle (fade + cursor hide, resets on any key/mouse move) — cursor hider is a topmost `TouchArea` with `enabled: !controls-visible`; `opacity:0` elements are still hit-testable so a per-element approach doesn't work
- [x] Mouse movement (without click) shows player controls overlay (`changed mouse-x/y` callbacks)
- [x] Click video area to pause/play
- [x] Resume background player to fullscreen with R key
- [x] Keyboard navigation in Settings screen
- [x] Search on library grid screens (Movies/TV full poster grid — typeahead filter by title)
- [x] Unseen episode count badge on series posters (`unplayed-count` pill, from `UserData.UnplayedItemCount`)
- [x] I key on dashboard card opens detail/series screen (Enter still plays directly)
- [x] Keyboard nav consistency audit: detail page scroll (ScrollView → Flickable + Up/Down), Settings Backspace/Escape exits rows, Settings Right enters rows, series season-row Enter enters episode list, settings-focused reset on tab-switch

**Resume position data freshness:**
- [ ] Fresh item fetch before playback — call `GET /Users/{userId}/Items/{itemId}` immediately before `start_playback` and use the returned `UserData.PlaybackPositionTicks` as the start position instead of `media_raw`. Fixes stale seek position for all play paths (Continue Watching row, library grid, detail page, series screen). `media_raw` is up to 6 h stale; the Continue Watching row's progress bar comes from a fresh home fetch so the two can disagree.
- [ ] Refresh home data after playback stops — call `fetch_home_data` in the background when `on_stop_playback` fires and push the result to the UI. Keeps the Continue Watching row progress bars accurate within a session without requiring an app restart.

**Startup & search architecture:**
- [ ] Server-side search — replace the browse list's client-side filter over `media_raw` with `GET /Users/{userId}/Items?searchTerm=<query>&recursive=true`. Results come from the server, always fresh, no local library needed. Debounce keystrokes before firing.
- [ ] Lazy-load the library grid — fetch the full item list only when the user opens the Movies or TV library grid, not at startup. Combined with server-side search, the full `get_all_items()` startup fetch and `items.json` cache become unnecessary, making cold starts as fast as warm starts.

**Keyboard navigation gaps:**
- [ ] Detail page button navigation — Tab/Left/Right cycles focus between Play, Resume, and secondary action buttons so every detail-page action is reachable by keyboard
- [ ] Secondary actions keyboard access — Mark Played/Unplayed, Favorite toggle, Play from Start; accessible from any card via a context menu (e.g. dedicated key like `M` or `*`) without needing a mouse
- [x] Library grid search activation — require `/` to enter search mode instead of typeahead on any keypress. Up from top row focuses the header search bar; Enter or `/` activates typing; Escape/Backspace exits search mode. All shortcuts work in navigation mode without carve-outs.
- [ ] Cast member photos on detail page — add `id` field to `CastMember`, fetch person portraits (`GET /Items/{personId}/Images/Primary`) using the same poster-loading pipeline, display above name/role in the cast row.
- [ ] Cast row keyboard navigation — Left/Right moves through cast members on detail page; Enter opens person detail screen (depends on person detail screen being built)

**Keyboard navigation refactor:**
- [ ] Audit every screen's `key-pressed` block against the universal contract (Enter/Right = enter/confirm, Backspace/Escape = back/cancel, Up/Down/Left/Right = navigate, same keys should always do the same thing). Document any deviation found and decide whether to align or intentionally keep it.
- [ ] Unify focus-entry behaviour — when switching into any screen (library grid, series, detail, settings, browse) always land on a consistent "first focused element" so the user always knows where they are after a transition.
- [ ] Unify focus-exit behaviour — every screen should reset its internal focus state (focused row, focused card, scroll position) when it closes so re-opening it starts fresh rather than at wherever the previous session left it.
- [ ] Eliminate copy-paste key-handling logic — several screens handle the same arrows/Escape/Backspace with near-identical code. Extract shared helpers or align the patterns so future changes only need to be made in one place.
- [ ] Global shortcut consistency — ensure F/F11, Q, 1/2/3, R, B are blocked or passed through uniformly across all non-player screens; currently some screens swallow them, others don't.
- [ ] Make the sidebar nav cycle fully symmetric — entering the sidebar from any screen should restore the previously focused tab (not reset to 0), so Back always returns you to where you were.
- [ ] Review and align with Phase 8 Slint split — the keyboard handler will need to move into the global `AppState` approach described in Phase 8 anyway; the refactor should be designed with that in mind to avoid doing the work twice.

---

## Phase 6 — Packaging ✅

- [x] PKGBUILD for Arch Linux (deploys via `makepkg -si` on HTPC)
- [x] Desktop file
- [x] Desktop icon — 10 SVG candidates (`assets/fjord_01.svg` … `fjord_10.svg`); PKGBUILD picks one at random (`$RANDOM % 10`) each build
- [x] `fjord.install` pacman script: `gtk-update-icon-cache`, deletes `icon-cache.kcache`, sends `org.kde.KIconLoader.iconChanged` D-Bus signal so KDE refreshes the icon live without logout
- [x] PKGBUILD strips debug symbols before install (avoids spurious `-debug` split package on HTPC)
- [x] Per-machine DeviceId: `ensure_device_id()` generates UUID from `/proc/sys/kernel/random/uuid` on first run, persisted in config

---

## Phase 7 — NVIDIA legacy fix ✅

Resolved choppy / corrupted playback on NVIDIA legacy Wayland/EGL.

- [x] Diagnose NVDEC stride corruption (GL_UNPACK_ROW_LENGTH ignored by NVIDIA legacy EGL)
- [x] Add `hwdec-image-format` setting (ineffective on this driver — kept for other platforms)
- [x] Add `vf` (video filter) setting with `format=yuv420p/yuv420p10le/nv12/p010` options
- [x] `vf=auto`: detects active decoder + bit depth at runtime, applies tight-packed format automatically
- [x] Expanded stats overlay: VID IN/OUT (pixel format), COLOR (HDR/SDR), AUD IN/OUT (passthrough detection), DISPLAY fps
- [x] Quieter logging: external crates (winit, sctk, calloop) capped at WARN

---

## Phase 8 — Code organisation

**Goal:** `main.rs` (2600 lines) and `main.slint` (3200 lines) are too large to navigate quickly. Split each into focused modules so it's obvious where to look when adding or fixing a feature.

### `fjord-app/src/` — Rust modules

The callback closures in `main.rs` all close over the same set of values (`Arc<Mutex<AppState>>`, `Arc<Mutex<VideoState>>`, `window.as_weak()`, `rt.handle()`). Move these into a shared `AppContext` struct passed into each module's wiring function so modules don't need long parameter lists.

- [ ] `config.rs` — `Config`, `AppState`, load/save config, item/home cache paths + freshness check
- [ ] `poster.rs` — `spawn_poster_loading`, `spawn_series_poster_loading`, `spawn_movies_poster_loading`, `decode_poster_buffer`, backdrop cache
- [ ] `series.rs` — `EpisodeRaw`, `open_series_screen`, `spawn_episode_thumb_loading`, season-select logic
- [ ] `stats.rs` — `update_stats_window`, all stats-formatting helpers
- [ ] `playback.rs` — `VideoState`, `start_playback`, FBO/GL helpers, mpv event-poll timer wiring
- [ ] `main.rs` — entry point + callback wiring only (imports everything above)

### `fjord-app/ui/` — Slint components

Slint explicitly supports splitting via relative imports (already used for `theme.slint`) and `global` singletons accessible from any file without property threading. The strategy: move all shared screen state out of `MainWindow` properties into a `global AppState { ... }` so split-out components can read/write state directly without needing properties passed down. The keyboard handler stays in `main.slint` and writes to the global; screen components read from it.

- [ ] `app_state.slint` — `global AppState`: all screen-mode flags (`is-playing`, `show-series`, `show-detail`, `show-library`, `show-browse`, `focused-section`, `focused-card`, `active-nav`, `settings-focused`, etc.) currently on `MainWindow`
- [ ] `player.slint` — fullscreen player, controls bar, stats overlay, track-select panels; reads `AppState.is-playing`
- [ ] `series.slint` — series drill-down screen (season tabs + episode list); reads/writes `AppState.show-series`
- [ ] `detail.slint` — item detail page (overview, cast, backdrop); reads/writes `AppState.show-detail`
- [ ] `home.slint` — `HomeDashboard`, `DashboardScreen` (Movies/TV), `SectionRow` card row component
- [ ] `browse.slint` — browse/search list overlay; reads/writes `AppState.show-browse`
- [ ] `settings.slint` — settings screen; reads/writes `AppState.settings-focused`
- [ ] `main.slint` — `MainWindow` shell: imports all components, keyboard handler (writes to `AppState`), callback declarations
- [ ] Update `CLAUDE.md` with the `AppContext` struct pattern (what it contains, how modules receive it) so the convention is documented for future additions

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
~/.cache/fjord/items.json      library list     < 6 h → skip network fetch
~/.cache/fjord/home.json       home row data    always refresh in background
~/.cache/fjord/posters/<id>    poster bytes     permanent (never expire)
```

On warm start: load all three caches synchronously before `window.run()` so the window opens in the fully populated state on the first frame.

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
focused-section == -1   →  sidebar mode  (Up/Down cycle tabs: 0↔1↔2↔10↔11)
focused-section >= 0    →  content mode  (arrow keys navigate card grid)
show-browse == true     →  browse mode   (Up/Down navigate list)
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

## Known bugs (code review findings)

- [x] **auth.rs — hardcoded DeviceId at login:** `AUTH_HEADER` embeds a static `DeviceId` for the `POST /Users/AuthenticateByName` call. `JellyfinClient::auth_header()` correctly uses `self.device_id` (the per-install UUID) for all subsequent calls, but the login request uses the wrong ID. Two machines share the same DeviceId at login time → Jellyfin invalidates the other machine's session. Fix: pass `device_id: &str` into `authenticate()` and interpolate it instead of `AUTH_HEADER`.

- [x] **main.rs — `on_resume_player` registered twice:** Second registration (line ~2490) replaces the first. The second handler omits `set_controls_visible(true)` and the `has_background_player` guard that the first had. Result: resuming from background mode leaves the controls bar invisible. Fix: delete the second registration; merge `set_has_background_player(false)` into the first handler.

- [x] **main.rs — `report_swap()` called without a preceding render:** `AfterRendering` calls `ctx.report_swap()` whenever `render_ctx` is `Some`, even on frames where `BeforeRendering` returned early without calling `ctx.render()` (e.g. FBO allocation failure). Gives mpv a false vsync signal → corrupts timing model, can desync A/V with `display-resample`. Fix: set a `did_render: bool` flag in `BeforeRendering` and gate `report_swap()` on it.

- [x] **mpv.rs — FBO leak on partial allocation failure:** If `create_fbo` succeeds for `fbos[0]` but fails for `fbos[1]`, the cleanup path calls `delete_fbo(vs.fbos[0])` which deletes GL object 0 (a no-op) rather than the newly created FBO, leaking a texture and FBO per failed resize. Fix: capture the allocated IDs from the partial-success branch and delete them before returning.

---

## Deferred / future

- Gamepad / remote control — d-pad maps directly to arrow keys so keyboard nav already works; formal evdev/udev support deferred until needed
- `--htpc` / `--fullscreen` CLI flags — not needed while keyboard nav covers the use case


