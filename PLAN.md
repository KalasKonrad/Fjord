# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

---

## Completed

Full curated version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Full implementation detail per feature: `CLAUDE.md`.

---

## Pending

- **Phase 183 (Deep Seerr integration, 5 new rows/sources across 4 screens) — live verification, largest untested surface after this session.** Build/clippy/test clean throughout, not live-tested at all (same standing sandboxed-environment limitation). In rough priority order: (1) Series Missing Seasons — does the row actually appear for a real partially-owned show, does the per-season pill correctly distinguish "requested"/"processing"/nothing, does clicking an unrequested season open Request Options pre-checked to exactly the right seasons (and only the still-unrequested ones), does clicking an already-requested season open RequestDetailScreen normally; (2) Person Other Work — does the TMDB person-id resolution actually succeed for real actors (the fuzzy name-search fallback is the least certain part of this whole feature), does the row correctly exclude titles already in the local filmography; (3) Collection Missing Items — does the two-tier TMDB-collection-id resolution work against a real franchise BoxSet (check `fjord.log` for which path — free ProviderIds vs. guaranteed member-movie fallback — actually resolved it), does it correctly show only genuinely-missing franchise entries; (4) Detail/Series Recommended — do the rows appear below More Like This with sensible not-owned recommendations; (5) Calendar — do ongoing library series now appear in Coming Up even without being watchlisted/requested, and does the existing watchlist/requested cap-20 slice still behave unchanged; (6) keyboard nav across all 4 new zones — especially the corrected Series Episode-row Down chain (Missing Seasons → Cast → Similar → Recommended) and each new row's own Left/Right/Up/Confirm; (7) a Seerr disconnect/reconnect or sign-out genuinely clears `person_tmdb_id_cache`/`person_other_work_cache` (no stale cross-account data).

- **Phase 182 (U+FE0E removal, 209 sites) — live HTPC verification, highest priority of the two glyph items below.** Not testable in this sandboxed dev environment (same standing screenshot-verification limitation this doc has noted before). Check specifically: the fullscreen player's pause/play icon (the exact one reported — `player.slint`'s controls-bar button and the 84px pause/play flash overlay), the music bar's transport row (⏮/⏸/▶/⏹/⏭/⇌/↺/⋮/♪), Now Playing's transport row, and a broad glance over Detail/Series/Season/Album/Artist/Collection/Context-menu/Settings for any remaining square — if one still shows up after a rebuild, check that specific glyph's cmap coverage fresh (per this phase's own honest caveat) rather than assuming the fix was incomplete.

- **Phase 181 (audio-only-video diagnostics) — watch for recurrence.** No fix was attempted (user's explicit choice) — just check every future `fjord.log`/`fjord.log.old` for: `no VideoReconfig event ... video may be stuck audio-only` (the new 5s one-shot warning), and any `mpv[...] warn:`/`mpv[...] error:` lines (mpv's own internal log, now surfaced — this is the one most likely to actually explain the root cause if it recurs). If either appears, that's the signal to come back and actually fix this rather than keep watching.

- **Phase 166 (Discover Filters) live verification** — build/clippy/test clean, not live-tested, largest untested surface after Phase 160-164: confirm the Genre/Provider popups actually populate from the real instance (both media types); confirm selecting 2+ genres and 2+ providers genuinely broadens results (OR, not AND) matching Seerr's own web UI with the identical selection; confirm Sort/Rating/Year actually narrow/reorder the filtered-browse grid correctly per media type (movies' `primary_release_date.*` vs TV's `first_air_date.*` sort keys); confirm Type=All genuinely interleaves movies and TV in one sorted grid rather than two blocks; confirm switching back to every filter at default returns to the original 6 landing rows unchanged; confirm filter selections survive an app restart (Config persistence); confirm client-side search filtering (genre/rating/year/sort on a typed query) doesn't break `discover-load-more` pagination (Phase 154) or the autofill-grid behavior (Phase 156); confirm the Provider pill visibly de-emphasizes (not fully hides) while a search query is active, and that it has no effect on search results (by design — TMDB's search response carries no provider data); confirm mouse clicks on filter pills and popup rows sync keyboard focus correctly, same focus-desync bug class as `RequestOptionsOverlay`'s own confirm row; confirm the popup overlay's fixed pixel dimensions (220×190 / 340×76) actually fit their real content without clipping/overflow at real font metrics — this couldn't be rendered in the sandboxed dev environment this was built in.

- **Phase 167 (Seerr keyboard navigation, 15-bug fix pass) live verification** — build/clippy/test clean throughout, not live-tested, the largest cross-cutting Seerr surface touched so far since it spans every screen rather than one feature. Particularly worth checking: (Group 1) clicking the search field then a filter pill no longer leaves both focused — a following keypress should reach the popup, not the hidden search field; clicking a landing-row card correctly moves keyboard focus to it (arrow keys after the click act on the clicked card/row, not a stale one); clicking the Storyline header on RequestDetailScreen, then pressing Enter, activates the storyline (not a stray button); clicking Request/Trailer/⋮More after previously arrowing to Back no longer closes the whole screen on the next Enter. (Group 2) with a filter active and the query empty, arrow keys/Enter in the content area act on the real filtered-browse grid, not invisible landing-row data; opening the Request Options modal, then pressing R while a background player exists, no longer leaves the modal stuck on top of a resumed fullscreen video. (Group 3) pressing R, or having stale music-bar/mini-player-bar focus from earlier navigation, no longer hijacks RequestDetailScreen/the Request Options modal's own keys. (Group 4) opening a filter popup, switching to another sidebar tab, then returning to Discover — the popup should NOT still be open; closing ConnectSeerrScreen mid-Quick-Connect and reopening it should show a fresh method picker, not a stale "waiting for approval" view. (Group 5) an already-requested item's context menu, opened from the search grid or a Trending/Popular/Upcoming card (not just the Requested row), correctly offers Edit/Cancel/View Request instead of Request; submitting a request from the search grid immediately updates that same card's context menu without needing to reopen Discover; approving a request from the context menu removes the "Cancel Request" option from that card (both in the Requested row and, if visible, the search grid) without a restart. (Group 6) if the connected account's Seerr permissions change server-side, revisiting the Discover tab (not just reconnecting) picks up the new Approve/Decline visibility. (Group 7) Down from the Discover search field now lands on the filter bar, matching Up, rather than jumping straight into results.

- **Phase 174 (Watchlist + Release Calendar) live verification** — build/clippy/test clean, not live-tested since Phase 174's fixes landed. Confirmed working live already: the threading fix (Phase 171), posters/day-cell-titles (Phase 172), and the watchlist toggle itself completing server-side (Coming Up correctly gains newly-watchlisted items with upcoming dates, per the Phase 174 report) — no need to re-check those. Check first: **the calendar header fix (Phase 173.1, second attempt)** — still worth a fresh screenshot since the first attempt looked fine in text review but wasn't; **Left/Right nav (Phase 173.2)** — Back just navigates, day-grid row edges roll into the adjacent month; **reopening the context menu on a Coming Up card now correctly shows "Remove from Watchlist"** for an item that's actually on the watchlist, not "Add" (Phase 174's `CalendarEntry.on_watchlist` fix — this was reliably wrong before, should now be reliably right); **an already-in-library item's context menu now has a Watchlist row at all** (Phase 174's new row 8 on the Jellyfin menu family) — check it's visible only when Seerr is connected and the item has a real TMDB match, and that toggling it works the same as the Discover-side row; **the toast question is still fully open** — check `fjord.log` for `seerr: discover_toggle_watchlist ...`/`show_toast: ...`/`seerr: patch_watchlist_on_all_models ... -> patched N card(s)` debug lines, since the toggle completing successfully doesn't yet explain why no toast was seen. Then, still worth checking (unchanged from before): **the RequestDetailScreen button row with a genuine 4th slot** (Request-or-pill + Trailer + ⋮More + Watchlist) actually lays out correctly and doesn't overflow (documented 3-attempt layout-bug history this session); Watchlist add/remove round-trips and is reflected immediately in the context menu, the RequestDetailScreen button, and the Coming Up row; movie release-date types/region resolution produce sane real dates for a few real movies, and that Discover Region genuinely resolves independently from Streaming Region against a live account; the month-grid calendar's real nested-Layout grid actually renders/scrolls correctly at real font metrics; the day-popup's keyboard nav and its two-level Back (popup then screen); the "📅 Full Calendar" sentinel card visually reads as distinct from a normal poster card AND its keyboard Enter/`C` special-case opens the calendar instead of a garbage item/context-menu; "New in Theaters" shows sane recent-looking releases, not stale or future ones; `AppMode::Calendar`/`CalendarDayPopup` don't leak into or get hijacked by `ResumePlayer`/music-bar/mini-player-bar/`QueuePanel`/`NowPlaying`; watchlist/calendar caches genuinely clear on sign-out and on Seerr disconnect/reconnect (no stale data from a previous account/server).

- **Phase 175/176/177 (Watchlist row + dashboard rows + universal star badge) live verification** — build/clippy/test clean. Two real bugs already confirmed fixed via direct `cargo run` reproduction on the dev machine: the startup self-deadlock (Phase 176, "fjord do not even start") and the star never surviving a screen rebuild (Phase 177, "the watch list symbol do not show up on items i the library screens" — confirmed the resync now finds real local matches, `-> 4 local match(es)`, on the second of its 4 trigger points). Neither has been checked visually in a real running window yet, only via log evidence — worth an actual eyes-on look at the Library Grid (Movies/TV) to confirm the star renders correctly, not just that the underlying data is now correct. Still not verified at all: the star badge's fixed pixel position doesn't visually collide with a simultaneous progress bar on a partially-watched + watchlisted in-library item; the new Discover Watchlist row (row 8) scrolls to correctly via keyboard past Coming Up's own `row-y` term, and its cards open the right thing (in-library redirect vs. RequestDetailScreen) on Enter; the Home/Movies/TV dashboard Watchlist rows appear immediately after login (confirmed populated in the log — `push_watchlist_rows -> mixed=5 movies=4 tv=1` — but not yet confirmed reachable/correct via actual keyboard nav), and Up/Down/Left/Right correctly reach/skip the new 6th row on all three dashboards — the highest-risk part of this change, since the corrected `app_state.slint` ladder edit could still be subtly wrong in a way no compiler catches; toggling watchlist on an in-library item shows/hides its star on every surface it appears (library grid, Continue Watching, etc.) without a restart, including the carry-forward paths (`refresh_row_preserving_posters`/`upsert_cards_in_model`) that were never live-exercised, only reasoned through; toggling watchlist on a Discover-only item updates its star on every Discover row it's visible in, including the new Watchlist row itself; a Seerr disconnect/reconnect clears the 3 new dashboard-watchlist properties instead of showing stale data from the previous connection; **also worth an HTPC pass specifically** — every fix in this whole chain (175-177) was found and fixed on the dev machine only, per the standing HTPC-testing-cadence gap.

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
