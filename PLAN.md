# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

---

## Completed

Full curated version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Full implementation detail per feature: `CLAUDE.md`.

---

## Live-test checklist

**This is the actionable list — check something off only once it's actually been clicked through on real hardware.** A clean `cargo build`/`clippy`/`test` means the code compiles and its pure-function logic holds; it says nothing about whether it works. Every item here started life as a "not live-tested" caveat buried in a `Pending` paragraph below — this section exists specifically so those caveats stop getting lost in narrative prose the moment the conversation moves on. **Add a line here the instant something ships "not live-tested," don't wait to be asked** — this exact list decayed back into narrative-only entries once already (see `8ea7aab`, "consolidate PLAN.md's Pending section into one live-test checklist"), and it's not allowed to happen a second time.

- [x] KDE double-launch fix (2026-09-04): pin Fjord to the taskbar, launch it, click the taskbar icon again while it's running — should raise the existing window, not spawn a second process (`pgrep -a fjord` should show exactly one process either way).
- [x] Bonfire Admin, Phase 6 (2026-09-04) — needs a real Jellyfin server-admin account on a Bonfire-enabled server:
  - [x] "Bonfire Admin" row shows only for a genuine Jellyfin server admin, never for a plain Bonfire household master (and the reverse: a server admin with no Bonfire household of their own should still see it).
  - [x] Mappings tab shows real masters + sub-profiles, correctly grouped with the right avatar colors/initials and PIN badges.
  - [x] Reset PIN actually clears a PIN server-side (confirm via a real switch attempt no longer demanding one).
  - [x] Profile-limit stepper cycles and applies correctly, including the "Default" (server-default) end of the cycle.
  - [x] Audit Logs shows real entries.
  - [x] Full D-pad nav through both tabs, the row list, and the Reset PIN confirm dialog — real gap found from the first test (2026-09-07): the "← Back" button had no keyboard focus state at all, only Escape/Backspace closed the screen. Fixed to mirror `BlocklistScreen`'s own Back-focus shape exactly; needs re-confirming.
  - [x] Reset PIN only shows on rows that actually have a PIN set (2026-09-07 fix) — previously shown/clickable unconditionally, including on `test`'s own PIN-less master row; harmless server-side (verified against the real controller source) but confusing. Now hidden when there's no PIN, and the D-pad column logic correctly lands on whatever's actually available on the row instead of assuming Reset PIN always exists.
- [x] LAN-bypass PIN staleness fix (2026-09-04): a profile with LAN bypass enabled no longer shows a stale lock icon in the picker while actually on that network.
- [x] Code-review fixes, 2026-09-06 (`4eeea49`):
  - [x] Sign out, then immediately re-log into the same account — no stale-admin-flag weirdness.
  - [x] An account with auto-lock + "Remember this login" OFF shows the lightweight PIN pad on idle-timeout, not a full password screen.
  - [x] Setting only "Launch behavior" to "default" (without touching "Default Account") shows a real, non-empty profile list.
  - [x] "Remember this login" confirm dialog keeps its Cancel/Confirm buttons inside the box across its different states (with/without an error line, spinner vs. button row).
- [x] Bonfire Phase 5, cross-household groups (2026-08-29 → 2026-08-31):
  - [x] `BonfireGroupScreen`'s restructured layout (owner section + member section can both show at once).
  - [x] Picker visual badges: gold ring on a household's master profile, compact "LINKED" pill, lock badge.
  - [x] "Switch Profile" merges every linked household into one sectioned picker with no duplicate/bogus section (the "Anton's Bonfire" self-duplication bug was fixed but never re-confirmed).
  - [x] Kicking a household via Jellyfin's own web UI (not Fjord) — the cold-start picker shouldn't still show it as a ghost entry.
- [x] Open mystery from Phase 4's first real test: Bonfire's `/switch` endpoint hit its own 429 rate-limit after only 1 Fjord-visible failed PIN attempt (Bonfire's docs say the limit is 5 in 15 minutes) — root cause never confirmed. If it recurs, grab the log immediately; that's the only way to actually pin it down.
- [ ] **Root-caused 2026-09-07, no new code needed — needs a real re-login, not a rebuild.** Anton's own account was computed as having 6 members (should be 4 — Anton+Anso+Akira+Raphael) because `test`/`test2` (a completely independent master+sub-profile pair, same server) show up merged into it. A live diagnostic against the real server (temporary test, this dev machine's own saved session, deleted after) confirmed conclusively that the *live* server reports only Anton's real 4-member household — `test`/`test2` are stale **local** `Config.profiles` residue from the already-documented 2026-08-29 "cross-household sub-profile theft" bug, corrupted while Anton's and test's households were genuinely linked via a Bonfire group at the time, before that group was later dissolved (confirmed directly by the user). That bug's own fix explicitly "self-heals nothing retroactively" — recovery is a fresh, real "+ Add Account" login for `test` (typing `test`'s actual password) on every device still showing this, at minimum the HTPC. Correction to the earlier draft of this note: the HTPC's own `switch_to_profile(b38cbfb7...)` was a switch to **Akira** (a real, correct household member), not `test2` as first assumed — that assumption was wrong and has been corrected in CLAUDE.md. **To verify**: do the "+ Add Account" re-login for `test`, then confirm `test`/`test2` no longer appear in Anton's own "Switch Profile"/Bonfire Admin lists, and check the log for the diagnostic line (`sync_bonfire_subprofiles: reported entry ...`, shipped in `0fcc23b`) showing `test2` correctly reporting `test`'s own id as its master on its own next sync.

---

## Pending

(none open — everything recently shipped is fully documented in [CHANGELOG.md](CHANGELOG.md) (user-facing) and `CLAUDE.md` (full technical narrative, dated sections); anything still needing real-hardware confirmation lives in the `Live-test checklist` above, not here)

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

- **Own the platform/event-loop layer, for true global input-activity detection (and possible HDR groundwork) — next up, right after Bonfire Phase 4 shipped, on a new `event-loop` branch.** Found while scoping Bonfire Phase 4's own best-effort mouse-activity tracking: Slint delivers pointer events only to the topmost element under the cursor, with no true "any mouse movement, anywhere" hook — a real fix means Fjord taking over its own event-loop/platform layer (a custom `slint::platform::Platform` wrapping the default backend, intercepting every event before Slint's own routing). Genuinely valuable, but foundational and high-risk (could touch how the whole app boots/renders) and entirely unscoped so far. Resolved via direct discussion, 2026-08-29: rather than park this indefinitely the way HDR originally was, the user pointed out Fjord is still Alpha with no release to protect, so there's no reason to defer to "someday" — create the branch and start a real `/plan` pass for it soon, right after Phase 4 ships, not bundled into it (Phase 4 itself is a complete, ready feature that shouldn't wait on an unscoped prerequisite). May also be useful groundwork for the deferred HDR work below, which separately needs Fjord to do its own Wayland surface-level negotiation beyond what the default Slint backend exposes — not assumed, worth confirming once this is actually scoped.
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
  - **User's explicit call, 2026-08-17**: build this, but after the Bonfire/profile work, and on a dedicated `hdr` git branch rather than `main`, since this could leave the app broken for an extended stretch in a way almost nothing else in this codebase's incremental history has — see the `feedback_branch_after_release` memory. Also explicitly flagged: this may be capped by the user's own aging Pascal-era GPU (GTX 1050 Ti, frozen on the `580.xx`/`581.xx` NVIDIA driver branch) regardless of how correctly the Fjord-side work is done. **Update, 2026-08-29**: Bonfire Phases 3-4 have since shipped; the immediate next item in the queue is actually the new `event-loop` branch above (own the platform/event-loop layer), not HDR directly — HDR may benefit from whatever that work produces, so it's reasonable for it to follow rather than jump the queue.
  - Not scoped in detail yet — the real first step when picked up is a proper `/plan` pass (matching how Watch Trailer/Bonfire were planned), not jumping straight to code. A cheap diagnostic worth doing first, on either branch: check `fjord.log` for an mpv-emitted warning about `target-colorspace-hint` being ignored for the active VO, to directly confirm the no-op theory before designing around it.
