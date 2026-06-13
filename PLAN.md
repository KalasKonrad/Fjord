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
- [x] Controls auto-hide after 3 s idle (fade + cursor hide, resets on any key/mouse move)
- [x] Mouse movement (without click) shows player controls overlay (`changed mouse-x/y` callbacks)
- [x] Click video area to pause/play
- [x] Resume background player to fullscreen with R key
- [ ] Keyboard navigation in Settings screen
- [x] Search on library grid screens (Movies/TV full poster grid — typeahead filter by title)
- [x] Unseen episode count badge on series posters (`unplayed-count` pill, from `UserData.UnplayedItemCount`)

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

**Goal:** `main.rs` and `main.slint` are too large to navigate quickly. Split each into focused modules so it's obvious where to look when adding or fixing a feature.

### `fjord-app/src/` — Rust modules

- [ ] `config.rs` — `Config`, `AppState`, load/save config, item/home cache paths + freshness check
- [ ] `poster.rs` — `spawn_poster_loading`, `spawn_series_poster_loading`, `spawn_movies_poster_loading`, `decode_poster_buffer`, backdrop cache
- [ ] `series.rs` — `EpisodeRaw`, `open_series_screen`, `spawn_episode_thumb_loading`, season-select logic
- [ ] `stats.rs` — `update_stats_window`, all stats-formatting helpers
- [ ] `playback.rs` — `VideoState`, `start_playback`, FBO/GL helpers, mpv event-poll timer wiring
- [ ] `main.rs` — entry point + callback wiring only (imports everything above)

### `fjord-app/ui/` — Slint components

- [ ] `player.slint` — fullscreen player, controls bar, stats overlay, track-select panels
- [ ] `series.slint` — series drill-down screen (season tabs + episode list)
- [ ] `detail.slint` — item detail page (overview, cast, backdrop)
- [ ] `home.slint` — `HomeDashboard`, `DashboardScreen` (Movies/TV), `SectionRow` card row component
- [ ] `browse.slint` — browse/search list overlay
- [ ] `settings.slint` — settings screen
- [ ] `main.slint` — `MainWindow` shell: imports all components, wires properties/callbacks, keyboard handler

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
  mpv_render_context_report_swap()   ← vsync feedback
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

## Deferred / future

- Gamepad / remote control — d-pad maps directly to arrow keys so keyboard nav already works; formal evdev/udev support deferred until needed
- `--htpc` / `--fullscreen` CLI flags — not needed while keyboard nav covers the use case