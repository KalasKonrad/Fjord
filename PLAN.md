# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.5.0`). Implementation detail per feature: `DEVLOG.md` (dated sections).

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### Still open from 0.5.0
- [ ] **Trailer retry (5c, 2026-10-13).** Fjord's answer to YouTube's intermittent HTTP 403s on yt-dlp's stream URLs (yt-dlp issue #17647): a stuck open used to take ~30 s while ffmpeg retried the same URL. If stuck trailers still show up often, update yt-dlp once upstream fixes it. Hard to provoke on purpose — watch for it while testing trailers (Discover, movie, series). A trailer that hasn't started after 7 s restarts by itself; log `trailer not open after 7 s — starting it once more` then `playing trailer (YouTube, retry)`; a second miss → "Trailer unavailable" toast and `trailer still not open 7 s after its retry — giving up`. Stopping a trailer during those 7 s must not restart it. Normal trailers (open in 1–2 s) show neither line.
- [ ] **Trickplay thumbnails (5d, 2026-10-14).** Dev 2026-10-14 (r1150): works on the one title with images (20 s, 2 thumbnails) — info fetched, tile decoded on remote seeks, user confirmed it shows; a title without images → `trickplay: none`. Still open: a long film (several tiles), HTPC. Needs titles with trickplay images (on 2026-10-14 only one 20-second movie had them — wait for the server's "Generate Trickplay Images" task). Play such a title: hovering the seek bar shows a 240 px wide thumbnail above the time bubble that follows the mouse and changes picture every Interval (10 s); Left/Right seeks (remote) show it above the bar at the target while the seek pill is up. A long film: thumbnails from later tiles appear too (moving across the whole bar), without stutter. A title without trickplay: nothing new on the bar. Log (Debug): `trickplay: <id> has N thumbnail(s) of 320x180, one every 10000 ms, 10x10 per tile`, `trickplay: tile K of <id> decoded (3200x1800)`; without data `trickplay: none for <id>`. Memory: no growth beyond ~50 MB while scrubbing (two decoded tiles).
- [ ] **S6 — owner-only files (2026-10-10).** Dev confirmed 2026-10-10 (600/700/600); HTPC still to check. After a start: `stat -c '%a %n' ~/.config/fjord/config.json ~/.cache/fjord/logs ~/.cache/fjord/logs/fjord.log*` → 600 / 700 / 600 on both machines; on the HTPC the share files stay readable from the dev machine; no `log permissions not tightened` line (or, on the HTPC, it explains why).
- [ ] **S4 — live sync over HTTPS (2026-10-10).** Proven against a public `wss://` echo server; with the own (http://) server nothing changes: `ws: connected` as before. Only testable for real against an https:// Jellyfin.

### Packages and branch model (2026-10-14)
- [ ] **fjord-bin (release).** `makepkg -si` in `fjord-bin/` downloads the v0.5.0 tarball, the checksum passes, and pacman replaces the installed Fjord package; Settings shows 0.5.0.
- [ ] **fjord-bin-nightly.** Install confirmed on dev 2026-10-15: replaced `fjord-git`, pacman shows `fjord-bin-nightly r1162.31c8a80-1`, the log `0.5.0 (r1162.31c8a80)`. Still open: the first push to `release-0.5.1` that touches code starts an Actions run for that branch and updates "nightly"; `makepkg -si` in `fjord-bin-nightly/` then installs it (pkgver `r<count>.<hash>` of the branch).
- [x] **fjord (source).** Confirmed on dev 2026-10-15: pacman replaced `fjord-bin-nightly` with `fjord 0.5.0-1`, Fjord started with `0.5.0 (r1162.31c8a80)`, no warnings. `makepkg -si` in the repo root builds the `v0.5.0` tag and replaces the installed `fjord-git`; pacman shows `fjord 0.5.0-1`, Settings 0.5.0 (r1162…).

---

## Pending

- [ ] **Remove the temporary `--cfg slint_debug_property` from `PKGBUILD`'s `build()` and from the nightly build in `build-release.yml`** once the Settings crash (Issues) is understood or hasn't come back for a while — it's there so a repeat names the property in the `PANIC` line.

## Issues

- [ ] **Settings crash — watching (2026-10-08, r1064).** Two Slint "Recursion detected" panics on the HTPC after toggling Settings → Video → "Separate video surface" with the mouse and clicking Home. Not reproduced since (dev machine, debug build, several tries). If Fjord closes, send the log: the `PANIC` line has a timestamp and (HTPC build) the property's name; `settings changed: …` / `nav-selected(…)` show the steps before it.
- [ ] **Discover marks some titles that are already in the library as not in the library (reported 2026-10-05).** Needs a few example titles to trace.

---

## Deferred / future

- **Pull-request checks, once outside contributors send PRs.** A workflow on `pull_request` running `cargo fmt --check`, `slint-lsp format` (diff must be empty), clippy and tests — on GitHub-hosted runners (needs libmpv from apt), **never** on the self-hosted runner, which would run strangers' code on the dev machine.
- **New HTPC GPU (user plans an AMD card, 2026-10-08).** Afterwards: Settings → Video hardware decoding `nvdec` → `auto`/`vaapi`, drop the NVDEC video-filter workaround, re-check HDR and 10-bit (KWin controls the bit depth on amdgpu). Optional: pick these defaults from the detected GPU vendor.
- **Own 10-bit buffers** (`dmabuf_plane.rs`, opt-in) only help where the driver can send 10-bit but EGL windows are 8-bit — e.g. NVIDIA Ampere+ over HDMI, or Pascal over DisplayPort (see DEVLOG, 2026-10-08). If they're ever used for real: explicit sync (`wp_linux_drm_syncobj_v1`) instead of `glFinish`.
- **Theming / layout customisation** — accent colours, dashboard row visibility, drag-to-reorder rows. Needs the full layout system first.
- **Poster/card scaling** — long titles shrink the poster on the smallest card breakpoint (115 px). Options: a larger-poster setting (multiplier or extra breakpoint tier on `dash-card-w`/`dash-card-h` in `main.slint`), or the title over a full-bleed poster with a dark scrim.
- **Gamepad / remote** — the D-pad maps to arrow keys today; formal evdev/udev support deferred.
- **Vulkan rendering path — blocked upstream (checked 2026-10-09).** Idea: Slint's UI and mpv's video on Vulkan instead of OpenGL ES. mpv's libmpv render API (what Fjord embeds mpv through) only offers `opengl` and `sw` — in 0.41 and in current master; an RFC adding a Vulkan render API (mpv PR #18258, July 2026) was closed unmerged. Only possible with a patched mpv until that lands. Also less gain than this entry used to claim: with an AMD card, `hwdec=vaapi` into OpenGL is already zero-copy (dmabuf import), and the GTX 1050 Ti's driver supports Vulkan, so nothing forces OpenGL either way.
- **Long term: a general media client** — e.g. Spotify via librespot.
