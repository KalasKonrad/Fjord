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
| 13 — EAC3 passthrough diagnosis (2026-06-20) | Root cause (#39): tsched software timer caused PipeWire RT thread to miss 21.3 ms deadline at 192 kHz IEC61937 rates under GPU load. Fix: `api.alsa.disable-tsched=true` (hardware IRQ wakeups) + `suspend-timeout-seconds=2`. Now a Settings toggle. Do not use `api.alsa.headroom` — shifts audio timeline, causes frame drops under `display-vdrop`. |
| 14 — Settings: SPDIF per-format toggles, HDR passthrough row, virtual rows (2026-06-21) | Per-format SPDIF toggles (AC3/EAC3/DTS/DTS-HD/TrueHD) replace single passthrough switch. Tone-mapping row hidden when HDR passthrough on. Video-latency-hacks row hidden unless display-resample active. Cross-section passthrough+display-resample conflict warning. |
| 15 — Audio output device selector (2026-06-21) | Dropdown in Settings → Audio populated from `mpv --audio-device=help`. Device stored in `Config.audio_device`; applied to mpv at playback start. Content-driven popup width; keyboard nav fixed. |
| 16 — PipeWire IRQ scheduling toggle (2026-06-21) | Settings → Audio toggle (visible when SPDIF on + PipeWire/auto device). Writes/deletes `~/.config/wireplumber/wireplumber.conf.d/fjord-alsa-irq.conf` and restarts WirePlumber on change. Config persists after exit; syncs down to false on startup if file missing. |
| 17 — Intro Skipper API v2 + generalized skip segments (2026-06-21) | Migrated from two old endpoints (`/IntroTimestamps`, `/Credits`) to single `GET /Episode/{id}/Timestamps` returning all 5 segment types. `EpisodeTimestamps` model with `Segment { start, end }` for Introduction, Credits, Recap, Preview, Commercial. Single generic skip overlay (`show-skip-segment` + `skip-segment-label`) replaces `show-skip-intro`. Timer checks segments in priority order (Intro → Recap → Preview → Commercial). Enter skips active segment (priority check at top of `dispatch_player`). Up Next banner: `next-ep-banner-focused` (0=Play Now, 1=Skip); Left/Right toggle, Enter activates. |
| 18 — Step 3: per-segment skip modes + configurable timers (2026-06-21) | Each of Intro/Recap/Preview/Commercial has 4 modes: `always-skip` (immediate seek, no overlay), `ask` (single "Skip →" button), `ask-timed` (two-button overlay "Skip"+"Don't Skip" with per-segment countdown that auto-skips on expiry), `never-skip` (no overlay). Each segment has its own `*-prompt-secs` (default 8 s), visible only when mode = `ask-timed`. Credits has 3 modes: `always-skip` (auto-advance immediately at credit start), `ask` (Up Next banner with configurable countdown), `never-skip` (no auto-advance). `up-next-timer-secs` (default 30 s) configures the banner countdown, visible only when Credits = `ask`. All 10 new `Config` fields (`skip_*_mode`, `skip_*_secs`) persisted in JSON with serde defaults. Settings → Player section extended with rows 4–13 + INTRO SKIPPER / CREDITS section headers. `VideoState` extended with `skip_segment_handled`, `skip_timed_shown_at`, `skip_timed_prompt_secs`. `dispatch_player` in keys.rs intercepts the timed overlay at the top with L/R focus toggle, Enter activates, Back/Esc dismisses. |
| 19 — Movie detail Steps 1–4: enrichment + collection row (2026-06-21) | Step 1: director, writer, tagline, studio from `get_item_detail`; `BackdropHero`/`PosterBlock`/`MetaLine` atoms extracted to `widgets.slint`. Step 2: `CastRow` atom; async portrait fetch via `JoinSet` + semaphore 6; `detail-cast-focused` keyboard nav (Right past last button enters cast, Left exits). Step 3: `fetch_movie_collections` background task builds `HashMap<movie_id,(boxset_id,boxset_name)>` in `FjordState`; spawned after login; O(1) lookup in `open_detail`; BoxSet siblings fetched with posters and pushed as collection `SectionRow`. Step 4: `get_similar_items`; "More Like This" `SectionRow` below collection row. |
| 21 — Detail page keyboard nav + stop-returns-to-detail (2026-06-21) | Bug 1: `open_detail` now calls `invoke_grab_keyboard_focus()` when showing the detail page — mouse-click entry paths were leaving keyboard focus on the card's TouchArea, making all keyboard nav dead until a page reload. Bug 2: playing from detail no longer closes it; `VideoState.from_detail` flag is set by `on_play_detail`/`on_resume_detail`, read+cleared in `start_playback` which sets `AppState.playback-from-detail`; `reset_playback_ui` checks that flag and restores `show_detail = true` on stop/natural-end. `on_minimize_player` clears both flags (user wants the normal UI when going to mini-card, not the detail page). `main.slint` hides DetailPage when `is_playing` to prevent it covering the player overlay. Auto-advance case handled: each new `start_playback` clears `from_detail`, so only the immediately preceding detail-play restores on stop. |
| 22 — Series season episode cache + race fix (2026-06-22) | Season episodes now cached in `FjordState.series_episode_cache` (HashMap<season_id, Vec<MediaItem>>); cleared when a new series is opened. Re-selecting an already-seen season renders instantly from cache — no network request. `series_season_generation` counter prevents stale async results from rapid season-tab navigation overwriting the correct season's episodes (each task checks its captured generation against the current value before applying). |
| 20 — CR5: post-Steps-1–4 bug fixes (2026-06-21) | (1) `fetch_movie_collections` now spawned in auto-login path (`main.rs`) — was only called in `do_login`, so warm-start sessions had empty collection map. (2) Portrait index mismatch fixed — `person_ids` now carries `(model_idx, id)` tuples preserving cast model row index across filtered actors. (3) `SectionRow.item-play` extended to pass `item_type` as second arg — collection and similar rows in `detail.slint` call `open-detail(id, itype)` instead of hardcoded "Movie". (4) `CastRow` focus ring now visible — outer border Rectangle has no `clip`, inner clipped Rectangle holds the image; `clip+border` on same element made ring invisible. (5) `keys.rs` Back from detail page now resets `detail-collection`, `detail-collection-title`, `detail-similar` to prevent stale row flash on next open. (6) `get_boxset_items` adds `SortBy=ProductionYear&SortOrder=Ascending` for chronological franchise ordering. |

---

## Open Work

### Navigation hierarchy

The full intended screen hierarchy:

```
Library grid
├── Movie detail          backdrop · poster · title · tagline · director · studio ·
│   │                    year · runtime · rating · genres · overview · cast (photos)
│   │                    Play / Resume buttons
│   ├── Collection row   "Part of [X]" horizontal poster row — Enter opens movie detail
│   └── Similar row      "More Like This" horizontal poster row — Enter opens movie detail
│
└── Series detail        backdrop · poster · title · tagline · studio · year · rating ·
    │                    genres · overview · cast (photos)
    │                    "Next Up" card (next unwatched episode, hidden when fully watched)
    │                    season tabs + episode list inline (quick play from here)
    ├── Similar row       "More Like This" horizontal poster row — Enter opens series detail
    ├── Season detail     season backdrop/poster · season overview · episode count · year
    │   │                 cast photos · episode list for that season only  [NEW]
    │   └── Episode detail  (existing DetailPage — enriched by Steps 1–2)
    └── Episode detail    (existing DetailPage — also reachable directly from series screen)
```

### Component architecture

Rather than a shared monolithic header, use **shared atom components** that each page composes freely. This keeps per-page layout flexibility while avoiding code duplication and making future theming changes apply everywhere:

- `BackdropHero` — full-width backdrop image with bottom fade gradient; extracted from `detail.slint` into `widgets.slint`
- `PosterBlock` — poster image with placeholder and rounded corners; extracted from `detail.slint` into `widgets.slint`
- `MetaLine` — year · runtime · rating chips; extracted from `detail.slint` into `widgets.slint`
- `CastRow` — horizontal scroll of cast cards (portrait photo + name + role); new component in `widgets.slint`
- `SectionRow` — already exists in `home.slint` and is the correct component for Similar and Collection horizontal poster rows on detail pages; do **not** create a duplicate `HorizontalScrollRow`

Each detail page composes these atoms in its own layout — movie detail, series detail, season detail, and episode detail can all look different while sharing the same building blocks. Theming a single atom updates every screen that uses it.

### Movie detail — enrichment steps

- [x] **Step 1 — Director, writer, tagline, studio** (zero extra API calls): add `Taglines` + `Studios` to `Fields` in `get_item_detail` and deserialize in `MediaItem`; extract first director and first writer from `People` by `Type`; push `detail-director`, `detail-writer`, `detail-tagline`, `detail-studio` to `AppState`; show tagline in italic under title, director + writer + studio in the meta area. Extract `BackdropHero`, `PosterBlock`, `MetaLine` as shared atoms from `detail.slint` into `widgets.slint`.
- [x] **Step 2 — Cast photos**: add `id: string`, `photo: image`, `has-photo: bool` to `CastMember` struct; include person `id` when building cast vec in `detail.rs`; spawn per-person portrait fetches reusing `fetch_poster_cached` (same `/Items/{id}/Images/Primary` endpoint); push photos into `VecModel` via `set_row_data` + `invoke_from_event_loop`; extract `CastRow` as a shared atom into `widgets.slint`. Left/Right keyboard nav through cast: `detail-cast-focused`; Right past last button enters cast, Left from first member exits. Person detail screen is deferred.
- [x] **Step 3 — Collection row**: Jellyfin has no reverse BoxSet link in movie detail — the membership map is built in the background at login. `fetch_movie_collections` fetches all BoxSets then all their members (4-concurrent, `JoinSet`), building `HashMap<movie_id, (boxset_id, boxset_name)>` stored in `FjordState.movie_collections`. `open_detail` does an O(1) lookup; if found, spawns a task to fetch sibling movies with posters and push a `SectionRow` titled with the BoxSet name. Sibling row excludes the current movie. Movies only.
- [x] **Step 4 — Similar row**: `get_similar_items` in `client.rs` (`GET /Items/{id}/Similar?userId=…&Limit=12&Fields=ProductionYear,UserData`); concurrent poster fetch; "More Like This" `SectionRow` below cast in `detail.slint`; click opens detail. Movies only for now — Step 5 wires the same API call for series.

### Series detail — enrichment

- [x] **Step 5 — Series detail enrichment**: enrich the existing series screen header using the shared atoms from Steps 1–2 — genres, rating, meta (year · rating · runtime), `CastRow` (directors + writers + actors with portraits). Add a "Next Up" card (SectionRow with one card) between the header and the season tabs via `get_next_up_for_series`; hidden when series is fully watched. Episode list migrated from vertical `[EpisodeEntry]` to horizontal `[CardItem]` SectionRow (title formatted "S01E02 · Title"); keyboard nav changed to Left/Right. Wire `get_similar_items` for series; "More Like This" SectionRow below cast. `series.slint` fully rewritten as a scrollable page (backdrop → header → Next Up → season tabs → episodes → cast → similar). `EpisodeEntry` struct removed. `ep_to_card` replaces `EpisodeRaw`/`make_episode_raw`/`raw_to_entry`. `context_menu.rs` updated to patch `series-episode-cards` (was `series-episodes`).

### Season detail — new screen

- [ ] **Step 6 — Season detail page**: the only genuinely new screen. Key assignment: Enter on a season tab continues to select it and load episodes as now; `I` on a focused season tab opens the season detail page. Season detail composes shared atoms — `BackdropHero`, `PosterBlock`, `MetaLine`, `CastRow` for that season's People array — then the episode list for that season only. `I` on a focused episode opens the existing episode detail page. Backspace returns to the series screen.

Note: episode detail already exists via `DetailPage` — episodes get Steps 1–2 enrichment automatically (episode-level director + writer + guest cast photos).

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
16 ms timer       mpv event poll, position update, skip-segment (all 5 types), controls idle, progress report
```

---

## Deferred / future

- **Theming / layout customisation**: accent colour palette, dashboard row visibility toggles, row reordering — needs the full layout system in place first before it makes sense to build.
- **Vulkan rendering path** — second render backend alongside the current OpenGL path. Requires: Slint WGPU backend, `MpvRenderCtx` initialized with `MPV_RENDER_API_TYPE_VULKAN`, Vulkan FBO management replacing the current `gl::*` code. Enables true zero-copy decode on AMD (`hwdec=vulkan`, no CPU roundtrip). Legacy NVIDIA hardware needs OpenGL; selection persists in Config as `gpu_renderer: "opengl" | "vulkan"` and takes effect on next restart. The `gpu-api` setting was removed (2026-06-19) because it had no effect with `vo=libmpv` + OpenGL render context — this feature replaces it properly.
- Gamepad / remote control — d-pad maps to arrow keys; formal evdev/udev support deferred
- **Person detail screen** — depends on cast row keyboard nav (Step 2); shows filmography, bio, portrait
- **Dashboard row reorder** — drag-to-reorder; part of the future theming/layout customisation update
