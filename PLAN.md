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
| 32 — Code review CR6 (2026-06-24) | 13 issues from full-codebase review. CR6-1: sign-out now clears episode/collection caches, `movies_fetched`, and all overlay AppState flags. CR6-2+12: consolidated XDG path helpers (`xdg_config_base`/`xdg_cache_base`) in `config.rs`; `$HOME` unset logs a `tracing::error!` instead of silently using a relative path. CR6-3: removed `next_ep_pending = None` from countdown task's `!still_playing` branch — natural-end path owns that field exclusively (race nearly guaranteed with fallback 30s trigger + 30s countdown). CR6-4: `get_all_movies` and `get_all_series` now paginate via shared `get_all_paged()` helper (parallel 1000-item pages); Jellyfin's `MaxPageSize` can no longer silently truncate large libraries. CR6-5: season screen deferred to match detail/series — portraits fetched in parallel before page is shown (no trickle-in). CR6-6: `spawn_collection` retry loop bails early when detail page moves on, releasing `Arc<JellyfinClient>` promptly. CR6-7: `person.rs` Left/Right return `false` in header row (were dead keys). CR6-8: removed duplicate `HomeData.recently_added` field (clone of `recently_added_tv`). CR6-9: `invoke_from_event_loop().ok()` → `let _ =`. CR6-10: `on_resume_player` resets `float_card_focused = -1`. CR6-11+13: extracted `push_decoded_section` and `push_decoded_series` helpers, eliminating ~130 lines of duplicated decode+push logic in `poster.rs`. |

---

## Pending — Code Review CR7 (2026-06-24)

Items found during full-codebase review covering Phases 21–32. Listed individually with rationale.

---

### 🔴 CR7-1 — `always-skip` auto-advance silently drops when episode ends before next-up fetch returns
**File:** `crates/fjord-app/src/playback.rs` line 1367

When `credits_mode = always-skip`, `start_playback` spawns a background task to call `get_next_up_for_series`. For short episodes or a slow server the video can reach EOF before that task finishes. The natural-end path (`PollResult::Finished`) runs, sees `next_ep_pending = None`, and returns without advancing. Later the background task sets `next_ep_pending`, but no consumer is left — the user sees playback stop permanently mid-series.

**Fix:** In the natural-end path, if `had_series && next_ep_pending.is_none()`, wait briefly (e.g. a short sleep + re-check) or store the series_id and re-fetch synchronously, rather than silently doing nothing.

---

### 🔴 CR7-2 — Episode cards in Similar / Collection rows always show blank poster
**File:** `crates/fjord-app/src/detail.rs` line 43

`fetch_card_posters` uses `item.id` as the poster key for every item, including Episodes. But Episode posters are keyed by `series_id` throughout the rest of the codebase (see `poster.rs:161-164`, `ep_to_card`). For episodes in "More Like This" or collection rows, `fetch_poster_cached` is called with the episode's own ID — which has no image — so all episode cards show blank posters even when the series poster is in the disk cache.

**Fix:** In `fetch_card_posters`, use `item.series_id.as_deref().unwrap_or(&item.id)` as the fetch key, mirroring `poster.rs:161-164`.

---

### 🔴 CR7-3 — Episode titles wrong format in Similar / Collection rows
**File:** `crates/fjord-app/src/detail.rs` line 63

`items_to_cards` sets `c.title = i.name.as_str().into()` instead of `i.display_name()`. For Episodes, `display_name()` returns `"S01E02 · Title"` but `name` is just `"Title"`. Every other place in the codebase that builds cards from episodes uses `display_name()` (dashboard sections, `ep_to_card`, series screen). The inconsistency is visible whenever an Episode appears in a detail-page row.

**Fix:** Change line 63 to `c.title = i.display_name().as_str().into();`.

---

### 🔴 CR7-4 — Next Up episode can never be un-favourited from the series screen
**File:** `crates/fjord-app/src/series.rs` line 369

The `CardItem` built for the Next Up row hardcodes `is_favorite: false` regardless of the actual `UserData`. When the user opens the context menu on this card, `context_menu_is_favorite = false` is set. Selecting "Add/Remove Favourite" always calls `set_favorite`, never `unset_favorite`. An episode that is already favourited cannot be un-favourited from the series Next Up row.

**Fix:** Populate `is_favorite` from `next.user_data.is_favorite` when building the Next Up CardItem.

---

### 🔴 CR7-5 — `context_play_from_start` uses `next.series_id` instead of the known `id`; kills Up Next if Jellyfin omits `SeriesId`
**File:** `crates/fjord-app/src/context_menu.rs` line 310

When playing from start via the series context menu, `series_id` is set from `next.series_id.clone()` — but `next` is the `NextUp` response item, and Jellyfin can return an episode without a `SeriesId` field. If it does, `series_id` is `None`, `start_playback` stores `playing_series_id = None`, and the Up Next banner never fires for the rest of the session. The correct series ID is already available as the local `id` variable in the same scope.

**Fix:** Use `Some(id.clone())` instead of `next.series_id.clone()` for the `series_id` argument to `start_playback`.

---

### 🟠 CR7-6 — TOCTOU between generation check and `next_ep_pending` write in countdown task
**File:** `crates/fjord-app/src/playback.rs` line 1254

The generation check (lines 1252–1255) acquires the lock, then drops it at the closing brace. `next_ep_pending` is then set under a second lock acquisition at line 1257. Between these two lock acquisitions, `start_playback` can increment `playback_generation` and clear `next_ep_pending`. The countdown task then overwrites it with the wrong episode, causing the wrong video to auto-play.

**Fix:** Hold the lock across both the check and the write — combine into a single lock scope:
```rust
let mut vs = video2.lock().unwrap();
if vs.player.is_none() || vs.playback_generation != my_gen { return; }
vs.next_ep_pending = Some(next.clone());
```

---

### 🟠 CR7-7 — Context menu `focused` not reset when item is marked played; Enter triggers Resume on fully-played item
**File:** `crates/fjord-app/src/context_menu.rs` line 376

If the user opens a context menu on a resumable item (`context_menu_focused = 0`, Resume row visible) and marks it played without moving focus, `has_played` flips to `true` and the Resume row disappears visually — but `focused` stays at 0. A subsequent Enter calls `invoke_item_play` which resumes from the old resume position rather than playing from the start.

**Fix:** In `on_context_mark_played`, after the played state changes to true, reset `context_menu_focused` to 1 (Play from Start row) when it was 0.

---

### 🟠 CR7-8 — Left from ♥ (btn=1) in season detail header does nothing; Back unreachable
**File:** `crates/fjord-app/src/season.rs` line 203

The Left handler for the season header: `if btn == 1 || btn == 2 { if btn > 1 { set btn-1 } }`. When `btn == 1`, the outer condition passes but the inner `if btn > 1` is false, so nothing happens. The Back button (`btn=0`) is permanently unreachable via Left from the ♥ button.

**Fix:** Change `if btn > 1` to `if btn >= 1`.

---

### 🟠 CR7-9 — Same Left-from-♥ dead-end in series screen header
**File:** `crates/fjord-app/src/series.rs` line 591

Identical bug to CR7-8 — the series header Left handler has `if b > 1` inside `if b == 1 || b == 2`, leaving Back unreachable from ♥ in the series screen too.

**Fix:** Change `if b > 1` to `if b >= 1` at series.rs:591.

---

### 🟠 CR7-10 — Series header unplayed-count badge goes stale after marking episode played from within the series screen
**File:** `crates/fjord-app/src/context_menu.rs` line 231

`update_series_unplayed_count` decrements `unplayed_count` on CardItem models in all dashboard rows but never updates `AppState.series_unplayed_count` (the property driving the badge in the series screen header). When an episode is marked played via the detail I-key or context menu while the series screen is open, the header badge keeps showing the old count until the series screen is closed and reopened.

**Fix:** In `update_series_unplayed_count` (or in its callers like `on_toggle_detail_played`), also call `g.set_series_unplayed_count(new_count)` after updating the card models.

---

### 🟡 CR7-11 — Season loading-progress 50% event has no stale season-ID guard
**File:** `crates/fjord-app/src/season.rs` line 116

The mid-load `invoke_from_event_loop` that sets `app_loading_progress = 0.5` fires unconditionally. `detail.rs:173` and `series.rs:226` both guard the equivalent call with an item-ID check. If the user opens season A then quickly opens season B, season A's async task fires the 50% update against season B's load, making the progress bar jump to 50% then reset.

**Fix:** Add a stale guard inside the closure: `if g.get_season_id().as_str() != sid { return; }` before setting the progress.

---

### 🟡 CR7-12 — Person loading-progress 50% event has no stale person-ID guard
**File:** `crates/fjord-app/src/person.rs` line 59

Same issue as CR7-11 but in the person screen. The `invoke_from_event_loop` that sets `app_loading_progress = 0.5` has no guard. `detail.rs:173` correctly guards with `get_detail_id() != id`. Rapid navigation between persons corrupts the loading bar of the second person.

**Fix:** Add `if g.get_person_id().as_str() != pid { return; }` inside the progress closure.

---

### 🟡 CR7-13 — `spawn_series_poster_loading` doesn't deduplicate IDs; premature push if server returns duplicate series
**File:** `crates/fjord-app/src/poster.rs` line 243

Unlike `spawn_poster_loading` which builds a `unique_ids` HashSet before spawning, `spawn_series_poster_loading` spawns one task per metadata entry. If the server returns a series with a duplicate ID, `pending` has only one entry. The first task to complete removes it from `pending`, empties the set, and fires `push_decoded_series` before the second task's bytes are in `poster_map` — one card shows no poster.

**Fix:** Deduplicate `meta` IDs before spawning, the same way `spawn_poster_loading` builds `unique_ids`.

---

### 🟡 CR7-14 — `last_nw_mov_refresh` / `last_nw_tv_refresh` not cleared on sign-out
**File:** `crates/fjord-app/src/main.rs` line 1239

`on_sign_out` clears all library data and session state but does not reset the Not Watched refresh timestamps. If the user signs out and logs back in within 10 minutes (e.g. to switch servers), the 30-second polling timer sees timestamps with `elapsed < 600 s` and skips the fetch. The Not Watched rows stay empty until the cooldown from the previous session expires.

**Fix:** Add `s.last_nw_mov_refresh = None; s.last_nw_tv_refresh = None;` to the sign-out block in `main.rs`.

---

### 🟢 CR7-15 — CLAUDE.md Keyboard Navigation section is stale: wrong AppMode variant count, missing handlers
**File:** `CLAUDE.md` line 157

The paragraph reads "active_mode() derives the current AppMode (8 variants)" but `keys.rs` declares 10 variants (`Person` and `Season` were added later). The per-module handler list omits `season::handle_key` and `person::handle_key`. The `ResumePlayer` exclusion list says "Player/Detail/ContextMenu" but the code also blocks it from `Person` and `Season` modes.

**Fix:** Update the Keyboard Navigation paragraph: change "8 variants" to "10 variants"; add `season::handle_key` and `person::handle_key` to the dispatch list; update the ResumePlayer exclusion to all 5 modes.

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
