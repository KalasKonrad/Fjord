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
- [ ] Item detail page (overview, cast, runtime, etc.)
- [x] Resume from saved position (Jellyfin tracks `PlaybackPositionTicks`)
- [ ] Seek bar / progress indicator in player controls

---

## Phase 5 — HTPC Polish

**Goal:** Comfortable to use from a couch with a keyboard or remote.

Keyboard navigation is rudimentarily implemented — basic arrow key / section
movement works but is not polished.

- [x] Basic keyboard navigation: arrow keys through card grid, Backspace to go back
- [x] Sidebar navigation: Up/Down cycle tabs, Right/Enter enters content grid
- [x] Number shortcuts: 1/2/3/S jump to tab, B opens browse list
- [x] Quit button in sidebar + Q shortcut
- [ ] Keyboard nav polish: scroll-to-focused in browse list and card rows, smoother section transitions
- [ ] Gamepad / remote control support (map d-pad to arrow keys)
- [ ] TV show → season list → episode list drill-down
- [ ] Episode auto-advance
- [ ] Subtitle track selection (list tracks, switch mid-playback)
- [ ] Audio track selection
- [ ] Search on home/library screens (not just browse)

---

## Phase 6 — Packaging

- [x] PKGBUILD for Arch Linux (deploys via `makepkg -si` on HTPC)
- [x] Desktop file
- [ ] Desktop icon (currently uses generic `multimedia-player`)
- [ ] `--htpc` / `--fullscreen` command line flags

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
focused-section == -1   →  sidebar mode  (Up/Down cycle tabs)
focused-section >= 0    →  content mode  (arrow keys navigate card grid)
show-browse == true     →  browse mode   (Up/Down navigate list)
```

Transitions:
- Sidebar: Right/Enter → content (find-first-section)
- Content: Up at row 0 → sidebar; Left at card 0 → sidebar; Backspace → sidebar
- Browse: Backspace/Escape → close browse
