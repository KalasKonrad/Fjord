# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html)
(applied loosely, since Fjord is an application, not a library with a public
API — a minor bump marks a genuine new capability pillar, a patch bump
marks further work within an already-established one; see the 0.2.x/0.3.x/
0.4.x entries below for what that looked like in practice).

Fjord didn't version its releases until 2026-07-29 — every commit before
that just carried the git-based build id (`r<commit-count>.<short-hash>`,
still logged on every startup and shown in Settings, alongside the semver
now). The versions from 0.1.0 through 0.4.2 were tagged retroactively on
that date, mapping the existing 806-commit history onto version boundaries
chosen at genuine, separately-shippable milestones (not evenly-spaced
commit counts) so this file reads like a real changelog rather than a
phase-by-phase commit dump. Going forward, new tags are cut the normal
way, at the point a version is actually released — `Cargo.toml`'s
`workspace.package.version`, a `git tag vX.Y.Z`, and a new section here
are bumped together as one step, not separately.

## [Unreleased]

- **Fixed: Bonfire sub-profile discovery never ran on a normal auto-login
  launch, making the whole feature invisible on any install that wasn't
  freshly logging in with a password every time.** Found from a real HTPC
  log after being asked "wierd i have bonfire and i have run it on the
  htpc as you can see in the log" — the log genuinely showed nothing,
  because `sync_bonfire_subprofiles` (the only thing that can discover
  additional profiles and ever trigger the profile picker) was wired into
  the fresh-login path but never into the far more common "resume an
  already-saved session" auto-login path. Fixed by adding the same call
  there. Also added logging throughout that function, which was previously
  silent on every outcome except a genuine error — the next log capture
  will show directly whether it ran and what it found.
- **New Settings toggle: mute-during-skip-fade for SPDIF passthrough can now
  be turned off independently of the video fade.** Direct request right
  after the passthrough mute shipped: "add a setting for mute during fade
  for audio passthrou so i only can turn off that and not the video fade."
  Settings → Audio → Passthrough → "Mute during skip fade" (default on,
  only shown while SPDIF is on). Turning it off leaves passthrough audio
  completely untouched during an intro/recap/preview/commercial skip —
  the video still fades to black exactly as before, and PCM audio (a
  separate, unaffected code path) still ramps regardless of this toggle.
- **Skip-fade duration is now a Settings row, and audio fades with it.**
  Direct follow-up feedback after the skip-fade feature shipped: "it was
  tiny better mabey the 200ms is to short? mabey make it a setting so i can
  test, and one other problem is the audio as i use passthrou it cant be
  faded right?" Duration is now Settings → Player → Seeking → "Skip fade
  duration" (Off/100/150/200/300/400/500/750/1000ms, default 200 unchanged),
  read live by both the Rust-side wait and the Slint-side animation so a
  change takes effect on the very next skip. Audio now fades alongside the
  video instead of cutting abruptly mid-black-screen: PCM gets a real
  volume ramp down-then-back-up (mpv can do this cleanly); SPDIF passthrough
  gets a mute held for the whole transition instead, since a raw compressed
  bitstream can't have its volume touched without corrupting the encoded
  frames — the same reason Volume Up/Down already skip themselves during
  passthrough. The passthrough mute specifically is untested against real
  AVR relock behavior (a possible click/delay when the receiver has to
  redetect the stream) — worth trying live and reverting to "Off (instant)"
  if it sounds worse than the plain hard cut it replaces. Not live-tested
  yet.
- **Video-only-audio black screen on first playback — root cause found and
  fixed.** A real HTPC report: "i statred the video but it only playde the
  audio, no video, and no video stats, untill i stoped and started it
  agian." mpv's own internal log (captured since an earlier diagnostics
  pass added it, but only actually caught firing this time) showed the real
  cause directly: mpv's `vo=libmpv` video driver can try to initialize
  video output before Fjord has created the OpenGL render context it needs
  to render into — a genuine race between mpv's own decode-driven timing
  and Slint's GL-thread frame scheduling, two things that were never
  ordered relative to each other. Once that VO-init fails, mpv never
  retries it for the rest of that instance's life ("No render context
  set."), while audio — a separate, unaffected pipeline — keeps playing
  normally, exactly matching the reported symptom. Fixed at the root:
  building the mpv core and issuing the actual "load this video" command
  are now two separate, explicit steps, with the second one deliberately
  held back until Fjord's render context has actually been created and
  attached — on the same thread, in the same tick, so the ordering is now
  guaranteed rather than a coin flip. Not live-tested yet — needs a fresh
  HTPC launch to confirm the very first playback shows video immediately.
  See CLAUDE.md's "NVIDIA HTPC: video-only-audio black screen" section for
  the full trace.
- **A second false "connection lost" — pausing for a while and resuming
  falsely triggered the stall-recovery reload.** Same live HTPC test as
  above, same underlying mechanism as the backward-seek regression fixed
  the day before (below): the stall watchdog's "last confirmed progress"
  checkpoint doesn't advance while paused (position genuinely isn't
  moving), but nothing was keeping it *fresh* either — so a long pause let
  the checkpoint's clock quietly age the whole time, and the very next tick
  after resuming read that entire pause duration as "no progress," well
  past the 5-second stall threshold. Confirmed live: paused ~3 minutes,
  resumed, 26ms later the stream reloaded from scratch — visible as a
  playback reset and audio renegotiating a few times before it settled.
  Fixed the same way the seek case was: a pause (or a genuine buffering
  wait) now keeps the checkpoint continuously refreshed, so resuming always
  starts the clock at zero instead of wherever it was left. Not
  live-tested yet — needs an extended pause/resume on the HTPC to confirm.
- **Intro/recap/preview/commercial skips now fade to black instead of
  cutting instantly.** Direct request: "make intro skip etc more gradual,
  it feels instant and jarring." The underlying mpv seek is still a hard
  cut — there's no way to smoothly scrub between two arbitrary points in a
  video file — but all three skip paths (automatic always-skip, ask-timed
  countdown expiry, and the manual "Skip →" confirm button) now dip the
  screen to black first, perform the seek while hidden, then fade back in
  on the new frame, rather than showing the jump directly. The 200ms fade
  duration is multiplied by the same global animation-speed setting
  (Settings → UI) every other transition in the app already respects, on
  both the Rust-side wait and the Slint-side animation — so a user who's
  set that to 0% gets back exactly today's instant seek, with no separate
  new setting needed to opt out. Deliberately scoped to same-video segment
  skips only; the credits-triggered jump to the *next episode* is a
  heavier operation (a full player reload for a different item) and was
  left as a possible separate follow-up rather than folded in here. Not
  live-tested yet.
- **Real regression in the network-outage stall recovery, found live the day
  after it shipped: seeking backward falsely triggered "connection lost."**
  Root-caused from a real HTPC log rather than guessed at. The stall
  detector's "has position genuinely advanced" check only ever looked
  forward — a backward seek left it comparing the new (lower) position
  against a stale, higher pre-seek checkpoint, so several seconds of
  completely normal post-seek playback looked identical to a real stall.
  Confirmed directly from the log: a seek to 1228.4s, then 4.65s later
  "stalled: 5.0s with no progress at 1232.57s" — position had genuinely
  advanced ~4.2s in that window, ordinary 1x playback, not a stall.
  Compounded by the per-item reload-attempt cap being effectively
  permanent once exhausted (by design, to stop an infinite retry loop
  against a truly dead server) — two false positives early in a video
  disabled real stall recovery for the rest of it, which is exactly what
  made the bug feel like it kept getting worse the more the user tried to
  seek. Fixed with two changes: a direction-agnostic jump detector treats
  any single-tick position jump of 2+ seconds (a seek, a chapter jump, a
  gapless track transition — anything a real 16ms tick of normal playback
  could never produce on its own) as "reset the baseline here," not
  evidence of a stall; and the reload-attempt cap now forgives itself
  after 120 seconds of sustained healthy playback on the same item, so an
  early resolved hiccup (real or false) doesn't permanently disable
  recovery for a later, unrelated one. Not live-tested yet — needs a real
  seek-heavy session on the HTPC to confirm. See CLAUDE.md's "Playback
  resilience: network outages" section for the full trace.
- **Bonfire Phase 2: native profile create/edit/delete.** Two new screens —
  `ManageProfilesScreen` (Settings → Profiles → "Manage Profiles", master
  accounts only — a Bonfire sub-profile can't manage siblings) and
  `ProfileEditScreen` (create or edit one sub-profile: name, avatar color,
  PIN, max parental rating, enabled libraries, blocked/allowed tags,
  auto-lock timeout, LAN PIN-bypass, device whitelist, plus the household
  master's own confirmation PIN when one is set). Scoped via a direct
  question before writing any code: Fjord's on-screen keyboard is numeric-
  only today (the full alphanumeric layout is still unstarted Phase 3), so
  text fields need a physical keyboard — same `LineEdit`-based shape
  LoginScreen/ConnectSeerrScreen already use, rather than pulling Phase 3's
  work forward early. That same reasoning extended to the whole screen:
  `ProfileEditScreen` is mouse + physical-keyboard driven throughout, not
  this app's usual D-pad dispatch system — confirmed first that
  `SettingsDropdown`/`VirtualKeyboard` both already work standalone via
  mouse alone, with no dependency on Settings' own keyboard machinery, so
  reusing them here needed no new plumbing. One real, accepted trade-off:
  the library/device checklists and the avatar-color swatches aren't
  D-pad-navigable on this one screen (everywhere else in the app, they
  would be). A real API gap surfaced while designing the edit-mode pre-fill:
  the profile list response has no parental-rating field at all, even
  though creating/updating a profile both accept one — the dropdown always
  starts blank in Edit mode, which is a UX gap (can't show what's currently
  set) but not a correctness bug, since leaving it untouched correctly
  omits it from the save request rather than clobbering the existing value.
  Not live-tested — needs a real master account + Bonfire-enabled server,
  unavailable in this sandboxed environment. See CLAUDE.md's Bonfire
  integration section for the full design writeup.
- **Bonfire Phase 1, step 8: the async-result session-guard audit — the
  last item of Phase 1's original scope, so this closes out the phase.**
  Dispatched a systematic audit across every "open a screen → fetch → commit
  into AppState" flow in Detail/Series/Season/Collection/Album/Artist/
  Person/Discover/poster-loading/prewarm, checking each against a genuine
  new risk: a Bonfire profile switch (or sign-out) mid-fetch. Real findings,
  not theoretical ones. `reset_session_state` closed every screen's
  `show_X` flag but never cleared the matching focused-id property, so a
  stale fetch could silently reopen a just-closed screen with the OUTGOING
  session's data — fixed structurally, once, for all 7 screens at the one
  shared teardown point. It also never closed 3 Discover overlay screens at
  all — fixed. Four screens (`collection.rs`/`album.rs`/`artist.rs`/
  `person.rs`) had **zero** session guard anywhere on their main open path;
  three more (`series.rs`/`season.rs`/`detail.rs`) had one only at their
  first network call, not their actual final commit, several async hops
  later — all seven now guarded correctly. A new `seerr_session_current`
  (Discover holds a different client type) was added and applied to
  `ensure_discover_landing` — the single most guard-less site found in the
  whole audit, no staleness check of any kind — and `open_discover_item_ex`.
  One site (the watchlist-row fetch chain) turned out to have already lost
  its `Arc` identity several calls upstream by the time it reaches the
  commit point; rather than force a wrong fix or a disproportionate
  re-plumb of a currently-working chain, it got an honest, coarser
  "still connected at all" check instead, with the residual gap (a switch
  to a DIFFERENT Seerr-connected profile) explicitly documented rather than
  silently left unaddressed. `poster.rs` was the highest-stakes and widest
  fix — Home dashboard rows and the TV library list are genuinely
  Jellyfin-restricted per-profile content, and had no staleness guard of
  any kind before this; the fix's own parameter threading fanned out to 18
  total call sites across 7 files, all found and fixed via the same
  "compiler as checklist" technique this project has used before.
  `prewarm.rs`'s image-prewarm sweep got the same guard shape its sibling
  metadata sweep already had (an inconsistency within the same file). Not
  live-tested — needs a real second profile and a real Bonfire-enabled
  server, neither available in this sandboxed environment. See CLAUDE.md's
  Bonfire integration section for the full per-file breakdown.
- **Bonfire Phase 1, step 7: the launch-policy Settings row.** Two new rows
  in Settings → Profiles: "Launch behavior" (Always Ask / Remember Last /
  Default Profile — a plain static dropdown) and a virtual "Default
  Profile" row, shown only when Launch behavior is set to Default Profile,
  whose option list is genuinely dynamic — whichever profiles are actually
  known on this device, the same shape the audio-device/font-family rows
  already use for the identical reason. Both `DeviceConfig` fields this
  drives (`launch_policy`/`default_profile_id`) have existed since step 1
  and were already read by `should_show_picker_at_startup`; this step is
  purely the UI to actually change them, nothing about the startup gate
  logic itself changed. The dynamic list is kept current from two places —
  app startup and every session-start path (login/switch/Add Account) — so
  a just-added profile shows up in the picker without needing a restart.
  Selecting a default profile is a 100% local write, no network round trip.
  Not live-tested. This closes out everything from Phase 1's original scope
  except the async-result session-guard audit.
- **Bonfire Phase 1, step 6: `ProfilePickerScreen` + full profile-switch
  flow, built full-scope per explicit user direction ("full scope now")
  rather than deferred to a minimal local-only version.** New
  `widgets.slint::VirtualKeyboard` (numeric layout, D-pad navigable — the
  alphanumeric layout is still Phase 3), `theme.slint::ProfileTile`, and
  `profile_picker.slint` (avatar-tile row + "+ Add Account" tile + a
  PIN-entry sub-panel with masked-dot display). Picker input is handled by
  a new raw-key dispatch tier in `keys.rs` at the same level as the
  existing `show-login` check — no new `AppMode` variant needed, since it
  runs before `active_mode()` is ever consulted. `ProfileSettings` gained
  identity fields (`display_name`/`avatar_color`/`avatar_initial`/
  `is_bonfire`/`master_user_id`/`has_pin`). `do_login` gained an `append:
  bool` — the picker's "Add Account" tile signs into a brand-new profile
  alongside every existing one instead of overwriting whichever is
  currently active. `finish_session_setup` was extracted verbatim from
  `do_login`'s old tail (fetch home/series/system-info/plugins, persist
  state, start the websocket, spawn poster loading) so a fresh password
  login and a token-based profile switch share one setup path and can't
  drift apart. New `profile.rs`: `should_show_picker_at_startup` (0 or 1
  known profile → always `false`, so every existing single-profile install
  is completely unaffected; 2+ → depends on the launch policy), and
  `switch_to_profile`, which resolves a valid token for the target profile
  — via `bonfire_switch_profile` on that profile's own master client for a
  Bonfire sub-profile, or the stored token re-validated through
  `check_auth()` for a plain account — **before** ever tearing down the
  currently-active session, so a failed switch leaves the current session
  untouched rather than half-torn-down. `sync_bonfire_subprofiles` runs
  fire-and-forget after every successful session setup, session-guarded via
  the established `Arc::ptr_eq` idiom, and add-only upserts any Bonfire
  sub-profiles it finds into `Config.profiles` (no pruning of removed ones
  yet — deliberately deferred, not an oversight). The startup flow in
  `main.rs` now checks `should_show_picker_at_startup` before running the
  pre-existing auto-login sequence, which is otherwise byte-for-byte
  unchanged when the check is `false`. See CLAUDE.md's Bonfire integration
  section for the full design writeup. Not live-tested — no part of this
  step has been exercised against a real Bonfire-enabled server or a real
  second profile yet.
- **Bonfire Phase 1, step 5: the Bonfire/JellyProfiles `fjord-api` module.**
  22 new `JellyfinClient` methods + 13 model structs covering the whole
  documented `/plugins/profiles/*` REST surface — profiles (list/switch/
  verify-pin/create/update/delete), libraries, devices, the cross-household
  Bonfire group, per-user preferences, and admin (mappings/reset-pin/
  profile-limit/audit-logs). Every field name verified directly against the
  plugin's own real API reference doc (fetched live, not carried over from
  an earlier planning session's own summary of it) — which also surfaced a
  real, previously-unknown `preferences` endpoint, now included. Not
  consumed by any UI yet, and not live-tested against a real server — pure
  groundwork so the profile-picker/switching work later in this phase
  doesn't need new crate-level work alongside it.
- **Bonfire Phase 1, step 4: shared plugin-availability registry.**
  `FjordState.available_plugins` now tracks every plugin name installed on
  the server, fetched once per login via a new `get_plugins()` (`GET
  /Plugins`, best-effort — never fails the login itself). Not consumed by
  anything yet; groundwork for Bonfire's own gate and, later, replacing
  Intro Skipper's current per-episode-404 detection with an upfront check.
- **Bonfire Phase 1, step 3: extracted `reset_session_state`.** The shared
  half of sign-out's teardown (stop playback, abort the websocket, clear
  every in-memory list/cache, force-close every content-bearing screen) is
  now a standalone function, verified against the real current sign-out
  handler rather than an earlier speculative draft — so a later profile-
  switch commit can reuse it instead of re-deriving the same ~150 lines
  and risking drift. Pure extraction, zero behavior change.
- **Bonfire Phase 1, step 2: on-disk caches namespaced per profile.** The
  seven flat library/home caches (`movies.json`, `series.json`, ...) and
  `screen_caches.json` now live under `~/.cache/fjord/profiles/<user_id>/`
  instead of a flat `~/.cache/fjord/`, so a second profile signing in on
  the same install won't briefly see the first profile's library/watch
  state at warm start. Every load/save function takes an explicit
  `user_id` — deliberately not resolved internally, since the data being
  cached is often fetched well before the save call runs, an async gap a
  profile switch could cross. A cheap, idempotent migration moves an
  existing install's flat cache files into place on first launch, so this
  doesn't cost anyone their instant warm start. Poster/backdrop caches are
  untouched — server-global artwork, not per-user.
- **Playback resilience: a real network outage on the HTPC caused a false
  end-of-file that auto-advanced to the wrong episode — root-caused from a
  shared `fjord.log`, fixed at four independent, compounding layers.**
  (1) The "Cache (MB)" Settings row never actually raised the real buffer —
  it only ever adjusted mpv's `cache-secs` (a time cap that's effectively
  unlimited by default) via an arbitrary conversion, never
  `demuxer-max-bytes` (the real byte ceiling that actually governs how much
  of an outage can be silently absorbed). Replaced with two honest rows,
  "Cache duration (seconds)" and "Max cache size (MiB)", each setting its
  own real mpv option. (2) mpv's own automatic HTTP reconnect had no
  explicit tuning at all; now set unconditionally via `stream-lavf-o`.
  (3) The stall-recovery watchdog (Phase 85) only ever caught a stall by
  coincidence — its fixed start-position check happened to work for this
  incident only because mpv's demuxer reset its position readout toward 0;
  generalized to a rolling "no progress in 5s" check that catches a stall
  anywhere in the video. (4) Recovery changed from a same-connection
  `seek_backward` (confirmed live to be exactly what produced the false EOF)
  to a full stream reload at the last known-good position, capped at 2
  attempts. A duration guard (`premature`) now permanently prevents ANY EOF
  landing far short of the real duration from being treated as a natural
  end — no mark-played, no advance to next episode/track — regardless of
  cause, as a backstop independent of the other three fixes. A new
  "Reconnecting…" overlay shows during recovery, distinct from the
  pre-existing buffering spinner, which stayed silent throughout the real
  incident since mpv was actively erroring, not calmly cache-waiting. See
  CLAUDE.md's "Playback resilience: network outages" section for the full
  incident timeline and reasoning.
- **Same-day follow-ups to the above, from direct questions rather than bug
  reports.** The stats overlay's CACHE row only ever showed
  `cache-buffering-state`, which the real mpv manual defines as "% until
  the player will unpause" (governed by a 1-second default wait), not "how
  full is the real buffer" — it reads ~100% almost immediately regardless
  of cache size, which is exactly why it looked useless. Now also shows
  the actual buffered seconds (`demuxer-cache-duration`) when available:
  `"100%  ·  42.3s buffered"`. "Max cache size (MiB)" gained an
  "Unlimited" choice — its old `0` meant "leave mpv's own 150 MiB stock
  default alone," the most *restrictive* value on the row, not unlimited;
  fixed by raising `demuxer-max-bytes` to a large fixed ceiling instead
  when chosen, so "Cache duration (seconds)" genuinely becomes the only
  real constraint. And confirmed directly from the manual: a bigger cache
  does not delay playback start — `--cache-pause-initial` (pre-buffer
  before starting) defaults to off and Fjord never sets it; the cache
  fills passively in the background once playback has already begun.
- **Bonfire Phase 1, step 1: `Config` restructured into `DeviceConfig` +
  `Vec<ProfileSettings>`.** The isolated, zero-behavior-change commit the
  Bonfire integration plan calls for first, ahead of any profile-switching
  UI. Every device-scoped setting (hwdec, audio device, seek step,
  animation speed, log level...) now lives on `Config.device`; everything
  that should follow the signed-in person instead (auth, subtitle/audio
  language, library sort, Seerr connection, Discover filters, skip
  modes...) lives on one entry of `Config.profiles`, accessed only via
  `Config::active()`/`active_mut()`. v1 still only ever has exactly one
  profile — no picker or switching UI yet — so this changes nothing about
  how the app behaves today; it's purely the foundation the rest of Phase 1
  builds on. An existing flat `config.json` migrates forward automatically
  on first load and re-saves once in the new shape. Verified against a
  scratchpad copy of a real on-disk `config.json` before being trusted, not
  just a hand-written test fixture.
- **Fixed (for real this time — the 5th report, root cause confirmed
  directly from Slint's own source): Reset to Defaults still only showed
  its top edge.** Attempt 4 computed a mathematically-correct scroll
  target from the button's real geometry, but the button was STILL cut
  off — turns out `Flickable` silently re-clamps any plain (unbound)
  `viewport-y` assignment back into `[height - viewport-height, 0]`
  whenever it's judged out of bounds (confirmed by reading
  `i-slint-core`'s `Flickable::init`'s `in_bound_change_handler` directly,
  not assumed), the same safety net that keeps a drag/flick gesture from
  scrolling past the content. Since `viewport-height` was bound to a value
  that could under-report the section's true height, my precise target
  kept getting silently overridden back to a shorter one. Fixed by having
  the Key Bindings section push its own real measured height up to the
  Flickable's `viewport-height` binding whenever focus changes, so the
  clamp bound itself can never be smaller than reality.
- **Fixed: the subtitle/audio language dropdowns' empty-value label read
  "Any," but the row's own description already said "use video default" —
  and a user asked directly whether that mismatch should be resolved.**
  Confirmed against the real fallback logic (`playback.rs`'s track
  auto-select, documented in this file's own Subtitle auto-select section)
  that an empty preference really does mean "fall through to the video
  container's own default track," not "no filtering" (the correct meaning
  for the Subtitle Type row, which was left as "Any"). Renamed to
  "Default" for Preferred Audio Language, Primary Subtitle Language, and
  Fallback Subtitle Language; the Fallback row's subtitle also now spells
  out the two-step fallback (primary → fallback → video default) explicitly
  rather than leaving the last step implicit.
- **Code review of Phase 0 (the Settings int→string rewrite, all of it —
  the base rewrite plus every live-testing fixup commit on top), 10
  findings fixed.** Requested directly ("we shuld fix everything") after a
  multi-angle automated review surfaced them; each was independently
  traced/verified against the real code before being fixed, not applied on
  faith. Most severe first:
  - **Mouse-driven navigation never cleared `keybinding-focused`.**
    Clicking a different Settings section (or leaving Settings entirely for
    a different sidebar tab) with the mouse left `keybinding-focused`
    wherever keyboard navigation had last set it — and `keys.rs`'s
    `AppMode::Settings` routing checks `keybinding-focused >= 0` *before*
    ever looking at which section is actually shown, so every subsequent
    keypress kept being silently hijacked by the (invisible) keybinding
    dispatcher. Worse: pressing Enter in that state armed rebind-capture,
    and the very next keypress anywhere rebound and persisted an arbitrary
    action with zero visible feedback. Fixed at both the in-Settings
    section-click handler (`settings.slint`) and the broader "leaving
    Settings for a different sidebar tab" case, the latter piggybacked onto
    `discover.rs`'s `on_nav_selected` handler — which turned out to be the
    ONLY one that actually fires (see the next fix).
  - **`AppState.nav-selected`'s callback was registered twice, and the
    second registration silently discarded the first.** Slint callbacks
    are single-handler; `browse::wire_browse` registered a handler to clear
    browse results on nav change, and `discover::wire_discover` (wired
    later in `main.rs`) registered its own — completely replacing browse's,
    which had been dead code ever since. Fixed by extracting browse's logic
    into `browse::clear_browse_results` and calling it explicitly from
    discover's surviving handler, which is also now the one place that
    resets the keybinding/confirm-dialog state above on any sidebar switch.
  - **The rebind-collision confirmation dialog — a feature added this same
    session specifically because the user asked for "block and require
    confirmation" — silently didn't fire for several real, bound actions.**
    It looked the colliding action up in the Settings screen's own row list
    to get a display label, and treated a miss as "no collision" instead of
    "collision, unknown label" — so rebinding onto `q` (Queue Panel), Delete
    (Delete Item), `l` (Lyrics), `m` (Now Playing), or any digit
    (seek-to-%), none of which have a settings row, silently stole the
    binding with no dialog at all. Fixed with a Debug-formatted fallback
    label for actions with no row, so a collision is always caught.
  - **Clicking a settings row while the keyboard-driven dropdown popup was
    open didn't close it — the popup has no backdrop, so background rows
    stayed clickable — and the next Confirm applied the newly-clicked row's
    value using the OLD popup's stale cursor position.** Concrete case:
    open the Hardware Decode dropdown, click the Deinterlace row instead,
    press Enter — Deinterlace gets set to whatever HWDEC's list had at that
    cursor position. Worse on a dynamic row (Audio Output): applies an
    arbitrary real device. Fixed by closing the popup on any row click.
  - **`Confirm`/`Right` acted on `settings-focused` with no check that the
    row was actually still visible** — unlike `Up`/`Down`, which already
    look it up and self-heal on a miss. A mouse interaction that changes a
    value and hides the currently-focused row (without going through the
    row-focus callback — e.g. clicking a `ToggleSwitch`/`SettingsDropdown`
    control directly, whose own `TouchArea` sits above the row's) left
    `settings-focused` pointing at a row no longer on screen; the next
    Enter/Right could still open a dropdown for it or silently mutate it.
    Fixed with the same self-heal Up/Down already have.
  - **The Reset-to-Defaults and rebind-collision confirm dialogs, plus a
    pending rebind, could survive leaving Settings via the mouse or signing
    out.** `SettingsScreen` unmounts instantly (no fade) on a sidebar
    click, and neither dialog's flags nor `FjordState.pending_keybind_rebind`
    were among the ~10 adjacent transient UI flags sign-out already clears.
    Fixed at both points — the sidebar-switch handler above, and sign-out.
  - **The Caps-Lock/case migration (previous session) could silently drop
    a binding via a genuine hash collision, not just a theoretical one** —
    verified with an actual compiled test harness extracting the real
    code: a pre-existing bare-uppercase legacy default (e.g. `"Z"`) and a
    user's own explicit `"shift+Z"` rebind both migrate to the identical
    `{key:"z", shift:true}` combo, and a plain derived `HashMap` deserialize
    just lets the later one in the file silently win with no trace.
    Replaced the derived `Deserialize` for `KeyMap` with a custom one that
    detects this and resolves it deliberately (preferring the
    explicitly-prefixed form, which carries real intent, over the bare
    legacy encoding) with a `warn!` logged either way, rather than a silent
    coin-flip.
  - **The same migration's shift-reconstruction also misfired on
    ctrl/alt-prefixed uppercase**, where it's provably wrong: a genuine old
    `"ctrl+Z"` (Ctrl+z with Caps Lock on, Shift not held — the old `Display`
    always wrote an explicit `"shift+"` prefix whenever Shift really was
    held) got reinterpreted as Ctrl+Shift+z. Narrow — no shipped default
    ever produces this string — but the heuristic was applied wider than
    its own justification. Restricted to bare letters with no modifier.
  - **Audio-device duplicate-description disambiguation (previous session)
    only fixed cross-backend collisions**, not two devices sharing the
    SAME backend and description (a real, plausible case: two identical USB
    interfaces both enumerating as e.g. "HD-Audio Generic/USB Stream
    Output" under `alsa`) — reproducing the original unselectable-second-
    entry bug in a narrower case. Added a second disambiguation pass
    falling back to the device's raw mpv name (guaranteed unique) for
    anything still colliding after the backend suffix.
  - **The empty/"none selected" value for the three language dropdowns
    (Audio Language, Subtitle Language ×2) showed "Any" via the
    keyboard-driven popup but "Off" via the mouse-driven inline dropdown**
    — same stored value, different label depending on input method, a
    direct symptom of this rewrite duplicating every dropdown's option list
    across two independently-maintained places. Unified on "Any" (matching
    the Subtitle Type row's own existing precedent).
  - Several file header/TOC comments (`keys.rs`, `config.rs`, `main.rs`,
    `app_state.slint`) weren't updated for symbols and behaviour this same
    diff added, violating this project's own stated Style convention; two
    stale literal `-1` sentinel references (a code comment, and this file's
    own Settings-navigation section) survived from before the property
    became string-typed. Both cleaned up.
  - Widened `kb-row-y`'s one genuinely-approximate term (the hint text
    block's height, which wraps and has no fixed size) after confirming
    the two `SectionHeader` terms and the per-row stride are already exact
    (`SectionHeader` has an explicit fixed `height: 28px`; each row is a
    fixed 44px `Rectangle`) — not a confirmed bug, just tightening the one
    remaining margin for error on the same reasoning that caused the
    Reset-button saga above.

- **Fixed: Reset to Defaults still only showed its top edge, cut off at
  the bottom — the 4th attempt at this bug, and the first to fix the
  actual root cause.** Every prior attempt (a row-offset formula, then a
  `right-inner.preferred-height`-based "exact bottom" formula, then a
  ChangeTracker-coincidental-value-equality fix) was still an ESTIMATE of
  where the button sits — and the estimate was consistently short by
  close to the button's own height, exactly the "only the top is visible"
  symptom. Replaced the estimate with a direct read of the button's own
  real, Slint-resolved layout position (`reset-btn.y`/`reset-btn.height`)
  computed from inside the section where that element actually exists —
  not an approximation at all, since Slint's layout engine has already
  resolved it precisely by the time it's read. This one mechanism is now
  the sole writer of the scroll position for the Reset row specifically
  (the general per-row/per-section scroll logic explicitly excludes that
  one transition, rather than both racing to write conflicting estimates).
- **Fixed (superseded by the above, kept for history): Reset to Defaults'
  scroll-into-view was unreliable due to a ChangeTracker missing a real
  navigation step.** The Flickable's `viewport-y` is only ever assigned
  from a `changed kb-y => {...}` ChangeTracker (a deliberate pattern so
  native mouse-wheel scroll keeps working — see the Slint gotchas
  section), which only fires when `kb-y`'s own computed VALUE actually
  changes between two evaluations. But `kb-y`'s own `clamp(...)` means
  many different focus positions near the bottom of a long list resolve
  to the identical pixel value, so navigating between two such positions
  produced no value CHANGE and the tracker silently never re-fired.
  Mirroring the raw navigation drivers instead of the derived, clamped
  value fixed that specific gap, but the underlying scroll TARGET was
  still an estimate and still fell short — see the entry above.
- **Fixed: loading an existing keybindings.json could silently drop a
  binding.** The Caps-Lock/case-normalization fix below lower-cases
  every key on load; harmless for actions that had both letter cases
  bound to the same thing, but for z/Z (sub-delay) and x/X
  (audio-delay) — genuinely different actions per case — it collided
  them, and whichever one HashMap deserialization happened to keep
  last silently dropped the other (showed as unbound in the Key
  Bindings screen). Fixed by reconstructing the original intent on
  load instead of colliding it.
- **Fixed: Reset to Defaults' scroll-into-view still wasn't reliable**
  after the first attempt at fixing it — now pinned to the exact
  bottom of the content instead of estimating a row offset for it,
  since it's provably the last thing in the list either way.
- **Fixed: the Reset/rebind-collision confirm dialog's buttons rendered
  outside the box** — a real layout bug in the shared `ConfirmDialog`
  component, not specific to either dialog using it.
- **Fixed: the Key Bindings "Reset to Defaults" button never scrolled
  into view** — the scroll-position math assumed a uniform row layout
  that hasn't been true since the "Enter rebinds…" hint text and the
  "PLAYER" section header were added.
- **Fixed: Video → Video filter (vf) showed even when it can't do
  anything** — it exists solely to fix NVDEC's own stride-corruption
  bug, so it's now hidden unless hardware decode is set to `nvdec` or
  `nvdec-copy`.
- **Corrected the OpenGL early flush / Video latency hacks subtitles**
  to state mpv's own real caveats (verified against its actual source/
  docs) instead of implying they're safe to leave on speculatively —
  `video-latency-hacks` in particular is documented by mpv itself as
  breaking interpolation and "not recommended" in general, regardless
  of GPU vendor.
- **Fixed: key bindings cared about letter case and Caps Lock.** Typing "n"
  with Caps Lock on registered as a different key than "n" with it off —
  the existing defaults worked around this by hand-registering both
  cases of nearly every letter, which is also why the Key Bindings
  screen showed doubled-up labels like "F  f" for those rows. Capture
  now normalizes on the physical Shift key state alone, independent of
  Caps Lock; existing `keybindings.json` files self-heal on next load.
  z/Z (sub-delay) and x/X (audio-delay) — genuinely different actions on
  Shift, not a case duplicate — are unaffected, now expressed correctly
  as an explicit Shift binding instead of relying on old fallback
  behavior that happened to work but wasn't really correct.
- **Fixed: rebinding a key never actually exited "waiting for a
  keypress" mode.** After successfully rebinding an action, the very
  next keypress — even just pressing Down to move to another row — was
  silently consumed as another rebind attempt, overwriting what was just
  set. Likely a real contributor to the Key Bindings screen feeling
  broken in general.
- **Fixed: rebinding a key already used by a different action silently
  stole it, with no warning.** Now shows a confirm dialog ("`Q` is
  already bound to Open Queue Panel — reassign it?", defaulting to
  Cancel) instead of overwriting anything automatically.
- **Settings screen rewritten to be fully data-driven (Phase 0 of the
  Bonfire/JellyProfiles multi-profile integration plan)** — every section
  and row now has a stable string key ("video.hwdec", "general", ...)
  instead of a positional int, so a section or row can be inserted
  anywhere without renumbering everything that follows it by hand. Added
  a new **Profiles** section (between General and Video) and moved Sign
  Out into it — General is now purely device/app-behavior settings; Phase
  1 of the Bonfire plan populates Profiles with actual profile switching.
  Internally, ~15 hand-duplicated "is this row hidden" conditions spread
  across the keyboard Up/Down handlers collapsed into one list-builder
  function per section (`section_row_keys`). No user-visible behavior
  change intended — see CLAUDE.md's Settings section for the full design;
  not yet live-tested (standard limitation for UI changes made in this
  sandboxed dev environment — this one is worth an especially careful
  pass given how large the rewrite is). Debug logging added ahead of the
  first live pass, since none of this — nav, focus, dropdown open/select
  — otherwise leaves any trace in the log; set `Settings → General → Log
  level` to Debug before testing.
- **First live test of the above (2026-08-07): a full keyboard walkthrough
  of every Settings section, traced against the debug log** — zero
  navigation/dispatch bugs from the rewrite itself, no warnings/errors/
  panics anywhere. One real bug found, confirmed pre-existing (the code
  path is untouched by Phase 0): **selecting the 4th entry in the Audio
  output or Passthrough output dropdown silently didn't stick.** Root
  cause: a device can be listed under more than one backend with an
  identical description (e.g. this dev machine's USB audio interface
  shows up as both `pipewire/...` and `pulse/...`, both described "UAC-2
  Digital Stereo (IEC958)") — selection round-trips purely through that
  description string, so the second entry was indistinguishable from the
  first at every step. Fixed by suffixing colliding descriptions with
  their backend (`... [pipewire]` / `... [pulse]`) so every entry is
  unique; purely a display-string fix, nothing stored on disk changes.
- **Debug logging substantially expanded, per request** — Key Bindings'
  own row navigation (`dispatch_keybinding_nav`), the rebind-capture flow,
  and the actual reset-to-defaults action were all previously silent in
  the log; all now log their transitions.
- **Fixed: the Key Bindings "Reset to Defaults" button gave no visual
  feedback when focused or pressed**, and fired immediately with no way
  to back out of an accidental press. Now shows the same focus/press
  styling every other button in the app has, and opens a confirm dialog
  (defaulting to Cancel) before actually resetting anything — reachable
  the same way whether triggered by keyboard or mouse. Also added a short
  on-screen hint explaining that Enter starts capturing the next keypress
  as a new binding (previously undocumented anywhere in the UI).
- **Added Seerr Blocklist support** — mark a movie/show as unrequestable
  on your Seerr server, for things you don't want to show up or don't
  want to collect. Add/remove from the Discover context menu and the
  request-detail screen's own button; bulk-blocklist a whole collection
  from the Collection screen (with a confirmation dialog, since it
  affects every movie in the franchise at once); browse and remove
  everything currently blocklisted from a new Settings → Integrations →
  Manage Blocklist screen. Requires the `MANAGE_BLOCKLIST` permission on
  your Seerr account, separate from admin/request-management rights.
- **Fixed: blocklisting an item only marked its card "Blocklisted" instead
  of making it disappear from Discover**, defeating the whole point of the
  feature. Blocklisted items are now filtered out of every Discover fetch
  at the source (landing rows, search, filtered browse) and removed
  immediately from whatever's already on screen when you block them.
- **Fixed: a blocklisted item that's also on your Watchlist kept
  resurfacing on the Movies/TV dashboard's own Watchlist row and the
  Coming Up rows**, since blocklisting a title doesn't remove it from the
  actual Seerr Watchlist and those specific rows were never covered by the
  fix above. Now filtered out of the Watchlist row and Coming Up
  everywhere they appear, same as the rest of Discover.
- **Fixed: the "Unwatched Collections" row could keep showing a fully-
  watched collection indefinitely.** Its removal filter compared a
  BoxSet card's own id against the just-watched movie's id, which can
  never match — finishing the last unwatched movie in a collection now
  correctly wakes the row's background refresh instead of relying on an
  unrelated favorite/resume change to coincidentally trigger it first.
- **A still-airing series stays on the watchlist even once fully caught
  up**, instead of being removed the moment its last aired episode is
  watched — you don't know if another season is coming. It's only
  removed once Jellyfin reports the show as no longer "Continuing" (and
  it's still fully watched at that point).
- **Adding an already-watched item to the watchlist now marks it unwatched**
  instead of just sitting there — re-adding it is read as "I want to watch
  this again." Resets played state via a real Jellyfin API call (not just
  a local flag), with a "Added to Watchlist — marked unwatched" toast when
  it happens.
- **Watched items are auto-removed from the Seerr watchlist.** Hooked into
  the WebSocket `UserDataChanged` handler (fires for a played-state change
  from any source — Fjord's own Mark Played, the credits auto-mark, or
  another Jellyfin client) rather than any individual mark-played call
  site: on a played=true transition, checks whether the item is on the
  watchlist and if so calls the existing watchlist-remove path (real
  `DELETE` to Seerr, patches every visible card, rebuilds the calendar).
- **"Coming Up" row added to the Home/TV Shows/Movies dashboards** (mixed
  on Home, filtered to each dashboard's own media type on TV/Movies) —
  same 3-way split pattern as the existing Watchlist dashboard rows.
  Along the way, caught and fixed a real gap the Watchlist row's own
  addition had left behind: `HomeScreen`'s and `DashboardScreen`'s
  scroll-position math never accounted for the Watchlist row's own
  height, since it was always the last row until now.
- **Discover search-grid flash while typing/scrolling.** Two real, live-
  reported causes ("did you also see the refreshes when i serched, i did
  make the grid flash several times"), both `discover-results` swapping in
  a brand-new `ModelRc` instead of updating the live one — this file's own
  established "Phase 96 flash bug" class, just never applied to Discover's
  own search path. (1) Every fresh query (after the 300ms debounce)
  rebuilt every card from scratch with no poster carried forward — typing
  "the bour" -> "the bourn" blanked and re-decoded every overlapping
  result's poster instead of reusing what was already showing.
  `spawn_discover_search`'s commit now carries posters forward by
  `(id, item_type)`, same pattern `apply_search_filters` already used for
  its own re-filter. (2) Auto-loading page 2/3/4 (`spawn_discover_search_
  more`, triggered by scrolling to the last row) rebuilt the *entire*
  model — already-shown cards included — just to append a handful of new
  rows, tearing down and reconstructing every existing card element and
  re-running its poster fade-in for nothing. Fixed with true incremental
  append: downcast the existing `ModelRc<CardItem>` back to the
  `VecModel<CardItem>` it's always actually constructed as
  (`Model::as_any()` + `downcast_ref`, the exact pattern Slint's own
  `VecModel` doc example demonstrates) and `extend()` the new rows onto
  the same live model instance — one `row_added` notification, zero
  existing elements touched.
- **Collection Missing Items never updated on its own.** Real bug,
  live-reported ("why did they not update when i was in it?"). Opening a
  Collection screen from a stale cache spawns two independent background
  tasks: `spawn_collection_revalidate` (refreshes `boxset_items_cache`/
  `item_detail_cache`) and `spawn_missing_items` (resolves the TMDB
  collection id from those same caches, driving the Missing Items row).
  When the caches were stale enough that resolution failed on the first
  try, the row just stayed empty — nothing ever retried it, even though
  the revalidate running in parallel was actively fixing the exact data
  the resolution needed, moments too late. `spawn_collection_revalidate`
  now returns its `JoinHandle`; on a first-attempt failure,
  `spawn_missing_items` awaits that handle and retries once against the
  now-fresher cache instead of firing a redundant fetch of its own. The
  actual resolution logic (free `ProviderIds` path, then the multi-member
  fallback loop) was extracted into `resolve_missing_items_collection_id`
  so both attempts share one implementation.
- Live testing of the Phase 183 Deep Seerr integration rows surfaced a real
  gap: every one of the 5 new rows had only error-path logging, nothing on
  the success path, making several of their own live-verification
  instructions unfollowable. Added `debug!` at every resolution branch and
  silent early-return across Person Other Work, Detail/Series Recommended,
  Collection Missing Items, Series Missing Seasons, and the Calendar's
  ongoing-series source.
- Music Bar's 8 transport/utility icons (Prev/Next/Shuffle/Repeat/Queue/
  Lyrics/VolDown/VolUp) restyled from a hand-rolled 36px TouchArea each to
  the shared 38px `IconCircleButton`, matching Now Playing's transport row
  — user-reported inconsistency: the volume buttons looked and felt
  different between the two screens, even though both already called the
  identical volume-up/-down callback underneath.
- Real bug, live-reported: clicking a BoxSet card with the mouse in the
  Collections library grid opened the generic Detail Page instead of
  `CollectionScreen` — `home.slint`'s `LibraryGrid` click handler always
  called `open-detail` with no check for item type, unlike the keyboard
  Confirm handler (`keys.rs`), which already checked `active-nav == 3`
  and called `open-collection` correctly. Mouse now matches keyboard.
- Log timestamps now local time instead of UTC (`chrono::Local`, avoiding
  the `time` crate's unsound-in-multithreaded-programs `LocalTime`) —
  user feedback that correlating log lines against wall-clock actions was
  confusing.
- **Screen-open cache staleness.** Root cause, traced from a live report
  ("Avatar's collection shows 3 unwatched but only 2 films"): Jellyfin's
  WebSocket only delivers `LibraryChanged` to the most-recently-connected
  client when multiple clients share a session (`JELLYFIN.md`'s own
  documented caveat) — editing through Jellyfin's own web UI while Fjord
  sits connected in the background can silently starve Fjord's connection
  of the event entirely, leaving a screen-open cache stale until the next
  full restart. Fixed by making every one of the 7 detail-style screens
  (Collection, Detail, Series, Season, Artist, Person, Album/Playlist)
  revalidate in the background on every open — even a cache hit, which
  still shows instantly, unchanged — and silently patch the cache and
  live UI if the fresh fetch differs, closing the gap for whatever's
  actually on screen right now. (A first pass also added a 10-minute
  repeating background sweep of all 6 caches; removed again the same day
  once revalidate-on-open covered the actual problem — nothing reads
  these caches for display outside the 7 screens they patch directly, so
  the sweep's only remaining value was a cosmetic "instant-show is
  already correct" edge case, not worth its own ~240-request/10min
  background cost.) Also fixed `spawn_missing_items` (Collection's
  "Missing From This Collection" row) to try every Movie-type BoxSet
  member in turn instead of giving up after the first — Avatar's own
  first member had zero Jellyfin `ProviderIds` at all, while a later
  member (e.g. a more recently-scanned sequel) was far more likely to
  resolve.
- **Code review of the above, requested after it shipped.** Two real bugs
  found and fixed: (1) none of the 7 screens' new revalidate paths
  checked `session_current` before writing into the shared caches — the
  same "background fetch writes per-user data into shared state" risk
  this codebase has already been bitten by and fixed twice before (once
  in `ws.rs`, once in `prewarm.rs`) — a sign-out or account switch on a
  shared HTPC mid-revalidation could have leaked one account's data into
  another's session; (2) Series screen's revalidate pass clobbered
  whatever season the user had since tabbed to back to season 0, since
  its season/episode fetch-and-store block wasn't gated by `revalidate`
  the way the equivalent UI update already was — playback still worked,
  but the episode title fell back to a raw id. Caught by an independent
  review pass, not the original author.
- **Browse All sidebar hitch.** Live-reported, diagnosed with debug
  logging before touching anything: `populate_browse_async` was
  unconditionally rebuilding the full ~800-item Slint list model on
  every single sidebar arrival at Browse All, with no "already built"
  guard — unlike Discover's landing rows, which already had exactly
  this kind of once-per-session gate. Fixed with a new
  `browse_populated` flag (same shape as `discover_landing_fetched`,
  invalidated in `ws.rs` on a real `LibraryChanged`) plus a ~120ms
  debounce on the still-unavoidable first build each session, so
  passing through the tab quickly never starts the rebuild at all.
- **`vf=auto` never applied the NVDEC stride fix for zero-copy `nvdec`.**
  Live-reported: a user's NVIDIA HTPC card needed `format=yuv420p10le`
  manually forced the whole time, because `auto`'s detection branched on
  `nvdec` vs `nvdec-copy` and only applied the real fix for the latter —
  for plain `nvdec` it set the filter to `format=nv12`/`format=p010`,
  which is a no-op (those are already NVDEC's native output formats).
  That distinction was never justified; `auto` now always applies the
  real fix by bit depth alone whenever any nvdec mode is active.
- **Video filter dropdown relabeled — the two "auto" entries were
  indistinguishable.** Follow-up UX request after the fix above: the
  blank `""` entry (displayed as "(none)") and the bare `"auto"` entry
  both read as some kind of "auto," with no way to tell from the
  dropdown that only one of them applies the NVDEC stride fix. Both
  behaviors were kept exactly as-is, just relabeled: `""` →
  `"auto: nv12/p010"` (native decoder output, no filter forced),
  `"auto"` → `"auto: yuv420p/yuv420p10le"` (the runtime stride-fix
  path). The four explicit `format=...` force-options are unchanged.
  Old `config.json` files self-heal to the new labels on next load via
  a small migration deserializer; nothing changes about what's actually
  passed to mpv, before or after.
- **A second, previously-unaddressed source of the sidebar navigation
  hitch: the Discover tab, not Browse All itself.** Live-reported on the
  HTPC, more noticeable there than the dev machine. The earlier Browse
  All fix didn't fully explain the report, because the actual slow step
  was one tab further along — passing through Discover (nav==6), right
  next to Browse All in the sidebar order. `refresh_seerr_admin_status`
  fired a real `GET /auth/me` network round trip on every single arrival
  at Discover with no guard at all; rapidly cycling the sidebar with a
  held arrow key passed through it dozens of times a minute, piling up
  concurrent requests whose delayed completions visibly collided with
  keypress processing. Rate-limited to once per 60s instead of a one-time
  fetch (a genuine mid-session permission change should still eventually
  be picked up, unlike Browse All's list which only needs building once).
- **The actual bigger contributor to the same hitch: Browse All's ~800-item
  list was fully destroyed and reconstructed on every sidebar pass through
  it, not just refetched.** User pushed back on the rate-limit fix above
  and asked directly whether the real cost could be optimized away rather
  than moved around. It could: `browse.slint`'s list has no virtualization
  (~800 real Slint elements for a real library), and as an ordinary AppShell
  content slot it was torn down and rebuilt on every single sidebar move
  through nav==5 — including a sub-200ms flick through it while holding an
  arrow key. Moved `BrowseScreen` out of AppShell's shared layout into its
  own permanently-mounted sibling: the ~800 elements are now constructed
  once per session, with every subsequent open/close just a `visible`
  toggle, no reconstruction. Zero Rust-side changes needed.
- **Broader performance sweep, user-requested.** Found and fixed two more
  real issues sharing the same bug classes as the two above: `LibraryGrid`
  (Movies/TV/Collections/Music library grids) had the identical AppShell
  mount-churn shape as Browse All — up to ~608 items with a heavier
  per-element cost than Browse's own list — consolidated into one
  permanently-mounted instance shared across all 4 nav tabs (simpler than
  Browse's fix, since all 4 already read the same underlying data). The 7
  screen "revalidate on cache hit" functions (Collection/Detail/Series/
  Season/Artist/Person/Album) had the identical missing-guard shape as the
  Discover rate-limit fix above — each fired a full item-detail + list +
  poster refetch on every reopen of an already-cached screen with no
  cooldown at all; fixed with the same 60s rate-limit pattern, keyed by
  item id. Also moved `save_config()` (JSON serialize + encrypt + disk
  write) outside the held state lock at ~7 call sites that were doing it
  synchronously while holding the shared mutex. Two smaller findings
  (Discover's own unbounded search-results grid; a lock-held cache clone in
  the 60s screen-cache-save timer) were investigated and deliberately
  deferred — real but narrow, not yet worth the added complexity.
- **Backlog-wide code review, 4 parallel agents over the remaining
  not-yet-live-tested Seerr features.** Found and fixed 7 real bugs: the
  Series screen's Missing Seasons row had no `Down` key handler at all,
  making Cast/Similar/Recommended keyboard-unreachable on any partially-
  owned show; clicking any Discover card left a stale filter-bar-focus flag
  that silently hijacked the next arrow key/Enter; Discover Filters'
  Type=All pagination broke the active sort order across every page
  boundary (each page was sorted in isolation, then just appended); the
  `RequestDetailScreen` button row had no overflow protection for its 4-5
  possible elements, a real risk on any window under ~1350px wide; a
  newer in-library Watchlist toggle had a silent parse-failure with no
  logging, unlike its Discover-menu sibling; and the fullscreen player's
  pause/play flash icon had no font pin at all, unlike its own OSD
  siblings two lines away — plausibly the real root cause of the tofu
  square originally reported, surviving the earlier variation-selector
  cleanup entirely since that pass never touched font pinning. The 209-site
  U+FE0E removal itself was re-verified and is clean, no stray occurrences
  or inconsistent pairs.
- **The 2 performance items deferred from the earlier sweep, addressed
  after being asked directly whether the deferral still stood.**
  `DiscoverScreen` had the same AppShell mount-churn shape as Browse
  All/LibraryGrid (smaller item counts in practice, but the identical
  risk) — converted to a permanently-mounted sibling, same pattern.
  `save_screen_caches`'s six-cache clone, which happens under the global
  state lock every time it runs, wasn't free once those caches are large
  (a real cost after the opt-in library prewarm raises their cap to fit
  the whole library) — fixed properly after a follow-up question ("why
  not make a full fix?") showed the original "would touch every call
  site" reasoning was wrong: `BoundedCache<V>` now wraps its storage in
  `Arc` with clone-on-write mutations (`Arc::make_mut`), making the clone
  O(1) with zero changes to any of the type's ~20 call sites across 7
  screens. Verified directly against a real 41MB `screen_caches.json`
  (11,670 cached items) to confirm the on-disk format stayed compatible.

## [0.4.2] — 2026-07-19 – 2026-07-29

Watchlist, calendar, and Seerr data woven into the app's existing screens
instead of only living behind the Discover tab.

- Seerr Watchlist: add/remove from the Discover context menu or a
  RequestDetailScreen button, with a universal ★ badge on every card
  everywhere it appears — Discover, the Home/Movies/TV dashboards, and
  native Jellyfin library cards for items already owned.
- Release Calendar: a month-grid screen plus a "Coming Up" Discover row,
  sourced from watchlisted items, active requests, and ongoing
  (`Status == Continuing`) library series.
- Deep Seerr integration into existing native screens: Person gains an
  "Other Work" row, Detail/Series gain "Recommended", Collection gains
  "Missing From This Collection", and Series gains "Missing Seasons" with
  per-season request-status pills and a preselected Request Options flow.
- Widened the subtitle position range (50–150%, up from 50–100%) after
  confirming against the real mpv manual that 100 was never meant to be
  the screen edge.
- A long tail of real bugs found and fixed via live HTPC testing: a
  Watchlist API deserialize gap, a Coming Up row that silently never
  reached the UI (an off-thread `AppState` write that failed silently — a
  recurring failure mode this project had to learn to watch for), a
  startup self-deadlock from a re-entrant mutex lock, a watchlist star
  that never survived a screen rebuild, and the removal of all 209
  U+FE0E variation selectors across the UI as a likely source of tofu
  glyphs on the HTPC's font stack.

*Range: [`759f3b0`](../../commit/759f3b0)..[`2b53bba`](../../commit/2b53bba) (20 commits)*

## [0.4.1] — 2026-07-16 – 2026-07-18

Quality profiles, filters, and the full request lifecycle.

- Radarr/Sonarr quality profile picker and tag picker for requests.
- Redesigned RequestDetailScreen: rating badge, collapsible overview,
  Cast & Crew row, streaming-provider panel.
- Discover Filters: type, genre, sort order, rating, year, and streaming
  provider — both a filtered-browse view and a client-side pass over
  search results.
- Watch Trailer, playing Seerr's YouTube trailer links through mpv's
  `ytdl_hook`.
- Streaming Region, Display Language, and Discover Language settings
  written back to the connected Seerr account; the `seerr_enabled` toggle
  now actually gates the whole integration live.
- Full request-lifecycle context menu: View Details, Request, Edit
  Request, Cancel Request, Approve, Decline.

*Range: [`4c2aea6`](../../commit/4c2aea6)..[`759f3b0`](../../commit/759f3b0) (35 commits)*

## [0.4.0] — 2026-07-15 – 2026-07-16

Discover screen + request flow — the pivot from "Jellyfin viewer" toward
"general media client" begins.

- New Discover tab: search, plus Trending/Popular/Upcoming landing rows.
- Per-item request submission via a Request Options modal (2K/4K quality,
  per-season picker for TV).
- A "Requested" row, and opening an already-owned item redirects straight
  to its real Jellyfin detail page instead of the Seerr one.
- Every new Seerr endpoint and response shape was verified directly
  against Seerr's real TypeScript source rather than the OpenAPI spec,
  which this integration repeatedly found to be incomplete or wrong.

*Range: [`1e9d424`](../../commit/1e9d424)..[`4c2aea6`](../../commit/4c2aea6) (8 commits)*

## [0.3.4] — 2026-07-09 – 2026-07-15

Feel and consistency polish.

- Universal press/click feedback animation, working identically for mouse
  and keyboard.
- Configurable global scroll/animation speed (0–500%), `FadeGate` for
  screen exit fades, `ease-out-expo` easing.
- Bundled Inter (default UI font, user-selectable) and Noto symbol fonts,
  so icon glyphs render consistently regardless of what's installed on a
  given HTPC.
- Fjord's own client version shown in Settings, configurable seek step,
  subtitle appearance settings, per-series remembered audio/subtitle track.

*Range: [`9be9c24`](../../commit/9be9c24)..[`1e9d424`](../../commit/1e9d424) (29 commits)*

## [0.3.3] — 2026-06-28 – 2026-07-09

Sync hardening, playlists, and a caching/performance pass.

- Incremental WebSocket delta sync with focus safety; self-healing 404s
  for ghost items; artwork revalidation via `ImageTags` sidecars.
- Full Jellyfin Playlist API (list/create/add/remove/items), a Playlists
  library view, and an Add-to-Playlist context-menu picker.
- Auto mark-played at the credits trigger, with a rewind-revert safety
  net so skipping back out of the credits un-marks it again.
- Screen-open result caching (six caches, persisted to disk) and an
  opt-in full-library metadata/image prewarm.
- Two full-codebase code reviews. **CR10** (25 findings): a
  `Handle::current()` panic in artist/album favorite/played callbacks, a
  Jellyfin auth token leaking into playback-URL log lines, the Quit
  keybinding being silently stealable by the queue panel (now a global
  `Ctrl+Q`), a WebSocket UTF-8 panic that could kill the reconnect loop
  permanently, sign-out deleting `device_id` and settings instead of just
  auth, and Up Next resolving the next episode read-only instead of
  marking it played prematurely. **CR11** (16 findings, everything added
  since CR10 — gapless playback, the persistent queue/playlist model, Now
  Playing, Jellyfin playlists): a BoxSet card that tried to play a dead
  stream URL, a WS delta-refresh task that wasn't covered by the
  sign-out abort handle, three screens missing the post-mouse-use
  keyboard-focus re-grab, and a gapless edge case that could report a
  track as playing when it never produced audio.

*Range: [`7e1ee12`](../../commit/7e1ee12)..[`9be9c24`](../../commit/9be9c24) (130 commits)*

## [0.3.2] — 2026-06-28

Playback queue, MusicPlayerBar, gapless, Now Playing, Lyrics.

- Playback queue: Play Next / Add to Queue from the context menu, a queue
  viewer panel.
- Playlist backend with Prev/Next/Shuffle/Repeat, and the MusicPlayerBar
  bottom-bar layout.
- Gapless music playback (same mpv instance, no gap between tracks).
- A fullscreen Now Playing screen with synced lyrics.

*Range: [`95c3829`](../../commit/95c3829)..[`7e1ee12`](../../commit/7e1ee12) (3 commits)*

## [0.3.1] — 2026-06-25 – 2026-06-27

Music library core.

- MusicDashboard, AlbumScreen, ArtistScreen, and audio playback.
- Favorites rows on the Music dashboard, an Artists/Albums view toggle
  with full keyboard navigation, and a redesigned music sort bar.
- **CR8**: six Collections-screen bugs (a blank screen on a failed BoxSet
  fetch instead of a toast, `show-collection` surviving sign-out and
  routing keys to a null client, a stale-request race on rapid re-open).
  **CR9**: a fallback so a BoxSet dashboard card still opens correctly
  before the library grid has ever populated `all_collections`, and two
  poster-loading races that could overwrite the wrong library grid if the
  user switched tabs mid-fetch.

*Range: [`a1eee6c`](../../commit/a1eee6c)..[`95c3829`](../../commit/95c3829) (60 commits)*

## [0.3.0] — 2026-06-25

WebSocket real-time events, chapter navigation, Collections screen.

- Phase 41: WebSocket-driven real-time sync begins (library changes,
  favorites, playback state).
- Chapter navigation (`,`/`.` keys, seek bar tick marks, OSD) and
  sub/audio delay adjustment (`z`/`Z`/`x`/`X`), both with mouse-accessible
  panels in the player controls bar.
- A new Collections library screen, with its own dashboard rows and disk
  cache.

*Range: [`d12a9d4`](../../commit/d12a9d4)..[`a1eee6c`](../../commit/a1eee6c) (9 commits)*

## [0.2.3] — 2026-06-21 – 2026-06-25

Mini-player redesign, library grid sort/filter, error toasts.

- Floating corner mini-player bar, replacing the old sidebar-docked
  version.
- Library grid sort/filter/search with an alphabet scrubber (Phase 34).
- Error toast notifications (Phase 35).
- Two full-codebase code reviews. **CR6** (13 findings): sign-out now
  actually clears episode/collection caches, `movies_fetched`, and every
  overlay flag; `get_all_movies`/`get_all_series` paginate through a
  shared helper instead of risking a silent truncation on a large
  library; season-screen portraits load in parallel instead of trickling
  in. **CR7** (15 findings): a duplicated always-skip auto-advance path
  that raced against the natural-end fallback, Next Up's favorite flag
  hardcoded false, and several dead/backwards keyboard-nav guards on the
  season/series header buttons.

*Range: [`7efff5f`](../../commit/7efff5f)..[`d12a9d4`](../../commit/d12a9d4) (61 commits)*

## [0.2.2] — 2026-06-21 – 2026-06-23

Detail/Series/Season content enrichment + Person screen.

- Cast and crew portraits, a collection row and similar-items row on the
  movie detail page, collapsible Storyline sections.
- A full season detail screen, series detail UX polish (watched button,
  poster badges, season focus).
- A new Person screen (portrait, bio, filmography).
- **CR5**: post-detail-enrichment bug fixes — a cast portrait index
  mismatch, `fetch_movie_collections` not spawned on the auto-login path,
  a CastRow focus-ring visibility bug, and stale collection/similar-items
  models surviving a Back navigation.

*Range: [`913c90c`](../../commit/913c90c)..[`7efff5f`](../../commit/7efff5f) (99 commits)*

## [0.2.1] — 2026-06-14 – 2026-06-21

Settings matures, player UX polish, Intro Skipper complete.

- Two-pane Settings screen with live keybinding rebinding.
- Netflix-style Up Next banner, a transient volume overlay (SPDIF
  passthrough-aware), subtitle language/type preferences.
- Player UI redesign (seek tooltip, buffering overlay), per-format SPDIF
  passthrough toggles (AC3/EAC3/DTS/DTS-HD/TrueHD).
- Per-segment Intro Skipper skip modes with configurable timers.
- Three full-codebase code reviews. **CR1** (10 findings): stale
  intro/credits background tasks, a report-ordering bug, pause desync, a
  semaphore bypass, auto-login timeout handling, and a Not-Watched timer
  timestamp bug — plus a `reset_playback_ui`/`fetch_image_cached` cleanup
  pass. **CR3** (9 findings): hidden video-latency-hacks activation, a
  SPDIF warning that misfired with every format switched off,
  seek-dragging getting stuck on Wayland, and a null-deref crash in
  deinterlace-setting deserialization. **CR4** (10 findings): a
  `Player::new` error-cleanup gap, poster-loader panics not flushing
  their `JoinSet`, an Up Next countdown off-by-one, and a mid-session 401
  not redirecting to the login screen.

*Range: [`cb48e1d`](../../commit/cb48e1d)..[`913c90c`](../../commit/913c90c) (202 commits)*

## [0.2.0] — 2026-06-12 – 2026-06-14

Context menu + canonical state store.

- Right-click / `C`-key context menu: Mark Played, toggle Favourite, Play
  from Start.
- A canonical `update_item_user_state` store keeps `FjordState` in sync
  across every card model after a played/favorite toggle, instead of each
  call site patching its own copy.

*Range: [`38cd536`](../../commit/38cd536)..[`cb48e1d`](../../commit/cb48e1d) (104 commits)*

## [0.1.0] — 2026-06-11 – 2026-06-12

Proof of concept.

- The mpv render API works: OpenGL FBO compositing with Slint,
  `report_swap()` called every frame for genuine vsync feedback — the
  reason this project exists, since every Flutter/`media_kit`-based
  Jellyfin frontend skips this call and stutters on NVIDIA legacy drivers
  under Wayland.
- A minimal but real client around it: login, movie/TV library grids with
  posters, item detail pages, series → season → episode drill-down, and a
  keyboard-driven player with subtitle/audio track panels.

*Range: [`2cf755a`](../../commit/2cf755a)..[`38cd536`](../../commit/38cd536) (45 commits)*
