# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux that plays video smoothly on NVIDIA legacy hardware using the mpv render API with real vsync feedback via `report_swap()`.

---

## Completed

| Phase | Summary |
|-------|---------|
| 1 — Foundation | Slint window, libmpv link, logging |
| 2 — Player | mpv render API, double-buffer FBO, vsync via `report_swap()`, audio passthrough, hwdec, playback reporting |
| 3 — Jellyfin API | Auth, library browse, continue watching / next up / recently added, direct-play URL, session persistence, auto-login |
| 4 — UI | Login, browse, home/movies/TV dashboards, posters, three playback modes, player controls overlay, settings screen, detail page, resume, seek bar |
| 5 — HTPC Polish | Resume freshness, server-side search, lazy library grid, full keyboard nav, context menu (`C` key + right-click), card badges, settings two-pane layout, Rust key handler with configurable bindings, disk caches (home/movies/series), `item-type` routing, canonical user state store |
| 6 — Packaging | PKGBUILD, desktop file, SVG icon, per-machine DeviceId |
| 7 — NVIDIA legacy fix | NVDEC stride diagnosis, `vf=auto` fix, expanded stats overlay |
| 8 — Code organisation | Split `main.rs`/`main.slint` into focused modules, `global AppState` singleton |
| 9 — Bug fixes & polish | Crash on background-play replacement, stop report reliability, screensaver inhibition, Up Next banner, volume overlay, intro-skip race fix, mouse hover on cards/browse, browse search mouse focus, subtitle track labels, subtitle language prefs, "Ends at" clock, settings hover, random sidebar icon, transparent SVG icons, mark-played visual update in dashboards/library |
| 10 — Code review CR1 (2026-06-18) | CR-1–10: stale intro/credits tasks, Up Next short-clip guard, report ordering, pause desync, semaphore bypass, auto-login timeout, context-menu stale state, missing SeriesId, NW timer stamp, countdown TOCTOU. CL-1–6: reset_playback_ui helper, cache_path helper, generic load/save_cache, context-menu state helper, fetch_image_cached, dead stats branch. UI-1–6: episode right-click, browse right-click, TrackPanel extract, dbl-click fullscreen, "Series →" button, seek-drag throttle + commit. |
| 11 — Code review CR3 (2026-06-20) | CR3-1–9: hidden VLH activation, stale dropdown flag, SPDIF warning with all-off formats, seek-dragging stuck on Wayland, deser_deinterlace null crash, language list duplication, header stale, default_true dedup, CLAUDE.md table errors. |
| 12 — Code review CR4 (2026-06-20) | CR4-1–10: Player::new error cleanup, JoinSet panic flush in all poster loaders, settings scroll for all sections, Up Next countdown off-by-one, movies semaphore, auto-advance window guard, mid-session 401 redirect, dropdown model dedup, dead VLH up-nav guard, .expect() in library crate. |
| 13 — EAC3 passthrough diagnosis (2026-06-20) | Full root-cause investigation of #39 (intermittent audio dropouts during EAC3 passthrough). Fix: `api.alsa.disable-tsched=true` + `session.suspend-timeout-seconds=2` in WirePlumber config. Frame-drop logging added to fjord.log at every stop and every 5 min. See Resolved Issues → #39 for full write-up. |

---

## Under Investigation

Do not implement fixes for these without HTPC reproduction data first.

- **#38 — Massive frame drops with vsync=audio (intermittent)** — sporadic large spike in dropped frames, recovered by switching vsync mode. Not reproduced since filing — may be resolved. Capture stats if it recurs.

---

## Resolved Issues

### #39 — EAC3 passthrough audio dropouts (fully resolved 2026-06-20)

**Symptom:** Intermittent 1–3 second audio silence every 3–9 minutes during EAC3 passthrough playback. AV receiver display briefly flashes its format re-detection sequence (HDMI audio stops and restarts). No video interruption. Occurred 8 times in a 44-minute session before investigation began.

---

#### Background: how EAC3 passthrough works

mpv sends a raw EAC3 bitstream wrapped in IEC61937 framing over HDMI, not PCM audio. The ALSA device runs at 192 kHz / S16_LE / stereo to carry the IEC61937 container. Each EAC3 audio block maps to a 1536-frame burst at 192 kHz, giving approximately 46 quanta per second (quantum = 4096 frames = 21.3ms). The AV receiver decodes the EAC3 from the bitstream. If even one quantum is missed — the ALSA ring buffer underruns — the IEC61937 burst is disrupted and the receiver loses EAC3 lock. It then re-detects the format when audio resumes, producing the visible silence + re-detect sequence. PCM would tolerate a glitch with only an audible pop; IEC61937 passthrough is binary — any gap breaks it.

---

#### How the root cause was identified

PipeWire daemon logging was enabled via:
```
~/.config/systemd/user/pipewire.service.d/debug.conf
[Service]
Environment=PIPEWIRE_DEBUG=3
```

At every dropout, the journal contained:
```
pw.node: (alsa_output.pci-0000_01_00.1.hdmi-surround-59) XRun!
```
followed immediately by a device close and reopen sequence. This confirmed that every audio dropout was a PipeWire ALSA RT thread xrun — not a network stall, not mpv demuxer underrun, not format negotiation.

ALSA device parameters at time of xrun:
```
hdmi:1p: format:S16_LE access:mmap-interleaved rate:192000 channels:2
         buffer frames 32768, period frames 4096, periods 8, frame_size 4 headroom 0
```

`headroom 0` means the RT thread has zero samples of slack: it must write each 4096-frame period in under 21.3ms, every time, with no tolerance for scheduler jitter. The NVIDIA legacy EGL driver creates GPU scheduling pressure during decode, and any load spike — NVDEC workload, system timer coalescing — caused the thread to miss its deadline.

---

#### WirePlumber config bugs (partial fix, did not resolve dropouts)

`~/.config/wireplumber/wireplumber.conf.d/51-disable-suspention.conf` silently did nothing due to three bugs:

| Bug | Wrong | Correct |
|-----|-------|---------|
| Property name | `session.suspend-on-idle` | `session.suspend-timeout-seconds` |
| Regex | `~alsa_output.pci-*` | `~alsa_output.*` |
| Action key | `update-properties` | `update-props` |

All three were fixed on 2026-06-20. This stopped the ALSA device from reopening mid-playback due to WirePlumber idle-suspend, which reduced the opportunity for xruns. But it did not fix the RT thread deadline misses — xruns still occurred every 3–9 minutes during playback.

---

#### Failed approach: `api.alsa.headroom = 1024`

Added `api.alsa.headroom = 1024` to the WirePlumber rule. This tells PipeWire to pre-fill 1024 frames (~5.3ms at 192 kHz) into the ALSA ring buffer as a safety margin, so the RT thread has 5.3ms of slack before a true underrun occurs.

**Result: zero xruns, zero audio dropouts — but 139–360+ video frame drops per session.**

**Why headroom caused frame drops:** `api.alsa.headroom` shifts the audio output timeline: PipeWire writes audio 5.3ms earlier relative to when it is consumed by the hardware. However, PipeWire does not fully correct the timing reference it reports back to mpv for A/V sync. Under `video-sync=display-vdrop`, mpv drops video frames when they fall behind audio. The display was running at 4K 23.976Hz (matching video exactly — a 1:1 display-to-content ratio), where zero frame drops are expected under normal conditions. The 5.3ms audio offset was just enough to make mpv believe video was consistently behind, producing hundreds of spurious drops per session.

**Do not use `api.alsa.headroom` with EAC3 passthrough on a display synced to content framerate.** It solves xruns at the cost of A/V sync accuracy.

---

#### Secondary problem discovered: pause xrun storm with `suspend-timeout-seconds = 0`

With `suspend-timeout-seconds = 0` (device stays open indefinitely), pausing mpv causes mpv to "cork" (mute) the PipeWire stream but leaves the ALSA device open and running at 192 kHz. With no data flowing, the RT thread must fill the ring buffer with silence 46 times per second. The IEC61937 ALSA device is not designed for silence fill — it generates continuous xruns at ~47 per 2 seconds while paused.

On resume, the accumulated timing drift from these xruns caused 178+ frame drops in the first 6 minutes of playback.

**Fix:** Changed `session.suspend-timeout-seconds = 2`. After 2 seconds of idle (pause or stop), WirePlumber closes the ALSA device cleanly. When playback resumes, the device reopens with a clean timeline — no accumulated xruns, no timing drift.

---

#### PIPEWIRE_DEBUG=3 caused scheduling pressure during testing

At EAC3's 46 quanta/second, info-level PipeWire logging (`PIPEWIRE_DEBUG=3`) generates hundreds of journal writes per second. This disk I/O and CPU pressure contributed additional frame drops during testing sessions when the daemon log was active. After diagnosis was complete, logging was reduced to `PIPEWIRE_DEBUG=1` (errors only).

---

#### Final fix: `api.alsa.disable-tsched = true`

Added `api.alsa.disable-tsched = true` to the WirePlumber rule. This switches the PipeWire ALSA plugin from software timer scheduling (tsched) to hardware IRQ-driven scheduling. With tsched, PipeWire uses a kernel software timer to wake the RT thread — timers can be delayed or coalesced by the scheduler under load, causing the RT thread to miss its 21.3ms deadline. With tsched disabled, the ALSA hardware interrupt drives wake-ups directly, which is deterministic regardless of system load or GPU activity.

Unlike `api.alsa.headroom`, `disable-tsched` does not shift the audio output timeline. It changes the scheduling mechanism without affecting the timing values reported to mpv. No A/V sync impact. No frame drops.

---

#### Final working configuration

**`~/.config/wireplumber/wireplumber.conf.d/51-disable-suspention.conf`:**
```
monitor.alsa.rules = [
  {
    matches = [
      { node.name = "~alsa_output.*" }
    ]
    actions = {
      update-props = {
        session.suspend-timeout-seconds = 2
        api.alsa.disable-tsched = true
      }
    }
  }
]
```

**`~/.config/systemd/user/pipewire.service.d/debug.conf`:**
```
[Service]
Environment=PIPEWIRE_DEBUG=1
```

Apply with: `systemctl --user restart wireplumber pipewire pipewire-pulse`

---

#### Confirmed test results

Two back-to-back full sessions after applying `disable-tsched=true` and `suspend-timeout-seconds=2`:
- Zero `XRun!` entries in PipeWire journal
- Zero audio dropouts observed
- Frame drops: 0–1 per session (within normal tolerance; display running at 1920×1080@119.88Hz — a perfect 5:1 ratio with 23.976fps content)

Frame-drop counts are now logged automatically to `~/.cache/fjord/fjord.log` at every stop and every 5 minutes (commit 3345896).

---

#### Summary: what to tell users

If you are running EAC3 (Dolby Digital Plus) or other bitstream passthrough over HDMI and experience intermittent audio dropouts with PipeWire, the likely cause is the software timer scheduler (tsched) causing PipeWire's RT thread to miss its deadline. The fix is:

1. Create or edit `~/.config/wireplumber/wireplumber.conf.d/` with a rule that sets `api.alsa.disable-tsched = true` for ALSA output nodes.
2. Set `session.suspend-timeout-seconds = 2` (not 0) so the device closes cleanly on pause rather than generating continuous xruns with no data.
3. Do **not** use `api.alsa.headroom` — it masks the xruns but breaks audio timing under `video-sync=display-vdrop`, causing spurious video frame drops.
4. Set `PIPEWIRE_DEBUG=1` (not 3 or higher) in production to avoid log write pressure.

---

## Open Work

### Settings — remaining steps

- [ ] **Step 3 — Playback section**: intro skipper mode (`always-ask` / `always-skip` / `never-skip`) in `Config`; `playback.rs` reads mode; toggle in Settings → Playback.
- [ ] **Step 4 — Appearance section**: accent colour selection from a small palette; layout variants if needed.
- [ ] **Step 5 — Dashboard section**: per-row visibility toggles for home/movies/TV rows; stored in `Config`.
- [ ] **Step 6 — Server section**: open Jellyfin server admin web UI (launch browser or embed WebView).

### Audio output device selector

Let the user choose which audio device mpv plays through — useful for any setup where the default device is wrong (e.g. desktop speakers selected instead of HDMI receiver).

**Config field:** `audio_device: String` (default `""` = mpv auto) in `Config`.

**Device discovery:** Shell out to `mpv --audio-device=help` at settings open time. Output is a list of `Name:` / `Description:` pairs. Parse into `(name, description)` tuples. First entry is always `auto` / "Autoselect device". Present descriptions in the dropdown; store the corresponding `Name` value in config.

**Apply:** In `start_playback`, if `audio_device` is non-empty, set mpv property `audio-device` before opening the file. If empty, leave mpv's default untouched.

**Settings UI:** Dropdown row in Settings → Audio, above the SPDIF rows. Always visible. Label: "Audio output". Device list is fetched once when the settings section is focused (or on a manual refresh). Falls back gracefully if `mpv` is not on `PATH`.

**Interaction with scheduling fix:** When a device is selected, `apply_irq_scheduling` targets only the matching PipeWire node (`node.name` == mpv device name) instead of all ALSA sinks.

**Steps:**
- [ ] Add `audio_device: String` to `Config` in `config.rs`
- [ ] Add `fetch_audio_devices() -> Vec<(String, String)>` helper (shells to `mpv --audio-device=help`, parses output)
- [ ] Add dropdown row to Settings → Audio in `settings.slint`; populate model when Audio section is focused
- [ ] Wire `on_settings_changed` for the device row in `settings.rs`
- [ ] Apply `audio-device` property in `start_playback` in `playback.rs`
- [ ] Update scheduling fix node targeting to use selected device when set
- [ ] Update CLAUDE.md Audio settings row table

### PipeWire IRQ scheduling for passthrough (in-app)

Apply `api.alsa.disable-tsched = true` automatically — only while Fjord is running and SPDIF passthrough is on — so users don't need manual WirePlumber config edits. `session.suspend-timeout-seconds` is left alone: the WirePlumber default (5 s) is already correct for most systems, and it only breaks if someone has explicitly set it to 0 themselves.

**Config field:** `alsa_irq_scheduling: bool` (default `false`) in `Config`.

**Settings UI:** Toggle row in Settings → Audio, below all SPDIF rows (after the conflict warning). Label: "IRQ audio scheduling (PipeWire)". Visible only when the master SPDIF toggle is on — xruns during PCM/resampled playback are tolerated as a brief glitch, not a full IEC61937 format re-detect.

**Implementation — `pipewire_fix.rs` (new module):**

- `apply_irq_scheduling(enable: bool)` — shells out to:
  1. Discover ALSA sink node IDs via `pw-dump --no-colors`. Parse the JSON array; keep entries where `type = "PipeWire:Interface:Node"`, `info.props["media.class"] = "Audio/Sink"`, and `info.props["api.alsa.card"]` is present. If a device is selected in `audio_device` config, target only the matching node (`node.name` == mpv device name); otherwise target all ALSA sinks.
  2. For each node ID:
     ```
     pw-cli set-param <id> Props '{ api.alsa.disable-tsched: <true|false> }'
     ```
     Restore value: `false` (PipeWire default).

- `is_pipewire_running() -> bool` — `pw-cli info` exits 0; silently skip on non-PipeWire systems.

**Lifecycle:**
- **Enable path:** toggle turned on in settings → call `apply_irq_scheduling(true)` from a Tokio task. Also call at startup if `alsa_irq_scheduling && spdif_enabled`.
- **Disable path:** toggle turned off, or SPDIF master turned off → call `apply_irq_scheduling(false)` to restore. Config value is kept so re-enabling SPDIF reactivates it automatically.
- **Exit path:** in `quit_cleanup`, if `alsa_irq_scheduling && spdif_enabled`, call `apply_irq_scheduling(false)` synchronously before the process exits.

**Constraints / known limitations:**
- `api.alsa.disable-tsched` is a creation-time ALSA parameter — takes effect the next time PipeWire opens the device. Apply before starting playback and it will be active for the session.
- If Fjord crashes, `disable-tsched` stays `true` until the next WirePlumber restart. Benign for PCM audio.
- Requires `pw-dump` and `pw-cli` (ship with `pipewire` on Arch).

**Steps:**
- [ ] Add `alsa_irq_scheduling: bool` to `Config` in `config.rs`
- [ ] Add `pipewire_fix.rs` module with `is_pipewire_running`, `apply_irq_scheduling`
- [ ] Wire apply call at startup in `main.rs` (after config load, before `window.run()`)
- [ ] Wire restore call in `quit_cleanup` in `playback.rs`
- [ ] Add toggle row to Settings → Audio in `settings.slint` (hidden when SPDIF off)
- [ ] Add row index constant and handle toggle in `settings.rs`
- [ ] Update CLAUDE.md Audio settings row table

### Phase 5 — remaining items

- [ ] **Cast member photos on detail page** — add `id` field to `CastMember`, fetch person portraits (`GET /Items/{personId}/Images/Primary`) via poster-loading pipeline, display above name/role.
- [ ] **Cast row keyboard navigation** — Left/Right through cast members on detail page; Enter opens person detail screen.

---

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
~/.cache/fjord/home.json       home row data    always refresh in background
~/.cache/fjord/movies.json     full movie list  refresh once per session on grid open
~/.cache/fjord/series.json     full series list refresh once per session on grid open
~/.cache/fjord/posters/<id>    poster bytes     permanent (never expire)
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
16 ms timer       mpv event poll, position update, intro-skip, controls idle, progress report
```

---

## Deferred / future

- **Vulkan rendering path** — second render backend alongside the current OpenGL path. Requires: Slint WGPU backend, `MpvRenderCtx` initialized with `MPV_RENDER_API_TYPE_VULKAN`, Vulkan FBO management replacing the current `gl::*` code. Enables true zero-copy decode on AMD (`hwdec=vulkan`, no CPU roundtrip). Legacy NVIDIA hardware needs OpenGL; selection persists in Config as `gpu_renderer: "opengl" | "vulkan"` and takes effect on next restart. The `gpu-api` setting was removed (2026-06-19) because it had no effect with `vo=libmpv` + OpenGL render context — this feature replaces it properly.
- Gamepad / remote control — d-pad maps to arrow keys; formal evdev/udev support deferred
- Person detail screen (depends on cast row nav above)
- Dashboard row reorder (drag-to-reorder, Phase 5 Step 5)
