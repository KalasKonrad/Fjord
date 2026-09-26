# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

---

## Completed

Full curated version history: [CHANGELOG.md](CHANGELOG.md) (git tags `v0.1.0`–`v0.4.2`). Full implementation detail per feature: `DEVLOG.md` (dated sections).

---

## Live-test checklist

**This is the actionable list — check something off only once it's actually been clicked through on real hardware.** A clean `cargo build`/`clippy`/`test` means the code compiles and its pure-function logic holds; it says nothing about whether it works. Every item here started life as a "not live-tested" caveat buried in a `Pending` paragraph below — this section exists specifically so those caveats stop getting lost in narrative prose the moment the conversation moves on. **Add a line here the instant something ships "not live-tested," don't wait to be asked** — this exact list decayed back into narrative-only entries once already (see `8ea7aab`, "consolidate PLAN.md's Pending section into one live-test checklist"), and it's not allowed to happen a second time.

- [ ] **event-loop branch, true global mouse-activity detection (2026-09-08).** Hover/wiggle the mouse over a dashboard card or button (something with its own `TouchArea` — the old bug never fired there) for longer than the configured `lockout_minutes`, with no keypress at all, and confirm the session now correctly locks (previously it wouldn't). Separately confirm every ordinary mouse interaction is completely unchanged everywhere, especially the fullscreen player: hover highlights, clicks, right-click menus, seek-bar drag, controls auto-hide/cursor-hider.
- [ ] **Watchlist/Coming Up dashboard cards — keyboard Enter/`I`/`C` fix (2026-09-16).** On Home/TV Shows/Movies, focus a Watchlist or Coming Up card for something requested but not yet in your library, via keyboard/remote only (not mouse — that already worked), and press Enter — confirm it now opens the Discover/RequestDetail screen instead of attempting playback. Also check `I` (should open the same screen) and `C` (should open the Discover-flavored context menu — Request/Cancel/Watchlist rows, not the Jellyfin one).
- [ ] **HDR Stage 4: real mpv HDR output — negotiation confirmed clean (2026-09-17), full picture-correctness confirmation still open.** With the toggle ON, `fjord.log` on the real HTPC now shows `Active (HDR10)` cleanly across 3 separate real launches of the current build, no new errors, the `max_cll`/`max_fall` validation-skip fix logging correctly each time. With the toggle OFF, plain SDR playback correctly never attempts negotiation (confirmed the same day, separate session). Still open: (1) confirm the real *video picture* itself looks correct (not crushed/washed) once the TV is genuinely in HDR mode — every real-hardware test so far either had the output still in SDR signal mode, or wasn't directly confirmed either way; (2) the specific "two different HDR items back to back in one running session" scenario that exercises Stage 4's own race-condition fix to Stage 3's `Unset` handling (every real negotiation so far has been a replay of the same title across separate app relaunches, not two distinct titles in one session).
- [ ] **Native display-mode-sync (display_sync), fully implemented, not yet live-tested (2026-09-18).** Ported the user's proven external `media_display_sync` script's `kscreen-doctor` mode-selection mechanism directly into Fjord (Settings → Video → DISPLAY SYNC, off by default) — see CLAUDE.md's own dated section for the full design, including why it's a structurally separate trigger branch from HDR Stage 3, not a merged condition. `cargo build`/`clippy`/`test` clean (36/36 tests incl. 9 new `compute_target_mode` unit tests). With the toggle off, confirm behavior is byte-for-byte identical to today — no `kscreen-doctor` calls, no delay to HDR Stage 3's own negotiation. With it on: back-to-back episodes of the same show/resolution shouldn't trigger a second mode-switch (no repeated 3s stall); a genuine stop should revert to the configured default within a few seconds, a replace-in-place (next episode/track) should never revert; HDR content should still reach `Active (HDR10)` in the stats overlay, now visibly *after* the display's own mode-switch settles (`fjord.log` timestamps: `sync_to_source` completing before `hdr worker: negotiated...`); a real 4K HDR title followed immediately by a 1080p SDR item (or vice versa) should exercise the full mode+HDR/WCG transition cleanly in both directions. **Default resolution/Hz fixed to real dynamic dropdowns the same day**, from live dev-machine feedback ("not many choises for default resolution and no default hz") — both now list the actual connected display's own supported modes (`kscreen-doctor -o`, via the newly `pub(crate)` `get_supported_modes`/new `supported_resolutions_and_hz`), refetched whenever Output changes, replacing the original fixed 3-resolution/7-Hz compile-time lists. `cargo build`/`clippy`/`test` clean (25/25, unchanged). Not yet re-confirmed live — check that both dropdowns now show real, display-specific values instead of the old fixed list, and that picking one still persists/applies correctly. **Output row now marks the primary display AND a friendly monitor name, same day** — real KDE `priority` field, verified `priority==1` means primary directly from `kscreenctl`'s own `set-primary.cpp` source, not guessed; a real name↔desc lookup (`FjordState.display_sync_outputs`, mirroring audio-device's own shape) keeps the annotated label separate from the persisted connector name. The friendly name (`"DP-3 — HP ZR24w (Primary)"`) reads real EDID data straight from `/sys/class/drm/*/edid` (a small, self-contained, unit-tested VESA EDID parser — no new external tool dependency) after the user asked "how dose kwin do it?" and shared a screenshot of KDE's own panel doing exactly this, which is what proved the data was genuinely available and worth pursuing. Verified end to end against this dev machine's real 3-monitor setup, exact match to the screenshot (`DP-3 — HP ZR24w (Primary)` / `HDMI-A-1 — Philips 245P` / `HDMI-A-2 — Philips 245P`). `cargo build`/`clippy`/`test` clean (28/28, +3 new EDID-parser tests using real, non-sensitive hardware EDID as permanent fixtures). Not yet clicked through in the actual Settings UI — also worth checking on the HTPC specifically, whose TV EDID may not carry a product-name descriptor at all (already handled gracefully — falls back to the plain connector name). **Real HTPC log investigation, 2026-09-24 (current build across the whole log rotation, r1025.f62d11e/r1026) — confirms two of this item's own open checklist points, and found+fixed a real bug in a third.** Confirmed **live**, not just in theory: (1) the Branch A/B ordering — `sync_to_source` genuinely completes (`display_sync: setting HDR on / WCG on`) before HDR Stage 3 negotiates (`hdr worker: negotiated HDR10 image description`), ~91ms apart, on a real 4K HDR title (2026-09-22 log); (2) a genuine stop reverts to the configured default within well under a second. But the SAME log, and independently today's own session, both showed **the exact same real bug**: two genuine-stop reverts landing ~9s apart (stop, then app quit moments later) both fired the real `kscreen-doctor` mode+HDR/WCG apply calls, even though the display was provably already sitting at default after the first one. Root cause: `revert_to_default`'s `needs_revert` check was `s.display_sync_current_mode.is_some() || ...` — true forever once ANY mode change has ever happened in the session (the field is only ever reassigned to `Some(...)`, including by `revert_to_default` itself), rather than comparing against the actual default target the way `sync_to_source`'s own `mode_changed`/`hdr_changed` checks correctly do. Practical effect: after the very first video of a session, every later stop re-ran the two heavyweight DRM/kscreen-doctor calls unconditionally, defeating half the point of the "don't redundantly re-apply" tracking for the rest of that session. **Fixed** — `needs_revert` now compares `display_sync_current_mode`/`_current_hdr` against the real default resolution/Hz/HDR-off target, matching `sync_to_source`'s own comparison shape exactly. `cargo build`/`clippy --workspace --all-targets`/`test --workspace` clean (28/28, unchanged — no new test, this is a plain comparison-logic fix with no new pure function to isolate). Still not re-confirmed live: reproduce a genuine stop followed shortly by app quit (or two stops in a row) and confirm the SECOND revert is now a silent no-op (no `display_sync: setting display mode`/`setting HDR` lines the second time) rather than repeating the calls. The "two different HDR items back to back" and "4K HDR followed immediately by 1080p SDR" scenarios from this item's own original checklist are still unconfirmed — every real negotiation seen in the logs so far has been one HDR item per session.
- [ ] **display-mode-prefetch — display switches before playback starts (2026-09-26, branch `display-mode-prefetch`).** With Settings → Video → Sync display to source ON, on the HTPC:
  - Play a 1080p24 movie while the display is at the default mode → the display switches (with the "Loading…" spinner) and the **first frame appears only after** the switch; no blink mid-playback. In `fjord.log`, `display_sync: setting display mode` comes before `mpv player started`.
  - Same with a 4K HDR title → mode + HDR/WCG switch before the first frame; HDR stats row still reaches `Active (HDR10)`.
  - Press Play then Stop within ~2 s → the display must not switch afterwards (nothing left in the item's mode).
  - Press Play, then Stop *while the screen is blanking/relinking* (mid-switch) → the display returns to the default mode a few seconds later (`display_sync prestart: playback stopped during the switch — reverting` in the log).
  - Resume a Continue Watching movie, press Stop during the switch → its resume position is still there afterwards (not reset to 0).
  - Press Play on item A, then quickly Play on item B → only B's video plays; no stray load of A.
  - Queue prev/next/jump and Detail → Play (the fallback-fetch paths) still start promptly.
  - A stall-recovery reload (pull the network briefly mid-playback) reloads immediately — no extra display-switch wait.
  - Next episode of the same show → no second `kscreen-doctor` mode-set (cached no-op).
  - Toggle OFF → playback starts exactly as before (no spinner delay, no `display_sync` lines).
  - Music tracks start immediately regardless of the toggle.
- [ ] **HDR UI-chrome color corruption — known limitation, confirmed live (2026-09-17), no fix planned yet.** With the "HDR passthrough" toggle on, any UI drawn over/alongside the video (e.g. the player controls) renders with wrong colors — confirmed root cause: Fjord's whole window is one Wayland surface, so tagging it as PQ/BT.2020 for the video also misinterprets Slint's own plain sRGB UI pixels through the same colorimetry. No cheap fix exists (confirmed against the real protocol spec, including the compositor's own `windows_scrgb` mixed-content mechanism, which doesn't help since it needs linear pixel values Slint doesn't produce) — the real fix needs video on its own Wayland subsurface, deliberately deferred as a separate, large future effort (see Deferred/future section). This item isn't really "test and check a box" — it's a standing reminder that turning the toggle on has this known, visible cost until the subsurface work lands; remove once that's built.

---

## Pending

- [ ] When HTPC testing of `display-mode-prefetch` is done (or it's merged): remove `#branch=display-mode-prefetch` from `PKGBUILD`'s `source=` **on main** — it's there so the HTPC's `makepkg -si` builds the branch.

(nothing else open — everything recently shipped is fully documented in [CHANGELOG.md](CHANGELOG.md) (user-facing) and `DEVLOG.md` (full technical narrative, dated sections); anything still needing real-hardware confirmation lives in the `Live-test checklist` above, not here)

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
- ~~**Display mode auto-sync (resolution/refresh-rate/HDR to match video content), KDE-only for now**~~ — implemented, 2026-09-18, as `display_sync.rs` (Settings → Video → DISPLAY SYNC, off by default). Planned via `/plan`, ported the user's own `media_display_sync` script's proven `kscreen-doctor` mechanism directly rather than the KWin/kscreen D-Bus route originally sketched below — see CLAUDE.md's own dated section for the full design and the `Live-test checklist` above for what's still unverified on real hardware.
  - **Sharper motivating hypothesis surfaced 2026-08-15, during the HDR passthrough investigation above**: the script changes resolution/Hz *after* the video has already started playing — a live connector mode-set happening concurrently with (or right after) mpv's own render-context setup and `target-colorspace-hint` negotiation is a plausible, sensible interference mechanism, genuinely the same *shape* of race this project already found and fixed once for a completely different reason (the VO-init-vs-render-context-creation race). **This is exactly what `display_sync.rs`'s own Branch A/B ordering fix (2026-09-18) was designed to prevent** — Branch B (display_sync) settles the physical mode/HDR/WCG BEFORE Branch A (HDR Stage 3) ever negotiates, both one-shot flags claimed synchronously so the two heavyweight Wayland/DRM operations can never race each other. Not yet confirmed live against a real HDR title exercising both subsystems at once — see the `Live-test checklist` above.
- **Poster/card scaling setting** — from Phase 117: with titles/episode names now always shown in full (no truncation), a very long title on the smallest card breakpoint (115px wide) shrinks that card's poster noticeably to make room. User's proposed fix: a settings toggle to use larger posters in the library/dashboard (more text budget per card before this becomes visible) rather than reintroducing a truncation cap. Would likely hang off the existing `dash-card-w`/`dash-card-h` breakpoint functions in `main.slint`, e.g. as a user-chosen size multiplier or an extra breakpoint tier. Alternative approach floated in the same conversation, not mutually exclusive: render the poster image full-bleed behind the title/subtitle text with a dark semi-transparent scrim, instead of a separate text block below the poster — sidesteps the whole shrink-to-fit problem structurally rather than giving the text more room. Neither implemented yet; revisit when picked up.
- **Real HDR passthrough — Stages 1-4 merged to `main` (2026-09-17); negotiation confirmed live on the real HTPC, full picture-correctness + the UI-chrome known limitation are the two open threads.** `Config.device.target_colorspace_hint` ("HDR passthrough" in Settings, opt-in, off by default) does nothing at all under Fjord's actual render path via mpv's own mechanism — mpv's manual states `--target-colorspace-hint` "Requires a supporting driver and `--vo=gpu-next`", but Fjord uses `vo=libmpv`. Real HDR passthrough means Fjord doing its own Wayland `color-management-v1` negotiation directly against its own surface, independent of mpv — see CLAUDE.md's HDR section for the full trace and history.
  - **Stage 1+2 (2026-09-10)** — reachable window handles + a one-shot capability diagnostic. **Live-confirmed** on the real HTPC: KWin genuinely advertises `wp_color_manager_v1` with a rich capability set (`St2084Pq`/`Bt2020` among them — exactly what real HDR10 negotiation needs), and the surface-wrap mechanism works. Zero playback impact either way, as designed.
  - **Stage 3 (2026-09-14) — negotiation confirmed working on the real HTPC.** For genuinely HDR10 (PQ+BT.2020) content, builds a real `wp_image_description_v1` (validated mastering-luminance/CLL/FALL from mpv's own per-file HDR10 metadata) and applies it to Fjord's surface via `wp_color_management_surface_v1.set_image_description`. HLG content and non-named/custom primaries are a deliberate v1 scope cut (this compositor doesn't advertise `Hlg`), not a bug.
  - **Stage 4 (2026-09-16) — mpv-side real HDR output, negotiation confirmed clean on real hardware (2026-09-17), picture correctness still open.** Widens `create_fbo()`'s FBO to `GL_RGB10_A2` (gated on the toggle alone, not per-item eligibility) and, once Stage 3's negotiation for the current item is confirmed `Active` (polled from `wire_mpv_timer`), sets `target-trc=pq`/`target-prim=bt.2020` live via `Player::apply_hdr_output()`. `target-peak` deliberately left at mpv's own `auto` — KWin (already informed by Stage 3) is what should do any final display-peak adaptation. Also fixed as part of this stage: a real latent race in Stage 3's own `Unset` handling. `fjord.log` shows `Active (HDR10)` cleanly across 3 real HTPC launches of the current build — see the Live-test checklist above for what's still open.
  - **Known limitation, confirmed live (2026-09-17), not yet fixed: UI chrome renders wrong colors while the toggle is on.** Fjord's whole window is one Wayland surface (UI + video both, composited by Slint into one scene) — tagging it as PQ/BT.2020 for the video also misinterprets Slint's own plain sRGB UI pixels. Confirmed no cheap single-surface fix exists, including the compositor's own `windows_scrgb` mixed-content mechanism (real protocol request this compositor advertises — it needs linear pixel values for the whole surface, which Slint doesn't produce). The only correct fix is putting video on its own Wayland subsurface with independent color-management state, separate from the UI's surface — see the new deferred item just below.
  - Also may still be capped by the user's own aging Pascal-era GPU (GTX 1050 Ti, `580.xx` driver branch) regardless of how correctly the Fjord-side work is done — the toggle being opt-in and off by default is the escape hatch if real hardware behaves badly.
- **HDR Stage 5 (not started, not planned in detail yet): video on its own Wayland subsurface, so UI chrome and video get independently-correct color treatment.** The real fix for the UI-chrome corruption above. Genuinely large and foundational — bigger than anything in Stages 1-4 — since it means giving mpv's own render-API output a presentation path separate from Slint's own compositing (its own EGL surface bound to a new `wl_subsurface`, kept in sync with wherever Slint's layout would otherwise draw the video, GL/EGL context sharing with whatever context Slint's own renderer uses, a transparent cutout in Slint's own scene so the subsurface shows through). Real open technical questions, not yet researched: whether Slint/winit exposes (or can be made to expose) the GL/EGL context handle its own renderer uses, so a second surface's context can share it without an expensive texture copy; how to keep the subsurface's position/size synced with Slint's own dynamic video-rectangle geometry across all 4 playback modes (fullscreen, video-behind-ui, mini-player, background). Deliberately scoped out of the `hdr` branch's own Stages 1-4 and deferred — start on a fresh branch when picked up (not a continuation of `hdr`, which was merged and deleted), and plan it properly via `/plan` first, matching every other HDR stage's own treatment.
