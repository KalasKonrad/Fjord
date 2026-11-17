# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.5.1`). Implementation detail per feature: `DEVLOG.md` (dated sections). Each version's live tests are summed up in DEVLOG → "0.5.x live tests".

---

## 0.5.2 (branch `release-0.5.2`)

**Release plan (user, 2026-11-13):** release Friday/Saturday 2026-11-20/21 (CLAUDE.md → Releasing); decide 0.5.2 vs 0.6.0 then (a game controller, a light theme, MPRIS and remote control of the player). All planned work is in, and its checklist was gone through by 2026-11-15 (DEVLOG → "0.5.2 live tests"). #45 and #46 were fixed and verified on r1359 (2026-11-16) — nothing left before the release.

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### 0.5.2
- [ ] **Bar focus per video (2026-11-17, review).** With the remote: Down, Right to ■ Stop, Enter → stopped. Play another video, Down → the focus is on ⏯ (not ■); OK pauses. Same after ← (minimize) and back to the player.
- [ ] **Detail page after a film's end (2026-11-17, review).** Open a film's page, play it (or skip to its last minutes) and let it end with nothing next → back on the page, Resume is gone, the film shows as watched and the focus is on Play. Log (Debug): `detail: refreshed <id> after playback`.
- [ ] **Back keeps its focus (2026-11-17, review).** Open a film's page and press Left at once to ← Back (before the Trailer button finishes checking) → the focus stays on Back when the check ends; Enter closes the page.
- [ ] **Pad unplugged mid-hold (2026-11-17, review).** Dev or HTPC, wired pad: hold the D-pad down in a library grid and pull the cable (or switch the wireless pad off) while holding → the cursor stops moving. Plug it back in → it works as before.
- [ ] **HD video in 4K needs Sync refresh rate (2026-11-17, review).** Settings → TV & display: switch Sync refresh rate off → HD video in 4K is grayed, "Needs Sync refresh rate on (below)", Enter/Right leave it alone; on again → usable. (The HTPC keeps refresh-rate sync on, so its 4K switching doesn't change.)
- [ ] **Discover Retry with Back (2026-11-17, review).** Seerr stopped: Discover → Right to Retry → Back → the sidebar has the focus (as with Left). Retry focused, then click another tab (Home) with the mouse → the sidebar's highlight is there with the remote. Retry with Enter while Seerr is still down → the keys keep working while it loads.

---

## Pending

- **CLAUDE.md rule for sizes (from 9d):** font sizes and corner radii only from `Theme` tokens (fs-*, radius-sm/radius/radius-lg; round things as half their size), spacing from the sp-* tokens where one fits — next to the existing "every colour from Theme" rule.

## Issues

- **UI stalls left at a playback start (HTPC, 2026-11-13, not urgent — the screen is re-syncing then anyway).** (1) A 150–280 ms slow frame when a display switch resizes the window (1080p ↔ 4K: Slint redraws everything at the new size). (2) ~350–370 ms `event loop busy` about a second into a 4K HDR start on the 4K mode (3 of 3 such starts since the Upscaler; none at 1080p SDR starts, none when the TV stayed at 1080p) — likely mpv compiling its HDR shaders for the 4K target on the render (UI) thread. mpv's `gpu-shader-cache` is on by default; whether it's used under the render API on the HTPC (GLES 3.2, `~/.cache/mpv`) is unchecked.

---

## Deferred / future

- **Pull-request checks, once outside contributors send PRs.** A workflow on `pull_request` running `cargo fmt --check`, `slint-lsp format` (diff must be empty), clippy and tests — on GitHub-hosted runners (needs libmpv from apt), **never** on the self-hosted runner, which would run strangers' code on the dev machine.
- **New HTPC GPU (user plans an AMD card, 2026-10-08).** Afterwards: Settings → Advanced → Hardware decode `nvdec` → `auto`/`vaapi`, drop the NVDEC video-filter workaround, re-check HDR and 10-bit (KWin controls the bit depth on amdgpu). Optional: pick these defaults from the detected GPU vendor.
- **Own 10-bit buffers** (`dmabuf_plane.rs`, opt-in) only help where the driver can send 10-bit but EGL windows are 8-bit — e.g. NVIDIA Ampere+ over HDMI, or Pascal over DisplayPort (see DEVLOG, 2026-10-08). If they're ever used for real: explicit sync (`wp_linux_drm_syncobj_v1`) instead of `glFinish`.
- **Layout customisation** beyond the dashboard rows (order, visibility, dragging — 0.5.2), the accent colour and the focus ring's look: needs a real layout system first.
- **IR remotes and HDMI-CEC** — untested (no receiver or CEC adapter here). The game pad (0.5.2), keyboards and anything sending media keys (MPRIS, 0.5.2) work.
- **Spacing 6/10 px → the scale (9d leftover).** 52 paddings/spacings of 6 and 10 px stayed literal: they sit in FjordButton's padding (fixed-width buttons), the on-screen keyboard grid, the OSD panels and card internals. Fold them to 8/12 with a screenshot pass of every screen.
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
