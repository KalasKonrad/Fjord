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

---

### Code review findings (2026-06-18)

**Correctness bugs — fix in priority order:**

- [x] **#CR-1 — Stale intro/credits tasks overwrite VideoState** (`playback.rs:413`) — Tokio tasks spawned for episode A carry no `item_id` guard; if they resolve slowly they unconditionally write `vs.intro_timestamps` / `vs.credits_start` after episode B has started. Fix: store `current_item_id` in `VideoState` before spawning; guard each write with `if vs.current_item_id == fetched_for_id`.
- [x] **#CR-2 — Up Next fallback fires immediately on short clips** (`playback.rs:768`) — `dur - pos <= 30.0` has no minimum-duration guard; any clip shorter than 30 s triggers the banner at second 0. Fix: add `&& dur >= 60.0` (or similar) to the fallback_fire condition.
- [x] **#CR-3 — report_playback_start sent before previous episode stopped** (`playback.rs:391`) — start report for the new item is spawned before `tear_down_player` stops the old one; Jellyfin briefly sees two concurrent sessions and may fail to save the previous episode's resume position. Fix: move `report_playback_start` to after teardown completes.
- [x] **#CR-4 — Pause state desync on mpv self-pause** (`controls.rs:33`) — `pause_play_toggle` inverts the Slint UI flag instead of querying mpv's actual state; if mpv self-pauses (cache underrun), subsequent Space presses are one phase off. Fix: query mpv property `pause` to derive the new UI state rather than inverting the cached flag.
- [x] **#CR-5 — Semaphore permit silently bypassed on closed semaphore** (`poster.rs:94`) — `acquire_owned().await.ok()` returns `None` when the semaphore is closed; `_permit = None` means no permit is held and all remaining fetch tasks run unlimited. Fix: use `let Ok(permit) = sem.acquire_owned().await else { return }` to bail on closed semaphore.
- [x] **#CR-6 — Auto-login API calls have no timeout** (`auth.rs:54`) — `tokio::join!` over `fetch_home_data`, `get_all_series`, `get_system_info` has no timeout; a server that accepts TCP but drops packets hangs the task forever with no error surfaced. Fix: wrap in `tokio::time::timeout` or set a timeout on the `reqwest::Client`.
- [x] **#CR-7 — context_menu_has_played set for wrong item on rapid navigation** (`context_menu.rs:155`) — the `invoke_from_event_loop` closure for mark-played doesn't check that the context menu is still open for the same item; rapid open→mark→open-different-item overwrites the second item's displayed played state. Fix: capture `item_id` in the closure and compare against `context_menu_item_id` before calling `set_context_menu_has_played`.
- [x] **#CR-8 — Missing SeriesId permanently disables Up Next for that session** (`context_menu.rs:257`) — if Jellyfin omits `SeriesId` on an episode, `series_id=None` flows into `start_playback` → `vs.playing_series_id=None`; the banner trigger guard `playing_series_id.is_some()` is always false. Fix: log a warning when `series_id` is None for an Episode item type; consider falling back to a series lookup by name.
- [x] **#CR-9 — Not-Watched timer stamps cooldown before fetch, silencing errors** (`home.rs:176`) — `last_nw_mov_refresh` is set before the async task runs; a network error causes the task to return early while the timestamp is already written, resetting the 10-minute cooldown with no retry and no user feedback. Fix: stamp the timestamp only after a successful fetch.
- [x] **#CR-10 — TOCTOU double-lock in Up Next countdown task** (`playback.rs:842`) — `player.is_some()` and `next_ep_pending.is_some()` are read under two separate `video2.lock()` calls; the 16 ms timer can tear down the player and take `next_ep_pending` between the two acquires, causing the countdown to call `.take()` on an already-consumed pending. Fix: merge both reads into a single lock scope.

---

### Cleanup (2026-06-18)

- [x] **#CL-1 — Extract `reset_playback_ui()` helper** (`playback.rs`) — 16 identical AppState setters are copy-pasted between `do_stop_playback` and the natural-end block in `wire_mpv_timer`. Also fixes a latent bug: the natural-end path never resets `active_nav` 4→0, leaving mini-card nav stuck when playback ends naturally.
- [x] **#CL-2 — Single `cache_path(filename)` helper** (`home.rs`) — The same 6-line XDG_CACHE_HOME resolution block is duplicated verbatim in `home_cache_path`, `movies_cache_path`, and `series_cache_path`; they differ only in the final filename.
- [x] **#CL-3 — Generic `load_cache<T>` / `save_cache<T>`** (`home.rs`) — Six near-identical load/save functions differ only in type and path call; collapse into two generic functions with `serde::Serialize` / `DeserializeOwned` bounds.
- [x] **#CL-4 — `open_context_menu_state()` helper** (`context_menu.rs`) — The same 7 AppState setters (including the focused-row formula `resume_pct > 0.0 && !has_played`) appear in all three `on_open_context_menu*` handlers; extract to a shared function.
- [x] **#CL-5 — Merge `fetch_poster_cached` / `fetch_backdrop_cached`** (`poster.rs`) — 95% identical functions; diverge only in path helper and API method. Consolidate via an `ImageKind` enum parameter.
- [x] **#CL-6 — Remove dead else-branch in `stats.rs` vid_out scale** (`stats.rs`) — The else branch formats `width×height` when `video_out_w == width && video_out_h == height`, producing the same string as the if-branch. Replace the entire conditional with `format!("{}×{}", s.video_out_w, s.video_out_h)`.

---

### UI Polish (2026-06-18)

- [x] **#UI-1 — Series episode right-click → context menu** (`series.slint`) — Episode `TouchArea` had only `clicked` (play); added `pointer-event` to call `open-context-menu-series-ep` on right-click, matching the existing `C` key behaviour for mouse users.
- [x] **#UI-2 — Browse list right-click → context menu** (`widgets.slint`, `browse.slint`) — `BrowseItem` had no `right-clicked` callback; added `pointer-event` + `right-clicked` wired to `open-context-menu-browse(i)`.
- [x] **#UI-3 — Extract `TrackPanel` component** (`player.slint`) — Sub/Audio/Video track panels were three identical 45-line blocks; extracted to a shared `TrackPanel` component with `title`, `tracks`, `current-id`, `has-off-row`, `container-h/w`, and `track-selected` callback. Removes ~90 lines.
- [x] **#UI-4 — Double-click to toggle fullscreen in player** (`player.slint`) — Added `double-clicked => { AppState.toggle-fullscreen(); }` to the main player `TouchArea`.
- [x] **#UI-5 — "Series →" button on episode detail page** (`app_state.slint`, `detail.slint`, `detail.rs`, `keys.rs`) — Added `detail-series-id` property (populated from episode's `SeriesId`); "Series →" button visible only for episodes; closes detail and opens the series screen. Keyboard: Left/Right cycle Play / Resume (if available) / Series; Enter activates the focused button.
- [x] **#UI-6 — Throttle seek-bar drag to prevent libmpv crash** (`controls.rs`, `app_state.slint`, `player.slint`) — `on_seek_to` was called on every mouse-move pixel during drag (hundreds/s) causing SIGABRT in libmpv. Fix: throttle `seek-to` to ≤10 seeks/s (100 ms gate via `Arc<Mutex<Instant>>`); add `seek-committed` callback that always seeks on mouse-up regardless of throttle so the final position is never dropped.

---

### Code review findings (2026-06-19)

**Correctness bugs — fix in priority order:**

- [ ] **#CR2-1 — seek_drag_started reads UI is-paused flag instead of mpv state** (`controls.rs:128`) — If mpv self-pauses on a cache underrun, the UI flag stays `false`; `seek_drag_started` incorrectly sets `should_resume=true`; `seek_committed` then calls `set_paused(false)`, overriding the stall and forcing premature resume. `on_pause_play_toggle` was fixed in CR-4 to query `p.is_paused()` directly — apply the same fix here.
- [ ] **#CR2-2 — show-next-ep-banner rendered in both main.slint and player.slint when is-playing=true** (`main.slint:65`) — Root-level banner has no `!AppState.is-playing` guard; when `PlayerScreen` is active both banners exist in the widget tree at the same position with different button layouts (root: Cancel only; player: Play Now + Skip). Add `if !AppState.is-playing` guard to the root-level banner or remove it entirely.
- [ ] **#CR2-3 — on_close_detail does not reset detail-scroll before hiding** (`main.rs:~570`) — `on_play_detail` and `on_resume_detail` both call `set_detail_scroll(0.0)` before hiding; `on_close_detail` does not. Next detail open starts pre-scrolled. Add `g.set_detail_scroll(0.0)` before `set_show_detail(false)`.
- [ ] **#CR2-4 — context_menu_series_id read after async API call, outside item-id guard** (`context_menu.rs:219`) — The item-id guard at line 208 protects only `set_context_menu_has_played`; the `context_menu_series_id` read used for `update_series_unplayed_count` is outside it. If the user opens a context menu for a different item before the mark-played response arrives, the wrong series badge is updated. Capture `series_id` at task-spawn time (same as `id2`) instead of re-reading it inside `invoke_from_event_loop`.
- [ ] **#CR2-5 — report_playback_* errors silently swallowed** (`playback.rs:379,482,789`) — All six call sites use `let _ = …await` with no error logging. A 401 or network failure during start/progress/stop reporting is never surfaced; Jellyfin never records the final position. Add at least a `warn!` on error; consider surfacing 401 as a re-auth trigger.
- [ ] **#CR2-6 — recently_added_tv duplicates recently_added fetch** (`home.rs:78`) — Both call `get_recently_added(Some("Series"))`; the two fields are identical on every home refresh. Change one to the correct filter or deduplicate to a single fetch shared by both dashboard rows.

**Performance:**

- [ ] **#CR2-7 — VideoState mutex held across entire GL BeforeRendering callback** (`playback.rs:556`) — The lock is acquired at the top of BeforeRendering and held through `ctx.render()` and `poll_stats()` (31 synchronous mpv IPC reads every 500 ms). The 16 ms timer locks the same mutex on every tick; during a poll_stats call the timer thread is blocked for the full IPC duration. Fix: release the lock before `ctx.render()` or move `poll_stats` off the GL thread.
- [ ] **#CR2-8 — poll_stats() runs unconditionally every 500 ms even when stats overlay is hidden** (`playback.rs:637`) — 31 mpv property reads every 500 ms with no `stats-visible` guard. Add an early-out when `AppState.stats-visible` is false; the lock-hold cost (#CR2-7) also drops to near-zero during normal playback.

**Cleanup:**

- [ ] **#CR2-9 — open_series_screen inlines decode_poster_buffer twice instead of calling the helper** (`series.rs:~192`) — The file already imports `decode_poster_buffer` from `poster.rs` and uses it in `spawn_episode_thumb_loading`; `open_series_screen` re-implements the same `image::load_from_memory → to_rgba8 → SharedPixelBuffer → Image::from_rgba8` pattern inline for both poster and backdrop. Replace both blocks with `decode_poster_buffer`.
- [ ] **#CR2-10 — Up Next countdown task spawned with no cancellation token** (`playback.rs:868`) — Rapid episode skips reset `next_ep_banner_shown` and spawn a new countdown task each time; the old task self-exits within ≤1 second (via `next_ep_pending` being cleared) but there is a brief concurrent overlap. Add a `CancellationToken` (or reuse the `playback_generation` counter) so the previous task exits immediately on a new episode start.

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


issues 
