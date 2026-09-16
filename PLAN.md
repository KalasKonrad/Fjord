# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

---

## Completed

Full curated version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Full implementation detail per feature: `CLAUDE.md`.

---

## Live-test checklist

**This is the actionable list — check something off only once it's actually been clicked through on real hardware.** A clean `cargo build`/`clippy`/`test` means the code compiles and its pure-function logic holds; it says nothing about whether it works. Every item here started life as a "not live-tested" caveat buried in a `Pending` paragraph below — this section exists specifically so those caveats stop getting lost in narrative prose the moment the conversation moves on. **Add a line here the instant something ships "not live-tested," don't wait to be asked** — this exact list decayed back into narrative-only entries once already (see `8ea7aab`, "consolidate PLAN.md's Pending section into one live-test checklist"), and it's not allowed to happen a second time.

- [ ] **event-loop branch, true global mouse-activity detection (2026-09-08).** Hover/wiggle the mouse over a dashboard card or button (something with its own `TouchArea` — the old bug never fired there) for longer than the configured `lockout_minutes`, with no keypress at all, and confirm the session now correctly locks (previously it wouldn't). Separately confirm every ordinary mouse interaction is completely unchanged everywhere, especially the fullscreen player: hover highlights, clicks, right-click menus, seek-bar drag, controls auto-hide/cursor-hider.
- [ ] **Watchlist/Coming Up dashboard cards — keyboard Enter/`I`/`C` fix (2026-09-16).** On Home/TV Shows/Movies, focus a Watchlist or Coming Up card for something requested but not yet in your library, via keyboard/remote only (not mouse — that already worked), and press Enter — confirm it now opens the Discover/RequestDetail screen instead of attempting playback. Also check `I` (should open the same screen) and `C` (should open the Discover-flavored context menu — Request/Cancel/Watchlist rows, not the Jellyfin one).
- [ ] **hdr branch, Stage 4: real mpv HDR output (2026-09-16).** With the "HDR passthrough" toggle ON, play the same real HDR10 title already confirmed negotiating cleanly on the HTPC — confirm `fjord.log` still shows a clean `Active (HDR10)` (unchanged from Stage 3) with no new errors. The actually-meaningful end-to-end test needs the output genuinely switched into real HDR mode first (via `media_display_sync` or the manual KDE toggle) — with that done, confirm the TV's own HDR indicator engages AND the picture looks correct (not crushed/too-dark, not blown-out/washed) — this is the one thing nothing software-side can substitute for, and the first time this combination (Stage 3 negotiation + Stage 4 real pixel output + output genuinely in HDR mode) has ever been tried together. With the toggle OFF, confirm behavior is byte-identical to before this stage (true by construction — worth a real check regardless). Also worth trying: playing two HDR items back to back, or an HDR item immediately followed by an SDR one — the specific scenario that exercises a real race-condition fix this stage made to Stage 3's own `Unset` handling (check `fjord.log` for a clean `Idle` reset between items, not a wrongly-early `apply_hdr_output` call for the second item).

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

- ~~**Own the platform/event-loop layer, for true global input-activity detection**~~ — done, 2026-09-08, on the `event-loop` branch. Turned out much smaller than the original framing: no from-scratch `Platform` needed at all, just `slint::BackendSelector::with_winit_custom_application_handler` (feature `unstable-winit-030`) tapping every raw winit event before Slint's own hit-testing. Also confirmed genuinely **unrelated** to HDR's own Wayland prerequisite (corrects this entry's own earlier hedge) — see CLAUDE.md's dated section for the full story.
- **Theming / layout customisation**: accent colour palette, dashboard row visibility toggles, row reordering — needs the full layout system in place first before it makes sense to build.
- **Vulkan rendering path** — second render backend alongside the current OpenGL path. Requires: Slint WGPU backend, `MpvRenderCtx` initialized with `MPV_RENDER_API_TYPE_VULKAN`, Vulkan FBO management replacing the current `gl::*` code. Enables true zero-copy decode on AMD (`hwdec=vulkan`, no CPU roundtrip). Selection would persist in Config as `gpu_renderer: "opengl" | "vulkan"`, taking effect on next restart. **Correction, 2026-08-17, live-questioned ("is this a fact?")**: this entry's original "Legacy NVIDIA hardware needs OpenGL" line (written 2026-06-24, the very first commit that added this item) was never actually verified — checked now and it's likely wrong, not right: the proprietary NVIDIA driver has shipped full Vulkan support since Vulkan's 2016 launch, Pascal (this project's own target GPU, GTX 1050 Ti) was a day-one Vulkan-generation architecture, and a live report confirms Vulkan 1.3.275 working on this exact card under driver 570.133.07 — well within the same 580.xx branch Arch already ships for it (see [phoronix.com](https://www.phoronix.com/review/nvidia-gtx-1050), [forums.developer.nvidia.com](https://forums.developer.nvidia.com/t/vulkan-support-on-the-gtx-1050-max-q/70986)). None of Fjord's own extensively-documented legacy-NVIDIA bugs (stride corruption, VO-init race, HDR shader-compile crash) are Vulkan-specific either — all are OpenGL/EGL-Wayland-path issues. So there's no known hardware/driver wall forcing OpenGL on this hardware; a toggle would still be worth having (this project's OpenGL/EGL path has a long track record of NVIDIA-Wayland-specific bugs, so Vulkan could plausibly be MORE reliable here, untested either way), just not because of a compatibility requirement — see the `hdr`-branch memory / CLAUDE.md's HDR section for why HDR work itself doesn't need or benefit from this migration regardless.
- Gamepad / remote control — d-pad maps to arrow keys; formal evdev/udev support deferred
- **Dashboard row reorder** — drag-to-reorder; part of the future theming/layout customisation update
- **Trickplay** — seek bar scrub thumbnail popup. Requires: fetch Jellyfin trickplay manifest (`GET /Videos/{id}/Trickplay/{width}/tiles`), parse tile sheet dimensions (tile size, columns, rows, interval), cache tile images per video, render a thumbnail above the seek bar while scrubbing (position computed from `seek-hover-pos`). Deferred because it's a separate subsystem from chapter nav and the API surface needs more investigation.
- ~~**Multi-account / multi-server support**~~ — done. Fully subsumed by the Bonfire/JellyProfiles `Config` restructuring (`DeviceConfig`/`Vec<ProfileSettings>`) and `ProfilePickerScreen`; each profile carries its own `server_url`, so this is genuine multi-server too, not just multi-account on one server. See CLAUDE.md's Bonfire section.
- **Display mode auto-sync (resolution/refresh-rate/HDR to match video content), KDE-only for now** — live-reported 2026-08-14, a real HTPC hitch traced to the user's own separate `media_display_sync` script (polls Jellyfin's API independently, hit a 5s timeout, briefly reverted the display mode then switched back ~10s later — a real, visible HDMI renegotiation, but not a Fjord bug; Fjord's own stream never noticed). Discussed as a genuine future candidate: building this into Fjord directly would be strictly better than the external-poller approach — Fjord already has the real container framerate/resolution/HDR metadata synchronously from mpv the instant a file loads (no separate network poll to time out), and a mode switch could ride the existing skip-fade-to-black mechanism instead of happening as a raw visible renegotiation mid-frame. The real cost is that it's Wayland-compositor-specific, not a Jellyfin/mpv API surface — the user's HTPC (and only currently-testable machine) runs KDE Plasma, so a first cut would go through KDE's own KWin/kscreen D-Bus interface and be KDE-exclusive until/unless a second compositor is in scope to test against; no wlr-output-management (Sway/Hyprland-style) support without a way to verify it. Not scoped further yet — worth a proper `/plan` pass (mirroring how Watch Trailer/Bonfire were planned) once actually picked up, and reading the user's own `media_display_sync` script first (repo visibility TBD) to see exactly which mechanism it already leans on.
  - **Sharper motivating hypothesis surfaced 2026-08-15, during the HDR passthrough investigation above**: the script changes resolution/Hz *after* the video has already started playing — a live connector mode-set happening concurrently with (or right after) mpv's own render-context setup and `target-colorspace-hint` negotiation is a plausible, sensible interference mechanism (a DRM mode-set is a heavyweight, connector-level operation; racing it against mpv's own HDR-hint negotiation could easily reset or drop whatever was being negotiated) — genuinely the same *shape* of race this project already found and fixed once for a completely different reason (the VO-init-vs-render-context-creation race, see "NVIDIA HTPC: video-only-audio black screen" above). User's own planned test, not yet run: same file/script, once with resolution/Hz already correctly set *before* Fjord even starts (no mid-playback mode-set at all) vs. the current after-the-fact behavior — if the pre-set case works and the mid-playback one doesn't, that's a clean, direct confirmation. User's own words: "if the script interfere then i will push up the integration into fjord" — i.e., a confirmed race here is the trigger condition for actually picking this item up, not a hypothetical someday. If/when that happens, the fix this points toward specifically is doing the mode-set *before* starting mpv's own playback pipeline for that item (matching the already-established "resolve first, don't create the race in the first place" discipline this project keeps landing on elsewhere), not just retrying/reordering after the fact.
- **Poster/card scaling setting** — from Phase 117: with titles/episode names now always shown in full (no truncation), a very long title on the smallest card breakpoint (115px wide) shrinks that card's poster noticeably to make room. User's proposed fix: a settings toggle to use larger posters in the library/dashboard (more text budget per card before this becomes visible) rather than reintroducing a truncation cap. Would likely hang off the existing `dash-card-w`/`dash-card-h` breakpoint functions in `main.slint`, e.g. as a user-chosen size multiplier or an extra breakpoint tier. Alternative approach floated in the same conversation, not mutually exclusive: render the poster image full-bleed behind the title/subtitle text with a dark semi-transparent scrim, instead of a separate text block below the poster — sidesteps the whole shrink-to-fit problem structurally rather than giving the text more room. Neither implemented yet; revisit when picked up.
- **Real HDR passthrough (`hdr` branch) — Stages 1-4 all implemented; Stage 3's negotiation confirmed live on the real HTPC, Stage 4 (and the real-display end-to-end combination) still awaiting live confirmation.** `Config.device.target_colorspace_hint` ("HDR passthrough" in Settings) does nothing at all under Fjord's actual render path via mpv's own mechanism — mpv's manual states `--target-colorspace-hint` "Requires a supporting driver and `--vo=gpu-next`", but Fjord uses `vo=libmpv`. Real HDR passthrough means Fjord doing its own Wayland `color-management-v1` negotiation directly against its own surface, independent of mpv — see CLAUDE.md's HDR section for the full trace.
  - **Stage 1+2 (2026-09-10, `abb48a6`)** — reachable window handles + a one-shot capability diagnostic. **Live-confirmed** on the real HTPC: KWin genuinely advertises `wp_color_manager_v1` with a rich capability set (`St2084Pq`/`Bt2020` among them — exactly what real HDR10 negotiation needs), and the surface-wrap mechanism works. Zero playback impact either way, as designed.
  - **Stage 3 (2026-09-14) — negotiation confirmed working on the real HTPC (2026-09-14/16).** For genuinely HDR10 (PQ+BT.2020) content, builds a real `wp_image_description_v1` (validated mastering-luminance/CLL/FALL from mpv's own per-file HDR10 metadata) and applies it to Fjord's surface via `wp_color_management_surface_v1.set_image_description`. Reuses the existing "HDR passthrough" Settings toggle (default off) and a live on-screen "HDR" row on the stats overlay. `fjord.log` on the real HTPC shows a clean `ready2` → `Active (HDR10)` for a genuine 4K title, no fatal Wayland errors, `hwdec-current="nvdec"` — real Pascal-generation hardware. The output was still in SDR signal mode at test time, so KWin correctly tone-mapped down rather than the TV entering real HDR mode — confirms the negotiation logic is right, not yet that a real HDR display goes into HDR mode. HLG content and non-named/custom primaries are a deliberate v1 scope cut (this compositor doesn't advertise `Hlg`), not a bug.
  - **Stage 4 (2026-09-16) — mpv-side real HDR output, implemented.** Widens `create_fbo()`'s FBO to `GL_RGB10_A2` (gated on the "HDR passthrough" toggle alone, not per-item eligibility — a real timing constraint, see CLAUDE.md's own dated section) and, once Stage 3's negotiation for the current item is confirmed `Active` (polled from `wire_mpv_timer`, not a callback — the HDR worker thread has no path to notify anything directly), sets `target-trc=pq`/`target-prim=bt.2020` live via a new `Player::apply_hdr_output()`. `target-peak` deliberately left at mpv's own `auto` — Fjord has no way to know the real display's peak, and KWin (already informed by Stage 3) is what should do any final adaptation. Also fixed, as part of this same pass: a real latent race in Stage 3's own `Unset` handling (an async-only status reset could leave a stale `Active` reading briefly visible to a brand-new item's own poll) — closed by making the reset synchronous on the calling thread. `cargo build`/`clippy --workspace --all-targets`/`test --workspace` clean, first attempt, zero fixes needed. **Not yet live-tested** — see this doc's own Live-test checklist entry above for the exact sequence, including the specific "two HDR items back to back" scenario that exercises the race fix.
  - Also may still be capped by the user's own aging Pascal-era GPU (GTX 1050 Ti, `580.xx` driver branch) regardless of how correctly the Fjord-side work is done — the already-existing, default-off Settings toggle is the escape hatch if real hardware behaves badly.
