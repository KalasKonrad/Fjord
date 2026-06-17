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

## Open Work

### Bug fixes — HTPC testing (June 2026)

Issues found during real-world HTPC testing, ordered by severity.

#### CRITICAL

- [ ] **#30 — Crash when starting a new video while another plays in background** — **investigate first**: reproduce, capture the panic/backtrace from `~/.cache/fjord/fjord.log`, then fix. Suspected causes: double-init of player/render context, or mutex deadlock during teardown race when `render_ctx`/`player` are not yet `None`.

#### HIGH — Playback & progress

- [ ] **#37 — Auto-advance overlay never shows; next episode starts only after video fully ends** — `wire_mpv_timer` detects end-of-file after `mpv_event::EndFile`; the 5-second countdown banner (`next_ep_pending`) is set too late or the timer branch is not firing. Investigate `on_end_file` / `playing_series_id` path.
- [ ] **#36 / #35 / #26 — Stop report reliability** — three related symptoms of the same problem: (a) #36 pressing Stop removes the continue-watching entry, (b) #35 continue-watching disappears after restart (intermittent), (c) #26 quitting Fjord skips the stop report entirely (`std::process::exit` kills in-flight tasks). Fix as one investigation: ensure the stop report always fires and completes before the home cache is written; on quit, block until the report is sent.

#### HIGH — Player overlay / HUD

- [ ] **#32 — mpv's own OSD shows when seeking or changing volume** — mpv renders its internal OSD (progress bar, volume indicator) into the FBO. Suppress it by setting `osd-level=0` in `PlayerConfig` (or `player.set_property("osd-level", 0)` after init).
- [ ] **#31 — Overlay progress bar shows time but no bar** — likely the `Rectangle` centering bug (plain Rectangle children are horizontally centred by default; progress bar needs `x: 0`). Check `player.slint` seek-bar or progress bar element.
- [ ] **#33 — No volume overlay when changing volume** — volume changes via `VolumeUp`/`VolumeDown` keys have no UI feedback. Add a transient volume level indicator to the player overlay (similar to intro-skip banner), shown for ~1.5 s then fading.
- [ ] **#34 — Add "ends at" clock to player** — compute `now + (duration - position)` and display as a formatted wall-clock time in the player controls bar.

#### HIGH — Platform

- [ ] **#28 — KDE dims screen / turns off display during playback** — `org.freedesktop.ScreenSaver.Inhibit` blocks blank/lock but not KDE's separate display-dim power management timer. Fix: also call `org.kde.PowerManagement.Inhibition` → `Inhibit(appname, reason)` at playback start and `UnInhibit(cookie)` at stop. (`playback.rs inhibit_screensaver`)
- [ ] **#27 — vsync setting has no effect; stats always shows "audio" mode** — if `video-sync=display-resample` isn't reaching mpv the whole NVIDIA vsync improvement is silently inactive. `video_sync` from `Config` may not be reaching `PlayerConfig` correctly, or mpv is overriding it. Log the active `video-sync` property after playback starts and verify the settings round-trip.
- [ ] **#19 — Backspace/Escape behaviour in player** *(UX redesign — confirm before implementing)* — user expects: Backspace always minimizes (mini card in sidebar even when "video in background" is off), Escape always stops. This changes the three-mode playback design: a third mode where Backspace puts video in the mini card regardless of the setting. Discuss and agree on the behaviour before touching `dispatch_player` in `keys.rs`.
- [ ] **#39 — Audio dropout when vsync=audio with bitstream passthrough** — investigate interaction between `video-sync=audio` and SPDIF passthrough; may need `video-sync=display-resample` when passthrough is active, or a different `audio-device` path.
- [ ] **#40 — Volume control should show it has no effect during passthrough** — when SPDIF passthrough is on, mpv volume control does nothing. Show a visual indicator ("Volume: passthrough") or disable the volume bar.

#### CRITICAL — Code review

- [ ] **Player settings wiped on re-login after sign-out** — `do_login` calls `load_config().unwrap_or_default()`, but sign-out deletes `config.json`, so re-login produces `Config::default()` and overwrites all player settings. Fix: read existing `s.config`, patch only auth fields (`server_url`, `user_id`, `token`, `device_id`), then save. (`auth.rs:37,65`)

#### HIGH — Code review

- [ ] **Sign-out doesn't stop active playback** — `on_sign_out` never calls `invoke_stop_playback`, clears `is_playing`, or clears `has_background_player`. mpv keeps running behind the login screen. (`main.rs`)
- [ ] **`item_type` never set in poster loaders** — `spawn_poster_loading`, `spawn_series_poster_loading`, `spawn_movies_poster_loading` all build `CardItem` without `item_type`, overwriting the correct type when posters arrive. Context-menu "View Details" on a Series card opens a movie detail page. (`poster.rs:122,191`, `movies.rs:59`)

#### MEDIUM — Code review

- [ ] **Sign-out doesn't reset `settings_section`, `settings_focused`, `keybinding_focused`** — stale nav state persists into next session. (`main.rs on_sign_out`)
- [ ] **`video.lock()` inside `invoke_from_event_loop`** — series/movie play-from-start paths lock the video mutex on the Slint event-loop thread, which can block if the GL rendering notifier holds the lock during `mpv_render_context_render`. (`context_menu.rs:239`)
- [ ] **"Reset to Defaults" button missing `refocus()`** — loses keyboard focus permanently after click. (`settings.slint:485`)

#### LOW — Code review

- [ ] **`.ok()` swallows `get_item_detail` error in play-from-start** — network failure silently disables intro-skip and auto-advance for the session. (`context_menu.rs:257`)

#### DOCS — Code review

- [ ] **Stale comment on `context-menu-focused`** — says old row order; actual: `0=Resume 1=PlayFromStart 2=MarkPlayed 3=Favourite 4=ViewDetails`. (`app_state.slint:161`)

#### MEDIUM — Keyboard & mouse nav

- [ ] **#20 — Can't navigate to Resume button in mini card with keyboard** — when a video is minimized to the sidebar "Now Playing" box, the Resume button is not reachable by keyboard. Add keyboard focus path from the sidebar to the mini card Resume button.
- [ ] **#22 — Subtitle list not scrollable with keyboard** — track panel subtitle list needs Up/Down nav. Currently the panel cursor moves but if the list overflows the visible area there is no scroll. Use `Flickable` or clamp `viewport-y` to the focused index.
- [ ] **#25 — Slight stutter when navigating to Browse All in sidebar** — the browse screen population (`populate_browse`) runs synchronously on the event loop thread; move to a background task or cache the filtered list so the UI is instant.
- [ ] **#10 — Library search: left-key nav scrolls the view slightly** — viewport-y changes by a small amount on Left press in the library grid header-focused mode. Investigate the `Flickable` / grid focus interaction.

#### LOW — UI polish

- [ ] **#21 — Subtitle list: no hover highlight, not mouse-scrollable** — add `hover-color` to track panel list rows and make the `Flickable` respond to mouse wheel scroll.
- [ ] **#11 — Stats overlay text cut off; redesign with section headers** — split into Video / Audio / Sync sections with headers; ensure long values (codec strings, etc.) wrap or truncate with ellipsis.
- [ ] **#24 — `I` key should only open stats overlay, not the full player overlay** — currently `I` calls `invoke_show_controls()` which shows the whole controls bar. Change to toggle only the stats panel (`show-stats`), leaving controls hidden if they were hidden.
- [ ] **#17 — Make icon backgrounds transparent** — the app icon SVG/PNG candidates have opaque white or coloured backgrounds; re-export with transparent background.
- [ ] **#18 — Add icon next to "Fjord" name in sidebar** — small logo/icon element in the left sidebar header area.

#### LOW — Features

- [ ] **#23 — Show subtitle track name instead of filename** — mpv provides `track-list/N/title` (the metadata title) alongside `track-list/N/external-filename`. Display `title` if available, fall back to the base filename.
- [ ] **#29 — Subtitle language preference setting** — user sets a preferred language (e.g. "en"); at playback start, auto-select the first subtitle track matching that language. Store in `Config`. Add to Settings → Playback section.
- [ ] **#38 — Investigate massive frame drops with vsync=audio (intermittent)** — sporadic large spike in dropped frames, recovered by switching vsync mode. Likely mpv audio clock drift or Wayland frame timing issue. Log `frame-drop-count` periodically; reproduce and capture stats.

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
