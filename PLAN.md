# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.5.1`). Implementation detail per feature: `DEVLOG.md` (dated sections). 0.5.1's live tests are summed up in DEVLOG → "0.5.1 live tests".

---

## 0.5.2 (branch `release-0.5.2`)

Scope chosen by the user on 2026-11-08. In the order of work: small and safe first, the game controller last (on its own feature branch, CLAUDE.md → Development workflow). Released per CLAUDE.md → Releasing once everything here is done and live-tested. With a game controller and a light theme this is more than a patch: decide 0.5.2 vs 0.6.0 at release time.

**1. Quieter missing log folder**
- [ ] While the log folder can't be opened (e.g. the network share isn't there), `LazyLogFile` prints `fjord: log folder … not ready` to stderr (→ the journal) on every 2 s retry. Print it once when it first fails, and once when the folder is back (the file already gets "log folder reachable after N s"). main.rs `LazyLogFile::write`. Unit-testable with a folder that appears later.

**2. Video sync default → `display-vdrop`**
- [ ] `default_video_sync()` (config.rs) and `PlayerConfig::default` (fjord-player) → `display-vdrop`. **New installs only** (user): a saved `audio` stays. Why: `audio` times frames by the audio clock and drifts against the display (the 0.5.1 choppiness); `display-vdrop` works with normal audio *and* passthrough. Not `display-resample`: verified in mpv 0.41's `player/video.c` (`handle_display_sync_frame`: `if (resample && using_spdif_passthrough(mpctx)) return;`) — with passthrough it leaves display sync off entirely. The Settings description says when to pick which; DEVLOG gets the mpv source reference.

**3. Shorter pause at playback start and episode switch**
- [ ] HTPC logs (r1291): `ui: event loop busy` 200–340 ms (avg 227 ms, 19 times) and slow frames of 150–183 ms in mode Player, at every start and at Up Next's switch (`playback replaced`). Step 1: time each part on the UI thread with `timed` — `start_playback`'s steps, `tear_down_player` (render context free, `Player` drop = mpv's terminate/destroy), the render context creation in `BeforeRendering` — and run it on the HTPC. Step 2: move what's slow off the UI thread (e.g. drop the old `Player` on a worker thread after its render context is freed on ours — CLAUDE.md: `MpvRenderCtx` before `Player`). Goal: no `event loop busy` over ~50 ms at a start.

**4. Try Interpolation**
- [ ] It was silently off until 0.5.1 (needs display sync) and works now. Live test, HTPC: an episode with Settings → Advanced → Interpolation on (tscale `oversample`), first with the display switched to the film's rate (ratio 1.0000: expect little visible change — oversample mostly smooths a rate mismatch), then with display sync's mode switching off (desktop 119.88 Hz = exactly 5 refreshes per 23.976 fps frame, so also little) and on a 60 Hz mode (3:2 judder → this is where it should help). Watch the log's slowest mpv render call and drops (GTX 1050 Ti load). Dev machine likewise. Outcome: the setting's description says when it helps; default stays off unless the test says otherwise.

**5. Accent colour choice**
- [ ] Settings → Interface → "Accent color", per profile (Profile tag, copied by "Copy settings from…"): a few colours (today's blue as default + e.g. teal, purple, orange, green, red — final list when building, checked for contrast with white text). `Theme.accent` / `accent-hover` / `accent-muted` (142 uses) become computed from the choice instead of fixed values, so no screen changes. A `settings::CHOICES` entry + `PROFILE_BINDS` line (CLAUDE.md). Error red stays red.

**6. Light theme**
- [ ] Settings → Interface → "Theme": Dark (default) / Light, per profile. Builds on 5: every `Theme` token gets a light value, chosen by the setting. First an audit for colours that bypass `Theme` (hex literals in screens — CLAUDE.md says none should, check it) and for what must stay dark in both: the player/OSD, text and scrims over pictures (`scrim*`, `osd-panel*`, `text-bright`/`text-soft`, `hero-fade-*`), the video area. The focus ring's white default needs a light-theme default (the per-profile focus colour stays the user's choice). Screenshots of every screen in both themes before ticking.

**7. Drag rows in the dashboard rows editor**
- [ ] Settings → Interface → Dashboard rows → Edit…: drag a row with the mouse to a new place (the remote keeps ✓ ▲ ▼). Same per-profile `*-row-order` lists; the drop updates the order through the existing move path, and keyboard focus follows the dragged row (CLAUDE.md: mouse sets the same state as keys). Drop indicator between rows; auto-scroll near the list's edges.

**8. Game controller** (feature branch off `release-0.5.2`)
- [ ] The user can test only a game controller (no IR receiver, no HDMI-CEC adapter) — which pad: ask before building. Read pads with the `gilrs` crate (evdev on Linux, hot-plug, SDL's controller mapping database; verify its version, build deps — libudev — and the PKGBUILDs' `depends`). Pads are readable by the logged-in user through udev's `uaccess` for joysticks — verify on the HTPC. Input only while Fjord's window has focus.
- [ ] Mapping to Fjord's actions, not fake key events where avoidable: D-pad and left stick (threshold + hysteresis) → Up/Down/Left/Right with the keyboard's repeat delay/rate when held; A → Enter; B → Back; X → context menu; Y → ?; Start → play/pause; Select → OSD/stats?; shoulders → seek steps; triggers → volume? (final table with the user). Every pad press counts as real input for the idle lock and the music screensaver (`activity.rs`).
- [ ] Later in the same item or after: show/rebind pad buttons in Settings → Key Bindings (`bindings.rs` gains pad buttons next to `KeyCombo`).
- [ ] Live test: every screen's D-pad contract with the pad (Enter/Right enter, Back, rows, …), hot-plug (pad switched on after Fjord started), Bluetooth and USB if available, held D-pad repeat, the screensaver waking on a pad press.

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### 0.5.2
- Nothing yet.

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
- **Long term: a general media client** — e.g. Spotify via librespot.
