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
| 10 — Code review (2026-06-18) | CR-1–10: stale intro/credits tasks, Up Next short-clip guard, report ordering, pause desync, semaphore bypass, auto-login timeout, context-menu stale state, missing SeriesId, NW timer stamp, countdown TOCTOU. CL-1–6: reset_playback_ui helper, cache_path helper, generic load/save_cache, context-menu state helper, fetch_image_cached, dead stats branch. UI-1–6: episode right-click, browse right-click, TrackPanel extract, dbl-click fullscreen, "Series →" button, seek-drag throttle + commit. |

---

## Under Investigation

Do not implement fixes for these without HTPC reproduction data first.

- **#39 — Audio dropout when vsync=audio with bitstream passthrough** — root cause unknown. To diagnose: reproduce on HTPC with stats overlay open (`I` key) during TrueHD/DTS-HD passthrough playback. Watch the SPEED row — a spike in `audio-speed-correction` at dropout time confirms AO clock drift. Also try `desync` in Settings → Player → Video sync; if dropouts stop, `video-sync=audio` is the culprit.
- **#38 — Massive frame drops with vsync=audio (intermittent)** — sporadic large spike in dropped frames, recovered by switching vsync mode. Not reproduced since filing — may be resolved. Capture stats if it recurs.

---

## Open Work

### Bug fixes (2026-06-20 code review)

- [x] **#CR3-1 — Hidden VID_VIDEO_LATENCY_HACKS row still activated by keyboard** (`settings.rs`) — If `settings-focused=9` (VID_VIDEO_LATENCY_HACKS) and the user changes `video-sync` away from `display-resample` while still in the right pane, row 9 becomes hidden but `sf` stays at 9. Pressing Enter or Right/Left invisibly toggles the hidden setting. Fix: add a visibility guard in `settings_row_action` that skips VID_VIDEO_LATENCY_HACKS when `video_sync != display-resample`.
- [x] **#CR3-2 — `settings-dropdown-open` not cleared when navigating away from Settings** (`keys.rs:nav_to, sidebar_nav`) — Global nav shortcuts (1/2/3/S) call `nav_to` before `dispatch_settings` is ever reached, so the open dropdown flag persists. On returning to Settings the stale overlay renders and the first Enter/Down fires inside the popup handler writing a value to an unrelated row. Fix: add `g.set_settings_dropdown_open(false)` to both `nav_to` and `sidebar_nav`.
- [x] **#CR3-3 — All SPDIF formats unchecked → empty passthrough string but UI warning still shows** (`config.rs:player_config`) — When the SPDIF master toggle is on but all five per-format bools are false, `f.join(",")` produces `""` and mpv never receives the `audio-spdif` option — no actual passthrough — but `settings.slint` still shows the "⚠ passthrough + display-resample" warning because it checks only the master bool. Fix: gate the warning on at least one format being selected, or disable the all-unchecked state in the UI.
- [x] **#CR3-4 — `seek-dragging` stuck if Wayland compositor delivers mouse-up to another surface** (`player.slint`, `controls.rs`) — `pointer-event(up)` handles out-of-bounds releases within the window but if the compositor steals pointer capture (another app grabs focus while the button is held), the up event never reaches `seek-ta`. `seek-dragging` stays `true` on AppState for the rest of the playback session, silently blocking Space/K/P. Fix: also clear `seek-dragging` in the Rust `on_seek_committed` callback as a safety path.
- [x] **#CR3-5 — `deser_deinterlace` fails on `null` JSON value, losing all settings** (`config.rs`) — A `null` value for the `deinterlace` field (hand-edited config) causes the `#[serde(untagged)] BoolOrStr` to fail deserialization, returning an error that makes `load_config` return `None` — all settings wiped, user forced to re-authenticate. Fix: add a `Null` variant or use `Option<BoolOrStr>` and map `None` to `"no"`.

### Cleanup (2026-06-20 code review)

- [x] **#CR3-6 — Language list duplicated between `LANG_MODEL` and cycling path** (`settings.rs:~428`) — `settings_row_action`'s Left/Right cycling for AUD_AUDIO_LANG / PLY_SUB_LANG / PLY_SUB_LANG2 hard-codes the same 20-element slice that `LANG_MODEL` already defines. Currently identical, but adding a language to `LANG_MODEL` won't update the cycling path. Fix: replace all three hard-coded slices with `LANG_MODEL`.
- [x] **#CR3-7 — `settings.rs` header not updated for new `AUD_SPDIF_*` constants** — Five new constants (`AUD_SPDIF_AC3` through `AUD_SPDIF_TRUEHD`) are absent from the file header. Update to list all audio row constants.
- [x] **#CR3-8 — `default_true()` duplicates `default_sub_enabled()`** (`config.rs`) — Both functions return `true` with no semantic difference. Consolidate to one.
- [x] **#CR3-9 — CLAUDE.md Settings row table stale** — Two errors: (1) `tone-mapping(6), target-colorspace(7)` is reversed — actual constants are `VID_TARGET_COLORSPACE=6`, `VID_TONE_MAPPING=7`; (2) Audio section lists `SPDIF(0), audio-lang(1)` but there are now seven rows: SPDIF(0), AC3(1), EAC3(2), DTS(3), DTS-HD(4), TrueHD(5), audio-lang(6). Update CLAUDE.md Settings section row table.

---

### Settings — remaining steps

- [ ] **Step 3 — Playback section**: intro skipper mode (`always-ask` / `always-skip` / `never-skip`) in `Config`; `playback.rs` reads mode; toggle in Settings → Playback.
- [ ] **Step 4 — Appearance section**: accent colour selection from a small palette; layout variants if needed.
- [ ] **Step 5 — Dashboard section**: per-row visibility toggles for home/movies/TV rows; stored in `Config`.
- [ ] **Step 6 — Server section**: open Jellyfin server admin web UI (launch browser or embed WebView).

---

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
