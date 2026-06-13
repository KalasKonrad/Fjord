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
- [ ] Secondary actions keyboard access — Mark Played/Unplayed, Favorite toggle, Play from Start; accessible from any card via a context menu (e.g. dedicated key like `M` or `*`) without needing a mouse.
- [ ] Cast member photos on detail page — add `id` field to `CastMember`, fetch person portraits (`GET /Items/{personId}/Images/Primary`) using the same poster-loading pipeline, display above name/role in the cast row.
- [ ] Cast row keyboard navigation — Left/Right moves through cast members on detail page; Enter opens person detail screen (depends on person detail screen being built).

**Settings screen keyboard nav bugs:**
- [x] Focus highlight drifts when tscale row is hidden — rows 12–17 in `row-approx-y` assume tscale is always present; when `settings-interpolation` is off the highlight is ~70px below the actual row.
- [x] Focus highlight drifts when SPDIF conflict warning appears — the conditional warning text between rows 1 and 2 pushes rows 2–17 down ~35px, not reflected in `row-approx-y`.
- [x] Fix root cause: replace `row-approx-y` hardcoded lookup table with named row elements so scroll/highlight positioning reads actual layout `.y` values instead of approximations.

**Keyboard nav bugs — global shortcut blockage:**
- [x] Series screen: `return reject` at end of series block swallows F/Q — can't toggle fullscreen or quit while series screen is open.
- [x] Detail page: `return reject` at end of detail block swallows F/Q — same problem on detail page.
- [x] Library header-focused mode: `return reject` at end of header block swallows F/Q — can't toggle fullscreen while search header is focused.
- [x] Settings: Down at row 17 (Sign Out) falls through to the global Down handler — cycles the sidebar from Settings (nav=10) to Quit (nav=11) and resets settings-focused. Guard `settings-focused < 17` needs to return accept to stay put.

**Keyboard navigation refactor:**
- [ ] Audit every screen's `key-pressed` block against the universal contract (Enter/Right = enter/confirm, Backspace/Escape = back/cancel, Up/Down/Left/Right = navigate). Document any deviation and decide whether to align or intentionally keep it.
- [ ] Unify focus-entry behaviour — when switching into any screen always land on a consistent "first focused element".
- [ ] Unify focus-exit behaviour — every screen should reset its internal focus state when it closes so re-opening starts fresh.
- [ ] Eliminate copy-paste key-handling logic — extract shared helpers or align patterns so future changes only need to be made in one place.
- [ ] Global shortcut consistency — ensure F/F11, Q, 1/2/3, R, B are blocked or passed through uniformly across all non-player screens.
- [ ] Make the sidebar nav cycle fully symmetric — entering the sidebar from any screen should restore the previously focused tab, not reset to 0.

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

## Deferred / future

- Gamepad / remote control — d-pad maps directly to arrow keys so keyboard nav already works; formal evdev/udev support deferred until needed
- `--htpc` / `--fullscreen` CLI flags — not needed while keyboard nav covers the use case
