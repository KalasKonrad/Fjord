# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

---

## Completed

Full curated version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Full implementation detail per feature: `CLAUDE.md`.

---

## Pending

Full on-screen keyboard rollout beyond Login (user request, 2026-08-23) — every text-entry surface live-confirmed working end to end for its OWN direct interaction (typing/backspace/Done), including the ProfileEditScreen focus-race fix (confirmed both by direct user testing and independently via the dev-machine log's own debug traces). Full technical detail in CLAUDE.md's dated Tier 2/Tier 3 + 2026-08-25/26 sections.

- [x] Discover search — live-confirmed.
- [x] Browse search — live-confirmed.
- [x] ConnectSeerr — live-confirmed for its own direct interaction; see the code-review entry below for real, separately-found gaps in this screen specifically.
- [x] Library search — live-confirmed.
- [x] PlaylistPicker naming — live-confirmed.
- [x] Shared background/positioning/Done-cursor-default fixes — live-confirmed.
- [x] ProfileEditScreen (Name / Blocked tags / Allowed tags) — live-confirmed, on the second fix attempt (the first, grab-then-release native focus, was retested and found still broken; the working fix never touches native focus at all — see CLAUDE.md's 2026-08-25 "take 2" entry).

**Code review of the full rollout, 2026-08-26 (`git diff b3785cb..HEAD`) — 12 real findings, all fixed the same session.** None of these were caught by the per-surface live tests above, since none of them specifically tried "open the keyboard, then switch sidebar tabs" or "open ConnectSeerr, click a different method tab mid-typing" — the exact cross-screen/cross-state interactions this pass targeted. **Not yet re-live-tested** — see CLAUDE.md's own dated section for the full finding list and fix detail.

- [x] Most severe: on-screen keyboard never closed on ANY sidebar nav switch (Browse/Discover/Library search are permanently-mounted siblings, so their own screen-hide never cleared it) — full app-wide input lockout, fixed in `discover.rs::on_nav_selected`.
- [x] ConnectSeerr: D-pad zone stranded once Quick Connect starts polling, or after any screen close/reopen at a non-zero zone — self-heals now in `keys.rs`, and `on_open_connect_seerr` resets the zone on every open.
- [x] ConnectSeerr: switching a method tab via mouse while the on-screen keyboard targeted the OLD tab's own field orphaned its only keystroke listener — fixed via `close-keyboard-if-orphaned()`.
- [x] ConnectSeerr: `MethodTab` had no persistent keyboard-focus ring, only a transient press flash.
- [x] ConnectSeerr: on-screen keyboard position had no bottom clamp (could render its own Done key off-screen on a short window) — now matches PlaylistPicker's own clamp.
- [x] ConnectSeerr: 6 retarget-while-open handlers defaulted the cursor to a letter key instead of Done.
- [x] ConnectSeerr: 3 text-field submit trackers (+ Quick Connect's own) had no busy-guard, unlike their mouse buttons — rapid double-Enter could fire two concurrent auth attempts.
- [x] `resolve_seerr_url` / `authenticate_with_fallback` misclassified a JSON-decode failure on a reachable HTTPS server as a connectivity failure, silently downgrading to plaintext HTTP — tightened to a real `is_connect()`/`is_timeout()` check, shared via `auth::is_connectivity_failure`.
- [x] ConnectSeerr's Quick Connect poll had no in-flight guard and swallowed a mid-poll resolve failure forever with no error and qc-polling never reset — added an `AtomicBool` in-flight guard + a bounded consecutive-failure counter that now surfaces an error and stops.
- [x] Library grid search backspace was the one search field never migrated to the grapheme-cluster-aware trim (2-presses-per-emoji bug).
- [x] 3 stale/missing TOC header entries (main.rs, discover.rs, connect_seerr.slint).

**Needs live testing on real hardware** (in priority order — none of this can be verified in the sandboxed dev environment):

- [x] Open the on-screen keyboard on Browse/LibraryGrid/Discover's own search field, then switch sidebar tabs — the app must stay fully keyboard/D-pad-responsive afterward, not go input-dead (the most severe fix in this pass).
- [x] ConnectSeerr → Quick Connect: start it, confirm zone 0 (tab row) is reachable again once polling begins and the code is approved; confirm `MethodTab`'s new focus ring is visible and visually distinct from its accent-filled "active" state.

- [x] ConnectSeerr: open the keyboard on one tab's field (e.g. API key), click a *different* method tab with the mouse mid-typing — the keyboard should close automatically rather than staying open and inert.
- [x] ConnectSeerr: close the screen while a text field is focused (zone 2+), reopen it — the D-pad should land back on the tab row (zone 0), not a stale dead zone.
- [x] ConnectSeerr on a real short/HTPC-resolution window: open the keyboard on the Jellyfin or Local tab (the two taller ones) and confirm Done is still fully on-screen.
- [x] ConnectSeerr: click between two different fields with the mouse while the keyboard is already open — Enter should still mean "close the keyboard" (cursor defaults to Done), not type a letter into the newly-focused field.
- [x] ConnectSeerr: a rapid double-Enter on a Save/Sign-In/Get-Code zone should never visibly double-submit (hard to fully confirm from the UI alone, but the busy state — "Connecting…"/"Signing in…" — should hold through a double-press without erroring or flickering).
- [x] A schemeless Seerr URL against a server whose HTTPS port answers with a non-JSON 200 (if such a setup is reachable to test) should surface a real error, not silently fall back to plaintext HTTP.
- [x] Simulate a network outage mid-Quick-Connect (e.g. disconnect Wi-Fi/unplug the Seerr server) — polling should surface an error and stop within roughly 20 seconds instead of spinning on "waiting for approval" forever.
- [x] Library grid search: paste or type a flag emoji or accented name, press Backspace once — the whole character should disappear in one press, matching Discover/Browse/PlaylistPicker's existing behavior.

**Live-test finding, same pass: "the keybord nav on seerr connect seams off it do not go where you are expekting" — real, confirmed, fixed 2026-08-26.** Zones 0 (tab row) and 1 (url-input) were numbered backwards relative to the screen's actual visual layout (url-field-wrap renders ABOVE the tab row) — since `next_zone`/`prev_zone` walk the zone list purely by position, Down from the tab row visually jumped UP the screen to the URL field, and Down from the URL field skipped the tab row entirely on the way back down. Renumbered so 0 = url-input (topmost, right below Close) and 1 = the tab row, matching true visual order; url-input also gained a root-level `init => { url-input.focus(); }` grab so the screen now opens with real focus already on it, mirroring Login's own precedent. Full trace in CLAUDE.md's dated section.

- [x] Re-verify ConnectSeerr's keyboard nav specifically: opening the screen should land directly on the URL field (not the tab row); Down from the URL field should reach the tab row; Down from the tab row should reach the fields below; Up should retrace the same path in reverse (tab row → URL field → Close) with nothing skipped or reversed.

**Two more live-reported bugs, same screenshots, found and fixed immediately: "why dose every button get a hilhight men you move down to it? and why is the text black? its not black on any other blue button i the whole program?"** Both real, both in `MethodTab` (`connect_seerr.slint`), neither related to the zone renumbering above. (1) `kbd-focused` is bound identically to `connect-seerr-zone == 1` on all 4 tab instances — since the tab row shares one zone for all 4 (Left/Right switches the active tab directly, no separate per-tab cursor), every tab lit up simultaneously the moment the D-pad reached the row; fixed by requiring `kbd-focused && active` for the ring, matching the press-pulse's own already-correct `active && kbd-focused` gate. (2) The active tab's text used `Theme.bg` (`#0d0d0d`, near-black) instead of white — confirmed against `FjordButton`/`VirtualKeyboardKey`, both of which use plain white text on their own accent-filled state; this was the one outlier, not a deliberate choice. Both fixed.

- [x] Confirm only the currently-active tab shows a focus ring when the D-pad cursor is on the tab row (not all 4), and its text is white, not black.

**On-screen keyboard: Settings → UI toggle, 2026-08-27, user request ("shuld we add a setting if a user dont want the virtual keybord to show up, as this shuld be installable and usable with out a keybord it shuld default to on").** New `Config.device.onscreen_keyboard_enabled` (default `true`) gates the feature entirely, in two places for defense in depth: every one of the 7 `QwertyKeyboard` mount conditions (Login/ProfileEditScreen/Discover/Browse/Library search/PlaylistPicker/ConnectSeerr) so the widget never renders when off, AND `keys.rs`'s own top-level dispatch gate, so a lingering `show-onscreen-keyboard=true` from before the setting was flipped off can never turn into a silent input lockout (that gate runs before every other tier and unconditionally consumes any key while active). No existing "open the keyboard" trigger site needed touching. Full detail in CLAUDE.md's dated section.

- [ ] Settings → UI → "On-screen keyboard" toggle: confirm it persists, and that turning it off makes every affected field's Enter key stop opening the keyboard (with typing/Backspace still working normally via a physical keyboard) while turning it back on immediately restores the keyboard everywhere.

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
