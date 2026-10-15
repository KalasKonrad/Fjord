# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.5.0`). Implementation detail per feature: `DEVLOG.md` (dated sections).

---

## 0.5.1 (branch `release-0.5.1`) — draft, scope to confirm

A small release: bug fixes, polish, minor features. Released per CLAUDE.md → Releasing once everything here is done and live-tested.

**Bug fixes**
- [x] **Discover marks some titles that are already in the library as not in the library (reported 2026-10-05).** Cause found 2026-10-15 (user: "it doesn't recognise 4K"): cards read only Seerr's regular-tier `status`, never `status4k` — fixed, live check in the checklist below. Original note: Needs a few example titles to trace. If no examples turn up: a one-off diagnostic that runs Discover's in-library match over the landing rows and lists titles found by name/year in the library but not matched.

**Polish**
- [x] **Long titles shrink the poster on the smallest card size (115 px)** (was in Deferred). User's choice 2026-10-15: the title over the poster on a dark fade, always the whole title — done (`MediaCard`), live check in the checklist.
- [ ] **Card size setting (user, 2026-10-15):** Normal / Large (~+25%) / Extra large (~+50%), **per profile**, for every poster card — dashboards, library grids, Discover, artist/collection pages; grids drop columns to fit.
- [x] **Trim the dated story comments in `build-release.yml`'s header** to what the workflow needs (0.5.0 step 4 left the workflow out); the history is in DEVLOG.

**Minor features**
- [ ] **Release helper:** a script for CLAUDE.md → Releasing step 3 — sets `pkgver` in `PKGBUILD` and `fjord-bin/PKGBUILD` and fjord-bin's `sha256sums` from the published tarball.

**Before tagging**
- [ ] **Settings crash watch (Issues):** if it hasn't come back, remove the temporary `--cfg slint_debug_property` (Pending) and close the issue; if it has, fix it first.
- [ ] The first nightly from `release-0.5.1` built and installed (Live-test checklist).

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### 0.5.1
- [ ] **Title over the poster (2026-10-15).** Every poster card (dashboards, library grids, Discover, search, artist/collection/person pages): the poster fills the card; title and year sit over its bottom on a dark fade, readable on light and dark posters; the whole title shows (a long one covers more of the poster); posters in a row are all the same size. Cards still loading their poster: the title at the bottom of the grey card. Badges: ♥ top-left, ★ under it (or top-left alone), ✓ / unwatched count / Seerr pill top-right, the "Available in 4K" pill under the Seerr pill (Discover "Requested" row), progress strip at the bottom edge. Hover, focus ring and click as before.
- [ ] **Discover: titles owned only in 4K (2026-10-15).** A title you have only in 4K shows "4K In Library" on its Discover/search card and on Home/TV/Movies watchlist cards (before: no badge); a title you have in the regular version still shows "In Library"; a title requested only in 4K shows "4K Requested". Its context menu and request page no longer offer "Add to Blocklist" (same as for any owned title). Opening it still goes to the Jellyfin page as before.

### Packages and branch model (2026-10-14)
- [ ] **fjord-bin-nightly.** Install confirmed on dev 2026-10-15: replaced `fjord-git`, pacman shows `fjord-bin-nightly r1162.31c8a80-1`, the log `0.5.0 (r1162.31c8a80)`. Still open: the first push to `release-0.5.1` that touches code starts an Actions run for that branch and updates "nightly"; `makepkg -si` in `fjord-bin-nightly/` then installs it (pkgver `r<count>.<hash>` of the branch).

---

## Pending

- [ ] **Remove the temporary `--cfg slint_debug_property` from `PKGBUILD`'s `build()` and from the nightly build in `build-release.yml`** once the Settings crash (Issues) is understood or hasn't come back for a while — it's there so a repeat names the property in the `PANIC` line.

## Issues

- [ ] **Settings crash — watching (2026-10-08, r1064).** Two Slint "Recursion detected" panics on the HTPC after toggling Settings → Video → "Separate video surface" with the mouse and clicking Home. Not reproduced since (dev machine, debug build, several tries). If Fjord closes, send the log: the `PANIC` line has a timestamp and (HTPC build) the property's name; `settings changed: …` / `nav-selected(…)` show the steps before it.

---

## Deferred / future

- **Pull-request checks, once outside contributors send PRs.** A workflow on `pull_request` running `cargo fmt --check`, `slint-lsp format` (diff must be empty), clippy and tests — on GitHub-hosted runners (needs libmpv from apt), **never** on the self-hosted runner, which would run strangers' code on the dev machine.
- **New HTPC GPU (user plans an AMD card, 2026-10-08).** Afterwards: Settings → Video hardware decoding `nvdec` → `auto`/`vaapi`, drop the NVDEC video-filter workaround, re-check HDR and 10-bit (KWin controls the bit depth on amdgpu). Optional: pick these defaults from the detected GPU vendor.
- **Own 10-bit buffers** (`dmabuf_plane.rs`, opt-in) only help where the driver can send 10-bit but EGL windows are 8-bit — e.g. NVIDIA Ampere+ over HDMI, or Pascal over DisplayPort (see DEVLOG, 2026-10-08). If they're ever used for real: explicit sync (`wp_linux_drm_syncobj_v1`) instead of `glFinish`.
- **Theming / layout customisation** — accent colours, dashboard row visibility, drag-to-reorder rows. Needs the full layout system first.
- **Gamepad / remote** — the D-pad maps to arrow keys today; formal evdev/udev support deferred.
- **Vulkan rendering path — blocked upstream (checked 2026-10-09).** Idea: Slint's UI and mpv's video on Vulkan instead of OpenGL ES. mpv's libmpv render API (what Fjord embeds mpv through) only offers `opengl` and `sw` — in 0.41 and in current master; an RFC adding a Vulkan render API (mpv PR #18258, July 2026) was closed unmerged. Only possible with a patched mpv until that lands. Also less gain than this entry used to claim: with an AMD card, `hwdec=vaapi` into OpenGL is already zero-copy (dmabuf import), and the GTX 1050 Ti's driver supports Vulkan, so nothing forces OpenGL either way.
- **Long term: a general media client** — e.g. Spotify via librespot.
