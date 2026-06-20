# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux that plays video smoothly on NVIDIA legacy hardware using the mpv render API with real vsync feedback via `report_swap()`.

---

## Completed

| Phase | Summary |
|-------|---------|
| 1 — Foundation | Slint window, libmpv link, logging |
| 2 — Player | mpv render API, double-buffer FBO, vsync via `report_swap()`, audio passthrough, hwdec, playback reporting |
| 3 — Jellyfin API | Auth, library browse, continue watching / next up / recently added, direct-play URL, session persistence, auto-login |
| 4 — UI | Login, browse, home/movies/TV dashboards, posters, three playback modes, player controls overlay, settings screen, detail page, resume, seek bar |
| 5 — HTPC Polish | Resume freshness, server-side search, lazy library grid, full keyboard nav, context menu (`C` key + right-click), card badges, settings two-pane layout, Rust key handler with configurable bindings, disk caches (home/movies/series), `item-type` routing, canonical user state store |
| 6 — Packaging | PKGBUILD, desktop file, SVG icon, per-machine DeviceId |
| 7 — NVIDIA legacy fix | NVDEC stride diagnosis, `vf=auto` fix, expanded stats overlay |
| 8 — Code organisation | Split `main.rs`/`main.slint` into focused modules, `global AppState` singleton |
| 9 — Bug fixes & polish | Crash on background-play replacement, stop report reliability, screensaver inhibition, Up Next banner, volume overlay, intro-skip race fix, mouse hover on cards/browse, browse search mouse focus, subtitle track labels, subtitle language prefs, "Ends at" clock, settings hover, random sidebar icon, transparent SVG icons, mark-played visual update in dashboards/library |
| 10 — Code review CR1 (2026-06-18) | CR-1–10: stale intro/credits tasks, Up Next short-clip guard, report ordering, pause desync, semaphore bypass, auto-login timeout, context-menu stale state, missing SeriesId, NW timer stamp, countdown TOCTOU. CL-1–6: reset_playback_ui helper, cache_path helper, generic load/save_cache, context-menu state helper, fetch_image_cached, dead stats branch. UI-1–6: episode right-click, browse right-click, TrackPanel extract, dbl-click fullscreen, "Series →" button, seek-drag throttle + commit. |
| 11 — Code review CR3 (2026-06-20) | CR3-1–9: hidden VLH activation, stale dropdown flag, SPDIF warning with all-off formats, seek-dragging stuck on Wayland, deser_deinterlace null crash, language list duplication, header stale, default_true dedup, CLAUDE.md table errors. |
| 12 — Code review CR4 (2026-06-20) | CR4-1–10: Player::new error cleanup, JoinSet panic flush in all poster loaders, settings scroll for all sections, Up Next countdown off-by-one, movies semaphore, auto-advance window guard, mid-session 401 redirect, dropdown model dedup, dead VLH up-nav guard, .expect() in library crate. |

---

## Under Investigation

Do not implement fixes for these without HTPC reproduction data first.

- **#39 — Audio dropout when vsync=audio with bitstream passthrough** — root cause unknown. To diagnose: reproduce on HTPC with stats overlay open (`I` key) during TrueHD/DTS-HD passthrough playback. Watch the SPEED row — a spike in `audio-speed-correction` at dropout time confirms AO clock drift. Also try `desync` in Settings → Player → Video sync; if dropouts stop, `video-sync=audio` is the culprit.
- **#38 — Massive frame drops with vsync=audio (intermittent)** — sporadic large spike in dropped frames, recovered by switching vsync mode. Not reproduced since filing — may be resolved. Capture stats if it recurs.

---

## Open Work

### Settings — remaining steps

- [ ] **Step 3 — Playback section**: intro skipper mode (`always-ask` / `always-skip` / `never-skip`) in `Config`; `playback.rs` reads mode; toggle in Settings → Playback.
- [ ] **Step 4 — Appearance section**: accent colour selection from a small palette; layout variants if needed.
- [ ] **Step 5 — Dashboard section**: per-row visibility toggles for home/movies/TV rows; stored in `Config`.
- [ ] **Step 6 — Server section**: open Jellyfin server admin web UI (launch browser or embed WebView).

### Phase 5 — remaining items

- [ ] **Cast member photos on detail page** — add `id` field to `CastMember`, fetch person portraits (`GET /Items/{personId}/Images/Primary`) via poster-loading pipeline, display above name/role.
- [ ] **Cast row keyboard navigation** — Left/Right through cast members on detail page; Enter opens person detail screen.

---

## Architecture notes

### mpv render API

mpv uses `vo=libmpv`. Two FBOs alternate each frame:

```
BeforeRendering:
  mpv_render_context_render(fbos[back])
  expose textures[back] as BorrowedOpenGLTexture → Slint draws it
  back = 1 - back

AfterRendering:
  if did_render: mpv_render_context_report_swap()   ← vsync feedback
```

`MpvRenderCtx` must be dropped before `Player`. Enforced in `VideoState` and the rendering teardown path.

### Disk cache

```
~/.cache/fjord/home.json       home row data    always refresh in background
~/.cache/fjord/movies.json     full movie list  refresh once per session on grid open
~/.cache/fjord/series.json     full series list refresh once per session on grid open
~/.cache/fjord/posters/<id>    poster bytes     permanent (never expire)
```

Warm start: all caches loaded synchronously before `window.run()` — window opens fully populated on the first frame.

### Poster loading pipeline

```
Tokio worker:
  fetch bytes (disk cache or HTTP, 8-connection semaphore)
  decode JPEG → SharedPixelBuffer<Rgba8Pixel>   ← Send
invoke_from_event_loop:
  Image::from_rgba8(buffer)                     ← !Send, must be on UI thread
  push HomeItem with poster into VecModel
```

### Thread model

```
main thread       Slint event loop + GL rendering notifier
tokio runtime     API calls, poster fetch/decode, home data refresh
16 ms timer       mpv event poll, position update, intro-skip, controls idle, progress report
```

---

## Deferred / future

- **Vulkan rendering path** — second render backend alongside the current OpenGL path. Requires: Slint WGPU backend, `MpvRenderCtx` initialized with `MPV_RENDER_API_TYPE_VULKAN`, Vulkan FBO management replacing the current `gl::*` code. Enables true zero-copy decode on AMD (`hwdec=vulkan`, no CPU roundtrip). Legacy NVIDIA hardware needs OpenGL; selection persists in Config as `gpu_renderer: "opengl" | "vulkan"` and takes effect on next restart. The `gpu-api` setting was removed (2026-06-19) because it had no effect with `vo=libmpv` + OpenGL render context — this feature replaces it properly.
- Gamepad / remote control — d-pad maps to arrow keys; formal evdev/udev support deferred
- Person detail screen (depends on cast row nav above)
- Dashboard row reorder (drag-to-reorder, Phase 5 Step 5)
