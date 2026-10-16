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

- **Fixed: "1 tracks", "1 episodes", "1 profiles"** and similar — counts
  now use the singular for one. Long series names and dialog titles wrap
  instead of being cut off, and the mouse shows a hand over every button and
  menu entry (a text cursor over search fields).
- **Improved: an empty search says so.** Searching a library or Browse All
  (or filtering a library by Unwatched/Favorites) with no matches now shows
  "No results for …" instead of a blank screen, like Discover does.
- **New: Settings → Interface → Text size** — Normal, Large or Extra large
  text everywhere (menus, cards, settings, the player), saved per profile.
  Headings stay as they are.
- **Improved: clearer error messages.** They now read "Couldn't … — reason"
  (for example "the server isn't reachable") instead of showing raw error
  text with web addresses; the details are in the log. Wording and spelling
  are consistent across the app (e.g. "Add to Favorites", "Key bindings").
- **Improved: setting descriptions are readable in full.** The selected row
  shows its whole description instead of cutting it off with "…", and the
  longest ones are shorter.
- **New: Settings → Profiles → Copy settings from…** — take another
  profile's settings (the ones tagged Profile: subtitles, audio language,
  skipping, trailer quality, Now Playing, card size) into yours, after a
  confirmation. No PIN needed: sign-in, Seerr and this device's settings
  are never copied.
- **Changed: Settings is organised by what you want to change.** The sections
  are now Playback (skipping, seeking, trailers), Subtitles & language,
  Display (HDR, display sync), Audio, Interface, Profiles, Integrations, Key
  Bindings and Advanced (decoder and sync switches, buffering, log level).
  Settings saved for your profile carry a small "Profile" tag. Every setting
  keeps its value.
- **Changed: Settings → Playback → Trailer quality** (was under Integrations,
  only with Seerr connected). It sets the resolution for every YouTube
  trailer, including the movie and series Trailer buttons that work without
  Seerr, so it's now always there.
- **New: Settings → Interface → Card size** — Normal, Large or Extra large poster
  cards on every screen: dashboards, library grids, Discover and the rows on
  detail pages. Saved per profile; library grids show fewer columns to fit.
- **Changed: posters fill the whole card, with the title over them.** The
  title and year now sit on a dark fade over the bottom of the poster instead
  of below it, so every poster in a row is the same size however long its
  title is (before, a long title shrank its poster). The ★ watchlist badge
  moved to the top-left corner, under ♥.
- **Fixed: Discover showed titles you have only in 4K as not in your
  library.** Seerr tracks the regular and the 4K version separately, and
  Discover only looked at the regular one. Cards (Discover, search,
  watchlist) now read "4K In Library" — or "4K Requested" — when only the 4K
  version is there.
- **Changed: three packages to choose from.** `PKGBUILD` (now `fjord`, was
  `fjord-git`) builds the latest release from source, `fjord-bin` installs
  the same release pre-built (checked against its checksum), and the new
  `fjord-bin-nightly` installs the current development build (what
  `fjord-bin` did before). Each replaces the others when installed.

## [0.5.0] — 2026-10-14

Household profiles, real HDR, display sync and a remote-friendly keyboard,
plus a security pass. The full story of each change is in `DEVLOG.md`.

### New

- **Household profiles** with the Bonfire/JellyProfiles plugin: a "Who's
  watching?" picker, profile switching from the sidebar without restarting,
  create/edit/delete profiles (name, avatar, PIN), auto-lock after sitting
  idle, connected households (Bonfire groups) and server-admin tools.
  Accounts and profiles are separate layers, each account with its own
  "Remember this login" choice.
- **HDR passthrough on Wayland.** The video is drawn on its own surface under
  Fjord's window, so only the video is marked HDR and menus keep their
  colours (Settings → Video → HDR → "Separate video surface", on by default),
  and 10-bit video stays 10-bit on a 10-bit screen. Opt-in: "Use Fjord's own
  10-bit buffers".
- **Sync display to source:** the display switches to the video's resolution,
  refresh rate and HDR mode before the video starts (KDE, via
  `kscreen-doctor`), with default modes, an output picker and an option for
  trailers.
- **On-screen keyboard** for every text field, with ◀ ▶ keys to move the
  caret, editing in the middle of the text, and a Settings switch to turn it
  off.
- **Full keyboard/remote navigation** for login, the account and profile
  pickers, profile editing, Manage Profiles and Connect Seerr.
- **Trailer button on movie and series pages** — a trailer file on your
  server first, else YouTube (needs yt-dlp).
- **Seek-bar preview thumbnails** when the server has trickplay images.
- **Seerr Blocklist**, a **"Coming Up"** row on the Home/TV/Movies dashboards,
  and a self-tidying watchlist (watched items are removed; still-airing
  series stay).
- **Skips fade** to black instead of cutting (duration in Settings; audio
  fades too, or mutes for passthrough).
- Confirmation before Sign Out, disconnecting Seerr, clearing the play queue,
  cancelling a request and deleting a profile.
- A context menu on season tabs; "Disc N" separators on multi-disc albums;
  cast and crew on Discover pages open their person page; "CLR IN"/"CLR OUT"
  rows in the stats overlay.
- `fjord-bin`: a pre-built binary package (no Rust toolchain needed).

### Improved

- **"Not watched" on the TV dashboard** now lists series you haven't started
  and appears at once (it took up to 20 seconds).
- **Library posters appear as they load**, top of the grid first; sidebar
  tabs fade instead of cutting, and switching between them is faster.
- **Trailers:** the Trailer button checks the trailer first ("Checking…" →
  "▶ Trailer" / "No trailer"), tries other trailers when one is blocked,
  remembers results for 30 days, and restarts a trailer that hangs while
  opening.
- **Playback holds up better:** a slow drive or a short network outage no
  longer ends playback or jumps to the wrong episode.
- Repeat All works outside albums.
- Server addresses can be typed without `http://`/`https://`.
- "View Details" is the last entry in every context menu; requesting a title
  shows a single toast.
- Ten previous session logs are kept, with local-time timestamps; the Tone
  Mapping setting explains each option.
- UI toolkit updated to Slint 1.18.

### Fixed

- **Security:** a Jellyfin server (or anyone tampering with an `http://`
  connection) could make Fjord write files anywhere in your home folder.
  Sign Out now also ends the session on the server. The settings file and
  logs are readable only by you, the log no longer contains your access key,
  Fjord says when a connection isn't encrypted, and libraries with known
  problems were updated.
- Live updates now work with `https://` Jellyfin servers.
- Turning HDR passthrough off didn't fully disable it, so the Tone Mapping
  curve could be skipped.
- 4K films pausing about two seconds in; a video or trailer that wouldn't
  open leaving a black screen; audio-only black screen on the first video.
- "Skip intro" missing and chapters lost when a video was slow to start; Up
  Next and skip countdowns running while paused; Stop right after Play
  resetting the resume position.
- Multi-disc albums playing out of order; Repeat One on a single song; new
  playlists showing up late.
- Subtitle type "Hearing Impaired"/"Forced" had no effect.
- Key bindings: case and Caps Lock mattered, and rebinding got stuck or
  silently took a key from another action (it now asks first).
- Browse All ignoring the arrow keys after a visit to its search field; the
  Movies library showing no posters when Discover was opened first; the
  Discover search briefly showing "No results".
- Hard-to-see or stuck focus highlights; raw HTML in overviews and bios; "0
  episodes" for unaired seasons; settings occasionally not saving; a second
  Fjord window opening from the launcher.

*Range: [`v0.4.2...v0.5.0`](https://github.com/KalasKonrad/Fjord/compare/v0.4.2...v0.5.0)*

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

*Range: [`f2f11f7`](../../commit/f2f11f7)..[`41676d2`](../../commit/41676d2) (20 commits)*

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

*Range: [`42ba2bc`](../../commit/42ba2bc)..[`f2f11f7`](../../commit/f2f11f7) (35 commits)*

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

*Range: [`cdef936`](../../commit/cdef936)..[`42ba2bc`](../../commit/42ba2bc) (8 commits)*

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

*Range: [`896c452`](../../commit/896c452)..[`cdef936`](../../commit/cdef936) (29 commits)*

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

*Range: [`53f1c59`](../../commit/53f1c59)..[`896c452`](../../commit/896c452) (130 commits)*

## [0.3.2] — 2026-06-28

Playback queue, MusicPlayerBar, gapless, Now Playing, Lyrics.

- Playback queue: Play Next / Add to Queue from the context menu, a queue
  viewer panel.
- Playlist backend with Prev/Next/Shuffle/Repeat, and the MusicPlayerBar
  bottom-bar layout.
- Gapless music playback (same mpv instance, no gap between tracks).
- A fullscreen Now Playing screen with synced lyrics.

*Range: [`561a190`](../../commit/561a190)..[`53f1c59`](../../commit/53f1c59) (3 commits)*

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

*Range: [`17f91e7`](../../commit/17f91e7)..[`561a190`](../../commit/561a190) (60 commits)*

## [0.3.0] — 2026-06-25

WebSocket real-time events, chapter navigation, Collections screen.

- Phase 41: WebSocket-driven real-time sync begins (library changes,
  favorites, playback state).
- Chapter navigation (`,`/`.` keys, seek bar tick marks, OSD) and
  sub/audio delay adjustment (`z`/`Z`/`x`/`X`), both with mouse-accessible
  panels in the player controls bar.
- A new Collections library screen, with its own dashboard rows and disk
  cache.

*Range: [`1c2691f`](../../commit/1c2691f)..[`17f91e7`](../../commit/17f91e7) (9 commits)*

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

*Range: [`c977864`](../../commit/c977864)..[`1c2691f`](../../commit/1c2691f) (61 commits)*

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

*Range: [`e067f6c`](../../commit/e067f6c)..[`c977864`](../../commit/c977864) (99 commits)*

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

*Range: [`a2419c9`](../../commit/a2419c9)..[`e067f6c`](../../commit/e067f6c) (202 commits)*

## [0.2.0] — 2026-06-12 – 2026-06-14

Context menu + canonical state store.

- Right-click / `C`-key context menu: Mark Played, toggle Favourite, Play
  from Start.
- A canonical `update_item_user_state` store keeps `FjordState` in sync
  across every card model after a played/favorite toggle, instead of each
  call site patching its own copy.

*Range: [`e5d397e`](../../commit/e5d397e)..[`a2419c9`](../../commit/a2419c9) (104 commits)*

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

*Range: [`b285c38`](../../commit/b285c38)..[`e5d397e`](../../commit/e5d397e) (45 commits)*
