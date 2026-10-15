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
- [x] **Card size setting (user, 2026-10-15):** Normal / Large (~+25%) / Extra large (~+50%), **per profile**, for every poster card — dashboards, library grids, Discover, artist/collection pages; grids drop columns to fit. Done, live check in the checklist.
- [x] **Trim the dated story comments in `build-release.yml`'s header** to what the workflow needs (0.5.0 step 4 left the workflow out); the history is in DEVLOG.

**Minor features**
- [x] **Trailer quality → Player section, always visible (user, 2026-10-15).** Done; live check in the checklist. It limits every YouTube trailer, also the movie/series Trailer buttons that need no Seerr, but the row sat in Integrations behind "Seerr connected". Config field unchanged.
- [x] **Settings restructure (user, 2026-10-15):** sections by task — Playback, Subtitles & language, Display, Audio, Interface, Profiles, Integrations, Key Bindings, Advanced — and a "Profile" tag on per-profile rows. Done, live check in the checklist.
- [x] **Copy settings from another profile (user, 2026-10-15).** Done, live check in the checklist. Settings → Profiles → "Copy settings from…": lists every other profile saved on this device; pick one → confirm → the current profile gets its **settings only**: subtitles (on/off, languages, type, size, position, colour, background, ASS styling), audio language, Now Playing auto-open, the five skip modes + seconds, card size, trailer quality. Never: sign-in (server, user id, token, remember login), identity (name, avatar, PIN, auto-lock, Bonfire links), Integrations (Seerr connection/on-off), remembered browsing (library sorts, Discover filters, request choices). No PIN: it only reads preferences from this device's config. Applied at once, saved, toast.
- [ ] **UI review (user, 2026-10-15)** — chosen from a code review of every visible text, colour and font size:
  - [x] **Readable setting descriptions:** done, live check in the checklist — 30 of 82 were cut off ("…", one line); the focused row shows its whole description, the longest ones get shorter.
  - [ ] **Consistent wording:** setting names in sentence case, buttons/dialogs/rows in Title Case ("Key bindings" in the sidebar, "Copy Settings from…?"), American spelling (Favorite, color), "Couldn't …" errors without raw technical text (that goes to the log).
  - [ ] **Theme colours + press flash:** 2–3 overlay/scrim tokens instead of 8 hand-picked darkness values (+ a token for white text on images); press flash on the clickables that lack it (Discover filter pills/popup options, calendar days/popup entries, Now Playing queue rows).
  - [ ] **Text size setting:** Settings → Interface → "Text size" Normal / Large (~+15%) / Extra large (~+30%), per profile (Profile tag, copied by "Copy settings from…"); every font size scales, small pills/badges grow with it.
- [x] **Release helper:** `scripts/bump-packages.sh` (done 2026-10-15) — a script for CLAUDE.md → Releasing step 3 — sets `pkgver` in `PKGBUILD` and `fjord-bin/PKGBUILD` and fjord-bin's `sha256sums` from the published tarball.

**Before tagging**
- [ ] **Settings crash watch (Issues):** if it hasn't come back, remove the temporary `--cfg slint_debug_property` (Pending) and close the issue; if it has, fix it first.
- [ ] The first nightly from `release-0.5.1` built and installed (Live-test checklist).

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### 0.5.1
- [ ] **Title over the poster (2026-10-15).** Every poster card (dashboards, library grids, Discover, search, artist/collection/person pages): the poster fills the card; title and year sit over its bottom on a dark fade, readable on light and dark posters; the whole title shows (a long one covers more of the poster); posters in a row are all the same size. Cards still loading their poster: the title at the bottom of the grey card. Badges: ♥ top-left, ★ under it (or top-left alone), ✓ / unwatched count / Seerr pill top-right, the "Available in 4K" pill under the Seerr pill (Discover "Requested" row), progress strip at the bottom edge. Hover, focus ring and click as before.
- [ ] **Discover: titles owned only in 4K (2026-10-15).** A title you have only in 4K shows "4K In Library" on its Discover/search card and on Home/TV/Movies watchlist cards (before: no badge); a title you have in the regular version still shows "In Library"; a title requested only in 4K shows "4K Requested". Its context menu and request page no longer offer "Add to Blocklist" (same as for any owned title). Opening it still goes to the Jellyfin page as before.

- [ ] **Card size setting (2026-10-15).** Settings → Interface → first row "Card size" (Normal / Large / Extra large), with keyboard (Enter opens the list, Right cycles) and mouse. Large/Extra large: bigger cards at once on Home/TV/Movies/Collections/Music, the Discover rows, the library grids (fewer columns, still left to right without gaps cut off), the Discover search grid, artist and collection grids, and the rows on movie/series/person/season/collection pages. Keyboard focus still scrolls the focused card into view everywhere (dashboard rows, grid rows, detail-page rows). Switching profile switches the size; restarting keeps it. Normal looks exactly as before. Log (Debug): the `settings changed` line names `card_size`.

- [ ] **Trailer quality in Player (2026-10-15).** Settings → Playback: "Trailer quality" is the last row, also with Seerr off; Integrations no longer has it. The saved value is unchanged; keyboard (Enter opens the list, Right cycles) and mouse work; a movie/series trailer plays at the chosen resolution (stats overlay `I`).

- [ ] **Settings restructure (2026-10-15).** The sidebar lists Playback, Subtitles & language, Display, Audio, Interface, Profiles, Integrations, Key Bindings, Advanced; Enter/Right from the app sidebar opens Playback. Each section, keyboard and mouse: Up/Down reach every row in screen order and the focused row scrolls into view (long ones: Display with Sync display on, Advanced, Subtitles & language); conditional rows still appear/disappear (subtitle rows with subtitles off, skip seconds for "Ask (timed)", Video filter only with nvdec, tscale with interpolation, 10-bit buffers with the separate surface, the passthrough rows with SPDIF on, display-sync rows). The "Profile" tag shows on the 23 per-profile rows (subtitles, audio language, skip modes + seconds, trailer quality, Now Playing auto-open, card size) and nowhere else. Every value is what it was before the update. The sidebar profile menu's "Profile settings" still opens Profiles.

- [ ] **Copy settings from another profile (2026-10-15).** Settings → Profiles: "Copy settings from…" (under Manage Profiles) only when another profile is saved on this device; the list shows every other profile, a name shared by two households with the account in brackets. Pick one (keyboard: Enter opens the list, Enter picks; Right also opens it; mouse) → "Copy settings from X?" (Left/Right/Enter/Back; mouse) → Copy: the 23 "Profile" rows now show X's values at once (e.g. subtitle language, a skip mode, card size changes the cards right away), a toast "Settings copied from X"; sign-in, Seerr, launch/remember-login and the device rows (Display, Audio output, Advanced…) are unchanged; restart keeps them. Cancel/Back changes nothing. No PIN asked, also for a PIN-protected source. Log: `copy settings: source picked — …`, `copy settings: copied the profile settings of <id> …`, then `settings changed: …` naming the copied fields.

- [ ] **Setting descriptions (2026-10-15).** In Settings, a row's description is one line ("…" if long) until the row is selected (keyboard or click); then it wraps and shows in full, and the rows below move down. Scrolling with Up/Down still keeps the selected row in view. The shortened texts (skip fade, HDR passthrough, separate video surface, display sync rows, 10-bit/dither tests, cache, on-screen keyboard…) read well.

### Packages and branch model (2026-10-14)
- [ ] **fjord-bin-nightly.** Install confirmed on dev 2026-10-15: replaced `fjord-git`, pacman shows `fjord-bin-nightly r1162.31c8a80-1`, the log `0.5.0 (r1162.31c8a80)`. Still open: the first push to `release-0.5.1` that touches code starts an Actions run for that branch and updates "nightly"; `makepkg -si` in `fjord-bin-nightly/` then installs it (pkgver `r<count>.<hash>` of the branch).

---

## Pending

- [ ] **Remove the temporary `--cfg slint_debug_property` from `PKGBUILD`'s `build()` and from the nightly build in `build-release.yml`** once the Settings crash (Issues) is understood or hasn't come back for a while — it's there so a repeat names the property in the `PANIC` line.

## Issues

- [ ] **Settings crash — watching (2026-10-08, r1064).** Two Slint "Recursion detected" panics on the HTPC after toggling Settings → Video (since 0.5.1: Display) → "Separate video surface" with the mouse and clicking Home. Not reproduced since (dev machine, debug build, several tries). If Fjord closes, send the log: the `PANIC` line has a timestamp and (HTPC build) the property's name; `settings changed: …` / `nav-selected(…)` show the steps before it.

---

## Deferred / future

- **Pull-request checks, once outside contributors send PRs.** A workflow on `pull_request` running `cargo fmt --check`, `slint-lsp format` (diff must be empty), clippy and tests — on GitHub-hosted runners (needs libmpv from apt), **never** on the self-hosted runner, which would run strangers' code on the dev machine.
- **New HTPC GPU (user plans an AMD card, 2026-10-08).** Afterwards: Settings → Advanced → Hardware decode `nvdec` → `auto`/`vaapi`, drop the NVDEC video-filter workaround, re-check HDR and 10-bit (KWin controls the bit depth on amdgpu). Optional: pick these defaults from the detected GPU vendor.
- **Own 10-bit buffers** (`dmabuf_plane.rs`, opt-in) only help where the driver can send 10-bit but EGL windows are 8-bit — e.g. NVIDIA Ampere+ over HDMI, or Pascal over DisplayPort (see DEVLOG, 2026-10-08). If they're ever used for real: explicit sync (`wp_linux_drm_syncobj_v1`) instead of `glFinish`.
- **Theming / layout customisation** — accent colours, dashboard row visibility, drag-to-reorder rows. Needs the full layout system first.
- **Gamepad / remote** — the D-pad maps to arrow keys today; formal evdev/udev support deferred.
- **Vulkan rendering path — blocked upstream (checked 2026-10-09).** Idea: Slint's UI and mpv's video on Vulkan instead of OpenGL ES. mpv's libmpv render API (what Fjord embeds mpv through) only offers `opengl` and `sw` — in 0.41 and in current master; an RFC adding a Vulkan render API (mpv PR #18258, July 2026) was closed unmerged. Only possible with a patched mpv until that lands. Also less gain than this entry used to claim: with an AMD card, `hwdec=vaapi` into OpenGL is already zero-copy (dmabuf import), and the GTX 1050 Ti's driver supports Vulkan, so nothing forces OpenGL either way.
- **Long term: a general media client** — e.g. Spotify via librespot.
