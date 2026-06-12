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

### Home screen and library rows
The home screen shows curated horizontal card rows: Continue Watching, Next Up, Recently Added. Movies and TV screens show similar rows filtered by type. Poster images are cached to `~/.cache/fjord/posters/` and decoded off the UI thread — JPEG decode runs on a Tokio worker producing `SharedPixelBuffer<Rgba8Pixel>` (which is `Send`), then `Image::from_rgba8` is called inside `invoke_from_event_loop` because `slint::Image` is `!Send`.

### Disk caches
- `~/.cache/fjord/items.json` — full library list. Fresh if < 6 h old; background refresh otherwise.
- `~/.cache/fjord/home.json` — home row data. Shown from cache immediately on warm start, always refreshed in the background.
- `~/.cache/fjord/posters/<id>` — raw poster bytes, one file per item.

On a warm start (valid saved session + fresh cache) the window opens in the logged-in state with content visible on the first frame — no loading flash.

### Keyboard navigation
A global zero-size `FocusScope` (`fs`) captures all keyboard input. `invoke_grab_keyboard_focus()` is called from Rust at startup to give it focus.

State is tracked via `focused-section: int` in MainWindow:
- **`-1` = sidebar**: Up/Down cycle nav tabs; Right/Enter enters the card grid at the first non-empty row.
- **`≥ 0` = content grid**: focused-section is the row index, `focused-card` is the column. Up/Down move between rows (Up at row 0 returns to sidebar); Left/Right move between cards (Left at card 0 returns to sidebar); Enter plays.
- **Browse list** (`show-browse = true`): Up/Down navigate the list; Enter plays; Backspace/Escape closes it.

Shortcuts: `1`/`2`/`3` jump to Home/Movies/TV; `S` to Settings; `B` opens the browse list; `F`/`F11` toggles fullscreen.

### Fullscreen
`window.window().set_fullscreen(bool)` / `is_fullscreen()` used directly. Toggle is wired to `on_toggle_fullscreen` callback (called by `F`/`F11` key). The "Launch in fullscreen" setting applies the flag before `window.run()` and also immediately when the checkbox is toggled.

### Workspace crates
- `fjord-api`: no UI, no mpv. Pure async HTTP + JSON. Testable in isolation.
- `fjord-player`: no UI, no HTTP. Just libmpv bindings + render context.
- `fjord-app`: thin wiring layer. Imports the other two, drives the Slint event loop.

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
- `GET /Items/{itemId}/Images/Primary` — poster image
- `GET /Users/{userId}/Items?Filters=IsResumable` — continue watching
- `GET /Shows/NextUp` — next unwatched episode per series
- `GET /Videos/{itemId}/stream?static=true&api_key=…` — direct-play URL
- `POST /Sessions/Playing` — report playback started
- `POST /Sessions/Playing/Progress` — report position
- `POST /Sessions/Playing/Stopped` — report stopped

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

## Style

- Standard Rust formatting (`cargo fmt`)
- Errors: use `anyhow::Result` at the top level, `thiserror` for library error types
- No `unwrap()` in library code — propagate errors
- Keep `fjord-api` and `fjord-player` free of Slint imports
