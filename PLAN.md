# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

## Completed

Version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.5.0`). Implementation detail per feature: `DEVLOG.md` (dated sections).

---

## 0.5.1 (branch `release-0.5.1`) — draft, scope to confirm

A small release: bug fixes, polish, minor features. Released per CLAUDE.md → Releasing once everything here is done and live-tested.

**Bug fixes**
- [x] **Discover marks some titles that are already in the library as not in the library (reported 2026-10-05).** Cause found 2026-10-15 (user: "it doesn't recognise 4K"): cards read only Seerr's regular-tier `status`, never `status4k` — fixed below. Original note: Needs a few example titles to trace. If no examples turn up: a one-off diagnostic that runs Discover's in-library match over the landing rows and lists titles found by name/year in the library but not matched.

**Polish**
- [x] **Long titles shrink the poster on the smallest card size (115 px)** (was in Deferred). User's choice 2026-10-15: the title over the poster on a dark fade, always the whole title — done (`MediaCard`).
- [x] **Card size setting (user, 2026-10-15):** Normal / Large (~+25%) / Extra large (~+50%), **per profile**, for every poster card — dashboards, library grids, Discover, artist/collection pages; grids drop columns to fit. Done.
- [x] **Trim the dated story comments in `build-release.yml`'s header** to what the workflow needs (0.5.0 step 4 left the workflow out); the history is in DEVLOG.

**Minor features**
- [x] **Trailer quality → Player section, always visible (user, 2026-10-15).** Done. It limits every YouTube trailer, also the movie/series Trailer buttons that need no Seerr, but the row sat in Integrations behind "Seerr connected". Config field unchanged.
- [x] **Settings restructure (user, 2026-10-15):** sections by task — Playback, Subtitles & language, Display, Audio, Interface, Profiles, Integrations, Key Bindings, Advanced — and a "Profile" tag on per-profile rows. Done.
- [x] **Copy settings from another profile (user, 2026-10-15).** Done. Settings → Profiles → "Copy settings from…": lists every other profile saved on this device; pick one → confirm → the current profile gets its **settings only**: subtitles (on/off, languages, type, size, position, colour, background, ASS styling), audio language, Now Playing auto-open, the five skip modes + seconds, card size, trailer quality. Never: sign-in (server, user id, token, remember login), identity (name, avatar, PIN, auto-lock, Bonfire links), Integrations (Seerr connection/on-off), remembered browsing (library sorts, Discover filters, request choices). No PIN: it only reads preferences from this device's config. Applied at once, saved, toast.
- [x] **UI review (user, 2026-10-15)** — chosen from a code review of every visible text, colour and font size:
  - [x] **Readable setting descriptions:** done — 30 of 82 were cut off ("…", one line); the focused row shows its whole description, the longest ones get shorter.
  - [x] **Consistent wording:** done — setting names in sentence case, buttons/dialogs/rows in Title Case ("Key bindings" in the sidebar, "Copy Settings from…?"), American spelling (Favorite, color), "Couldn't …" errors without raw technical text (that goes to the log).
  - [x] **Theme colours + press flash:** done (theme tokens by role, only the two odd shadows changed; press flash added where it was missing) — 2–3 overlay/scrim tokens instead of 8 hand-picked darkness values (+ a token for white text on images); press flash on the clickables that lack it (Discover filter pills/popup options, calendar days/popup entries, Now Playing queue rows).
  - [x] **Text size setting:** done — Settings → Interface → "Text size" Normal / Large (~+15%) / Extra large (~+30%), per profile (Profile tag, copied by "Copy settings from…"); every font size scales, small pills/badges grow with it.
- [x] **Dashboards: hero band + customizable rows (user, 2026-10-16)** — done — on all five dashboards (Home, TV Shows, Movies, Collections, Music):
  - [x] **1. Rows from an order list** (no visible change): each dashboard draws its rows from `row-order`; keyboard (`find-*-section`) and scroll (`section-y`) follow it.
  - [x] **2. Hide / reorder rows**, per profile: done — Settings → Interface → "Dashboard rows" → Edit… — tabs per dashboard, each row with ✓ (show/hide), ▲, ▼ buttons; copied by "Copy settings from…".
  - [x] **3. Hero band** at the top: the focused card's backdrop, title, year/rating/runtime and 2–3 lines of overview (fades as focus moves, short delay so fast scrolling doesn't fetch every card); Discover cards (Watchlist/Coming Up) via Seerr details; Music uses the album cover. Settings → Interface on/off (default on, per profile).
- [x] **Release helper:** `scripts/bump-packages.sh` (done 2026-10-15) — a script for CLAUDE.md → Releasing step 3 — sets `pkgver` in `PKGBUILD` and `fjord-bin/PKGBUILD` and fjord-bin's `sha256sums` from the published tarball.

**Maintainability (refactors, no visible change)**
- [x] **Done 2026-10-16/17, live-tested 2026-10-22:** detail page scrolls to the real row; one dashboard component + card-rows list (Discover's landing rows too); Settings dropdowns from one table (`CHOICES`); Settings row list as a table + guard test; Settings ↔ config mirroring from one list per side + round-trip test; context menu built from one Rust list; playback tick split into steps; key router as tables (`RAW_TIERS`, `GLOBAL_SHORTCUTS`); `wire_discover` / `wire_controls` split by area; the stacked pages as one `StackScreen` table + a `StackPage` base (main.slint's 11 page blocks: gate + `z` + `fade`); the album and artist pages' header keys as one `keys::header_keys` (2026-10-17).
- [x] **Live test round** — done 2026-10-22: the user went through the Live-test checklist; the HTPC logs confirmed playback (HDR/display sync, the slow-first-open fix, the log folder). What's still open is in the checklist.
- [ ] **Keyboard zones from one list** (after the test round): the detail-style pages step through button/zone lists that Slint repeats as `if` conditions — `detail::existing_detail_buttons`, `series::existing_series_buttons`, `profile_edit::existing_profile_edit_zones`, `discover::keys::existing_zones` (+ the Bonfire group / Connect Seerr zone lists). Build them in Rust and draw from them in Slint, like the context menu — removes CLAUDE.md's last "change both sides together" rule. `series::handle_key` (549 lines) and `detail::handle_key` (368) shrink with it. The biggest of these four.
- [ ] **`playback::start_playback` split by step** (569 lines: setup, mpv config, video surface, the display-sync pre-decode wait), like the tick — pure move; needs a playback test (dev + HTPC).
- [ ] **`widgets.slint` split into files** (2,738 lines, ~30 components): cards, settings widgets, player bars, keyboards… — pure move + imports, low risk.
- [ ] **`discover::open_discover_item_ex` in phases** (515 lines): fetch, build, apply — the request page opener.
- Not planned (low gain or high churn): splitting `AppState` into several globals (every Rust access would change), `reset_session_state` (a list by nature), `main()` (setup + `wire_*` calls).

**Before tagging**
- [ ] **Settings crash watch (Issues):** if it hasn't come back, remove the temporary `--cfg slint_debug_property` (Pending) and close the issue; if it has, fix it first.
- [x] The first nightly from `release-0.5.1` built and installed (2026-10-15; every push since builds one).

---

## Live-test checklist

**Check an item off only once it has been clicked through on real hardware** — a clean build/clippy/test says nothing about whether it works. Add a line here the moment something ships "not live-tested". Ticked items are removed at the next cleanup; their story lives in `DEVLOG.md`.

### 0.5.1
- [x] **Up Next glow all round (2026-10-22).** Music playing, Now Playing, Focus ring style with a glow: Down into Up Next → the focused cover's glow shows above it and, on the first card, on its left (it was cut off there); Right to the last card → its glow shows on the right. Cards and the "Up Next" label stay where they were at Soft; at Strong/Very strong the strip is a little taller and the glow still isn't cut. The focused card's glow lies over its neighbours. Mouse: click and wheel on the strip work as before.
- [x] **Keys on the account screen at startup (2026-10-22).** Start Fjord with two or more accounts saved (it opens on the account screen): without touching the mouse, Left/Right move between the accounts and "+ Add Account", Down reaches Quit, Enter on an account opens its profiles. Same for a single account with several profiles (starts on the profile screen). Signed out (starts on Login): the Server or Password field still has the cursor and typing works.
- [x] **Jellyfin's current addresses (2026-10-22).** Lists, item pages, watched and favorites moved from `/Users/{id}/…` to the 10.9+ routes (a read-only check on 12.2 gave the same counts for movies, series and libraries). On 12.2: Home/TV/Movies/Music rows fill as before (Continue Watching, Latest, Latest music, Favorites), a movie's and an episode's page open, the library grids show every title; mark an episode watched and unwatched (context menu) and favorite/unfavorite a movie → the rows follow; no `HTTP 404` in the log. **Confirmed 2026-10-22** (dev r1265): rows, the library grids (617 movies, 47 artists, 122 albums, 3 playlists), an artist page and Mark as Played (the server echoed played=true) worked; no warning or error in the whole log. Mark as Unplayed and favorite/unfavorite weren't tried.
- [x] **Live updates on Jellyfin 12.2 (2026-10-22).** Since the server update the log showed `ws: connect error: HTTP error: 403 Forbidden` and no live updates. Fixed: start Fjord → the log shows `ws: connected` (no 403). Then mark an episode watched (or favorite a movie) in the Jellyfin web client → within a few seconds Fjord's rows follow (Continue Watching / Favorites) without a refresh. Playback links now carry the token as `ApiKey=`: a movie and a song still play, and the log shows `ApiKey=REDACTED`, never the token. **Confirmed 2026-10-22** (dev r1265): `ws: connected` with no 403, UserDataChanged arrived (playback progress, Mark as Played); a 4K movie and a FLAC song played with `ApiKey=REDACTED`, no token in the log. A change made in the web client wasn't tried.
- [x] **Copy settings from another profile (2026-10-15).** Settings → Profiles: "Copy settings from…" (under Manage Profiles) only when another profile is saved on this device; the list shows every other profile, a name shared by two households with the account in brackets. Pick one (keyboard: Enter opens the list, Enter picks; Right also opens it; mouse) → "Copy settings from X?" (Left/Right/Enter/Back; mouse) → Copy: every row tagged "Profile" now shows X's values at once (e.g. subtitle language, a skip mode, card size changes the cards right away), a toast "Settings copied from X"; sign-in, Seerr, launch/remember-login and the device rows (Display, Audio output, Advanced…) are unchanged; restart keeps them. Cancel/Back changes nothing. No PIN asked, also for a PIN-protected source. Log: `copy settings: source picked — …`, `copy settings: copied the profile settings of <id> …`, then `settings changed: …` naming the copied fields. **Confirmed 2026-10-22** (dev r1252, the user's copy + log + config): picked from the list, confirmed, toast "Settings copied from …", all 45 copyable fields equal the source's, sign-in/identity/Bonfire/Seerr untouched. Cancel/Back and a PIN-protected source weren't exercised — accepted by the user.
- [x] **Screensaver: the pointer hidden (2026-10-22).** The rest of the screensaver was checked 2026-10-22; on that build the mouse pointer stayed visible over the black screen. Fixed since: when the screen goes black, the pointer disappears at once (without touching the mouse); moving the mouse brings back the picture and the pointer. Waking it with a key: picture back, the pointer shows again once the mouse moves.
- [x] **Music bar corner + Up Next titles (2026-10-22).** Music playing, Focus ring style with a glow: Down into the bar, Left to the art + title zone → it slides up and right out of the screen's corner (by the glow's reach: 17px at Soft, more at Strong/Very strong) with the glow visible on all four sides, over the page edge and the elapsed time; Right → it slides back. Mouse hover: no move. Line style: no move. The rightmost button (☾ or ⏹) has a gap to the screen edge with its glow showing on the right. Now Playing → Up Next: a long title wraps to a second line, then "…"; a song with an artist shows it under the title, one without has no empty line.

---

## Pending

- [ ] **Remove the temporary `--cfg slint_debug_property` from `PKGBUILD`'s `build()` and from the nightly build in `build-release.yml`** once the Settings crash (Issues) is understood or hasn't come back for a while — it's there so a repeat names the property in the `PANIC` line.

## Issues

- [ ] **Settings crash — watching (2026-10-08, r1064).** Two Slint "Recursion detected" panics on the HTPC after toggling Settings → Video (since 0.5.1: Display) → "Separate video surface" with the mouse and clicking Home. Not reproduced since (dev machine, debug build, several tries). If Fjord closes, send the log: the `PANIC` line has a timestamp and (HTPC build) the property's name; `settings changed: …` / `nav-selected(…)` show the steps before it. No PANIC / "Recursion detected" in any log still kept (dev and HTPC, 2026-10-17 → 2026-10-22).

---

## Deferred / future

- **Pull-request checks, once outside contributors send PRs.** A workflow on `pull_request` running `cargo fmt --check`, `slint-lsp format` (diff must be empty), clippy and tests — on GitHub-hosted runners (needs libmpv from apt), **never** on the self-hosted runner, which would run strangers' code on the dev machine.
- **New HTPC GPU (user plans an AMD card, 2026-10-08).** Afterwards: Settings → Advanced → Hardware decode `nvdec` → `auto`/`vaapi`, drop the NVDEC video-filter workaround, re-check HDR and 10-bit (KWin controls the bit depth on amdgpu). Optional: pick these defaults from the detected GPU vendor.
- **Own 10-bit buffers** (`dmabuf_plane.rs`, opt-in) only help where the driver can send 10-bit but EGL windows are 8-bit — e.g. NVIDIA Ampere+ over HDMI, or Pascal over DisplayPort (see DEVLOG, 2026-10-08). If they're ever used for real: explicit sync (`wp_linux_drm_syncobj_v1`) instead of `glFinish`.
- **Theming / layout customisation** — accent colours, dashboard row visibility, drag-to-reorder rows. Needs the full layout system first.
- **Gamepad / remote** — the D-pad maps to arrow keys today; formal evdev/udev support deferred.
- **Vulkan rendering path — blocked upstream (checked 2026-10-09).** Idea: Slint's UI and mpv's video on Vulkan instead of OpenGL ES. mpv's libmpv render API (what Fjord embeds mpv through) only offers `opengl` and `sw` — in 0.41 and in current master; an RFC adding a Vulkan render API (mpv PR #18258, July 2026) was closed unmerged. Only possible with a patched mpv until that lands. Also less gain than this entry used to claim: with an AMD card, `hwdec=vaapi` into OpenGL is already zero-copy (dmabuf import), and the GTX 1050 Ti's driver supports Vulkan, so nothing forces OpenGL either way.
- **Long term: a general media client** — e.g. Spotify via librespot.
