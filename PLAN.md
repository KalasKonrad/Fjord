# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Implementation detail per feature: `DEVLOG.md` (dated sections).

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### 0.5.0 steps 1–2 smoke test (branch `release-0.5`)
- [ ] **Rust 2024 edition (2026-10-10).** No behaviour change intended; the Slint run below covers it. Watch especially: video in-window and on the separate surface (GL helpers), HDR title (hdr.rs), stall recovery and Up Next/gapless (playback.rs let-chains), profile switch and sign-in (profile.rs).
- [ ] **Slint 1.18.1 (2026-10-10).** Login; every dashboard (Home/TV/Movies/Music/Discover) with keyboard/remote and mouse wheel (scrolling still follows focus, wheel still scrolls); playback in-window (Separate video surface off) and on the subsurface; HDR title; Back → mini-player thumbnail in the right place; "Video in background" behind a menu; Settings (all sections, toggles, dropdowns); on-screen keyboard; typing in Login / Connect Seerr / Profile Edit fields keeps focus while typing. Held Left on the first card of a row stays there; a fresh Left press enters the sidebar.

### Security fixes (0.5.0 step 0)
- [x] **S7 — pinned actions (2026-10-10).** The next push to `main` still produces the "nightly" release (Actions tab green, new `fjord-x86_64.tar.gz`). Confirmed 2026-10-10: the nightly run for `d1a54da` (pinned actions) succeeded.
- [ ] **S6 — owner-only files (2026-10-10).** After a start: `stat -c '%a %n' ~/.config/fjord/config.json ~/.cache/fjord/logs ~/.cache/fjord/logs/fjord.log*` → 600 / 700 / 600 on both machines; on the HTPC the share files stay readable from the dev machine; no `log permissions not tightened` line (or, on the HTPC, it explains why).
- [ ] **S5 — plain HTTP is visible (2026-10-10).** Settings → the SERVER block at the bottom of the left pane shows "Not encrypted (http://)" in red for the own server (it's http://); SEERR too if Seerr is http://. Signing in with an address typed without `http(s)://` to a server that only answers on http → one toast "Jellyfin: connected without encryption …" (log `didn't answer over https`). Typed `http://…` → no toast.
- [ ] **S4 — live sync over HTTPS (2026-10-10).** Proven against a public `wss://` echo server; with the own (http://) server nothing changes: `ws: connected` as before. Only testable for real against an https:// Jellyfin.
- [ ] **S3 — Sign Out ends the server session (2026-10-10).** Sign Out → log `sign-out: ended the server session of <id>` for the account (and each linked sub-profile that had a token); the session disappears from Jellyfin's Dashboard → Devices. Signing in again works as before.
- [ ] **S2 — trailer links (2026-10-10).** Discover trailers still show "▶ Trailer" and play (log `trailer check: … plays`); nothing is skipped as `not an https YouTube URL` for normal titles.
- [ ] **S1 — cache paths (2026-10-10).** Posters, backdrops and dashboard rows still load and stay cached (start Fjord twice: the second start shows posters instantly; `~/.cache/fjord/posters/` keeps growing with 32-hex names). Discover posters too.

---

## Pending

- [ ] **`cargo audit` again after the Slint 1.18 upgrade and before tagging 0.5.0** — 2 advisories left (quick-xml 0.39.4 via Slint's accessibility stack, not reachable; see DEVLOG "Security review before 0.5.0").
- [ ] **0.5.0 release work happens on branch `release-0.5`** (plan: `~/.claude/plans/velvety-mixing-flute.md`); `PKGBUILD` on `main` builds it (`#branch=release-0.5`) — remove the fragment when it's merged.
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
- **Vulkan rendering path — blocked upstream (checked 2026-10-09).** Idea: Slint's UI and mpv's video on Vulkan instead of OpenGL ES. mpv's libmpv render API (what Fjord embeds mpv through) only offers `opengl` and `sw` — in 0.41 and in current master; an RFC adding a Vulkan render API (mpv PR #18258, July 2026) was closed unmerged. Only possible with a patched mpv until that lands. Also less gain than this entry used to claim: with an AMD card, `hwdec=vaapi` into OpenGL is already zero-copy (dmabuf import), and the GTX 1050 Ti's driver supports Vulkan, so nothing forces OpenGL either way.
- **Long term: a general media client** — e.g. Spotify via librespot.
