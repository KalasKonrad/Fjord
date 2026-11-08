# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.5.1`). Implementation detail per feature: `DEVLOG.md` (dated sections). 0.5.1's live tests are summed up in DEVLOG → "0.5.1 live tests".

---

## 0.5.2 (branch `release-0.5.2`)

Scope chosen by the user on 2026-11-08. In the order of work: small and safe first, the game controller last (on its own feature branch, CLAUDE.md → Development workflow). Released per CLAUDE.md → Releasing once everything here is done and live-tested. With a game controller and a light theme this is more than a patch: decide 0.5.2 vs 0.6.0 at release time.

**1. Quieter missing log folder**
- [x] **Done 2026-11-08** (live check in the checklist; also handles a folder lost while the file is open). While the log folder can't be opened (e.g. the network share isn't there), `LazyLogFile` prints `fjord: log folder … not ready` to stderr (→ the journal) on every 2 s retry. Print it once when it first fails, and once when the folder is back (the file already gets "log folder reachable after N s"). main.rs `LazyLogFile::write`. Unit-testable with a folder that appears later.

**2. Video sync default → `display-vdrop`**
- [x] **Done 2026-11-08** (live check in the checklist). `default_video_sync()` (config.rs) and `PlayerConfig::default` (fjord-player) → `display-vdrop`. **New installs only** (user): a saved `audio` stays. Why: `audio` times frames by the audio clock and drifts against the display (the 0.5.1 choppiness); `display-vdrop` works with normal audio *and* passthrough. Not `display-resample`: verified in mpv 0.41's `player/video.c` (`handle_display_sync_frame`: `if (resample && using_spdif_passthrough(mpctx)) return;`) — with passthrough it leaves display sync off entirely. The Settings description says when to pick which; DEVLOG gets the mpv source reference.

**3. Shorter pause at playback start and episode switch**
- [ ] HTPC logs (r1291): `ui: event loop busy` 200–340 ms (avg 227 ms, 19 times) and slow frames of 150–183 ms in mode Player, at every start and at Up Next's switch (`playback replaced`). Step 1: time each part on the UI thread with `timed` — `start_playback`'s steps, `tear_down_player` (render context free, `Player` drop = mpv's terminate/destroy), the render context creation in `BeforeRendering` — and run it on the HTPC. Step 2: move what's slow off the UI thread (e.g. drop the old `Player` on a worker thread after its render context is freed on ours — CLAUDE.md: `MpvRenderCtx` before `Player`). Goal: no `event loop busy` over ~50 ms at a start. **Step 1 done 2026-11-08:** debug lines `play start on the UI thread: …`, `teardown on the UI thread: position read …, render context freed …, mpv shut down …`, `mpv render context created in N ms` and `loadfile sent in N ms` — waiting for an HTPC run (checklist).

**4. Try Interpolation**
- [ ] It was silently off until 0.5.1 (needs display sync) and works now. Live test, HTPC: an episode with Settings → Advanced → Interpolation on (tscale `oversample`), first with the display switched to the film's rate (ratio 1.0000: expect little visible change — oversample mostly smooths a rate mismatch), then with display sync's mode switching off (desktop 119.88 Hz = exactly 5 refreshes per 23.976 fps frame, so also little) and on a 60 Hz mode (3:2 judder → this is where it should help). Watch the log's slowest mpv render call and drops (GTX 1050 Ti load). Dev machine likewise. Outcome: the setting's description says when it helps; default stays off unless the test says otherwise.

**5. Accent colour choice**
- [x] **Done 2026-11-08** (live check in the checklist; blue, purple `#aa5cc3`, teal `#009e8e`, green `#2e9e4f`, orange `#e0701b`, pink `#d6457f` — each with at least the blue's contrast with white text and ≥ 4.6:1 on bg). Settings → Interface → "Accent color", per profile (Profile tag, copied by "Copy settings from…"): a few colours (today's blue as default + e.g. teal, purple, orange, green, red — final list when building, checked for contrast with white text). `Theme.accent` / `accent-hover` / `accent-muted` (142 uses) become computed from the choice instead of fixed values, so no screen changes. A `settings::CHOICES` entry + `PROFILE_BINDS` line (CLAUDE.md). Error red stays red.

**6. Light theme**
- [ ] Settings → Interface → "Theme": Dark (default) / Light, per profile. Builds on 5: every `Theme` token gets a light value, chosen by the setting. First an audit for colours that bypass `Theme` (hex literals in screens — CLAUDE.md says none should, check it) and for what must stay dark in both: the player/OSD, text and scrims over pictures (`scrim*`, `osd-panel*`, `text-bright`/`text-soft`, `hero-fade-*`), the video area. The focus ring's white default needs a light-theme default (the per-profile focus colour stays the user's choice). Screenshots of every screen in both themes before ticking.

**7. Drag rows in the dashboard rows editor**
- [ ] Settings → Interface → Dashboard rows → Edit…: drag a row with the mouse to a new place (the remote keeps ✓ ▲ ▼). Same per-profile `*-row-order` lists; the drop updates the order through the existing move path, and keyboard focus follows the dragged row (CLAUDE.md: mouse sets the same state as keys). Drop indicator between rows; auto-scroll near the list's edges.

**9. UI/UX polish pass** (user, 2026-11-08 — all 16 items chosen from a code survey). Order: 9d's colours before items 5–6 (accent and light theme need every colour in `Theme`); the rest after 7, before 8. The type, spacing and radius scales change how things look slightly — screenshots of every screen before and after.
- **9a. Loading, empty and error states**
  - [ ] **Loading on 4 pages:** album, artist, collection and person pages have no loading state (detail, series, season and the request page do) — with the server's disks waking up that's 5–15 s of an empty page. Use the request page's delayed spinner (appears only after ~300 ms, `spawn_delayed_spinner`).
  - [ ] **In-page error + Retry:** a page whose data fails to load shows only a toast and stays blank. Show "Couldn't load … — Retry / Back" in the page (the offline screen already has Retry).
  - [ ] **Missing empty states:** calendar with no entries, a person with no credits, an empty collection or playlist, the empty queue — say so instead of blank space (the existing ones: "Nothing blocklisted", "No results for …", "No lyrics available").
  - [ ] **Seerr unreachable:** Discover's rows are simply empty when Seerr can't be reached (e.g. at startup before the network is up) — a message and Retry.
- **9b. Navigation flow**
  - [ ] **Remote pass with the user:** every screen with the remote; each note becomes a line here and gets fixed. Last in 9, after the other fixes.
  - [ ] **Focus after closing:** closing anything returns focus exactly where it was — the context menu → its card, Request Options → its button, the player → the page playback started from. Audit every close path; one helper if it's per-screen code today.
  - [ ] **Edges of every screen:** Up from the top row and Left from the first column escape the same way (header / sidebar) everywhere; no dead keys.
  - [ ] **Mouse = remote audit:** every clickable sets the same focus/zone state as the keys (CLAUDE.md rule) — check each `TouchArea` for a missing focus update.
- **9c. Player and music screens** — each a code review first; the findings become sub-items for the user to choose, like 0.5.1's UI review.
  - [ ] **Player OSD:** layout, seek bar, chapter marks, time display, what the remote reaches, show/hide timing.
  - [ ] **Track & subtitle menus:** wording, order, what's shown per track, remote navigation.
  - [ ] **Music bar & Now Playing:** leftovers after 0.5.1's rework — alignment, focus, queue panel, lyrics view.
  - [ ] **End-of-episode prompts:** Up Next, Skip Intro/Credits and their countdowns — consistent look and placement, remote behaviour.
- **9d. Visual consistency** (measured 2026-11-08 over the 43 `.slint` files; every `animate` already follows the speed settings)
  - [x] **Done 2026-11-08** (live check in the checklist) — **All colours via `Theme`:** 72 hand-picked colours outside theme.slint (player 17, cards 7, main 7, player bars 6, …: scrims, OSD backgrounds, greys) → `Theme` tokens. The avatar palette in profile_edit.slint stays literal by design.
  - [ ] **Type scale:** 21 font sizes (9–64 px; 11–14 px used ~250 times) → about 6 named sizes in `Theme`, every `Text` using one (still `× AppState.text-scale` under 24 px).
  - [ ] **Spacing scale:** about 20 padding/spacing values → a small scale (4/8/12/16/24/48) as `Theme` tokens.
  - [ ] **Corner radii:** 14 values → 3–4 tokens (small, card, panel, pill), including FocusGlow/FocusHalo `radius:`.
  - CLAUDE.md gets the rule once the tokens exist (sizes, spacing, radii and colours only from `Theme`).

**8. Game controller** (feature branch off `release-0.5.2`)
- [ ] The user can test only a game controller (no IR receiver, no HDMI-CEC adapter) — which pad: ask before building. Read pads with the `gilrs` crate (evdev on Linux, hot-plug, SDL's controller mapping database; verify its version, build deps — libudev — and the PKGBUILDs' `depends`). Pads are readable by the logged-in user through udev's `uaccess` for joysticks — verify on the HTPC. Input only while Fjord's window has focus.
- [ ] Mapping to Fjord's actions, not fake key events where avoidable: D-pad and left stick (threshold + hysteresis) → Up/Down/Left/Right with the keyboard's repeat delay/rate when held; A → Enter; B → Back; X → context menu; Y → ?; Start → play/pause; Select → OSD/stats?; shoulders → seek steps; triggers → volume? (final table with the user). Every pad press counts as real input for the idle lock and the music screensaver (`activity.rs`).
- [ ] Later in the same item or after: show/rebind pad buttons in Settings → Key Bindings (`bindings.rs` gains pad buttons next to `KeyCombo`).
- [ ] Live test: every screen's D-pad contract with the pad (Enter/Right enter, Back, rows, …), hot-plug (pad switched on after Fjord started), Bluetooth and USB if available, held D-pad repeat, the screensaver waking on a pad press.

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### 0.5.2
- [ ] **Accent color (2026-11-08).** Settings → Interface → Accent color: each choice changes buttons, pills, progress bars, the selected sidebar tab and Settings section at once; white text on accent-filled buttons and pills stays readable in all six. Switch profiles → each keeps its own; "Copy settings from…" copies it. Focus ring color "Accent color" follows the choice. Restart → kept.
- [ ] **Colours from `Theme` (2026-11-08, nothing should look different but a few small unifications).** Dev: every screen, the player (OSD, seek bar, track panels, stats overlay), the music bar and queue, toasts, a Settings toggle, the startup loading bar. Slight changes on purpose: the dim behind the account/profile pickers and one overlay is a bit darker (75 → 80 %), a few shadows match the others, the volume % in the music bar and the detail page's text over a backdrop are a little brighter, the startup loading bar's track is the seek bar's grey.
- [ ] **Play-start timings (2026-11-08, needs Log level = Debug).** HTPC: start an episode from a page, let Up Next switch to the next one, stop, start a movie (display sync switching), play a song, then a second song. Send the log — the `play start on the UI thread: …` and `teardown on the UI thread: …` lines, with `ui: event loop busy` next to them, show which step holds the UI (item 3's step 2 follows from them).
- [ ] **Video sync default (2026-11-08).** Settings → Advanced → Video sync: the description reads "display-vdrop (default): smooth, also with passthrough · display-resample: smoothest, but off with passthrough · …". The HTPC keeps its saved display-vdrop. The dev machine keeps whatever it has saved (likely `audio`); set display-vdrop there and play a video: the log says `display sync on: video-sync=display-vdrop, …`.
- [ ] **Log folder going away (2026-11-08).** HTPC, the next time the log share is unavailable — at startup, or dropping out while Fjord runs: `journalctl --user` has one `fjord: log folder … not available` line and one `… is back after N s` line, not one every 2 s. Afterwards fjord.log has the lines from the gap, after `log folder reachable again after N s` (or `reachable after` at startup).

---

## Pending

- Nothing open.

## Issues

- None open.

---

## Deferred / future

- **Pull-request checks, once outside contributors send PRs.** A workflow on `pull_request` running `cargo fmt --check`, `slint-lsp format` (diff must be empty), clippy and tests — on GitHub-hosted runners (needs libmpv from apt), **never** on the self-hosted runner, which would run strangers' code on the dev machine.
- **New HTPC GPU (user plans an AMD card, 2026-10-08).** Afterwards: Settings → Advanced → Hardware decode `nvdec` → `auto`/`vaapi`, drop the NVDEC video-filter workaround, re-check HDR and 10-bit (KWin controls the bit depth on amdgpu). Optional: pick these defaults from the detected GPU vendor.
- **Own 10-bit buffers** (`dmabuf_plane.rs`, opt-in) only help where the driver can send 10-bit but EGL windows are 8-bit — e.g. NVIDIA Ampere+ over HDMI, or Pascal over DisplayPort (see DEVLOG, 2026-10-08). If they're ever used for real: explicit sync (`wp_linux_drm_syncobj_v1`) instead of `glFinish`.
- **Theming / layout customisation** — accent colours, drag-to-reorder dashboard rows (0.5.1 has hide + ▲/▼ per row, and the focus ring's colour/style). Needs the full layout system first.
- **Gamepad / remote** — the D-pad maps to arrow keys today; formal evdev/udev support deferred.
- **Vulkan rendering path — blocked upstream (checked 2026-10-09).** Idea: Slint's UI and mpv's video on Vulkan instead of OpenGL ES. mpv's libmpv render API (what Fjord embeds mpv through) only offers `opengl` and `sw` — in 0.41 and in current master; an RFC adding a Vulkan render API (mpv PR #18258, July 2026) was closed unmerged. Only possible with a patched mpv until that lands. Also less gain than this entry used to claim: with an AMD card, `hwdec=vaapi` into OpenGL is already zero-copy (dmabuf import), and the GTX 1050 Ti's driver supports Vulkan, so nothing forces OpenGL either way.
- **Refactors not planned** (low gain or high churn): splitting `AppState` into several globals (every Rust access would change), `reset_session_state` (a list by nature), `main()` (setup + `wire_*` calls).
- **Windows build (possible future, discussed 2026-11-08).** The core runs there as is: Slint (winit + OpenGL/WGL), libmpv's render API (`libmpv-2.dll`), reqwest/WebSocket (native TLS → SChannel), the bundled fonts, yt-dlp (`yt-dlp.exe`). Needed:
  - **Platform gates:** the Wayland/EGL/D-Bus modules (`video_surface.rs`, `dmabuf_plane.rs`, `hdr.rs` and the Wayland part of `activity.rs`) behind `cfg(target_os = "linux")`, and their crates as Linux-only dependencies. Windows uses the in-window video path.
  - **Windows replacements:** `SetThreadExecutionState` instead of `inhibit.rs`; `%APPDATA%`/`%LOCALAPPDATA%` instead of `HOME`/`XDG_*`; a uuid crate instead of `/proc/…/uuid`; a font crate instead of `fc-list`; mpv's `audio-device-list` property instead of `mpv --audio-device=help` (better on Linux too); `windows_subsystem = "windows"` and no console window for yt-dlp.
  - **Build:** libmpv linked through an MSVC import library, `libmpv-2.dll` shipped next to `fjord.exe`. A zip from a GitHub-hosted Windows runner (never the self-hosted one).
  - **Later:** display sync via `EnumDisplaySettings`/`ChangeDisplaySettingsEx`. HDR needs a D3D11 swapchain for the video — open-ended; until then SDR (mpv tone-maps). Unsigned builds get a SmartScreen warning.
  - **Size and testing:** about 1–2 weeks to a playing SDR build; the dev machine dual-boots Windows. Frame pacing under Windows' compositor is untested.
- **Android app — Android TV and phones (possible future, discussed 2026-11-08).** Carries over: the UI-free crates (`fjord-api`, `fjord-seerr`, `fjord-player`), Slint (has an Android backend, `backend-android-activity`), mpv (libmpv built with mpv-android's scripts), the D-pad key contract and the on-screen keyboard (TV). Needed:
  - **Prototype first — decides whether it's worth it:** hardware decoding into Slint's OpenGL texture. mpv's MediaCodec → AImageReader → GL route is zero-copy, but whether it works through the render API is unverified. The CPU-copy fallback is likely too slow for 4K on TV boxes. About 1–2 weeks to get video playing smoothly in Slint on an Android TV device.
  - **Lifecycle:** the GL surface is destroyed when the app goes to the background → free and rebuild mpv's render context and FBOs, keep the playback state.
  - **Platform pieces through JNI:** keep-screen-on, app storage folders, AudioTrack output and passthrough, media keys. Frame-rate matching via `Surface.setFrameRate`; HDR passthrough needs mpv on its own surface (SDR, tone-mapped, at first).
  - **Build:** NDK + cargo-ndk, arm64 libmpv/FFmpeg, rustls instead of native-tls (no OpenSSL on Android), APK signing; sideload, F-Droid or Play.
  - **Gaps:** no yt-dlp → only server trailer files unless another extractor is found. The UI effects (glow, blurred backgrounds, card rows) may be heavy for cheap TV-box GPUs.
  - **Phones:** touch and portrait layouts for most screens — much more than the TV version. TV first, then phones.
  - **Size:** about 1–2 months to a usable Android TV app after the prototype. The Windows platform gates are shared groundwork. Needs an Android TV device (Fire TV boxes are Android too) and a phone to test.
- **Long term: a general media client** — e.g. Spotify via librespot.
