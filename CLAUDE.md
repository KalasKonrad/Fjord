# Fjord — Claude Code Context

Fjord is a Jellyfin media frontend built in Rust with Slint as the GUI toolkit and libmpv for video playback. It is built by KalasKonrad as a personal project, partly as a learning exercise in Rust and partly to solve a real problem: every existing Flutter-based Jellyfin frontend (Fladder, Jellyflix) uses media_kit which embeds mpv into a Flutter texture. That path never calls `mpv_render_context_report_swap()`, so mpv has no vsync feedback and playback is choppy on NVIDIA legacy drivers on Wayland. Fjord fixes this by using the mpv render API so mpv renders into an OpenGL FBO that Slint composites, with `report_swap()` called after every frame.

## Project structure

```
Fjord/
├── Cargo.toml                  workspace root
├── PLAN.md                     development roadmap
├── crates/
│   ├── fjord-api/              Jellyfin REST API client
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── auth.rs         authentication (username+password → token)
│   │       ├── client.rs       JellyfinClient struct, all API calls
│   │       └── models.rs       serde types for Jellyfin responses
│   ├── fjord-player/           libmpv wrapper
│   │   └── src/
│   │       ├── lib.rs
│   │       └── mpv.rs          Player struct, MpvRenderCtx, FBO rendering
│   └── fjord-app/              Slint UI + main binary
│       ├── build.rs            compiles .slint files
│       ├── src/main.rs
│       └── ui/
│           ├── main.slint      all UI components and MainWindow
│           └── theme.slint     color palette, spacing tokens, HomeItem struct
```

## Key design decisions

### mpv render API (not X11 embedding)
mpv uses `vo=libmpv` and `mpv_render_context`. It never opens its own window. Each frame:
1. Slint's `BeforeRendering` notifier fires on the GL thread
2. mpv renders into the back FBO (`mpv_render_context_render`)
3. The FBO texture is exposed to Slint as a `BorrowedOpenGLTexture`
4. Slint's `AfterRendering` notifier calls `report_swap()` for vsync feedback

**Double-buffer FBO:** Two GL texture/FBO pairs alternate each frame. Single-buffer caused Slint to skip re-renders because the texture ID was unchanged (Slint's change detection). Alternating IDs force a re-render every frame.

**Drop ordering:** `MpvRenderCtx` must be dropped before `Player`. This is enforced in `VideoState` and in the rendering teardown path.

### Three playback modes
1. **Fullscreen player** (`is-playing = true`): covers the full window, shows controls bar + inline stats overlay.
2. **Video behind UI** (`has-background-player + video-behind-ui = true`): video fills the window at 88% opacity, UI overlays it.
3. **Mini card** (`has-background-player + video-behind-ui = false`): sidebar shows a live thumbnail card with a Resume button.

The "Video in background" setting (persisted) controls whether Back during playback enters mode 2 or mode 3.

### Dashboards and library grid

There are three dashboard screens (horizontal `SectionRow` card rows) and one library grid:

- **Home dashboard** (`HomeDashboard`, `active-nav == 0`, 4 rows): Continue Watching, Next Up, Recently Added Shows (`Series`), Recently Added Movies. Shows both movies and series.
- **Movies dashboard** (`DashboardScreen`, `active-nav == 1`, 3 rows): Continue Watching Movies, Recently Added Movies, Not Watched Movies.
- **Series dashboard** (`DashboardScreen`, `active-nav == 2`, 4 rows): Continue Watching TV, Next Up, Recently Added Shows (`Series`), Not Watched Shows (`Series`).
- **Library grid** (`LibraryGrid`, `show-library == true`): full poster grid of every item in a category. Opened by pressing Enter from the Movies or TV sidebar tab. Separate concept from the dashboards — do not call this a "dashboard."

Episode cards in dashboard rows display the series poster (`series_id` used as the fetch key), not the episode thumbnail. `spawn_poster_loading` carries a `poster_id` field alongside `item_id` in its metadata tuple for exactly this reason.

Not Watched rows use `SortBy=Random` so each fetch returns a different selection. A 30-second polling timer (`timer_nw`) refreshes the Not Watched row when the relevant tab is visible, no playback is active, and 10 minutes have elapsed since the last refresh. Timestamps `last_nw_mov_refresh` / `last_nw_tv_refresh` in `AppState` track this independently per tab.

Poster images are cached to `~/.cache/fjord/posters/` and decoded off the UI thread — JPEG decode runs on a Tokio worker producing `SharedPixelBuffer<Rgba8Pixel>` (which is `Send`), then `Image::from_rgba8` is called inside `invoke_from_event_loop` because `slint::Image` is `!Send`.

`HomeItem` (defined in `theme.slint`) carries `has-played: bool`, `resume-pct: float`, and `unplayed-count: int` — populated from `UserData.Played`, `UserData.PlaybackPositionTicks / RunTimeTicks`, and `UserData.UnplayedItemCount`. `MediaCard` renders a ✓ badge when `has-played`, a progress bar when `resume-pct > 0 && !has-played`, and an episode-count pill when `unplayed-count > 0 && !has-played` (series posters only).

Card dimensions are computed by breakpoint pure functions (`dash-card-w`, `dash-card-h`) and passed down into `SectionRow` as `card-w`/`card-h` properties so all cards scale with the window width.

### Disk caches
- `~/.cache/fjord/items.json` — full library list. Fresh if < 6 h old; background refresh otherwise.
- `~/.cache/fjord/home.json` — home row data. Shown from cache immediately on warm start, always refreshed in the background.
- `~/.cache/fjord/posters/<id>` — raw poster bytes, one file per item.

On a warm start (valid saved session + fresh cache) the window opens in the logged-in state with content visible on the first frame — no loading flash.

### Keyboard navigation
A global zero-size `FocusScope` (`fs`) captures all keyboard input. `invoke_grab_keyboard_focus()` is called from Rust at startup to give it focus.

State is tracked via `focused-section: int` in MainWindow:
- **`-1` = sidebar**: Up/Down cycle nav tabs; Right/Enter enters the card grid at the first non-empty row.
- **`≥ 0` = content grid**: focused-section is the row index, `focused-card` is the column. Up/Down move between rows (Up at row 0 stays in content); Left/Right move between cards; Enter plays.
- **Browse list** (`show-browse = true`): Up/Down navigate the list; Enter plays; Backspace/Escape closes it.
- **Library grid** (`show-library = true`): 2D arrow nav across the poster grid; Enter opens item.
- **Series screen** (`show-series = true`): Left/Right navigate season tabs; Up/Down navigate episode list; Up at episode 0 jumps to season row; Backspace closes.

**Hold vs tap Left:** `KeyEvent.repeat` is `true` for auto-repeat (held key) and `false` for a fresh press. At `focused-card == 0`, held Left stays at card 0; a single tap Left at card 0 exits to the sidebar.

Shortcuts: `1`/`2`/`3` jump to Home/Movies/TV; `S` to Settings; `B` opens the browse list; `F`/`F11` toggles fullscreen; `Q` quits.

### Fullscreen
`window.window().set_fullscreen(bool)` / `is_fullscreen()` used directly. Toggle is wired to `on_toggle_fullscreen` callback (called by `F`/`F11` key). The "Launch in fullscreen" setting applies the flag before `window.run()` and also immediately when the checkbox is toggled.

### Session identity (DeviceId)

`JellyfinClient` carries a `device_id: String` field used in the `Authorization` header (`DeviceId="…"`). On first run, `ensure_device_id()` reads `/proc/sys/kernel/random/uuid`, saves it to `~/.config/fjord/config.json`, and uses it for the lifetime of the install. This is critical: if two machines share the same DeviceId, Jellyfin invalidates one machine's token when the other authenticates, causing 401 errors on all API calls.

On startup, after loading a saved session, `check_auth()` does a cheap `GET /Users/{id}/Items?Limit=0&Recursive=true` probe. On 401 the login screen is shown; any other error is ignored and the app proceeds (transient network issue). Passwords are never stored — Jellyfin tokens don't expire under normal use.

### Workspace crates
- `fjord-api`: no UI, no mpv. Pure async HTTP + JSON. Testable in isolation.
- `fjord-player`: no UI, no HTTP. Just libmpv bindings + render context.
- `fjord-app`: thin wiring layer. Imports the other two, drives the Slint event loop.

### Episode auto-advance
When playback finishes and `VideoState.playing_series_id` is set, a background task calls `get_next_up_for_series(series_id)` to get the true next episode (crossing season boundaries). If one exists it's stored in `AppState.next_ep_pending` and a 5-second countdown banner is shown via `invoke_from_event_loop`. Setting `next_ep_pending = None` cancels the countdown (wired to `cancel-auto-advance` callback). After the countdown the stored episode is played by calling `start_playback` from inside `invoke_from_event_loop`.

Every `start_playback` call site must set `video.lock().unwrap().playing_series_id = series_id` immediately after the call so auto-advance works for plays from any screen.

### Intro Skipper plugin
When starting playback of an Episode, `start_playback` spawns a background task calling `client.get_intro_timestamps(item_id)` (`GET /Episode/{id}/IntroTimestamps` — provided by the Intro Skipper Jellyfin plugin). On success the `IntroTimestamps` is stored in `VideoState.intro_timestamps`. The 16 ms timer loop checks current playback position against `show_skip_prompt_at` / `hide_skip_prompt_at` and toggles `show-skip-intro` on the window. The `on_skip_intro` callback calls `player.seek_to(intro_end)`. Returns `None` gracefully when the plugin is absent (404).

### Async strategy
Tokio for all async. The Slint event loop runs on the main thread. Background tasks (API calls, poster fetching) use `tokio::spawn`. Communication back to the UI uses `slint::invoke_from_event_loop` or channels.

## Build

```bash
cargo build                     # debug build
cargo build --release           # release
cargo run -p fjord-app          # run the app
```

Requires `mpv` and `libmpv` to be installed (`pacman -S mpv`).

## Dependencies (key ones)

| Crate | Purpose |
|-------|---------|
| `slint` | GUI framework |
| `slint-build` | build.rs compiler for .slint files |
| `libmpv2` | libmpv bindings |
| `reqwest` | HTTP client for Jellyfin API |
| `serde` / `serde_json` | JSON serialization |
| `tokio` | async runtime |
| `image` | JPEG/PNG decode for poster thumbnails |
| `gl` / `euclid` | OpenGL FBO management for mpv render API |
| `anyhow` / `thiserror` | error handling |

## What is Jellyfin

Jellyfin is an open-source media server. It exposes a REST API for browsing libraries (movies, TV shows, music) and getting playback URLs. Auth is username+password → returns an API token that goes in every subsequent request header as `X-Emby-Token` (Jellyfin kept the Emby header name).

Key API endpoints used:
- `POST /Users/AuthenticateByName` — login
- `GET /Users/{userId}/Items` — browse/search items
- `GET /Users/{userId}/Items/{itemId}` — item detail (overview, cast, backdrop tags, etc.)
- `GET /Items/{itemId}/Images/Primary` — poster image
- `GET /Items/{itemId}/Images/Backdrop/0` — backdrop image
- `GET /Users/{userId}/Items?Filters=IsResumable` — continue watching
- `GET /Shows/NextUp` — next unwatched episode per series (home row)
- `GET /Shows/NextUp?SeriesId=…` — next episode for a specific series (auto-advance)
- `GET /Shows/{seriesId}/Seasons` — season list
- `GET /Shows/{seriesId}/Episodes?seasonId=…` — episode list for a season
- `GET /Videos/{itemId}/stream?static=true&api_key=…` — direct-play URL
- `POST /Sessions/Playing` — report playback started
- `POST /Sessions/Playing/Progress` — report position
- `POST /Sessions/Playing/Stopped` — report stopped
- `GET /Episode/{itemId}/IntroTimestamps` — intro segment bounds (Intro Skipper plugin, optional)

## Testing setup

Two machines:
- **Dev machine** (this repo): AMD GPU, Wayland, Vulkan. Used for development.
- **HTPC**: NVIDIA legacy GPU, Wayland/EGL. The primary target. Logs land in `/home/htpc/.cache/fjord/fjord.log`.

Deploy workflow: push to GitHub → on the HTPC run `makepkg -si` with the `PKGBUILD` at the repo root. The PKGBUILD pulls from `https://github.com/KalasKonrad/Fjord.git` and does a native `cargo build --release --locked`, installing the binary to `/usr/bin/fjord`.

The HTPC is the harder target — it is what motivated the render API design in the first place.

## Known platform issues

### NVIDIA legacy Wayland: NVDEC stride corruption
**Symptom:** Diagonal stripe artifact (raw YUV scan lines) when using hardware decoding (`nvdec`, `nvdec-copy`). Software decoding is clean.

**Root cause:** NVDEC aligns decoded frame rows to 256-byte boundaries (e.g., a 1920-pixel-wide video gets a 2048-byte stride). mpv uploads via `glTexSubImage2D` with `GL_UNPACK_ROW_LENGTH=2048`. The NVIDIA legacy EGL driver silently ignores `GL_UNPACK_ROW_LENGTH`, so GL reads each row 128 bytes too tight — each successive row is offset from the previous, producing the diagonal slant.

**Fix:** Set `hwdec-image-format=yuv420p` in Settings → Video. This tells mpv to reformat the NVDEC output to tight-packed yuv420p before the GL upload, eliminating the stride hint entirely. For 10-bit HDR content use `yuv420p10le` to preserve bit depth.

**AMD Vulkan:** `vulkan-copy` works correctly with no stride workaround needed.

### PlayerConfig fields (fjord-player/src/mpv.rs)
All fields are logged at playback start so the log shows exactly what options were active. Key fields:
- `hwdec` — decoder selection (`auto`, `nvdec-copy`, `vulkan-copy`, etc.)
- `hwdec_image_format` — post-decode pixel format override (empty = mpv default). Set to `yuv420p` for NVIDIA legacy.
- `video_sync` — `audio` (default) or `display-resample` (locks to display refresh via `report_swap()` timing).
- `opengl_early_flush` — flush GL after each frame; may help with EGL pipeline ordering on NVIDIA.
- `video_latency_hacks` — compensates for imprecise Wayland vsync timestamps on NVIDIA 5xx legacy.

## Known Slint gotchas

These have each caused real bugs in this codebase:

**`Flickable` is the only reliable keyboard-scrollable container.** `ScrollView` ignores declarative `viewport-y` bindings (it manages its own scroll internally). `ListView` also writes to `viewport-y` from its own scroll handler, silently overwriting any binding you set. The correct pattern for any keyboard-driven scrollable list is `Flickable { viewport-height: ...; VerticalLayout { for ... } }` with `viewport-y` bound to a `clamp(...)` expression that tracks the focused index.

**Plain `Rectangle` children are horizontally centred by default.** If you need a fill bar or overlay anchored to the left edge, you must set `x: 0` explicitly. Omitting it centres the element and produces the "progress bar starts from the middle" bug.

**`KeyEvent.repeat`** is `true` when a key is held (auto-repeat) and `false` on the initial press. Use it to distinguish "hold Left to scroll" from "tap Left to exit to sidebar".

**`invoke_from_event_loop` closures must be `'static + Send`.** Capture owned values (`String`, `Arc<…>`) not references. This is the correct pattern for communicating from Tokio tasks back to Slint UI state.

**`TouchArea.moved` fires only during drag (button held), not plain cursor movement.** To react to mouse movement without a button press, use `changed mouse-x => { ... }` and `changed mouse-y => { ... }` callbacks. This is how the player controls overlay auto-show is implemented.

## Style

- Standard Rust formatting (`cargo fmt`)
- Errors: use `anyhow::Result` at the top level, `thiserror` for library error types
- No `unwrap()` in library code — propagate errors
- Keep `fjord-api` and `fjord-player` free of Slint imports
