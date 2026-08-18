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
- [ ] A wrong PIN, or any other failed switch, leaves the outgoing session fully intact and usable, **and** the PIN screen has a real, D-pad/mouse-reachable way back (not Escape-only)
  **FIXED 2026-08-17**: added a real "← Cancel" button to the PIN-entry modal (`profile_picker.slint`), reachable via Down-past-the-keypad or mouse click; PIN pad also now accepts real-keyboard digit/Backspace input (see below)
  **FIXED 2026-08-18**: the Cancel button's own highlight showed at the same time as the PIN grid's last-focused key — the grid's cursor now clears to "none" whenever focus has moved to Cancel, matching how ProfileEditScreen's own PIN pads already handle the identical handoff
- [ ] Unchecking "Remember this login" forces a real password re-entry next launch; re-checking it should be possible again without a full sign-out
  **FIXED 2026-08-17** (design change, resolved via `AskUserQuestion`): "Remember this login" is now a Settings → Profiles toggle you can flip anytime — turning it OFF is immediate (no proof needed); turning it back ON opens a small "confirm your password" modal (not a full re-login) before it takes effect. RequireLogin's initial keyboard focus now lands on Password (not Server) when server+username are already known, and Escape now works to cancel out of it.
  **FIXED 2026-08-18**: with "Remember this login" off, switching between PROFILES of the account you're already actively signed into wrongly forced a full re-login every time, and landed on the account's own root afterward instead of the profile you picked — a real, pre-existing gap in the 2-tier redesign, not something the toggle itself introduced. Switching within an account you're already live in no longer re-checks `remember_login` at all.
- [ ] Add Account's Cancel/Back returns to wherever it was opened from — the Account Picker's own "+" tile returns to the picker, Settings' "Add Account" row returns to Settings
  **FIXED 2026-08-18**: two real bugs. (1) The previous Escape fix never actually fired in practice — `LoginScreen` always grabs native keyboard focus into a text field on open, and the global key-dispatch tier is a sibling of this screen, not an ancestor, so it never saw the keypress at all; fixed with the same per-field `key-pressed` hook `ProfileEditScreen`'s own text fields already use. (2) The button's label named a specific destination ("← Back to Profiles") that didn't match where it actually goes (the account picker) — simplified to a plain "← Back" per the user's own suggestion, so the label can't drift out of sync with the logic again.

  **FIXED 2026-08-17**: the button already existed but had no keyboard path at all (Ctrl+Q — quit the whole app — was the only reachable key); Escape now invokes it
- [ ] Default Account / Default Profile behave sensibly together after a real restart
  **Design clarified via `AskUserQuestion`**: any account can be the default (already worked); a Default Profile can only ever be a profile *under* the current Default Account — picking one from a different account was a silently-dead setting (confirmed from `should_show_picker_at_startup`'s own tier-2 resolution, which only ever searches within the resolved account's own profiles). **FIXED 2026-08-17**: the Default Profile dropdown is now scoped to only show the current Default Account's own profiles, so this mismatch can no longer be created via the UI.
- [ ] After extended real use, `~/.config/fjord/config.json` should never show `is_bonfire: true` with a `master_user_id` pointing at a profile that also lists it as a sub-profile (the self-reference/cycle corruption bug fixed 2026-08-14/16) — covered by a permanent regression test (`repairs_bonfire_master_user_id_cycle`) and a real-corrupted-file repro at the time, no specific action needed unless it recurs
### ProfileEditScreen — full D-pad keyboard navigation
- [ ] Tab still moves natively between the 3 text fields, including wrapping from the last field back to the first
  **FIXED 2026-08-17**: mid-chain Tab already worked; only the wrap-around at either end needed an explicit hand-off (Slint's native focus chain has nothing else in this screen to wrap into)
- [ ] Every zone reachable via Up/Down in order, including both conditional checklists (Libraries/Devices) appearing/disappearing correctly
- [ ] Both PIN pads' 12-key grid nav + Enter-confirm match the mouse path exactly, **and** work from a real keyboard (digits + Backspace)
  **FIXED 2026-08-17**: neither PIN pad (nor the profile-picker's own separate login PIN entry, which had the identical gap) accepted raw digit keys at all, and Backspace closed the whole screen instead of deleting a digit — both fixed on both surfaces
- [ ] Both dropdown popups (Max parental rating / Auto-lock) open/scroll/confirm/cancel correctly, with a visible keyboard highlight
  **FIXED 2026-08-17**: the cursor row's background tint (25% alpha) was real but too subtle to read on a real display; added a steady white focus-border ring matching every other keyboard-focus indicator in the app
- [ ] Whole-screen auto-scroll animates smoothly, not an instant jump
  **FIXED 2026-08-17**: this was the one screen missing `animate viewport-y` — every other scrollable screen in the app already has it
### Native profile management (Manage Profiles / Edit Profile)
- [ ] Create a new sub-profile end to end (name, avatar, PIN, rating, libraries, tags, lockout, LAN bypass, device whitelist, master PIN); "+ Add Profile" correctly disappears once the real per-master cap is reached; the dialog box resizes to fit its content
  **FIXED 2026-08-17**: both were real bugs. The cap was hardcoded to `5` everywhere, but Bonfire's real cap (`BonfireProfile.max_sub_profiles`) is per-master and admin-adjustable — now read from the server instead. The box's width was a fixed 640px unrelated to tile count (5 tiles alone already exceed it, with no `clip:true` to hide the overflow) — now sized from the real content width, matching how its height already worked.
- [ ] Edit an existing sub-profile — every field pre-fills correctly except parental rating, which Bonfire itself never reports back (confirmed against its real API docs — a genuine upstream gap, not fixable client-side)
  **Improved 2026-08-17**: the field no longer silently defaults to "Any" (which looked like a confirmed value but wasn't) — it now shows an honest "Unknown" until you actually pick something
### Discover / Seerr
- [ ] Person detail from a Discover cast member: local Jellyfin Person match resolves correctly; TMDB-only fallback works when there's no match
  **FIXED 2026-08-17**: the underlying fetch/resolve logic was already working correctly (confirmed from the dev-machine log — real filmography data was landing every time); the screen was invisible because `PersonScreen` was declared *before* `RequestDetailScreen` in `main.slint`, so the request-detail overlay silently painted over it. Moved after it, matching every other overlay's "declared last = on top" ordering.
- [ ] Discover search grid: rapid typing / pagination doesn't flash or re-fade already-loaded posters
  **FIXED 2026-08-17**: two separate causes. Every fresh search rebuilt every card from scratch with no poster carried forward (fixed by carrying forward already-decoded posters by id, same as the existing filter-reapply path). Loading page 2/3/4 always swapped in a brand-new model even though every existing card was unchanged, which makes Slint destroy/recreate every element regardless (fixed with a true incremental `VecModel::extend` instead of a full rebuild).
- [ ] Toggling Watchlist (star icon / context menu) shows a confirmation toast, not just the silent state change
  Reworded from an unclear "toast mystery" note — the add/remove itself was already confirmed working; only the toast feedback was ever in question, and it's still unconfirmed either way (no new evidence this session).

### Misc UI
- [ ] Missing Seasons row shows "Upcoming · date" instead of "0 episodes" for an unaired season
  **FIXED 2026-08-17**: the previous fix gated on `episode_count == 0`, but TMDB can publish a real episode count for an announced-but-unaired season well before it airs — now checks the air date first (still-in-the-future or unknown = upcoming), falling back to episode count only when no air date is known at all

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
