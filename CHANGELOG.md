# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(applied loosely, since Fjord is an application, not a library with a public
API — a minor bump marks a genuine new capability pillar, a patch bump
marks further work within an already-established one; see the 0.2.x/0.3.x/
0.4.x entries below for what that looked like in practice).

Fjord didn't version its releases until 2026-07-29 — every commit before
that just carried the git-based build id (`r<commit-count>.<short-hash>`,
still logged on every startup and shown in Settings, alongside the semver
now). The versions from 0.1.0 through 0.4.2 were tagged retroactively on
that date, mapping the existing 806-commit history onto version boundaries
chosen at genuine, separately-shippable milestones (not evenly-spaced
commit counts) so this file reads like a real changelog rather than a
phase-by-phase commit dump. Going forward, new tags are cut the normal
way, at the point a version is actually released — `Cargo.toml`'s
`workspace.package.version`, a `git tag vX.Y.Z`, and a new section here
are bumped together as one step, not separately.

## [Unreleased]

- Live testing of the Phase 183 Deep Seerr integration rows surfaced a real
  gap: every one of the 5 new rows had only error-path logging, nothing on
  the success path, making several of their own live-verification
  instructions unfollowable. Added `debug!` at every resolution branch and
  silent early-return across Person Other Work, Detail/Series Recommended,
  Collection Missing Items, Series Missing Seasons, and the Calendar's
  ongoing-series source.
- Music Bar's 8 transport/utility icons (Prev/Next/Shuffle/Repeat/Queue/
  Lyrics/VolDown/VolUp) restyled from a hand-rolled 36px TouchArea each to
  the shared 38px `IconCircleButton`, matching Now Playing's transport row
  — user-reported inconsistency: the volume buttons looked and felt
  different between the two screens, even though both already called the
  identical volume-up/-down callback underneath.
- Real bug, live-reported: clicking a BoxSet card with the mouse in the
  Collections library grid opened the generic Detail Page instead of
  `CollectionScreen` — `home.slint`'s `LibraryGrid` click handler always
  called `open-detail` with no check for item type, unlike the keyboard
  Confirm handler (`keys.rs`), which already checked `active-nav == 3`
  and called `open-collection` correctly. Mouse now matches keyboard.
- Log timestamps now local time instead of UTC (`chrono::Local`, avoiding
  the `time` crate's unsound-in-multithreaded-programs `LocalTime`) —
  user feedback that correlating log lines against wall-clock actions was
  confusing.
- **Screen-open cache staleness.** Root cause, traced from a live report
  ("Avatar's collection shows 3 unwatched but only 2 films"): Jellyfin's
  WebSocket only delivers `LibraryChanged` to the most-recently-connected
  client when multiple clients share a session (`JELLYFIN.md`'s own
  documented caveat) — editing through Jellyfin's own web UI while Fjord
  sits connected in the background can silently starve Fjord's connection
  of the event entirely, leaving a screen-open cache stale until the next
  full restart. Fixed by making every one of the 7 detail-style screens
  (Collection, Detail, Series, Season, Artist, Person, Album/Playlist)
  revalidate in the background on every open — even a cache hit, which
  still shows instantly, unchanged — and silently patch the cache and
  live UI if the fresh fetch differs, closing the gap for whatever's
  actually on screen right now. (A first pass also added a 10-minute
  repeating background sweep of all 6 caches; removed again the same day
  once revalidate-on-open covered the actual problem — nothing reads
  these caches for display outside the 7 screens they patch directly, so
  the sweep's only remaining value was a cosmetic "instant-show is
  already correct" edge case, not worth its own ~240-request/10min
  background cost.) Also fixed `spawn_missing_items` (Collection's
  "Missing From This Collection" row) to try every Movie-type BoxSet
  member in turn instead of giving up after the first — Avatar's own
  first member had zero Jellyfin `ProviderIds` at all, while a later
  member (e.g. a more recently-scanned sequel) was far more likely to
  resolve.
- **Code review of the above, requested after it shipped.** Two real bugs
  found and fixed: (1) none of the 7 screens' new revalidate paths
  checked `session_current` before writing into the shared caches — the
  same "background fetch writes per-user data into shared state" risk
  this codebase has already been bitten by and fixed twice before (once
  in `ws.rs`, once in `prewarm.rs`) — a sign-out or account switch on a
  shared HTPC mid-revalidation could have leaked one account's data into
  another's session; (2) Series screen's revalidate pass clobbered
  whatever season the user had since tabbed to back to season 0, since
  its season/episode fetch-and-store block wasn't gated by `revalidate`
  the way the equivalent UI update already was — playback still worked,
  but the episode title fell back to a raw id. Caught by an independent
  review pass, not the original author.

## [0.4.2] — 2026-07-19 – 2026-07-29

Watchlist, calendar, and Seerr data woven into the app's existing screens
instead of only living behind the Discover tab.

- Seerr Watchlist: add/remove from the Discover context menu or a
  RequestDetailScreen button, with a universal ★ badge on every card
  everywhere it appears — Discover, the Home/Movies/TV dashboards, and
  native Jellyfin library cards for items already owned.
- Release Calendar: a month-grid screen plus a "Coming Up" Discover row,
  sourced from watchlisted items, active requests, and ongoing
  (`Status == Continuing`) library series.
- Deep Seerr integration into existing native screens: Person gains an
  "Other Work" row, Detail/Series gain "Recommended", Collection gains
  "Missing From This Collection", and Series gains "Missing Seasons" with
  per-season request-status pills and a preselected Request Options flow.
- Widened the subtitle position range (50–150%, up from 50–100%) after
  confirming against the real mpv manual that 100 was never meant to be
  the screen edge.
- A long tail of real bugs found and fixed via live HTPC testing: a
  Watchlist API deserialize gap, a Coming Up row that silently never
  reached the UI (an off-thread `AppState` write that failed silently — a
  recurring failure mode this project had to learn to watch for), a
  startup self-deadlock from a re-entrant mutex lock, a watchlist star
  that never survived a screen rebuild, and the removal of all 209
  U+FE0E variation selectors across the UI as a likely source of tofu
  glyphs on the HTPC's font stack.

*Range: [`759f3b0`](../../commit/759f3b0)..[`2b53bba`](../../commit/2b53bba) (20 commits)*

## [0.4.1] — 2026-07-16 – 2026-07-18

Quality profiles, filters, and the full request lifecycle.

- Radarr/Sonarr quality profile picker and tag picker for requests.
- Redesigned RequestDetailScreen: rating badge, collapsible overview,
  Cast & Crew row, streaming-provider panel.
- Discover Filters: type, genre, sort order, rating, year, and streaming
  provider — both a filtered-browse view and a client-side pass over
  search results.
- Watch Trailer, playing Seerr's YouTube trailer links through mpv's
  `ytdl_hook`.
- Streaming Region, Display Language, and Discover Language settings
  written back to the connected Seerr account; the `seerr_enabled` toggle
  now actually gates the whole integration live.
- Full request-lifecycle context menu: View Details, Request, Edit
  Request, Cancel Request, Approve, Decline.

*Range: [`4c2aea6`](../../commit/4c2aea6)..[`759f3b0`](../../commit/759f3b0) (35 commits)*

## [0.4.0] — 2026-07-15 – 2026-07-16

Discover screen + request flow — the pivot from "Jellyfin viewer" toward
"general media client" begins.

- New Discover tab: search, plus Trending/Popular/Upcoming landing rows.
- Per-item request submission via a Request Options modal (2K/4K quality,
  per-season picker for TV).
- A "Requested" row, and opening an already-owned item redirects straight
  to its real Jellyfin detail page instead of the Seerr one.
- Every new Seerr endpoint and response shape was verified directly
  against Seerr's real TypeScript source rather than the OpenAPI spec,
  which this integration repeatedly found to be incomplete or wrong.

*Range: [`1e9d424`](../../commit/1e9d424)..[`4c2aea6`](../../commit/4c2aea6) (8 commits)*

## [0.3.4] — 2026-07-09 – 2026-07-15

Feel and consistency polish.

- Universal press/click feedback animation, working identically for mouse
  and keyboard.
- Configurable global scroll/animation speed (0–500%), `FadeGate` for
  screen exit fades, `ease-out-expo` easing.
- Bundled Inter (default UI font, user-selectable) and Noto symbol fonts,
  so icon glyphs render consistently regardless of what's installed on a
  given HTPC.
- Fjord's own client version shown in Settings, configurable seek step,
  subtitle appearance settings, per-series remembered audio/subtitle track.

*Range: [`9be9c24`](../../commit/9be9c24)..[`1e9d424`](../../commit/1e9d424) (29 commits)*

## [0.3.3] — 2026-06-28 – 2026-07-09

Sync hardening, playlists, and a caching/performance pass.

- Incremental WebSocket delta sync with focus safety; self-healing 404s
  for ghost items; artwork revalidation via `ImageTags` sidecars.
- Full Jellyfin Playlist API (list/create/add/remove/items), a Playlists
  library view, and an Add-to-Playlist context-menu picker.
- Auto mark-played at the credits trigger, with a rewind-revert safety
  net so skipping back out of the credits un-marks it again.
- Screen-open result caching (six caches, persisted to disk) and an
  opt-in full-library metadata/image prewarm.
- Two full-codebase code reviews. **CR10** (25 findings): a
  `Handle::current()` panic in artist/album favorite/played callbacks, a
  Jellyfin auth token leaking into playback-URL log lines, the Quit
  keybinding being silently stealable by the queue panel (now a global
  `Ctrl+Q`), a WebSocket UTF-8 panic that could kill the reconnect loop
  permanently, sign-out deleting `device_id` and settings instead of just
  auth, and Up Next resolving the next episode read-only instead of
  marking it played prematurely. **CR11** (16 findings, everything added
  since CR10 — gapless playback, the persistent queue/playlist model, Now
  Playing, Jellyfin playlists): a BoxSet card that tried to play a dead
  stream URL, a WS delta-refresh task that wasn't covered by the
  sign-out abort handle, three screens missing the post-mouse-use
  keyboard-focus re-grab, and a gapless edge case that could report a
  track as playing when it never produced audio.

*Range: [`7e1ee12`](../../commit/7e1ee12)..[`9be9c24`](../../commit/9be9c24) (130 commits)*

## [0.3.2] — 2026-06-28

Playback queue, MusicPlayerBar, gapless, Now Playing, Lyrics.

- Playback queue: Play Next / Add to Queue from the context menu, a queue
  viewer panel.
- Playlist backend with Prev/Next/Shuffle/Repeat, and the MusicPlayerBar
  bottom-bar layout.
- Gapless music playback (same mpv instance, no gap between tracks).
- A fullscreen Now Playing screen with synced lyrics.

*Range: [`95c3829`](../../commit/95c3829)..[`7e1ee12`](../../commit/7e1ee12) (3 commits)*

## [0.3.1] — 2026-06-25 – 2026-06-27

Music library core.

- MusicDashboard, AlbumScreen, ArtistScreen, and audio playback.
- Favorites rows on the Music dashboard, an Artists/Albums view toggle
  with full keyboard navigation, and a redesigned music sort bar.
- **CR8**: six Collections-screen bugs (a blank screen on a failed BoxSet
  fetch instead of a toast, `show-collection` surviving sign-out and
  routing keys to a null client, a stale-request race on rapid re-open).
  **CR9**: a fallback so a BoxSet dashboard card still opens correctly
  before the library grid has ever populated `all_collections`, and two
  poster-loading races that could overwrite the wrong library grid if the
  user switched tabs mid-fetch.

*Range: [`a1eee6c`](../../commit/a1eee6c)..[`95c3829`](../../commit/95c3829) (60 commits)*

## [0.3.0] — 2026-06-25

WebSocket real-time events, chapter navigation, Collections screen.

- Phase 41: WebSocket-driven real-time sync begins (library changes,
  favorites, playback state).
- Chapter navigation (`,`/`.` keys, seek bar tick marks, OSD) and
  sub/audio delay adjustment (`z`/`Z`/`x`/`X`), both with mouse-accessible
  panels in the player controls bar.
- A new Collections library screen, with its own dashboard rows and disk
  cache.

*Range: [`d12a9d4`](../../commit/d12a9d4)..[`a1eee6c`](../../commit/a1eee6c) (9 commits)*

## [0.2.3] — 2026-06-21 – 2026-06-25

Mini-player redesign, library grid sort/filter, error toasts.

- Floating corner mini-player bar, replacing the old sidebar-docked
  version.
- Library grid sort/filter/search with an alphabet scrubber (Phase 34).
- Error toast notifications (Phase 35).
- Two full-codebase code reviews. **CR6** (13 findings): sign-out now
  actually clears episode/collection caches, `movies_fetched`, and every
  overlay flag; `get_all_movies`/`get_all_series` paginate through a
  shared helper instead of risking a silent truncation on a large
  library; season-screen portraits load in parallel instead of trickling
  in. **CR7** (15 findings): a duplicated always-skip auto-advance path
  that raced against the natural-end fallback, Next Up's favorite flag
  hardcoded false, and several dead/backwards keyboard-nav guards on the
  season/series header buttons.

*Range: [`7efff5f`](../../commit/7efff5f)..[`d12a9d4`](../../commit/d12a9d4) (61 commits)*

## [0.2.2] — 2026-06-21 – 2026-06-23

Detail/Series/Season content enrichment + Person screen.

- Cast and crew portraits, a collection row and similar-items row on the
  movie detail page, collapsible Storyline sections.
- A full season detail screen, series detail UX polish (watched button,
  poster badges, season focus).
- A new Person screen (portrait, bio, filmography).
- **CR5**: post-detail-enrichment bug fixes — a cast portrait index
  mismatch, `fetch_movie_collections` not spawned on the auto-login path,
  a CastRow focus-ring visibility bug, and stale collection/similar-items
  models surviving a Back navigation.

*Range: [`913c90c`](../../commit/913c90c)..[`7efff5f`](../../commit/7efff5f) (99 commits)*

## [0.2.1] — 2026-06-14 – 2026-06-21

Settings matures, player UX polish, Intro Skipper complete.

- Two-pane Settings screen with live keybinding rebinding.
- Netflix-style Up Next banner, a transient volume overlay (SPDIF
  passthrough-aware), subtitle language/type preferences.
- Player UI redesign (seek tooltip, buffering overlay), per-format SPDIF
  passthrough toggles (AC3/EAC3/DTS/DTS-HD/TrueHD).
- Per-segment Intro Skipper skip modes with configurable timers.
- Three full-codebase code reviews. **CR1** (10 findings): stale
  intro/credits background tasks, a report-ordering bug, pause desync, a
  semaphore bypass, auto-login timeout handling, and a Not-Watched timer
  timestamp bug — plus a `reset_playback_ui`/`fetch_image_cached` cleanup
  pass. **CR3** (9 findings): hidden video-latency-hacks activation, a
  SPDIF warning that misfired with every format switched off,
  seek-dragging getting stuck on Wayland, and a null-deref crash in
  deinterlace-setting deserialization. **CR4** (10 findings): a
  `Player::new` error-cleanup gap, poster-loader panics not flushing
  their `JoinSet`, an Up Next countdown off-by-one, and a mid-session 401
  not redirecting to the login screen.

*Range: [`cb48e1d`](../../commit/cb48e1d)..[`913c90c`](../../commit/913c90c) (202 commits)*

## [0.2.0] — 2026-06-12 – 2026-06-14

Context menu + canonical state store.

- Right-click / `C`-key context menu: Mark Played, toggle Favourite, Play
  from Start.
- A canonical `update_item_user_state` store keeps `FjordState` in sync
  across every card model after a played/favorite toggle, instead of each
  call site patching its own copy.

*Range: [`38cd536`](../../commit/38cd536)..[`cb48e1d`](../../commit/cb48e1d) (104 commits)*

## [0.1.0] — 2026-06-11 – 2026-06-12

Proof of concept.

- The mpv render API works: OpenGL FBO compositing with Slint,
  `report_swap()` called every frame for genuine vsync feedback — the
  reason this project exists, since every Flutter/`media_kit`-based
  Jellyfin frontend skips this call and stutters on NVIDIA legacy drivers
  under Wayland.
- A minimal but real client around it: login, movie/TV library grids with
  posters, item detail pages, series → season → episode drill-down, and a
  keyboard-driven player with subtitle/audio track panels.

*Range: [`2cf755a`](../../commit/2cf755a)..[`38cd536`](../../commit/38cd536) (45 commits)*
