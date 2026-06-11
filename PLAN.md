# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux (initially) that plays video smoothly on NVIDIA legacy hardware by using mpv with a native window instead of an embedded texture.

---

## Phase 1 — Foundation (get it compiling and opening a window)

**Goal:** `cargo run -p fjord-app` opens a Slint window.

- [x] Verify `cargo build` succeeds with slint + libmpv2 dependencies
- [x] Slint hello-world window (`ui/main.slint` → MainWindow)
- [x] Confirm libmpv2 crate links against system libmpv correctly
- [x] Basic app structure: main loop, tracing/logging setup

**Done when:** A window appears with the Fjord title. ✅

---

## Phase 2 — Player (mpv with native window)

**Goal:** Pass a URL on the command line, it plays in a native mpv window with audio passthrough and hardware decode.

Using **option 2** (separate fullscreen mpv window) for Phase 2. `wid` embedding
deferred to Phase 4 once the UI exists.

- [x] `Player` struct in `fjord-player` wrapping `libmpv2::Mpv`
- [x] Set `audio-spdif` for AC3/DTS/TrueHD passthrough
- [x] Set `hwdec=auto-safe` (tries vaapi/nvdec, falls back to software)
- [x] mpv event loop running on a background thread
- [x] Basic controls: play/pause (spacebar), seek (left/right), quit (q/Esc) — handled natively by mpv in its own window
- [x] `vsync-ratio` observed in event loop (debug log confirms it's non-null)
- [ ] Confirm `vsync-ratio` is non-null on real hardware (manual test)
- [ ] `wid` embedding into Slint window — deferred to Phase 4

**Done when:** A local video file plays smoothly with working audio passthrough.

---

## Phase 3 — Jellyfin API client

**Goal:** `fjord-api` can authenticate and return a list of libraries.

- [ ] `JellyfinClient` struct with server URL + API token
- [ ] `authenticate(server, username, password) → Client` 
- [ ] `get_libraries() → Vec<Library>` 
- [ ] `get_items(library_id, ...) → Vec<Item>` (movies, episodes)
- [ ] `get_playback_info(item_id) → PlaybackInfo` (stream URL, codecs)
- [ ] Progress reporting: started / progress (every 10s) / stopped
- [ ] Persist server URL + token to disk (`~/.config/fjord/config.toml`)

**Done when:** Can print a list of movies from a real Jellyfin server.

---

## Phase 4 — Basic UI

**Goal:** Navigate libraries, pick a movie, it plays.

- [ ] Login screen: server URL + username + password fields
- [ ] Library grid: poster thumbnails in a scrollable grid
- [ ] Item detail page: title, overview, play button
- [ ] Keyboard navigation (arrow keys, Enter, Backspace)
- [ ] Poster images fetched from Jellyfin image API
- [ ] Wire player: pressing Play fetches playback URL and opens mpv
- [ ] Resume playback (Jellyfin tracks position server-side)

**Done when:** Can browse and play a movie from a real Jellyfin server end to end.

---

## Phase 5 — TV Shows + Polish

- [ ] TV show → season list → episode list navigation
- [ ] "Continue watching" / "Next up" rows on home screen
- [ ] Episode auto-advance
- [ ] Settings page: server management, audio/video preferences
- [ ] HTPC mode: large text, gamepad-friendly, quit button
- [ ] On-screen player controls overlay (fade in on mouse move)
- [ ] Subtitle track selection
- [ ] Audio track selection

---

## Phase 6 — Packaging

- [ ] PKGBUILD for Arch Linux
- [ ] Desktop file + icon
- [ ] `--htpc` command line flag

---

## Architecture notes

### mpv window embedding strategy

Two options, in order of preference:

1. **X11 embedding** (`--wid=<XID>`): mpv renders directly into an X11 window. Reliable, gives mpv full vsync control. Works via XWayland on Wayland compositors. On NVIDIA 580.xx this is the best path.

2. **Separate fullscreen mpv window**: When playback starts, hide the Slint window and let mpv open its own fullscreen window. On exit, restore the Slint window. Simplest approach, works everywhere, loses overlay controls.

Start with option 2 (simpler), move to option 1 once the API and UI are working.

### Thread model

```
main thread          Slint event loop
tokio runtime        API calls (reqwest), image loading
mpv thread           mpv event loop (libmpv2 event polling)
```

Slint UI updates from other threads: use `slint::invoke_from_event_loop(|| { ... })`.

### Jellyfin playback URL

`GET /Videos/{itemId}/stream?static=true&api_key={token}` for direct play.
For transcoded: use `PlaybackInfo` response which returns an `HLS` or `dash` URL.
Prefer direct play always — mpv handles every codec natively.
