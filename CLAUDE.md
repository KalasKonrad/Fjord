# Fjord — Claude Code Context

Fjord is a Jellyfin media frontend for Linux HTPCs: Rust + Slint (GUI) + libmpv (playback), with
Seerr (media requests) and the Bonfire/JellyProfiles plugin (household profiles). Personal project by
KalasKonrad. The UI is **D-pad/remote-first**; the long-term goal is a general media client.

**Why it exists:** Flutter Jellyfin clients embed mpv via media_kit and never call
`mpv_render_context_report_swap()`, so playback stutters on NVIDIA legacy drivers under Wayland. Fjord
uses the mpv render API: mpv renders into an OpenGL FBO that Slint composites, and `report_swap()` runs
after every frame.

## Commands

```bash
cargo run -p fjord-app                  # run the app
cargo build                             # debug build (cargo build --release for release)
cargo clippy --workspace --all-targets  # must be clean before committing
cargo test --workspace                  # must pass before committing
```

Needs `mpv`/libmpv (`pacman -S mpv`). Optional at runtime: `yt-dlp` (trailers), `kscreen-doctor` (display sync).

## Where to look

| Where | What |
|---|---|
| `PLAN.md` | Roadmap: **Live-test checklist**, Pending (open work only), Issues, Deferred |
| `CHANGELOG.md` | User-facing changes (`[Unreleased]` on top) |
| `DEVLOG.md` | ~1.3 MB archive of dated design write-ups and bug investigations. Not auto-loaded: `grep -n '^### ' DEVLOG.md`, then read the section you need. Code comments that say "see CLAUDE.md's X section" mean **DEVLOG.md**. |
| `SLINT.md` | Slint patterns + full write-ups of every Slint gotcha below |
| `JELLYFIN.md`, `SEERR.md` | Verified API references (endpoints, params, caveats) |
| TOC header of each `.rs`/`.slint` file | What that file owns — read it before editing the file |

## Workspace

- `crates/fjord-api` — Jellyfin REST client (`client.rs`), Bonfire plugin endpoints (`bonfire.rs`), models. No Slint, no mpv.
- `crates/fjord-player` — libmpv wrapper: `Player`, `MpvRenderCtx`, `PlayerConfig` (`mpv.rs`). No Slint, no HTTP.
- `crates/fjord-seerr` — Seerr REST client + models. No Slint, no Jellyfin coupling.
- `crates/fjord-app` — the binary: `ui/*.slint` + `src/*.rs`, one module per screen/concern.

`fjord-app/src` at a glance:
- `main.rs` — entry point, module wiring, most `AppState` callbacks. Shared helpers: `show_toast`,
  `session_current`/`seerr_session_current`, `reset_session_state`, `close_login_screen`,
  `apply_cards_preserving_identity`, `item_to_card_item`/`items_to_model`, `strip_html_to_text`,
  `trim_last_grapheme`, `is_unauthorized`/`is_rate_limited`, `should_revalidate`, `timed`.
- `config.rs` — `Config { device: DeviceConfig, profiles: Vec<ProfileSettings>, active_profile_id }`,
  `FjordState` (runtime state), `BoundedCache`, load/save + migrations, per-profile cache paths.
- `keys.rs` — `Action`, `Keybindings`, `AppMode`, `active_mode()`, `handle_key()` dispatcher.
- `playback.rs` — `VideoState`, `start_playback`, `tear_down_player`, `wire_rendering_notifier` (GL/FBO),
  `wire_mpv_timer` (16 ms tick: position, skip segments, Up Next, stall recovery, gapless, HDR/display-sync).
- `controls.rs` (player callbacks), `stats.rs` (stats overlay).
- Screens: `detail`, `series`, `season`, `collection`, `album` (albums + playlists), `artist`, `person`,
  `browse`, `home`/`movies`/`poster` (dashboards, library grid, poster loading), `discover` (Discover,
  RequestDetail, Calendar, Watchlist), `blocklist`, `settings`, `context_menu`.
- Accounts/profiles: `auth.rs` (login, `finish_session_setup`), `profile.rs` (pickers, `switch_to_profile`,
  Bonfire sync, idle lock), `profile_edit.rs`, `bonfire_admin.rs`, `secrets.rs` (secrets encrypted at rest).
- Integrations/platform: `seerr_auth.rs`, `ws.rs` (Jellyfin WebSocket delta sync), `prewarm.rs`, `hdr.rs`
  (Wayland color-management worker), `display_sync.rs` (kscreen-doctor mode matching), `activity.rs`
  (winit event tap: idle detection + Wayland handles), `pipewire_fix.rs`.

## Architecture rules

### Rendering
- mpv runs `vo=libmpv` + `mpv_render_context` and never owns a window. `BeforeRendering` renders into the
  back FBO, Slint shows it as a `BorrowedOpenGLTexture`, `AfterRendering` calls `report_swap()`.
- **Two FBOs alternate every frame** — a single texture id makes Slint skip redraws.
- **Drop `MpvRenderCtx` before `Player`.**
- `Player::new()` only builds the core; `Player::load()` runs in `BeforeRendering` once the render context
  exists and `VideoState.pending_load_url` is set. Loading earlier races VO init → audio-only black screen.
- With display sync on, `start_playback` withholds `pending_load_url` and `play_start` until the display
  mode has been switched (pre-decode task, 20 s cap). Stamp `play_start` only when decode is actually
  requested — every stall/diagnostic watchdog is gated on it.
- All teardown goes through `tear_down_player`; all fresh-playback resets through `reset_video_state_for_playback`.

### Threads and shared state
- Tokio for async, Slint event loop on the main thread. Return to the UI with `slint::invoke_from_event_loop`
  (closures are `'static + Send` — capture owned values).
- **`slint::Weak::upgrade()` silently returns `None` off the UI thread.** Anything reachable from a Tokio
  task that touches `AppState` must do so inside `invoke_from_event_loop`. This has shipped as a silent
  no-op bug several times.
- `slint::Image` and `CardItem` are `!Send`: build Send-safe data off-thread (`SharedPixelBuffer`, meta
  structs) and construct Slint types inside the closure.
- **`std::sync::Mutex` is not reentrant.** Never call something that locks `state`/`video` while holding that
  lock — it hangs with no error. Clone what you need, drop the guard, then call. Same for disk I/O
  (`save_config`): clone under the lock, save after dropping it.
- Async results can land after a profile switch or sign-out: guard writes with `session_current(&state, &client)`
  (`Arc::ptr_eq`) or `seerr_session_current`. Per-screen `*-open-gen` counters only guard against the same
  screen reopening for another item.
- Surface errors with `show_toast(ww, msg)` (safe from any thread).

### Config and sessions
- Device-wide settings go in `DeviceConfig`, per-person settings in `ProfileSettings`. Access profile settings
  **only** via `Config::active()` / `active_mut()`. New fields get `#[serde(default)]`; `FjordState.config`
  is the single in-memory copy.
- Caches live under `~/.cache/fjord/profiles/<user_id>/`; pass `user_id` explicitly (prefer `client.user_id`).
  Posters/backdrops are shared across profiles.
- `reset_session_state` is the one teardown for sign-out and profile switch. **Every new transient flag,
  cache, overlay, or input-dispatch state must be reset there** — Seerr-scoped caches also in
  `seerr_auth::clear_connection` / `commit_connection`.
- `token` and the Seerr key/cookie are AES-GCM encrypted by `load_config`/`save_config` only.

### UI models
- Rebuild card rows through `apply_cards_preserving_identity` (mutates rows in place when ids/order match).
  A fresh `ModelRc` recreates every delegate → visible flash.
- Reuse an existing row's `poster` handle; carry every `CardItem` field (poster, `on_watchlist`, counts…)
  through every code path that writes the same model.
- Every model holding `CardItem`s must be listed in `context_menu::update_card_in_all_models`.
- These lists exist twice, in Rust and as Slint `if` conditions, with no shared source of truth —
  **change both sides together**: context-menu rows (`existing_*_menu_rows` ↔ `context_menu.slint`),
  settings rows (`settings::section_row_keys` ↔ `settings.slint`), D-pad zone lists (`existing_*_zones`),
  dashboard nav ladders in `app_state.slint`.
- Adding a dashboard row: extend the screen's `section-y`/`row-y` scroll helper with a term for it.

### Keyboard / D-pad
- One global `FocusScope` (`fs`) feeds `keys::handle_key()`; `active_mode()` maps `AppState` flags to an
  `AppMode` and routes to per-screen handlers. Overlays with text entry use raw-key tiers checked first.
- Contract: Enter/Right enter, Back/Escape go back, Up/Down change rows, Left/Right move within a row.
  Every button must be D-pad reachable — a keyboard shortcut alone is not enough.
- **Mouse clicks must set the same focus/zone state the keyboard path uses.**
- A focused `LineEdit` gets keys before `fs` (`fs` is a sibling, not an ancestor): handle Escape/Up/Down/Enter
  in the field's own `key-pressed`, and hand focus back with `AppState.refocus()` /
  `invoke_grab_keyboard_focus()` when a screen with native widgets closes.
- New overlay checklist: add it to `active_mode()`, to the ResumePlayer / music-bar / mini-player-bar
  exclusion lists in `keys.rs`, to the `sidebar-kb-active` exclusions, and give its `FadeGate` a
  `!AppState.is-playing` guard.
- The on-screen-keyboard gate in `keys.rs` swallows everything but Ctrl+Q while open: every close path must
  clear `show-onscreen-keyboard` (use choke points like `close_login_screen`).
- Clickable elements get the `PressPulse` border flash (`kb-activate-pulse`); focus rings use `Theme.focus-border`.

### Screens, animation, glyphs
- Free-floating overlays mount via `FadeGate` (fades in and out). Screens sharing AppShell's
  `HorizontalLayout` content slot must never be mounted two at once — dashboards switch via the sequential
  `shown-nav`/`nav-fade` mechanism in `main.slint`.
- `BrowseScreen`, `LibraryGrid`, `DiscoverScreen` stay mounted and toggle `visible:` (big lists built once).
- Multiply every `animate` duration by `AppState.settings-animation-speed` (`settings-scroll-speed` for scrolling).
- Icons: only glyphs covered by the bundled fonts in `assets/fonts` (Noto Sans Symbols 2/Math; Adwaita Sans
  for ♥ ✓). Pin `font-family` on icon `Text`; verify coverage with `fc-query --format='%{charset}'`.
  No U+FE0E variation selectors, no color emoji.

## Slint gotchas (full write-ups in SLINT.md)
- Keyboard-scrollable lists: `Flickable` only. Don't bind `viewport-y` (kills mouse wheel) — compute `kb-y`
  and assign it in `changed kb-y => { fl.viewport-y = kb-y; }`.
- In an `if`-mounted layout child, don't read `self.width/height` in a `changed` tracker — use `root.*`.
  A child must never size itself from its parent **Layout's** width (binding loop) — use `root.width`.
- No explicit x/y/width/height derived from `root` on a layout element inside a reusable component — use `padding`.
- A `Layout` placed directly in a plain `Rectangle` needs `height: self.preferred-height;` when anything reads
  its height. An unconstrained Layout fills its parent's width; it does not hug its content.
- `opacity: 0` is still hit-testable (use `visible: false`). Plain `Rectangle` children are centered (set `x: 0`).
- Ternaries only track the evaluated branch — read reactive values into a `let` first.
- `changed` only watches properties on the current component — mirror `AppState` values into a local property.
- `TouchArea.moved` fires only while dragging (use `changed mouse-x/mouse-y`). `KeyEvent.repeat` is unreliable.
- `Timer.running = true` doesn't restart a running timer (set `false` then `true`).
- `parent` in a reusable component is its immediate parent — expose an `out property` instead.
- Element ids must be unique across the whole component, even in different `if` blocks.
- Slint strings have no `.length`/substring — do that in Rust (e.g. `trim_last_grapheme`).

## External APIs
- **Verify every endpoint and field against real source or a live server before modeling it** — the published
  docs/OpenAPI specs have been wrong many times. Record verified caveats in `JELLYFIN.md` / `SEERR.md`.
- Jellyfin models are `PascalCase`, **including Bonfire** (it runs inside Jellyfin, whatever its docs show).
  Seerr models are `camelCase`.
- Optional request fields: `#[serde(skip_serializing_if = "Option::is_none")]` — sending `null` has caused
  500s. Seerr user-settings POSTs replace the whole object: GET, mutate, POST.
- On a non-2xx, read the response body into the error so logs/toasts show the server's message.
- One-off live diagnostics: a temporary `#[tokio::test]` using the saved config/token — run once, then delete.
  Never commit machine paths or real ids.
- Jellyfin auth uses the `Authorization: MediaBrowser … Token="…"` header (not legacy `X-Emby-*`).
  `DeviceId` comes from `ensure_device_id()`; two machines must never share one.

## Platform notes (HTPC: NVIDIA Pascal legacy driver, KDE Wayland)
- NVDEC stride corruption → Settings → Video filter `auto: yuv420p/yuv420p10le`.
- HDR passthrough (opt-in) tags the whole window surface as PQ/BT.2020, so UI chrome renders with wrong
  colors while it's on. The real fix (video on its own Wayland subsurface) is deferred.
- Some tone-mapping curves fail to compile on the NVIDIA GLSL compiler and stall the GL thread; switch curve
  (e.g. `bt.2390`) if HDR→SDR playback freezes.
- More in `DEVLOG.md` → "Known platform issues".

## Development workflow
1. Read the relevant files (and their TOC headers), then plan.
2. Implement; run build, clippy, and tests (commands above).
3. Update the TOC header of every modified `.rs`/`.slint` file (symbols added/removed **and** behaviour changes).
4. Docs, in the same commit:
   - `CHANGELOG.md` `[Unreleased]` — user-facing summary.
   - `PLAN.md` — a `- [ ]` line under **Live-test checklist** for anything not verified live. `Pending` is
     for genuinely open work only.
   - `DEVLOG.md` — the technical narrative (root cause, what was tried, why), dated, under the relevant topic.
   - `CLAUDE.md` — **only** when a durable rule, command, convention, or module boundary changes. Keep it
     short; no dated narratives here.
5. Commit, then push right away (the HTPC builds from GitHub). Use a feature branch for large/risky work.

## Testing
- Dev machine: AMD, KDE Wayland. HTPC: NVIDIA legacy (Pascal), KDE Wayland — the primary target, tested less often.
- Logs: `~/.cache/fjord/logs/fjord.log` (rotated `.1`–`.10`). HTPC logs: the `HTPC logs/` symlink in the
  repo root (no SSH).
- Each log starts with `fjord version: r<count>.<hash>` — compare with `HEAD` before treating it as a live repro.
- Debug output needs Settings → General → Log level = Debug (or `RUST_LOG`). Log generously at `debug!` on
  async/debounced/multi-step paths — silent paths have made live bugs undiagnosable before.
- The UI can't be driven from here: say "not live-tested" plainly and add the checklist item.
- Install on the HTPC: `makepkg -si` in the repo root (`PKGBUILD`, builds from source) or in `fjord-bin/`
  (downloads the `nightly` release built by `.github/workflows/build-release.yml` on a self-hosted runner).

## Style
- `cargo fmt`. `anyhow::Result` at the top level, `thiserror` for library errors, no `unwrap()` in library code.
- `fjord-api`, `fjord-player`, `fjord-seerr` never import Slint.
- Every `.rs`/`.slint` file opens with a `// ── <crate> · <filename> ──` header listing its major symbols
  (one line each); long files add `// ──` markers before major functions/blocks.
