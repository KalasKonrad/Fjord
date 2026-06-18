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

### Bug fixes (2026-06-19 review)

- [x] **#CR2-1 — seek_drag_started reads UI is-paused flag instead of mpv state** (`controls.rs:128`) — If mpv self-pauses on a cache underrun, the UI flag stays `false`; `seek_drag_started` sets `should_resume=true` and `seek_committed` then calls `set_paused(false)`, overriding the stall. `on_pause_play_toggle` was fixed (CR-4) to query `p.is_paused()` directly — apply the same fix here.
- [x] **#CR2-2 — show-next-ep-banner rendered in both main.slint and player.slint when is-playing=true** (`main.slint:65`) — Root-level banner has no `!AppState.is-playing` guard; both banners coexist in the widget tree with different button layouts (root: Cancel only; player: Play Now + Skip). Add `if !AppState.is-playing` guard to the root-level banner or remove it.
- [x] **#CR2-3 — on_close_detail does not reset detail-scroll before hiding** (`main.rs:~570`) — `on_play_detail` and `on_resume_detail` both call `set_detail_scroll(0.0)`; `on_close_detail` does not. Next detail open starts pre-scrolled. Add `g.set_detail_scroll(0.0)` before `set_show_detail(false)`.
- [x] **#CR2-4 — context_menu_series_id read after async API call, outside item-id guard** (`context_menu.rs:219`) — The item-id guard protects only `set_context_menu_has_played`; the `series_id` read for `update_series_unplayed_count` is outside it. Rapid menu reopen for a different series causes the wrong badge to update. Capture `series_id` at task-spawn time instead of re-reading inside `invoke_from_event_loop`.
- [x] **#CR2-5 — report_playback_* errors silently swallowed** (`playback.rs:379,482,789`) — All six call sites use `let _ = …await` with no error logging. A 401 or network failure is never surfaced; Jellyfin never records the final position. Add at least a `warn!` on error.
- [x] **#CR2-6 — recently_added_tv duplicates recently_added fetch** (`home.rs:78`) — Both call `get_recently_added(Some("Series"))`; fields are identical on every refresh, wasting a network round-trip. Fix the filter on one or deduplicate to a single shared fetch.

### Performance (2026-06-19 review)

- [ ] **#CR2-7 — VideoState mutex held across entire GL BeforeRendering callback** (`playback.rs:556`) — Lock held through `ctx.render()` and `poll_stats()` (31 synchronous mpv IPC reads every 500 ms); the 16 ms timer locks the same mutex every tick, blocking during the IPC reads. Fix: release before `ctx.render()` or move `poll_stats` off the GL thread.
- [ ] **#CR2-8 — poll_stats() runs unconditionally every 500 ms even when stats overlay is hidden** (`playback.rs:637`) — 31 mpv property reads with no `stats-visible` guard. Add an early-out when the overlay is hidden; combines with #CR2-7 to eliminate the lock-contention window during normal playback.

### Cleanup (2026-06-19 review)

- [x] **#CR2-9 — open_series_screen inlines decode_poster_buffer twice instead of calling the helper** (`series.rs:~192`) — The file already imports `decode_poster_buffer` and uses it in `spawn_episode_thumb_loading`; `open_series_screen` re-implements the same pipeline inline for both poster and backdrop. Replace with calls to the helper.
- [ ] **#CR2-10 — Up Next countdown task spawned with no cancellation token** (`playback.rs:868`) — Rapid episode skips spawn a new countdown task each time; the old task self-exits within ≤1 s but briefly overlaps. Add a `CancellationToken` or reuse `playback_generation` so the previous task exits immediately on a new episode start.

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

- Gamepad / remote control — d-pad maps to arrow keys; formal evdev/udev support deferred
- Person detail screen (depends on cast row nav above)
- Dashboard row reorder (drag-to-reorder, Phase 5 Step 5)
