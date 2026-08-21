# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

---

## Completed

Full curated version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Full implementation detail per feature: `CLAUDE.md`.

---

## Pending

Everything below is already implemented, and passes `cargo build`/`clippy --workspace --all-targets`/`test --workspace` — this is purely the "confirm it actually works on real hardware" list. Nothing here is unbuilt work — see **Deferred / future** below for that. Full design/investigation writeups live in `CLAUDE.md` (dated sections); `CHANGELOG.md` has the curated commit history. Anything already live-confirmed has been dropped from this list rather than kept as a completed checkbox.

Live-testing pass 2026-08-17 found real bugs in most of the items below (marked **FIXED 2026-08-17**, code changed but not yet re-tested on real hardware — leave unchecked until confirmed). A few items were genuine open design questions, resolved via `AskUserQuestion` and now implemented; a couple of confusing/untestable entries were reworded or dropped.

### Account & Profile picker
- [ ] PIN-entry modal's Cancel button is D-pad/mouse-reachable and shows a clean single highlight, no double-highlight with the grid behind it
  **FIXED 2026-08-17/18**: real "← Cancel" button added; **FIXED 2026-08-19** (2nd sighting of the same bug class): the grid's own cursor stayed visible at the same time as Cancel's ring — now clears when focus moves to Cancel
- [x] Switching profiles while "Remember this login" is off, within an account you're already using, doesn't force a re-login
  **FIXED 2026-08-17/18**: toggle + the same-account skip — see CLAUDE.md for the full history
- [ ] Add Account's Cancel/Back returns to wherever it was opened from, reachable via keyboard, with a label matching what it actually does
  **FIXED 2026-08-19**: the round-2 Escape fix was dead code in practice — `LoginScreen` always grabs native `LineEdit` focus on open, and the global key-dispatch tier never sees a key while that's true (same class of gap this app has hit before). Fixed with a per-field `key-pressed` hook, the same pattern `ProfileEditScreen`'s fields already use. Label simplified to a plain "← Back" (was "← Back to Profiles," which didn't match where it actually goes).
  **FIXED 2026-08-19** (direct pushback — "why?" — on the "accepted scope boundary" note above): full D-pad reaches every zone now, same pattern `ProfileEditScreen` already uses — Up/Down move through server/username/password (native field focus, unchanged) then Remember toggle then Connect button and back; Enter on Connect submits via a Slint-side pulse tracker (Rust can't read live field text); mouse clicks on Remember/Connect sync keyboard state too. See CLAUDE.md for the full design.
  **FIXED 2026-08-21** (real regression, first live test): "it only works after you pressd tab or a text box with the mouse" — a race at cold start: `main.rs`'s own startup code grabs the global keyboard-dispatch focus as the very last thing before the event loop starts, unconditionally overriding this screen's own field-focus grab (which ran earlier, during window construction). Fixed with a deferred one-shot timer, the same mechanism already used once before for the identical class of "must run after the event loop starts" problem on this same screen.
- [ ] "Remember this login" toggle isn't cramped against its own border
  **FIXED 2026-08-21**: same shape as the ProfileEditScreen fix below, just never applied here — 4px padding, bumped to 12px.
- [ ] Profile picker's "Back" button does what "back" should mean: return to wherever you actually came from, not always "Accounts"
  **FIXED 2026-08-19**: opening the picker via the sidebar's "Switch Profile" (live session, never touched the account tier) showed "← Back to Accounts" and went there anyway — now shows a plain "← Back" that just closes the picker and keeps your current profile; genuinely arriving via the account tier still correctly shows "← Back to Accounts"
- [ ] A sub-profile deleted on the Bonfire server itself eventually disappears from Fjord's own picker too, instead of staying forever as an unreachable, unmanageable ghost tile
  **FIXED 2026-08-19**: confirmed via a live diagnostic against the real server that this was genuinely happening (a deleted "test 2" sub-profile stayed listed in "Who's watching," permanently, since the add-only sync this app shipped with never pruned anything) — `sync_bonfire_subprofiles` now removes a local entry once the server stops reporting it, scoped tightly to that one household so it can never touch an unrelated account
- [x] Default Account / Default Profile behave sensibly together after a real restart
- [x] After extended real use, `config.json` should never show a self-referencing/cyclic `master_user_id` — covered by a permanent regression test, no action needed unless it recurs
- [ ] `home.json` (Continue Watching/Next Up/Recently Added/Favorites) actually updates on disk after a login or profile switch, not just live on screen
  **FIXED 2026-08-21**: live-reported ("close fjord, still shows the old cache before it reloads") — `finish_session_setup` (the function both a fresh login AND every profile switch go through) saved the fresh series list but never imported or called `save_home_cache` at all. Only a plain cold auto-login (no switch involved) ever wrote `home.json`. Every switch/login showed fresh data live, then silently never persisted it — so the next launch's warm-start kept reading a stale snapshot from however many sessions ago the last cold auto-login happened to be.
- [ ] The sidebar's own profile tile updates immediately after a switch, and the dashboard lands on Home instead of wherever the sidebar cursor happened to be
  **FIXED 2026-08-21**: real screen recording showed the sidebar avatar/name staying blank for the whole ~2s+ fetch window after a switch, even though everything it needs is local, no-network-needed data — pushed early now instead of waiting for the full fetch to finish. Separately (confirmed real but explicitly not the thing being reported): `active-nav` was never reset on a switch at all, so a switch triggered from the sidebar's own Profile row (nav 7, which has no dashboard content of its own) left the content area with nothing to render regardless of how fast the data arrived — now resets to Home.
### ProfileEditScreen — full D-pad keyboard navigation
- [x] Both dropdown popups (Max parental rating / Auto-lock) show a visible keyboard highlight
  **FIXED 2026-08-17**: inside the popup, once opened — the cursor row's background tint was too subtle, added a focus-border ring
  **FIXED 2026-08-19**: this was still missing on the *row itself*, before ever opening the popup — every other zone in this screen has its own bordered wrapper reacting to keyboard focus; the two dropdown rows never got one. Both now do (confirmed needed for both Max Parental Rating and Auto-lock).
- [ ] "Skip PIN on this network" row's toggle isn't cramped against its own border
  **FIXED 2026-08-19**: padding was 4px each side (too tight once the 2px focus ring is added on top); bumped to 12px, matching the equivalent row shape elsewhere in the app
  it is still missalinge with the toggle
- [x] Tab wraps from the last text field back to the first
- [x] Every zone reachable via Up/Down in order, including both conditional checklists
- [x] Both PIN pads' 12-key grid nav + Enter-confirm work from a real keyboard (digits + Backspace)
- [x] Whole-screen auto-scroll animates smoothly
### Native profile management (Manage Profiles / Edit Profile)
- [x] Create a new sub-profile end to end; "+ Add Profile" disappears at the real per-master cap; the dialog box resizes to fit its content
  **FIXED 2026-08-17**: cap now reads the real server value, not a hardcoded 5; box width now sized from real content
- [x] Edit an existing sub-profile — parental rating shows an honest "Unknown" rather than a misleading "Any" (Bonfire itself never reports the current value — confirmed upstream gap, not fixable client-side)
### Discover / Seerr
- [] Person detail from a Discover cast member actually opens and stays open
  **FIXED 2026-08-17**: the data-fetch/z-order issue (screen was correctly populated but painted-over by another overlay)
  **FIXED 2026-08-19**: still broken after the above — log showed 6 identical fetches for the same cast member in ~1 second, with no re-entrancy guard anywhere in the flow. Rapid repeat presses could leave the loading overlay permanently stuck covering an already-correctly-loaded screen — indistinguishable from "nothing happens," which is exactly why repeat presses kept happening. A repeat press for the same target while its fetch is already in flight is now a no-op.
  **FIXED 2026-08-19 (2nd sighting, real root cause)**: user reported having the clicked person locally, yet nothing opened — the log showed the local-library search finding 0 candidates for a person confirmed to genuinely exist. Live-diagnosed against the real server: the local-person search used the wrong Jellyfin endpoint entirely (`/Items?IncludeItemTypes=Person&Recursive=true`, which structurally can never match — Person entries aren't part of the recursive folder tree that endpoint walks) instead of the real, dedicated `GET /Persons?SearchTerm=...` endpoint. This meant a Discover cast member's local match NEVER resolved correctly, for anyone, the whole time this feature has existed — every click silently fell to the TMDB-only fallback screen instead of the real native one. Also closed the redundant-refetch gap the fix above didn't fully cover (a fast repeat press could still slip past that guard via a cache-hit race) with a single choke-point guard at the actual entry function.
  **STILL OPEN, 2026-08-21**: "check latest log the issue is still there." Confirmed from a fresh log that the endpoint fix genuinely works now (Tom Holland resolves to his real local match, `open_person_screen` is invoked, not the TMDB fallback) — but the actual commit closure that sets `show-person=true` had zero logging of its own, so there was no way to confirm from the log whether it ran to completion or silently bailed on a guard. Added logging to every exit point of that closure. Re-checked `main.slint`'s z-order for a regression (the original 2026-08-17 bug) — still correct. Genuinely unresolved pending one more test with this logging in place.
- [ ] Discover search grid doesn't flash or re-fade already-loaded posters while typing/paging
  **FIXED 2026-08-17/18**: poster carry-forward + true in-place model updates — re-confirmed 2026-08-19 that both mechanisms are genuinely still in place as shipped (checked the actual current code directly, not just the earlier commit message)
  **Still reported flashing 2026-08-19, not yet independently reproduced or fixed** — need more specific detail next time this comes up: does the *whole grid* re-fade, do individual *cards* visibly jump, or do posters just *pop in one at a time* as they finish downloading (which, for a genuinely first-ever query with nothing to carry forward from, is expected/correct, not a bug)? A screenshot or slow-motion description of the exact moment would help pin down whether there's a real third mechanism still to find.
  **FIXED 2026-08-21**: real screen recording this time, watched frame by frame — the actual bug was the very first search from the landing rows going straight to a blank "No results for X" screen for the whole ~300ms debounce window, before any search had even run. `discover-searching` was only set true *after* the debounce sleep, not before it — so the empty-state text's own (already-correct) `!discover-searching` guard was fed a stale `false` for the entire gap. Every later transition in the same recording (typing further, pagination, poster load-in) was already correct.
- [x] Toggling Watchlist (star icon / context menu) shows a confirmation toast
  Genuinely unconfirmed either way — no new evidence this session

### Misc UI
- [x] Missing Seasons row shows "Upcoming · date" instead of "0 episodes" for an unaired season

---
## Issues
(none open)


## future additons

(none open)


## Architecture notes

### mpv render API

mpv uses `vo=libmpv`. Two FBOs alternate each frame:

```
BeforeRendering:
  mpv_render_context_render(fbos[back])
  expose textures[back] as BorrowedOpenGLTexture → Slint draws it
  back = 1 - back

AfterRendering:
  if did_render: mpv_render_context_report_swap()   ← vsync feedback
```

`MpvRenderCtx` must be dropped before `Player`. Enforced in `VideoState` and the rendering teardown path.

### Disk cache

```
~/.cache/fjord/home.json         home row data    always refresh in background
~/.cache/fjord/movies.json       full movie list  refresh once per session on grid open
~/.cache/fjord/series.json       full series list refresh once per session on grid open
~/.cache/fjord/posters/<id>      poster bytes     permanent (never expire)
~/.cache/fjord/backdrops/<id>    backdrop bytes   permanent (never expire)
```

Warm start: all caches loaded synchronously before `window.run()` — window opens fully populated on the first frame.

### Poster loading pipeline

```
Tokio worker:
  fetch bytes (disk cache or HTTP, 8-connection semaphore)
  decode JPEG → SharedPixelBuffer<Rgba8Pixel>   ← Send
invoke_from_event_loop:
  Image::from_rgba8(buffer)                     ← !Send, must be on UI thread
  push HomeItem with poster into VecModel
```

### Thread model

```
main thread       Slint event loop + GL rendering notifier
tokio runtime     API calls, poster fetch/decode, home data refresh
16 ms timer       mpv event poll, position update, skip-segment (Intro/Recap/Preview/Commercial), credits auto-advance check, controls idle, progress report
```

---

## Deferred / future

- **Theming / layout customisation**: accent colour palette, dashboard row visibility toggles, row reordering — needs the full layout system in place first before it makes sense to build.
- **Vulkan rendering path** — second render backend alongside the current OpenGL path. Requires: Slint WGPU backend, `MpvRenderCtx` initialized with `MPV_RENDER_API_TYPE_VULKAN`, Vulkan FBO management replacing the current `gl::*` code. Enables true zero-copy decode on AMD (`hwdec=vulkan`, no CPU roundtrip). Selection would persist in Config as `gpu_renderer: "opengl" | "vulkan"`, taking effect on next restart. **Correction, 2026-08-17, live-questioned ("is this a fact?")**: this entry's original "Legacy NVIDIA hardware needs OpenGL" line (written 2026-06-24, the very first commit that added this item) was never actually verified — checked now and it's likely wrong, not right: the proprietary NVIDIA driver has shipped full Vulkan support since Vulkan's 2016 launch, Pascal (this project's own target GPU, GTX 1050 Ti) was a day-one Vulkan-generation architecture, and a live report confirms Vulkan 1.3.275 working on this exact card under driver 570.133.07 — well within the same 580.xx branch Arch already ships for it (see [phoronix.com](https://www.phoronix.com/review/nvidia-gtx-1050), [forums.developer.nvidia.com](https://forums.developer.nvidia.com/t/vulkan-support-on-the-gtx-1050-max-q/70986)). None of Fjord's own extensively-documented legacy-NVIDIA bugs (stride corruption, VO-init race, HDR shader-compile crash) are Vulkan-specific either — all are OpenGL/EGL-Wayland-path issues. So there's no known hardware/driver wall forcing OpenGL on this hardware; a toggle would still be worth having (this project's OpenGL/EGL path has a long track record of NVIDIA-Wayland-specific bugs, so Vulkan could plausibly be MORE reliable here, untested either way), just not because of a compatibility requirement — see the `hdr`-branch memory / CLAUDE.md's HDR section for why HDR work itself doesn't need or benefit from this migration regardless.
- Gamepad / remote control — d-pad maps to arrow keys; formal evdev/udev support deferred
- **Dashboard row reorder** — drag-to-reorder; part of the future theming/layout customisation update
- **Trickplay** — seek bar scrub thumbnail popup. Requires: fetch Jellyfin trickplay manifest (`GET /Videos/{id}/Trickplay/{width}/tiles`), parse tile sheet dimensions (tile size, columns, rows, interval), cache tile images per video, render a thumbnail above the seek bar while scrubbing (position computed from `seek-hover-pos`). Deferred because it's a separate subsystem from chapter nav and the API surface needs more investigation.
- ~~**Multi-account / multi-server support**~~ — done. Fully subsumed by the Bonfire/JellyProfiles `Config` restructuring (`DeviceConfig`/`Vec<ProfileSettings>`) and `ProfilePickerScreen`; each profile carries its own `server_url`, so this is genuine multi-server too, not just multi-account on one server. See CLAUDE.md's Bonfire section.
- **Display mode auto-sync (resolution/refresh-rate/HDR to match video content), KDE-only for now** — live-reported 2026-08-14, a real HTPC hitch traced to the user's own separate `media_display_sync` script (polls Jellyfin's API independently, hit a 5s timeout, briefly reverted the display mode then switched back ~10s later — a real, visible HDMI renegotiation, but not a Fjord bug; Fjord's own stream never noticed). Discussed as a genuine future candidate: building this into Fjord directly would be strictly better than the external-poller approach — Fjord already has the real container framerate/resolution/HDR metadata synchronously from mpv the instant a file loads (no separate network poll to time out), and a mode switch could ride the existing skip-fade-to-black mechanism instead of happening as a raw visible renegotiation mid-frame. The real cost is that it's Wayland-compositor-specific, not a Jellyfin/mpv API surface — the user's HTPC (and only currently-testable machine) runs KDE Plasma, so a first cut would go through KDE's own KWin/kscreen D-Bus interface and be KDE-exclusive until/unless a second compositor is in scope to test against; no wlr-output-management (Sway/Hyprland-style) support without a way to verify it. Not scoped further yet — worth a proper `/plan` pass (mirroring how Watch Trailer/Bonfire were planned) once actually picked up, and reading the user's own `media_display_sync` script first (repo visibility TBD) to see exactly which mechanism it already leans on.
  - **Sharper motivating hypothesis surfaced 2026-08-15, during the HDR passthrough investigation above**: the script changes resolution/Hz *after* the video has already started playing — a live connector mode-set happening concurrently with (or right after) mpv's own render-context setup and `target-colorspace-hint` negotiation is a plausible, sensible interference mechanism (a DRM mode-set is a heavyweight, connector-level operation; racing it against mpv's own HDR-hint negotiation could easily reset or drop whatever was being negotiated) — genuinely the same *shape* of race this project already found and fixed once for a completely different reason (the VO-init-vs-render-context-creation race, see "NVIDIA HTPC: video-only-audio black screen" above). User's own planned test, not yet run: same file/script, once with resolution/Hz already correctly set *before* Fjord even starts (no mid-playback mode-set at all) vs. the current after-the-fact behavior — if the pre-set case works and the mid-playback one doesn't, that's a clean, direct confirmation. User's own words: "if the script interfere then i will push up the integration into fjord" — i.e., a confirmed race here is the trigger condition for actually picking this item up, not a hypothetical someday. If/when that happens, the fix this points toward specifically is doing the mode-set *before* starting mpv's own playback pipeline for that item (matching the already-established "resolve first, don't create the race in the first place" discipline this project keeps landing on elsewhere), not just retrying/reordering after the fact.
- **Poster/card scaling setting** — from Phase 117: with titles/episode names now always shown in full (no truncation), a very long title on the smallest card breakpoint (115px wide) shrinks that card's poster noticeably to make room. User's proposed fix: a settings toggle to use larger posters in the library/dashboard (more text budget per card before this becomes visible) rather than reintroducing a truncation cap. Would likely hang off the existing `dash-card-w`/`dash-card-h` breakpoint functions in `main.slint`, e.g. as a user-chosen size multiplier or an extra breakpoint tier. Alternative approach floated in the same conversation, not mutually exclusive: render the poster image full-bleed behind the title/subtitle text with a dark semi-transparent scrim, instead of a separate text block below the poster — sidesteps the whole shrink-to-fit problem structurally rather than giving the text more room. Neither implemented yet; revisit when picked up.
- **Real HDR passthrough — deferred, dedicated-branch work, not yet scoped in detail.** `Config.device.target_colorspace_hint` ("HDR passthrough" in Settings) very likely does nothing at all under Fjord's actual render path — mpv's own manual states `--target-colorspace-hint` "Requires a supporting driver and `--vo=gpu-next`", but Fjord uses `vo=libmpv` (the render API), which that option's sub-flags explicitly exclude. This plausibly explains the entire earlier "TV never enters HDR mode" investigation (see CLAUDE.md's HDR tone-mapping section and its NVIDIA-driver-support research pass) more directly than either the driver-support or KWin-bug theories chased first — see CLAUDE.md's "The likely real explanation for the whole HDR passthrough mystery" section for the full trace (mpv manual read directly, corroborated by a real KWin HDR developer's own blog post on Wayland color-management negotiation).
  - Real HDR passthrough would mean Fjord doing its own Wayland `color-management-v1` negotiation directly against its own Slint-owned surface, independent of mpv's `gpu-next`-only mechanism — a genuine rendering-layer redesign (touches how Fjord's window/surface is created and presented to the compositor), not a settings fix.
  - **User's explicit call, 2026-08-17**: build this, but after the Bonfire/profile work (Phases 3-6 of that roadmap are still genuinely unstarted, not just "in progress"), and on a dedicated `hdr` git branch rather than `main`, since this could leave the app broken for an extended stretch in a way almost nothing else in this codebase's incremental history has — see the `feedback_branch_after_release` memory. Also explicitly flagged: this may be capped by the user's own aging Pascal-era GPU (GTX 1050 Ti, frozen on the `580.xx`/`581.xx` NVIDIA driver branch) regardless of how correctly the Fjord-side work is done.
  - Not scoped in detail yet — the real first step when picked up is a proper `/plan` pass (matching how Watch Trailer/Bonfire were planned), not jumping straight to code. A cheap diagnostic worth doing first, on either branch: check `fjord.log` for an mpv-emitted warning about `target-colorspace-hint` being ignored for the active VO, to directly confirm the no-op theory before designing around it.
