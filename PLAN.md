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
| 33 — Code review CR7 (2026-06-24) | 15 findings from full-codebase review (Phases 21–32). CR7-1: always-skip auto-advance fallback when EOF beats next-up fetch (captures `playing_series_id` pre-teardown, spawns fresh `get_next_up_for_series` if `next_ep_pending` is None). CR7-4: Next Up episode `is_favorite` hardcoded false — now reads `user_data.is_favorite`. CR7-5: `context_play_from_start` used `next.series_id` (Jellyfin can omit it) instead of known `id`. CR7-6: TOCTOU between generation check and `next_ep_pending` write merged into single lock scope. CR7-7: context menu `focused` not reset when item marked played — Enter would invoke Resume on fully-played item. CR7-8/9: Left from ♥ (btn=1) in season and series header did nothing (`> 1` → `>= 1`). CR7-10: `update_series_unplayed_count` now also updates `AppState.series_unplayed_count` when the series screen is open. CR7-11/12: loading-progress 50% invokes in season and person screens now guarded by stale season-ID / person-ID check. CR7-13: `spawn_series_poster_loading` now deduplicates IDs before spawning (prevents premature push on duplicate server entries). CR7-14: sign-out clears `last_nw_mov_refresh` / `last_nw_tv_refresh`. CR7-15: CLAUDE.md updated — AppMode variant count 8→10, added `person::handle_key` + `season::handle_key` to dispatch table, ResumePlayer exclusion list updated. |
| 34 — Sort, filter, and alphabet scrubber in library grid (2026-06-25) | Client-side sort (Name A-Z/Z-A, Year↓/↑, Random via LCG pseudo-shuffle) and Unwatched/Favorites filters applied to cached `all_movies`/`all_series`. Sort persisted per library type in `Config.library_movies_sort`/`library_series_sort`. `refresh_library_display` central function (browse.rs) rebuilds `library-display` and `library-alpha-offsets` (27-element vec, A-Z+#, each -1 when letter has no items). Sort bar UI (40 px strip) with 5 sort pills + 2 filter toggles. Alphabet scrubber: right-edge 22 px strip, visible only when sort=A-Z and no query; click or A-Z key jumps grid. Full arrow-key nav chain: grid → search → sort bar → Back button → mini-player (each layer reachable via Up). Sort pills auto-apply on Left/Right navigation (no Enter needed); filter toggles (Unwatched/Favorites) still require Enter and keep the bar focused. Focus indicator: 2 px border (accent when inactive, white ring when on the active pill) + surface-overlay background so cursor is always visible. Down from sort bar moves to search field (not grid). `library-back-focused` state: Back button gets kbd-focused ring; Enter/Esc closes library; Down returns to sort bar. Loading overlay and LoadingSpinner improved: 20 px dots (was 14 px), 260 ms cadence, 75% dim (was 53%), 6 px progress bar (was 4 px, 320 px wide). |
| 35 — Error toast notifications (2026-06-25) | `toast-message: string` + `toast-visible: bool` added to `AppState`. `ToastNotification` component in `widgets.slint`: bottom-center pill, dark semi-transparent background, 4 px red left accent stripe, word-wrap text. Positioned and sized by caller (main.slint). Z-order above ContextMenu (last element in MainWindow). `show_toast(ww, msg)` helper in `main.rs`: any-thread safe via `invoke_from_event_loop`. Auto-dismiss: local `_toast-vis` mirror property + `changed _toast-vis` fires `toast-timer` (4 s Slint Timer, self-stops after trigger). Wired into: `start_playback` Player::new failure ("Couldn't start playback — check your server connection"), `context-mark-played` failure ("Couldn't update watch status"), `context-toggle-fav` failure ("Couldn't update favourite"). |

---

## Pending

---

### ⏸ Phase 36 — Playback speed control *(deferred — maybe later)*

mpv exposes `speed` as a runtime property. Common workflow: watch recap episodes at 1.5×, slow down for dialogue. Seek buttons and drag scrubbing cover most skip needs, so this is low priority.

---

### 🟠 Phase 37 — Chapter navigation

mpv exposes `chapter-list` and `chapter` as properties. Jellyfin also returns chapter markers in item detail (`Chapters` field). Useful for anime (skip OP/ED at exact boundaries), concerts, long films.

**Plan:**
- On playback start, read `chapter-list` from mpv after the file loads (poll until non-empty or timeout). Store in `VideoState.chapters: Vec<(f64, String)>` (start_secs, name).
- Render chapter markers on the seek bar (small tick marks at the correct fraction).
- Keys: `N` next chapter, `P` previous chapter → `player.command(&["add", "chapter", "1"])` / `"-1"`.
- Show chapter name in an OSD toast (same system as Phase 35) when seeking to a chapter boundary.

---

### 🟠 Phase 38 — Subtitle and audio delay adjustment

Common need for poorly-synced releases. mpv has `sub-delay` and `audio-delay` as runtime properties.

**Plan:**
- Player keys: `Z` / `z` nudge `sub-delay` by ±100 ms; `Shift+A` / `Shift+a` nudge `audio-delay` by ±100 ms (matches mpv defaults, remappable).
- Show current offset in an OSD toast when changed ("Sub delay: +200 ms").
- Reset both to 0 on `reset_playback_ui`.
- No persistence — per-session only (same rationale as speed).

---

### 🟠 Phase 39 — Ratings and genres on detail page

`CommunityRating` and `Genres` already come back from `GET /Users/{id}/Items/{itemId}` but are never displayed.

**Plan:**
- Add `detail-rating: float` and `detail-genres: [string]` to `AppState`; populate in `detail.rs` `spawn_main`.
- Render: star + numeric rating (e.g. "★ 7.4") inline in `MetaLine` next to year/runtime/rating. Genres as small chips below the MetaLine, wrapping if long.
- Same for series screen (`series-rating`, `series-genres`).

---

### 🟠 Phase 40 — Collections home section and browsable screen

BoxSets (collections) can only be reached via the detail page of a member item. There's no direct navigation to a collection, and no way to browse all collections.

**Plan:**
- Add a "Collections" home dashboard row (movies only): `GET /Users/{id}/Items?IncludeItemTypes=BoxSet&Recursive=true&SortBy=SortName`. Fetch posters like other rows.
- Entering a collection opens a new screen (`CollectionScreen`) — backdrop + title + grid of member items sorted by `ProductionYear`. Same keyboard nav as library grid.
- `AppMode::Collection` added to `keys.rs`; Back closes the screen.
- Reuse `fetch_movie_collections` map already built in `home.rs` — no extra API calls needed.

---

### 🟡 Phase 41 — WebSocket real-time events

Jellyfin pushes events over WebSocket: `LibraryChanged` (new items added), `PlaybackStart`/`Stop` from other clients, `UserDataChanged` (mark-played from phone), `KeepAlive`. Currently everything is polling.

**Plan:**
- Add `fjord-api` websocket support: `tokio-tungstenite` connecting to `wss://{host}/socket?api_key=…&deviceId=…`.
- Handle `LibraryChanged` → trigger home/movies/series cache refresh (debounced 5 s).
- Handle `UserDataChanged` → call `update_card_in_all_models` with the new played/fav state so the UI updates when something is marked from a phone.
- Handle `PlaybackStart` from another session → optional toast "Playback started on [device]".
- Reconnect with exponential backoff on disconnect; disable if server < Jellyfin 10.8.

---

### 🟡 Phase 42 — Poster cache cleanup

The poster cache at `~/.cache/fjord/posters/` grows forever. Items deleted from Jellyfin leave orphaned files.

**Plan:**
- On startup (after library fetch completes), collect the set of all known item IDs (`all_movies` + `all_series` + their season/episode IDs). Walk the cache directory; delete any file whose name is not in the set.
- Run this as a low-priority background task with a 24 h minimum interval (stored in config) so it doesn't run on every cold start.
- Cap: if the library ID set is empty (network error during fetch), skip cleanup to avoid wiping everything.

---

### 🟢 Phase 43 — Music library

Jellyfin has a full music library (Artists, Albums, Tracks, Playlists). Completely unimplemented — different UX paradigm from movies/TV.

**Plan (high level — needs its own detailed design):**
- New sidebar nav entry "Music" (nav=4, shifting Settings to nav=10 offset or adding it after Browse).
- `MusicDashboard`: Recently Added Albums, Recently Played, Favourite Artists rows.
- `ArtistScreen`: portrait + bio + albums grid.
- `AlbumScreen`: cover + tracklist, play-all button.
- Player adapted for music: no video layer, album art in place of video, track title + artist in controls bar.
- Queue management required for playlist/album playback.

---

### 🟢 Phase 44 — Queue / playlist management

Play-next, add-to-queue, shuffle — needed for music but useful for movies too (watch party queues, double features).

**Plan (high level):**
- `VideoState.queue: VecDeque<MediaItem>` with shuffle flag.
- Context menu gains "Add to Queue" and "Play Next" entries.
- Mini-player bar gains "Queue" button showing item count.
- Auto-advance for movies uses the queue instead of prompting the user.

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
