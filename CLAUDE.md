# Fjord — Claude Code Context

Fjord is a Jellyfin media frontend built in Rust with Slint as the GUI toolkit and libmpv for video playback. It is built by KalasKonrad as a personal project, partly as a learning exercise in Rust and partly to solve a real problem: every existing Flutter-based Jellyfin frontend (Fladder, Jellyflix) uses media_kit which embeds mpv into a Flutter texture. That path never calls `mpv_render_context_report_swap()`, so mpv has no vsync feedback and playback is choppy on NVIDIA legacy drivers on Wayland. Fjord fixes this by giving mpv a native window handle directly, so it owns its own vsync loop.

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
│   │       └── mpv.rs          Player struct, window embedding, properties
│   └── fjord-app/              Slint UI + main binary
│       ├── build.rs            compiles .slint files
│       ├── src/main.rs
│       └── ui/main.slint       Slint UI definitions
```

## Key design decisions

### Why mpv gets its own native window
The whole point of this project is smooth playback. mpv must control its own window so it gets direct vsync feedback from the display system. The approach:
1. Create a Slint window for the UI
2. When playback starts, get the native window ID (X11 `Window` or Wayland handle)
3. Pass it to libmpv via the `wid` property so mpv renders inside it
4. On Wayland + NVIDIA legacy, prefer X11 embedding via XWayland since Wayland window embedding is complex and NVIDIA 580.xx Wayland support is poor

### Workspace crates
- `fjord-api`: no UI, no mpv. Pure async HTTP + JSON. Testable in isolation.
- `fjord-player`: no UI, no HTTP. Just libmpv bindings + window logic.
- `fjord-app`: thin wiring layer. Imports the other two, drives the Slint event loop.

### Async strategy
Tokio for all async. The Slint event loop runs on the main thread. Background tasks (API calls, mpv events) use `tokio::spawn`. Communication back to the UI uses Slint's `invoke_from_event_loop` or channels.

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
| `anyhow` / `thiserror` | error handling |

## What is Jellyfin

Jellyfin is an open-source media server. It exposes a REST API for browsing libraries (movies, TV shows, music) and getting playback URLs. Auth is username+password → returns an API token that goes in every subsequent request header as `X-Emby-Token` (Jellyfin kept the Emby header name).

Key API endpoints used:
- `POST /Users/AuthenticateByName` — login
- `GET /Users/{userId}/Views` — top-level library list  
- `GET /Users/{userId}/Items` — browse items in a library
- `GET /Items/{itemId}/PlaybackInfo` — get stream URL + codec info
- `POST /Sessions/Playing` — report playback started
- `POST /Sessions/Playing/Progress` — report playback position (every 10s)
- `POST /Sessions/Playing/Stopped` — report playback stopped

## Style

- Standard Rust formatting (`cargo fmt`)
- Errors: use `anyhow::Result` at the top level, `thiserror` for library error types
- No `unwrap()` in library code — propagate errors
- Keep `fjord-api` and `fjord-player` free of Slint imports
