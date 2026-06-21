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
    │                    season tabs + episode list inline (quick play from here)
    ├── Similar row       "More Like This" horizontal poster row — Enter opens series detail
    ├── Season detail     season backdrop/poster · season overview · episode count · year
    │   │                 episode list for that season only
    │   └── Episode detail  thumbnail · title · S01E01 · runtime · air date · overview
    │                       guest cast · Play / Resume buttons
    └── Episode detail    (also reachable directly from series detail episode list)
```

### Movie detail — enrichment steps

- [ ] **Step 1 — Director, writer, tagline, studio** (zero extra API calls): add `Taglines` + `Studios` to `Fields` in `get_item_detail` and deserialize in `MediaItem`; extract first director and first writer from `People` by `Type`; push `detail-director`, `detail-writer`, `detail-tagline`, `detail-studio` to `AppState`; show tagline in italic under title, director + writer + studio in the meta area in `detail.slint`. Applies to movies — series gets the same treatment in Step 5.
- [ ] **Step 2 — Cast photos**: add `id: string`, `photo: image`, `has-photo: bool` to `CastMember` struct; include person `id` when building cast vec in `detail.rs`; spawn per-person portrait fetches reusing `fetch_poster_cached` (same `/Items/{id}/Images/Primary` endpoint); push photos into `VecModel` via `set_row_data` + `invoke_from_event_loop`; update cast cards in `detail.slint` to show portrait above name/role. Add Left/Right keyboard nav through cast members (`detail-cast-focused`).
- [ ] **Step 3 — Collection row**: if the fetched item belongs to a BoxSet (`CollectionId` field), fetch sibling items (`GET /Users/{userId}/Items?ParentId={collectionId}&SortBy=ProductionYear`); show as "Part of [Collection Name]" horizontal row — same card style, Enter opens that movie's detail.
- [ ] **Step 4 — Similar movies row**: add `get_similar_items(item_id)` to `client.rs` (`GET /Items/{id}/Similar?userId=…&Limit=12&Fields=ProductionYear,PrimaryImageAspectRatio`); show as "More Like This" horizontal row below collection row; Enter opens detail.

### Series detail — rework + enrichment

- [ ] **Step 5 — Series detail page**: currently `open_detail` for a Series routes directly to the series screen, bypassing the detail page. Change it to show a proper series detail page (same layout as movie detail: backdrop, poster, tagline, studio, overview, cast photos, similar series row) with a "Browse Episodes →" button that opens the existing series screen. Season tabs + episode list remain on the series screen as the drill-down. Creator/showrunner shown in place of director (Jellyfin returns them as `Type == "GuestStar"` / `"Director"` / `"Writer"` on the series item itself — extract `Type == "Director"` and `Type == "Writer"` from the series People array just like movies).
- [ ] **Step 6 — Season detail page**: from the series screen, selecting a season tab and pressing Enter (or a dedicated key) opens a season detail page — season backdrop/poster, season overview, episode count, year, and the episode list for that season only. Backspace returns to the series screen.
- [ ] **Step 7 — Episode detail page**: from the series screen or season detail, pressing `I` on a focused episode opens an episode detail page — thumbnail, title, S01E01 badge, runtime, air date, full overview, guest cast (photos). Play/Resume buttons. Backspace returns to wherever you came from.

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
- Person detail screen (depends on cast row nav above)
- Dashboard row reorder (drag-to-reorder, Phase 5 Step 5)
