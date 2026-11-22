# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.5.2`). Implementation detail per feature: `DEVLOG.md` (dated sections). Each version's live tests are summed up in DEVLOG → "0.5.x live tests".

---

## 0.5.2 (branch `release-0.5.2`)

**Released 2026-11-21** (`v0.5.2`): game controller support and pad rebinding, the player's controls with the remote, MPRIS, a light theme and accent color, the Upscaler and HD video in 4K, Settings regrouped, and the fixes of the test week (#45, #46, the code review, the pickers' server check). Live tests: DEVLOG → "0.5.2 live tests".

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### 0.5.3
- [ ] **Quick Connect sign-in (2026-11-22).** Sign out (or + Add Account), type only the server, press Quick Connect with the remote: a code card shows; enter the code in the Jellyfin web page or app (user menu → Quick Connect) → Fjord signs in within ~2 s and lands like a password login (Remember this login respected; with Add Account the new account is added next to the others). Also: Cancel and Escape on the code card return to the password card; a wrong address says it can't reach a server; leaving a code for 10 minutes says it expired. Enter on the code card's Cancel must not request a new code.
- [ ] **Spacings 6/10 → 8/12 px (2026-11-21).** Dev, Text size Normal and Huge: every screen still looks right — nothing cut off or overlapping, a pixel or two more room in places: the sidebar, dashboards and card rows (the remote's scrolling still centers the focused card), a film/album/artist/collection page, the context menu and the playlist picker, Settings (section headers, rows), the Dashboard rows editor (✓ ▲ ▼ centered in each row), the on-screen keyboard (letters and the PIN pad), the profile picker and Edit Profile, Connect Seerr, the calendar, a request page (2K/4K buttons, tag pills), the player's controls, the Sub/Audio panels (Up/Down still keeps the focused track in view in a long list), the Sync ▾ panel (all four buttons whole, also at Huge), the music bar, the queue panel and Now Playing.

---

## Pending


## Issues


---

## Deferred / future

- **UI freezes at a 4K HDR film's first start — understood, accepted (2026-11-21).** A 150–320 ms frame when a display switch resizes the window (Slint redraws at the new size), and 330–540 ms of busy UI thread about a second into a 4K HDR film — only the first time that film plays: NVIDIA's driver compiles its tone-mapping shaders (which likely carry the film's HDR values) and keeps them in `~/.cache/nvidia/GLCache`, so the next start of the same film doesn't freeze. Both happen while the TV switches modes, so nothing visible. Only fix: mpv rendering off the UI thread — not planned. DEVLOG → "UI freezes at a 4K HDR start".
- **Pull-request checks, once outside contributors send PRs.** A workflow on `pull_request` running `cargo fmt --check`, `slint-lsp format` (diff must be empty), clippy and tests — on GitHub-hosted runners (needs libmpv from apt), **never** on the self-hosted runner, which would run strangers' code on the dev machine.
- **New HTPC GPU (user plans an AMD card, 2026-10-08).** Afterwards: Settings → Advanced → Hardware decode `nvdec` → `auto`/`vaapi`, drop the NVDEC video-filter workaround, re-check HDR and 10-bit (KWin controls the bit depth on amdgpu). Optional: pick these defaults from the detected GPU vendor.
- **Own 10-bit buffers** (`dmabuf_plane.rs`, opt-in) only help where the driver can send 10-bit but EGL windows are 8-bit — e.g. NVIDIA Ampere+ over HDMI, or Pascal over DisplayPort (see DEVLOG, 2026-10-08). If they're ever used for real: explicit sync (`wp_linux_drm_syncobj_v1`) instead of `glFinish`.
- **Layout customisation** beyond the dashboard rows (order, visibility, dragging — 0.5.2), the accent colour and the focus ring's look: needs a real layout system first.
- **IR remotes and HDMI-CEC** — untested (no receiver or CEC adapter here). The game pad (0.5.2), keyboards and anything sending media keys (MPRIS, 0.5.2) work.
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
