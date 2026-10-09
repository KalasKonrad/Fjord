# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Implementation detail per feature: `DEVLOG.md` (dated sections).

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

(nothing open)

---

## Pending

- [ ] **Remove the temporary `--cfg slint_debug_property` line from `PKGBUILD`'s `build()`** once the Settings crash (Issues) is understood or hasn't come back for a while — it's there so a repeat names the property in the `PANIC` line.

## Issues

- [ ] **Settings crash — watching (2026-10-08, r1064).** Two Slint "Recursion detected" panics on the HTPC after toggling Settings → Video → "Separate video surface" with the mouse and clicking Home. Not reproduced since (dev machine, debug build, several tries). If Fjord closes, send the log: the `PANIC` line has a timestamp and (HTPC build) the property's name; `settings changed: …` / `nav-selected(…)` show the steps before it.
- [ ] **Discover marks some titles that are already in the library as not in the library (reported 2026-10-05).** Needs a few example titles to trace.
- [ ] **YouTube trailers sometimes take ~30 s to start or fail (2026-10-08).** Upstream: YouTube intermittently answers HTTP 403 to yt-dlp's stream URLs (yt-dlp issue #17647, open; latest stable 2026.08.19 is installed). Plain mpv: 2 of 3 runs opened in 4–5 s, 1 got a 403. In Fjord, ffmpeg retries the same URL with growing pauses (1/3/7/15 s) instead of failing. Options when wanted: update yt-dlp once upstream fixes it; or let Fjord re-open a trailer once with a fresh lookup after ~8 s without `FileLoaded`.

---

## Deferred / future

- **New HTPC GPU (user plans an AMD card, 2026-10-08).** Afterwards: Settings → Video hardware decoding `nvdec` → `auto`/`vaapi`, drop the NVDEC video-filter workaround, re-check HDR and 10-bit (KWin controls the bit depth on amdgpu). Optional: pick these defaults from the detected GPU vendor.
- **Own 10-bit buffers** (`dmabuf_plane.rs`, opt-in) only help where the driver can send 10-bit but EGL windows are 8-bit — e.g. NVIDIA Ampere+ over HDMI, or Pascal over DisplayPort (see DEVLOG, 2026-10-08). If they're ever used for real: explicit sync (`wp_linux_drm_syncobj_v1`) instead of `glFinish`.
- **Trickplay** — seek-bar scrub thumbnails. Jellyfin trickplay manifest (`GET /Videos/{id}/Trickplay/{width}/tiles`), tile-sheet geometry (tile size, columns, rows, interval), cached tiles per video, a thumbnail above the seek bar from `seek-hover-pos`.
- **Theming / layout customisation** — accent colours, dashboard row visibility, drag-to-reorder rows. Needs the full layout system first.
- **Poster/card scaling** — long titles shrink the poster on the smallest card breakpoint (115 px). Options: a larger-poster setting (multiplier or extra breakpoint tier on `dash-card-w`/`dash-card-h` in `main.slint`), or the title over a full-bleed poster with a dark scrim.
- **Gamepad / remote** — the D-pad maps to arrow keys today; formal evdev/udev support deferred.
- **Vulkan rendering path** — a second backend (Slint WGPU, `MPV_RENDER_API_TYPE_VULKAN`, Vulkan FBOs instead of `gl::*`), chosen in Config (`gpu_renderer`), applied on restart. Enables zero-copy `hwdec=vulkan` on AMD. Not a hardware requirement anywhere: the GTX 1050 Ti's driver supports Vulkan (checked 2026-08-17; the old "legacy NVIDIA needs OpenGL" note was wrong).
- **Long term: a general media client** — e.g. Spotify via librespot.
