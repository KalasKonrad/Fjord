# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

---

## Completed

Full curated version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Full implementation detail per feature: `CLAUDE.md`.

---

## Pending

All 4 items below had a rigorous code-review pass (2026-07-31, 4 parallel agents) — 7 real bugs found and fixed (see CHANGELOG.md), everything else listed here checked out clean in the code but still needs an actual live pass.

- **Phase 183 (Deep Seerr integration) — live verification.** Confirmed via a real 2026-08-01 dev-machine log: Calendar ongoing-series, Missing Seasons keyboard nav, and **Collection Missing Items** (watched it live — Avengers/Bourne collections both initially failed to resolve on a stale cache-hit, then correctly resolved via the multi-member loop on the next open once the background revalidate had refreshed `boxset_items_cache`'s ProviderIds — the multi-member fix and the revalidate-on-open system working together as designed, not just individually); **Detail's Recommended row** also confirmed with real data (13 not-owned recommendations for a real movie). Since that log: fixed the "next open" part above — a first-attempt failure now awaits the parallel revalidate and retries once in the same open, no reopen needed (needs its own live check). Still open: (1) Series' own `spawn_recommended` (Detail's twin implementation — not exercised in the same log, only Detail's was); (2) Series Missing Seasons — per-season pill correctness, Request Options preselect on click; (3) Person Other Work — TMDB person-id resolution accuracy for real actors; (4) a Seerr disconnect/reconnect or sign-out clears `person_tmdb_id_cache`/`person_other_work_cache`.

- **Discover search-grid flash — live verification.** Two fixes (2026-08-02): fresh-query commits now carry posters forward by id instead of blanking overlapping results, and page 2/3/4 auto-load now appends to the live model instead of swapping it (true incremental append). Build/clippy/test clean; not yet watched live.

- **Coming Up dashboard rows + watchlist watched-state sync — live verification.** All 2026-08-02, user-requested. Coming Up now shows on Home (mixed)/TV (series-only)/Movies (movies-only) dashboards, same split as the Watchlist rows; a `section-y` scroll-position gap for the Watchlist row's own height (never needed until Coming Up landed right after it) was caught and fixed in the same pass. Watched items are now kept in sync with watchlist membership both directions: marking something watched removes it from the watchlist (hooked into WS `UserDataChanged`, covers Mark Played/credits auto-mark/other clients alike), and re-adding an already-watched item to the watchlist marks it unwatched instead (a real `mark_unplayed` call, not just a local flag). Build/clippy/test clean; not yet watched live — especially whether Jellyfin reports a Series as played (and thus triggers removal) only once every episode is watched, or sooner.

- **Phase 182 (U+FE0E removal) — live HTPC verification.** The removal itself is confirmed clean (re-verified via direct byte-sequence grep). Most likely actual fix for the originally-reported tofu square: the pause/play flash icon's missing font pin, now fixed. Check that specific icon on the HTPC first.

- **Phase 181 (audio-only-video diagnostics) — watch for recurrence.** No fix attempted (user's choice) — check future logs for `no VideoReconfig event` or `mpv[...] warn/error:` lines if it recurs.

- **Phase 166+167 (Discover Filters + keyboard nav) — live verification.** Stale filter-bar-focus on card clicks and Type=All pagination sort-order are fixed. Still open: Genre/Provider popups populate correctly from a real instance; multi-select genuinely broadens results (OR not AND); Sort/Rating/Year narrow/reorder correctly; popup overlay fixed pixel dimensions fit real content; the remaining Groups 1/4/5 of the keyboard-nav pass (mouse-click focus sync on Storyline/Request buttons, filter-popup staying closed across tab switches, context-menu request-state completeness).

- **Phase 174-177 (Watchlist + Calendar + dashboard rows) — live verification.** Calendar ongoing-series, dashboard Watchlist rows, star-badge resync, and the RequestDetailScreen button-row overflow are confirmed/fixed. Still open: the watchlist-toggle toast mystery (confirmed NOT a silent-return bug — every path through `discover_toggle_watchlist` calls `show_toast`, root cause still unknown); calendar month-grid rendering at real font metrics; day-popup keyboard nav; Home/Movies/TV dashboard Watchlist row keyboard reachability (Up/Down/Left/Right hitting the new 6th row correctly); star badge doesn't visually collide with a progress bar; a full HTPC pass — everything in this whole feature has only ever been tested on the dev machine.

---
## Issues
(none open)


## future additons

(none open — the two items previously listed here, watchlist/calendar and search filters, both shipped in Phases 166/168 and are documented in CLAUDE.md's Seerr integration section)


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
- **Trickplay** — seek bar scrub thumbnail popup. Requires: fetch Jellyfin trickplay manifest (`GET /Videos/{id}/Trickplay/{width}/tiles`), parse tile sheet dimensions (tile size, columns, rows, interval), cache tile images per video, render a thumbnail above the seek bar while scrubbing (position computed from `seek-hover-pos`). Deferred because it's a separate subsystem from chapter nav and the API surface needs more investigation.
- **Multi-account / multi-server support** — currently Fjord stores one server URL + one user session in `config.json`. To support multiple accounts: `Config` would need a `Vec<ServerProfile>` (each holding server URL, device ID, username, token) with an `active_profile: usize` index; the login screen would gain a server-picker step; sign-out would become "switch profile" rather than "clear everything"; the `FjordState` runtime fields (`all_movies`, `all_series`, caches, etc.) would be cleared and repopulated whenever the active profile changes. CR6-1 (sign-out cleanup) is a prerequisite — it establishes the correct invariant that switching users produces a clean slate, which multi-account support then relies on.
- **Poster/card scaling setting** — from Phase 117: with titles/episode names now always shown in full (no truncation), a very long title on the smallest card breakpoint (115px wide) shrinks that card's poster noticeably to make room. User's proposed fix: a settings toggle to use larger posters in the library/dashboard (more text budget per card before this becomes visible) rather than reintroducing a truncation cap. Would likely hang off the existing `dash-card-w`/`dash-card-h` breakpoint functions in `main.slint`, e.g. as a user-chosen size multiplier or an extra breakpoint tier. Alternative approach floated in the same conversation, not mutually exclusive: render the poster image full-bleed behind the title/subtitle text with a dark semi-transparent scrim, instead of a separate text block below the poster — sidesteps the whole shrink-to-fit problem structurally rather than giving the text more room. Neither implemented yet; revisit when picked up.
