# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Implementation detail per feature: `DEVLOG.md` (dated sections).

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### `hdr-10bit` branch
- [ ] **Settings crash (r1064).** Two Slint "Recursion detected" panics on the HTPC after toggling Settings → Video → "Separate video surface" **with the mouse** and then **clicking Home** in the sidebar. Reproduce on the dev machine with the debug build — `CARGO_TARGET_DIR=target/debugprop RUSTFLAGS="--cfg slint_debug_property" cargo run -p fjord-app` — the `PANIC` line then names the property; `settings changed: …` and `nav-selected(…)` show the steps before it. **Not reproduced on the dev machine (2026-10-08, debug build r1071):** toggled with the mouse three times each way, Home clicked after each — no panic. Watch for it on the HTPC (the release build there names the property too).
- [ ] **Own 10-bit buffers are opt-in (HTPC).** Setting "Use Fjord's own 10-bit buffers" off (default): `video backplane ready: … presenting through EGL window R8G8B8A0 (own 10-bit buffers off in Settings)`, path line `mpv depth 8`, playback as before. On + restart Fjord: `own dmabuf buffers XBGR2101010 …`.
- [ ] **Own 10-bit buffers on the AMD dev machine.** Setting on + restart: `presenting through own dmabuf buffers …`, path line `mpv depth 10`. Picture upright (subtitles at the bottom) and colours right in fullscreen, the mini-player thumbnail and "Video in background"; window resize; Stop → next video without a flash of the old frame; an HDR title; `Fjord 10-bit gradient test (2026)` smooth. At stop: `… N frame(s) skipped` ≈ 0. Setting off again → `EGL window R10G10B10A0 …, mpv depth 10`.
- [ ] **"Turn off dithering (test)".** On → `mpv player started … dither-depth=no`; the gradient clip shows steps on an 8-bit path. Turn it off again afterwards.
- [x] **No Jellyfin token in the log (2026-10-08).** `ws: connecting to …?api_key=REDACTED&deviceId=…`; nowhere a raw `api_key=` value. Confirmed on the dev machine 2026-10-08.
- [x] **First open waits for the server's disks (2026-10-08).** First play after the server has been idle: `still opening after 5.0s (server disks waking up?) … waiting up to 15s before a reload`, and no `playback stalled … reloading` during the spin-up. Confirmed on the dev machine 2026-10-08: the first film took 11.5 s to open, grace logged at 5 s, no reload (a mid-film stall wasn't exercised; that path is unchanged).

### Earlier work
- [ ] **Failed opens no longer leave a black screen (2026-10-05).** A Discover trailer that won't play (e.g. *The Apothecary Diaries*, `3lfb_KeqdEM` "Video unavailable") closes with "Trailer unavailable" within a couple of seconds (`mpv: file failed to open/play`). A library item that won't open retries (`file failed to open/play … — reloading stream`) and, past the budget, stops with "Couldn't play this — the file wouldn't open".
- [ ] **Trailer button check (2026-10-05).** Discover title → greyed "Checking…", then "▶ Trailer" or greyed "No trailer" (`trailer check: … plays` / `won't play`); the D-pad skips a greyed button; reopening is instant; a blocked first trailer with a working second one plays the second.
- [x] **"Sync display for trailers" (2026-10-05, default off).** Display sync on + this off: a trailer plays with no `display_sync: switching` lines; on: it switches like a normal video.
- [ ] **Caret editing in every text field (2026-10-05).** Physical keyboard in Discover/Browse/Library search, the new-playlist name and the Bonfire join code: Left/Right/Home/End move the caret, typing inserts there, Backspace/Delete remove before/after it, results follow; Right at the end of the playlist name still creates it. On-screen keyboard: last row 123 · ◀ · space · ▶ · Done (cursor lands on Done); ◀ ▶ also move the cursor in Login, Connect Seerr and Profile Edit fields; each opening starts at the end of the field.
- [x] **No display-mode set at startup (2026-10-05).** Display sync on, start Fjord, pick a profile → no `display_sync: switching …` until something plays and stops.
- [x] **One "Requested — added to Watchlist" toast (2026-10-05)** when requesting a title that wasn't on the watchlist; plain "Requested" when it was.
- [x] **No `save_config: rename … failed` at startup (2026-10-05)** in either machine's log.
- [x] **Subtitle type "Hearing Impaired" (2026-10-06).** A film with both an English and an English SDH subtitle track → the SDH one is chosen.
- [x] **Repeat One for a single song (2026-09-26).** One song on its own with Repeat One (↺¹) starts again at the end; a different single song picked during an album with Repeat One repeats that song; Repeat One inside an album still repeats the current track, gapless (`repeat one: replaying …` / `gapless: preloaded next track`).
- [x] **Display sync edge cases (2026-09-24).** Stop, then quit (or stop twice) shortly after → the second revert does nothing (no second `display_sync: switching` line). A 4K HDR title followed right away by a 1080p SDR item (and the other way round) switches cleanly both ways.

---

## Pending

- [ ] **Finish `hdr-10bit`:** fix the Settings crash above, docs touch-up, merge into `main`, then on `main` remove `#branch=hdr-10bit` from `PKGBUILD`'s `source=` **and** the `--cfg slint_debug_property` RUSTFLAGS line in `build()` (both there only so the HTPC's `makepkg -si` builds the branch with Slint's property names in panics).

## Issues

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
