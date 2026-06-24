# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

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
| 13 — EAC3 passthrough diagnosis (2026-06-20) | Root cause (#39): tsched software timer caused PipeWire RT thread to miss 21.3 ms deadline at 192 kHz IEC61937 rates under GPU load. Fix: `api.alsa.disable-tsched=true` (hardware IRQ wakeups) + `suspend-timeout-seconds=2`. Now a Settings toggle. Do not use `api.alsa.headroom` — shifts audio timeline, causes frame drops under `display-vdrop`. |
| 14 — Settings: SPDIF per-format toggles, HDR passthrough row, virtual rows (2026-06-21) | Per-format SPDIF toggles (AC3/EAC3/DTS/DTS-HD/TrueHD) replace single passthrough switch. Tone-mapping row hidden when HDR passthrough on. Video-latency-hacks row hidden unless display-resample active. Cross-section passthrough+display-resample conflict warning. |
| 15 — Audio output device selector (2026-06-21) | Dropdown in Settings → Audio populated from `mpv --audio-device=help`. Device stored in `Config.audio_device`; applied to mpv at playback start. Content-driven popup width; keyboard nav fixed. |
| 16 — PipeWire IRQ scheduling toggle (2026-06-21) | Settings → Audio toggle (visible when SPDIF on + PipeWire/auto device). Writes/deletes `~/.config/wireplumber/wireplumber.conf.d/fjord-alsa-irq.conf` and restarts WirePlumber on change. Config persists after exit; syncs down to false on startup if file missing. |
| 17 — Intro Skipper API v2 + generalized skip segments (2026-06-21) | Migrated from two old endpoints to single `GET /Episode/{id}/Timestamps` returning all 5 segment types. `EpisodeTimestamps` model with `Segment { start, end }` for Introduction, Credits, Recap, Preview, Commercial. Single generic skip overlay replaces `show-skip-intro`. Timer checks segments in priority order (Intro → Recap → Preview → Commercial). Up Next banner: `next-ep-banner-focused` (0=Play Now, 1=Skip); Left/Right toggle, Enter activates. |
| 18 — Per-segment skip modes + configurable timers (2026-06-21) | Each of Intro/Recap/Preview/Commercial has 4 modes: `always-skip`, `ask`, `ask-timed` (auto-skips on countdown expiry), `never-skip`. Credits has 3 modes. All 10 new `Config` fields persisted in JSON. Settings → Player section extended with rows 4–13 + INTRO SKIPPER / CREDITS section headers. `VideoState` extended with `skip_segment_handled`, `skip_timed_shown_at`, `skip_timed_prompt_secs`. |
| 19 — Movie detail enrichment: cast, collection, similar (2026-06-21) | Director, writer, tagline, studio; `BackdropHero`/`PosterBlock`/`MetaLine` atoms extracted to `widgets.slint`. `CastRow` atom with async portrait fetch. `fetch_movie_collections` background task builds BoxSet membership map; collection `SectionRow`. `get_similar_items` "More Like This" `SectionRow`. |
| 20 — CR5: post-enrichment bug fixes (2026-06-21) | `fetch_movie_collections` now spawned in auto-login path. Portrait index mismatch fixed. `SectionRow.item-play` passes `item_type`. CastRow focus ring visibility fix. Back from detail resets stale collection/similar models. BoxSet items sorted by ProductionYear. |
| 21 — Detail page keyboard nav + stop-returns-to-detail (2026-06-21) | `open_detail` calls `invoke_grab_keyboard_focus()`. `VideoState.from_detail` flag restores detail page on stop/natural-end. `on_minimize_player` clears flags. `main.slint` hides DetailPage when `is_playing`. |
| 22 — Series season episode cache + race fix (2026-06-22) | Season episodes cached in `FjordState.series_episode_cache`; cleared on series switch. `series_season_generation` counter prevents stale async results from rapid tab navigation. |
| 23 — Series detail UX polish (2026-06-22) | "✓ Watched" button on series detail. `PosterBlock` extended with played/resume/unplayed badges. Season row focus indicator (accent bottom border). C key on Next Up card. Default focus on episode row; Next Up steals focus when data arrives. |
| 24 — Back button + series/detail header keyboard nav (2026-06-22) | `series-focused-btn` (-1=not in header, 0=Back, 1=♥, 2=✓). Detail `detail-focused-btn = -1` = Back. Season `season-focused-back`. All Back buttons gain `kbd-focused` ring. |
| 25 — Crash fix: series screen "Recursion detected" (2026-06-23) | `kb-x` in season tabs replaced `self.width` with `root.width` to break layout cache re-entrancy cycle. |
| 26 — UI polish: backdrop header, icon circle buttons, ends-at, load-then-show (2026-06-23) | "Ends HH:MM" below action buttons. `IconCircleButton` component (38 px circle). Backdrop fills header block height. `open_detail`/`open_series_screen` defer show until `spawn_main` completes; loading overlay with spinner + progress bar. |
| 27 — UI polish: icon centering, spinner size, portrait preload, progress bar (2026-06-23) | `IconCircleButton` text centred (explicit width/height), font-size 20 px. Spinner dots 14 px. Cast portraits fetched before page shown (no trickle-in). `app-loading-progress` property; 240 px animated progress bar in loading overlay. |
| 28 — Person detail screen (2026-06-23) | Enter on any cast member opens PersonScreen: portrait + bio + filmography SectionRow. `AppMode::Person` (priority above Detail). `get_person_filmography` API endpoint. `CastRow.item-selected` callback wired from detail/series/season screens. Mouse click on cast card also opens person. `close-person` Back button + keyboard Back. Loading overlay (spinner + progress bar) matching deferred show pattern of detail/series screens. |
| 29 — Player: minimal keyboard-pause bar (2026-06-23) | Space-to-pause no longer reveals the full controls bar. Instead shows a slim 52 px minimal bar (seek progress + current/total time + "Ends at") via `pause-bar-visible` flag. Space-to-resume immediately hides both the minimal bar and the full controls (even if the full bar was open from mouse). Mouse click resume also clears `pause-bar-visible`. |
| 30 — Player: seek accumulation OSD (2026-06-23) | Keyboard Left/Right accumulate into a debounced seek instead of seeking immediately. OSD shows direction + total delta + target time ("▶▶ +20s → 1:23:45"). Seek executes ~480 ms after the last key press. Rapid presses add up. Mouse button seeks remain immediate. |
| 31 — Mini-player bar redesign (2026-06-24) | Replaced sidebar "Now Playing" card with `MiniPlayerBar`: full-width bar docked at top, window-aware (all screens offset by `bar-h`). Mode 3 (!video-behind-ui): 108px with live thumbnail + title + buttons. Mode 2 (video-behind-ui): 56px compact bar (no thumbnail, video fills window) with NOW PLAYING label + title + FjordButton Resume/Stop. Video-behind-UI uses dual video layers in main.slint: layer 1 (z=below AppShell) shows video through transparent screen roots on dashboard/library; layer 2 (z=above AppShell, only when overlay open) prevents library cards from ghosting through detail/series/season/person screens. Dim overlay #00000044 (~27%). `float-card-focused` (-1/0/1); `focus_bar_on_up` called at end of every mode arm — Up returns `false` from all screens when at topmost position (Back button in detail/season, header in person, no-prev-section in dashboard, search header in library) so the bar is reachable via Up from any screen. |

---

## Pending — Code Review CR6 (2026-06-24)

Items found during full-codebase review. Listed individually with rationale.

---

### ~~🔴 CR6-1 — Sign-out leaves stale state that bleeds into the next session~~ ✓ Fixed
**File:** `crates/fjord-app/src/main.rs` lines 1224–1250

`on_sign_out` clears the movie and series lists but misses three things. First, `series_episode_cache` (the in-memory map of cached episode lists) is never cleared — so if you sign back in as a different user on a different server, the episode cache still holds data from the previous account. Second, `movie_collections` (the BoxSet membership map built from the previous user's library) is also left populated, so the collection row on detail pages could show the wrong collection. Third, `movies_fetched` is left `true`, so the next login never re-fetches movies from the new server.

On the AppState / UI side, overlay flags (`show_detail`, `show_series`, `show_season`, `show_person`, `show_library`, `show_next_ep_banner`, `has_background_player`) are not reset. If you're watching a video and sign out, the video player stops (correct), but the detail/series/season screens remain open and show behind the login form.

**Fix:** Add these to the sign-out block:
```rust
s.series_episode_cache.clear();
s.movie_collections.clear();
s.movies_fetched = false;
g.set_show_detail(false); g.set_show_series(false); g.set_show_season(false);
g.set_show_person(false); g.set_show_context_menu(false); g.set_show_library(false);
g.set_show_next_ep_banner(false); g.set_has_background_player(false);
```

---

### ~~🔴 CR6-2 — Missing `HOME` env var silently writes auth token to the wrong path~~ ✓ Fixed
**Files:** `crates/fjord-app/src/home.rs` line 44, `crates/fjord-app/src/config.rs` (same pattern)

Both files do `std::env::var("HOME").unwrap_or_default()`. If `HOME` is unset (e.g. a systemd unit or a misconfigured environment), `unwrap_or_default()` returns an empty string, so the path becomes `.config/fjord/config.json` relative to the current working directory. The write succeeds silently. On the next launch from a different working directory (a `.desktop` launcher, a terminal in a different folder), the config file is not found and the app shows the login screen — the user's saved session appears lost with no explanation.

**Fix:** Use `dirs::home_dir()` (already a transitive dependency) or return an error when `HOME` is empty. At minimum log a clear error rather than silently using a relative path.

---

### ~~🟠 CR6-3 — Auto-advance race between the countdown task and the natural-end path~~ ✓ Fixed
**File:** `crates/fjord-app/src/playback.rs` lines 1283–1293

When the countdown task wakes from its per-second sleep and finds `vs.player.is_none()` (the video ended naturally while it was sleeping), it sets `vs.next_ep_pending = None` and exits. A moment later the natural-end path in the 16 ms timer fires, takes the lock, calls `next_ep_pending.take()` — and gets `None` because the countdown task already cleared it. The episode advance is silently lost and the series stops after one episode.

The window is narrow but real on any CPU load spike (the countdown task sleeping slightly longer than expected, while the natural-end path fires in the same interval).

**Fix:** Remove `vs.next_ep_pending = None` from the countdown task's `!still_playing` branch entirely. The countdown task's job is only to count down and advance; when it finds the player gone it should just `return` and trust the natural-end path to handle `next_ep_pending`. Only the natural-end path should `take()` the pending episode.

---

### 🟠 CR6-4 — Movie library silently truncated for large libraries
**File:** `crates/fjord-api/src/client.rs` line 455

`get_all_movies` uses `Limit=10000`. Jellyfin servers impose their own `MaxPageSize` (commonly 5,000 on default installs). The API silently returns `min(10000, server_max)` items with no error — there is no flag in the response saying "there are more items you didn't get." A user with more than the server's page size worth of movies will see a partial library with no warning.

**Fix:** After fetching, compare `items.len()` against `response.total_record_count`. If `items.len() < total_record_count`, either log a clear warning or implement `StartIndex`-based pagination (like `get_all_series` should also use) to fetch all pages.

---

### 🟠 CR6-5 — Season screen shows before cast portraits are loaded (inconsistent with detail/series)
**File:** `crates/fjord-app/src/season.rs` line 47

`set_show_season(true)` fires immediately on the UI thread, then the async task fetches cast portraits one-by-one and trickles them into the model via separate `invoke_from_event_loop` calls. This produces blank portrait placeholders that pop in individually while the page is already visible. The detail screen and series screen both defer their show until all cast portraits are fetched, then update everything in a single `invoke_from_event_loop` call (no trickle-in). The season screen should match this behaviour.

**Fix:** Follow the `detail.rs`/`series.rs` pattern: set `app_content_loading = true` before spawning the task, defer `set_show_season(true)` to the final `invoke_from_event_loop` callback that delivers portrait data, poster, and metadata all at once.

---

### 🟠 CR6-6 — Collection retry loop in `spawn_collection` has no generation guard
**File:** `crates/fjord-app/src/detail.rs` lines 291–306

When a movie detail page opens, a background task retries up to 10 times (500 ms apart, 5 seconds total) waiting for `movie_collections` to be populated. This loop has no stale-open guard. If the user opens a detail page, signs out, then signs back in as a different user, the still-running retry loop will eventually fire against the new user's `movie_collections` map. It also holds an `Arc<JellyfinClient>` from the previous session alive for up to 5 seconds, preventing it from being dropped.

A stale-open guard already exists one level deeper (line 318 checks `get_detail_id() != id_c`), but the retry loop continues looping even when the detail page is closed, wasting time and keeping the old client alive.

**Fix:** Check `AppState::get(&w).get_detail_id().as_str() != id_c` at the top of each retry iteration and `break` early if the detail page is no longer showing that item.

---

### 🟡 CR6-7 — `person.rs` Left/Right consume the event when in the header row (nothing happens)
**File:** `crates/fjord-app/src/person.rs` lines 112–117

When the person screen header is focused (`in_film = false`), `Action::Left` and `Action::Right` return `true` — they consume the keypress and do nothing. This means left-arrow from the person header is a dead key. It's also inconsistent with `Action::Up` in the same file, which correctly returns `false` when at the top of the screen (allowing `focus_bar_on_up` to focus the mini-player bar).

**Fix:** Return `false` for Left and Right when `!in_film`, so the event is not consumed.

---

### 🟡 CR6-8 — `recently_added_tv` and `recently_added` are the same data
**File:** `crates/fjord-app/src/home.rs` lines 88–94

Both `HomeData.recently_added_tv` (used by the Series dashboard row) and `HomeData.recently_added` (used by the Home dashboard's "Recently Added" row) are populated from the same `get_recently_added(Some("Series"))` call and cloned into both fields. The Home dashboard "Recently Added" row shows only TV shows instead of mixed recently-added content (movies + shows). Other Jellyfin clients show mixed content on the home screen.

Either: (a) this is intentional — in that case remove the duplicate field and both should share one query; or (b) the home row should show mixed content — in which case issue a separate `get_recently_added(None)` call for `recently_added` and keep the series-only call for `recently_added_tv`.

---

### 🟡 CR6-9 — `invoke_from_event_loop` error silently dropped with `.ok()`
**File:** `crates/fjord-app/src/detail.rs` line 316

One `invoke_from_event_loop(...)` call uses `.ok()` to discard the result instead of `let _ = ...` like every other call site in the file. Both patterns ignore the error, but `.ok()` is visually easy to miss as an explicit discard while `let _ =` is the established convention throughout the codebase. In debug builds this makes it harder to notice if the call fails (e.g. event loop already shut down).

**Fix:** Change `.ok()` to `let _ =` for consistency with the rest of the file.

---

### 🟡 CR6-10 — `on_resume_player` doesn't reset `float_card_focused`
**File:** `crates/fjord-app/src/controls.rs` lines 407–418

When the user presses Resume on the mini-player bar, `on_resume_player` fires, sets `is_playing = true`, and clears `has_background_player`. It does not reset `float_card_focused = -1`. This is benign in practice (the pre-dispatch check won't fire while `is_playing` is true), but it's asymmetric: `reset_playback_ui` (called on stop) explicitly clears `float_card_focused`. The resume path should mirror the stop path for clarity and to prevent any edge-case where the flag lingers unexpectedly.

**Fix:** Add `g.set_float_card_focused(-1);` to the `on_resume_player` callback.

---

### 🟢 CR6-11 — ~65-line decode-and-push block duplicated in `spawn_poster_loading`
**File:** `crates/fjord-app/src/poster.rs` lines 124–193

The inner section-push block (lines 124–155) and the post-loop flush path (lines 159–193) are near-verbatim copies of each other. They differ only in one log warning call. This means any future change to how posters are decoded and pushed to the model has to be made in two places.

**Fix:** Extract the shared logic into a local closure or small helper function and call it from both the inner loop and the flush path.

---

### ~~🟢 CR6-12 — `HOME` path logic duplicated between `config.rs` and `home.rs`~~ ✓ Fixed (with CR6-2)
**Files:** `crates/fjord-app/src/config.rs`, `crates/fjord-app/src/home.rs`

Both files independently compute `~/.config/fjord/` and `~/.cache/fjord/` from `std::env::var("HOME")`. The same pattern (including the silent empty-string fallback from CR6-2) is duplicated. If one is fixed, the other must also be updated manually.

**Fix:** Move `fjord_config_dir()` and `fjord_cache_dir()` helpers into `config.rs` (which `home.rs` already imports), and have `home.rs` call them rather than recomputing the paths independently.

---

### 🟢 CR6-13 — `spawn_series_poster_loading` dual-completion path lacks a comment
**File:** `crates/fjord-app/src/poster.rs` lines 270–303

There are two separate paths that push completed section results to the UI: the normal path inside the while loop (fires when `pending.is_empty()` after the last poster in a section arrives) and a post-loop flush (fires after all items have been iterated, catching any section whose last poster was the last item overall). Both are correct and necessary, but there is no comment explaining why both exist or when each fires. The code looks like duplication at first glance but is actually a subtle correctness requirement.

**Fix:** Add a short comment before the post-loop flush explaining: "Sections whose last poster coincides with the last item in the channel don't get flushed inside the loop — flush them here."

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
~/.cache/fjord/home.json         home row data    always refresh in background
~/.cache/fjord/movies.json       full movie list  refresh once per session on grid open
~/.cache/fjord/series.json       full series list refresh once per session on grid open
~/.cache/fjord/posters/<id>      poster bytes     permanent (never expire)
~/.cache/fjord/backdrops/<id>    backdrop bytes   permanent (never expire)
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
16 ms timer       mpv event poll, position update, skip-segment (Intro/Recap/Preview/Commercial), credits auto-advance check, controls idle, progress report
```

---

## Deferred / future

- **Theming / layout customisation**: accent colour palette, dashboard row visibility toggles, row reordering — needs the full layout system in place first before it makes sense to build.
- **Vulkan rendering path** — second render backend alongside the current OpenGL path. Requires: Slint WGPU backend, `MpvRenderCtx` initialized with `MPV_RENDER_API_TYPE_VULKAN`, Vulkan FBO management replacing the current `gl::*` code. Enables true zero-copy decode on AMD (`hwdec=vulkan`, no CPU roundtrip). Legacy NVIDIA hardware needs OpenGL; selection persists in Config as `gpu_renderer: "opengl" | "vulkan"` and takes effect on next restart.
- Gamepad / remote control — d-pad maps to arrow keys; formal evdev/udev support deferred
- **Dashboard row reorder** — drag-to-reorder; part of the future theming/layout customisation update
- **Multi-account / multi-server support** — currently Fjord stores one server URL + one user session in `config.json`. To support multiple accounts: `Config` would need a `Vec<ServerProfile>` (each holding server URL, device ID, username, token) with an `active_profile: usize` index; the login screen would gain a server-picker step; sign-out would become "switch profile" rather than "clear everything"; the `FjordState` runtime fields (`all_movies`, `all_series`, caches, etc.) would be cleared and repopulated whenever the active profile changes. CR6-1 (sign-out cleanup) is a prerequisite — it establishes the correct invariant that switching users produces a clean slate, which multi-account support then relies on.
