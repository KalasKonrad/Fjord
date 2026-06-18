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

---

## Under Investigation

Do not implement fixes for these without HTPC reproduction data first.

- **#39 — Audio dropout when vsync=audio with bitstream passthrough** — root cause unknown. To diagnose: reproduce on HTPC with stats overlay open (`I` key) during TrueHD/DTS-HD passthrough playback. Watch the SPEED row — a spike in `audio-speed-correction` at dropout time confirms AO clock drift. Also try `desync` in Settings → Player → Video sync (added in 43ef45a); if dropouts stop, `video-sync=audio` is the culprit.

---

## Open Work

### Bug fixes

Ordered purely by severity. GitHub issue numbers (#N) are linked to the tracker; items without a number came from internal code review.

#### CRITICAL

- [x] **#30 — Crash when starting a new video while another plays in background** — Fixed: `start_playback` now calls `tear_down_player` first, which drops `render_ctx` then `player` in the correct order before creating a new instance. Also sends a stop report for the interrupted video.
- [x] **Player settings wiped on re-login after sign-out** *(code review)* — Fixed: `do_login` now clones `state.config` instead of calling `load_config()`, so all player/app settings survive sign-out + re-login. Only auth fields are overwritten. `Config` derives `Clone`. (`auth.rs`, `config.rs`)

#### HIGH

- [x] **#37 — "Up Next" banner redesign** — Fixed: banner now appears *during* playback at the credits start (Intro Skipper `/Credits` endpoint) or 30 s before end (fallback). 30-second countdown with "Play Now" and "Skip" buttons. "Play Now" starts next episode immediately; "Skip" hides banner and lets video play to natural end with no auto-advance; countdown reaching 0 or natural end with banner pending both auto-advance. `next_ep_pending` moved from `FjordState` to `VideoState` so it's cleared atomically when a new video starts. (`fjord-api/src/client.rs`, `fjord-app/src/playback.rs`, `fjord-app/src/main.rs`, `fjord-app/ui/app_state.slint`, `fjord-app/ui/player.slint`)
- [x] **#36 / #35 / #26 — Stop report reliability** — Fixed: home refresh now runs sequentially after the stop report in both `do_stop_playback` and the natural-end block of `wire_mpv_timer`. Previously both were spawned concurrently; if the home fetch won the race it returned stale continue-watching data before Jellyfin processed the stop. Single task: `await stop_report → fetch_home → push`. (`playback.rs`)
- [x] **#28 — KDE dims screen / turns off display during playback** — Fixed: three inhibitors active during playback: (1) `org.freedesktop.ScreenSaver.Inhibit` — screen blank/lock on all DEs; (2) `org.kde.PowerManagement.Inhibition.Inhibit` — display dim + sleep on KDE Plasma; (3) `systemd-inhibit --what=idle:sleep` child process — sleep/suspend on GNOME, XFCE, and any systemd-based DE. All released at playback stop. Each is best-effort/silent no-op when unavailable. (`playback.rs`)
- [x] **#27 — vsync setting has no effect; stats always shows "audio" mode** — Fixed: stats was inferring sync mode from `vsync-ratio == 0.0`, but the render API never populates `vsync-ratio`. Now reads `video-sync` property directly from mpv and displays it in the VSYNC stat row. Also logs the effective `video-sync` value in `log_decoder_info()` so the HTPC log confirms the setting took effect. (`fjord-player/src/mpv.rs`, `fjord-app/src/stats.rs`)
- [x] **#32 — mpv's own OSD shows when seeking or changing volume** — Fixed: `osd-level=0` set unconditionally in `Player::new()` init block. Not a `PlayerConfig` field — never appropriate to show mpv's OSD when we have a Slint UI. (`fjord-player/src/mpv.rs`)
- [x] **#31 — Overlay progress bar shows time but no bar** — Fixed: `alignment: center` on the seek-row `HorizontalLayout` was zeroing `horizontal-stretch: 1` on `seek-track`. Removed the alignment; bar now fills available width. (`player.slint:86`)
- [x] **#33 — No volume overlay when changing volume** — Fixed: top-center toast overlay shows "Vol ▓▓▓▓░ XX%" for ~1.5 s after each volume key press. Generation counter ensures rapid presses extend the visible window correctly (only the latest task hides the overlay). (`fjord-player/src/mpv.rs`, `fjord-app/src/controls.rs`, `fjord-app/ui/app_state.slint`, `fjord-app/ui/player.slint`)
- [x] **#19 — Backspace/Escape behaviour in player** — Fixed: new `Action::MinimizePlayer` (Backspace in player map) closes open panel first then minimizes. Escape resolves to `Action::Back` via normal-map fallthrough and stops playback. When video is already minimized (`is-playing = false`) Escape is plain nav Back — video keeps playing. Also added "■" stop button to the Now Playing sidebar card so minimized playback can be stopped without a keyboard shortcut. (`keys.rs`, `layout.slint`)
- [x] **Sign-out doesn't stop active playback** *(code review)* — Fixed: `on_sign_out` now calls `do_stop_playback` first, tearing down mpv, resetting all player UI state, and sending a stop report before clearing session state. (`main.rs`)
- [x] **`item_type` never set in poster loaders** *(code review)* — Fixed: `spawn_poster_loading` now threads `i.item_type` through the metadata tuple and sets `h.item_type`; `spawn_series_poster_loading` hardcodes `"Series"`; `spawn_movies_poster_loading` hardcodes `"Movie"`. Context-menu "View Details" now routes correctly. (`poster.rs`, `movies.rs`)

#### MEDIUM

- [x] **#20 — Can't navigate to Resume button in mini card with keyboard** — Fixed: Now Playing card is now a stop in the sidebar nav cycle as `active-nav == 4`, inserted between Browse All (3) and Settings (10) when `has-background-player`. Up/Down navigate into/out of it; Left/Right toggle focus between Resume (0) and Stop (1) buttons; Enter activates. `mini-card-focused` prop tracks which button is highlighted. `do_stop_playback` resets `active-nav` from 4 → 0 when the card disappears. (`app_state.slint`, `layout.slint`, `keys.rs`, `playback.rs`)
- [x] **#22 — Subtitle list not scrollable with keyboard** — Fixed: all three track panels (sub/audio/video) used `sub-fl.height` and `sub-fl.viewport-height` self-references in the `viewport-y` binding, which Slint does not reliably track as binding dependencies. Replaced with `parent.height` (the outer panel Rectangle) and the content layout's `preferred-height` directly. Also removed the spurious `+16px` on `viewport-height` since `preferred-height` already includes padding. (`player.slint`)
- [x] **#25 — Slight stutter when navigating to Browse All in sidebar** — Fixed: `populate_browse` now runs off the UI thread. The event loop snapshots `all_movies + all_series` under lock, spawns a Tokio task for filtering and `display_names`, then uses `invoke_from_event_loop` to push the result back. An `AtomicU64` generation counter discards stale results when the user types quickly. (`browse.rs`)
- [x] **#40 — Volume control should show it has no effect during passthrough** — Fixed: `update_stats_window` (runs every 500 ms during playback) now sets `audio-passthrough-active` based on whether `audio-out-params/format` starts with "iec61937". When active: volume up/down skip `adjust_volume` and the overlay shows "Vol · passthrough" instead of the bar+%. (`app_state.slint`, `stats.rs`, `controls.rs`, `player.slint`)
- [x] **#10 — Library search: left-key nav scrolls the view slightly** — Fixed: same Slint Flickable self-reference gotcha as #22. The `viewport-y` binding referenced `fl.height` and `self.viewport-height` (the Flickable's own layout props), which Slint doesn't reliably track as binding dependencies. Wrapped the Flickable in `fl-container := Rectangle` and replaced all self-references with `fl-container.height` and the `root.grid-content-height` helper property. (`home.slint`)
- [x] **Sign-out doesn't reset `settings_section`, `settings_focused`, `keybinding_focused`** *(code review)* — Fixed alongside sign-out playback fix: all three reset to -1 in `on_sign_out`. (`main.rs`)
- [x] **`video.lock()` inside `invoke_from_event_loop`** *(code review)* — Fixed: `start_playback` now accepts `series_id: Option<String>` and sets `playing_series_id` inside the player-init lock scope. The post-call `video.lock().playing_series_id = …` line is gone from all 9 callsites across `context_menu.rs`, `main.rs`, and `playback.rs` auto-advance.
- [x] **"Reset to Defaults" button missing `refocus()`** *(code review)* — Fixed: added `AppState.refocus()` to the button's clicked handler. (`settings.slint`)

#### LOW

- [x] **#21 — Subtitle list: no hover highlight, not mouse-scrollable** — Fixed: named each row's `TouchArea` per panel (`off-ta`, `sub-row-ta`, `aud-row-ta`, `vid-row-ta`) and added `has-hover ? Theme.surface` to the background ternary. Removed the `viewport-y` binding from all three Flickables (it blocked Flickable's internal scroll handler); keyboard scroll now uses a `property <length> kb-y: clamp(...)` on the outer Rectangle with `changed kb-y => { fl.viewport-y = kb-y; }`. Applied the same kb-y/changed pattern to every other scrollable Flickable in the codebase (detail, series, home/movies/TV dashboards, library grid, settings right pane) to enable native mouse-wheel scrolling app-wide. Also fixed self-reference bugs (`fl.height`, `self.viewport-height`) in those bindings by replacing with container/content heights. (`player.slint`, `detail.slint`, `series.slint`, `home.slint`, `settings.slint`)
- [x] **#11 — Stats overlay text cut off; redesign with section headers** — Replaced "Playback Stats" title + thin dividers with three bold spaced-caps section headers (VIDEO / AUDIO / SYNC). `StatRow` now uses `wrap: word-wrap` instead of `overflow: elide` so long codec/format strings wrap to a second line rather than being cut off. Labels shortened to IN/OUT within each section for clarity. (`widgets.slint`, `player.slint`)
- [x] **#24 — `I` key should only open stats overlay, not the full player overlay** — The player-mode key handler was calling `invoke_show_controls()` unconditionally before dispatching any action. Skip that call when `action == Action::ToggleStats` so pressing `I` toggles only `stats-visible`, leaving the controls bar untouched. (`keys.rs`)
- [ ] **#17 — Make icon backgrounds transparent** — the app icon SVG/PNG candidates have opaque white or coloured backgrounds; re-export with transparent background.
- [ ] **#18 — Add icon next to "Fjord" name in sidebar** — small logo/icon element in the left sidebar header area.
- [x] **#23 — Show subtitle track name instead of filename** — Added `external_filename` field to `TrackInfo` (reads `track-list/N/external-filename`). `build_track_model` now uses `title` if non-empty, otherwise falls back to the base filename (no path, extension kept) for external tracks. (`fjord-player/src/mpv.rs`, `fjord-app/src/playback.rs`)
- [ ] **#29 — Subtitle language preference setting** — user sets a preferred language (e.g. "en"); at playback start, auto-select the first subtitle track matching that language. Store in `Config`. Add to Settings → Playback section.
- [ ] **#34 — Add "ends at" clock to player** — compute `now + (duration - position)` and display as a formatted wall-clock time in the player controls bar.
- [ ] **#38 — Investigate massive frame drops with vsync=audio (intermittent)** — sporadic large spike in dropped frames, recovered by switching vsync mode. Likely mpv audio clock drift or Wayland frame timing issue. Log `frame-drop-count` periodically; reproduce and capture stats.
- [x] **Up Next banner and intro-skip never triggered from series screen** *(code review)* — `get_season_episodes` did not request `SeriesId` in its `Fields` parameter, so `MediaItem.series_id` was always `None` for episodes fetched from the series screen. The `playing_series_id.is_some()` guard in the 16 ms timer therefore never passed, suppressing both the Up Next banner and (implicitly) any series-aware logic. Fixed by adding `SeriesId` to the episode `Fields` query, plus a belt-and-suspenders fallback in `on_play_series_episode` that uses `series_open_id` when the field is still absent. (`fjord-api/src/client.rs`, `fjord-app/src/main.rs`)
- [ ] **`.ok()` swallows `get_item_detail` error in play-from-start** *(code review)* — network failure silently disables intro-skip and auto-advance for the session. (`context_menu.rs:257`)

#### DOCS

- [ ] **Stale comment on `context-menu-focused`** *(code review)* — says old row order; actual: `0=Resume 1=PlayFromStart 2=MarkPlayed 3=Favourite 4=ViewDetails`. (`app_state.slint:161`)

---

### Settings — remaining steps

- [ ] **Step 3 — Playback section**: intro skipper mode (`always-ask` / `always-skip` / `never-skip`) in `Config`; `playback.rs` reads mode; toggle in Settings → Playback. Also: subtitle language preference (#29).
- [ ] **Step 4 — Appearance section**: accent colour selection from a small palette; layout variants if needed.
- [ ] **Step 5 — Dashboard section**: per-row visibility toggles for home/movies/TV rows; stored in `Config`.
- [ ] **Step 6 — Server section**: open Jellyfin server admin web UI (launch browser or embed WebView).

---

### Remaining Phase 5 items

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
- `--htpc` / `--fullscreen` CLI flags — keyboard nav covers the use case for now
- Person detail screen (depends on cast row nav above)
- Dashboard row reorder (drag-to-reorder, Phase 5 Step 5)
