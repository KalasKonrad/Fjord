# Rust Split Plan — `fjord-app/src/main.rs`

`main.rs` is currently 2607 lines. The goal is to move cohesive blocks of code
into focused modules so it becomes obvious where to look for any given concern.
`main()` itself stays in `main.rs` and contains only callback wiring — no
structs, no logic, just closures that call into the modules.

---

## Guiding rules

- **No behaviour change.** Every step is a pure move/rename. No logic changes
  until the split is complete.
- **One module at a time.** After each move, run `cargo build` before moving on.
  Fix any compile errors before touching the next module.
- **Public only what is needed.** Use `pub` only on items that `main.rs` or
  another module actually calls. Everything else stays `pub(crate)` or private.
- **Don't introduce AppContext yet.** The callback closures already capture their
  dependencies by `Arc::clone`. Keep that pattern — it works and requires no
  structural change. AppContext can come later if desired.

---

## Module map

### `config.rs`
**Lines to move:** ~22–300

Contains persisted settings and disk I/O for config and item library only.
No Slint, no mpv, no tokio. Home screen data moves to `home.rs`.

```
Config                      (struct + serde defaults)
AppState                    (struct + impl: new, apply_from_config, player_config,
                             apply_filter, apply_nav, refilter)
config_path()
item_cache_path()
poster_cache_path()
backdrop_cache_path()
load_config() / save_config()
load_item_cache() / save_item_cache() / is_item_cache_fresh()
ensure_device_id()
non_empty()
fmt_resume_label()
default_hwdec/gpu_api/video_sync/tscale/tone_mapping()
```

**Dependencies:** `serde`, `serde_json`, `fjord_player::PlayerConfig`, `std::fs`,
`std::time::Duration`

---

### `home.rs`
**Lines to move:** ~603–686, 1499–1561

Home screen data fetching, cache, and the not-watched refresh timer.

```
HomeData                    (struct + serde)
home_cache_path()
load_home_cache() / save_home_cache()
fetch_home_data()
push_home_data()
home_data_sections()
wire_nw_timer()             (30 s poll: refreshes Not Watched rows when tab visible
                             + 10 min elapsed since last refresh)
```

`wire_nw_timer` returns the `slint::Timer`; `main()` calls `std::mem::forget` on it.

**Dependencies:** `fjord_api`, `serde`, `serde_json`, `slint`, `tokio`, `config::AppState`,
`poster::spawn_poster_loading`

---

### `poster.rs`
**Lines to move:** ~94–138, 383–390, 486–777

All poster/backdrop fetching, decoding, and batch-loading tasks.

```
fetch_poster_cached()
fetch_backdrop_cached()
decode_poster_buffer()
spawn_poster_loading()
spawn_series_poster_loading()
spawn_movies_poster_loading()
home_data_sections()
```

**Dependencies:** `fjord_api`, `slint`, `tokio`, `image`, `config::poster_cache_path`,
`config::backdrop_cache_path`

---

### `detail.rs`
**Lines to move:** ~2031–2220 (the `on_open_detail`, `on_play_detail`,
`on_resume_detail`, `on_close_detail` callback bodies)

Detail page logic — fetch item detail, build cast, load backdrop, format
metadata strings. Already ~130 lines of logic mixed into main().

```
open_detail()               (extracted from on_open_detail closure)
```

The callbacks stay in `main()` as thin wrappers that call `detail::open_detail(...)`.

**Dependencies:** `fjord_api`, `slint`, `tokio`, `config::AppState`,
`poster::fetch_poster_cached`, `poster::fetch_backdrop_cached`

---

### `movies.rs`
**Lines to move:** ~486–547

Movie library grid logic. Thin now, but the right home for future movie-specific
features (collections, trailers, play-from-start, etc.).

```
spawn_movies_poster_loading()
```

**Dependencies:** `fjord_api`, `slint`, `tokio`, `image`, `config::poster_cache_path`,
`poster::decode_poster_buffer`, `poster::fetch_poster_cached`

---

### `series.rs`
**Lines to move:** ~779–970

Series drill-down screen logic.

```
EpisodeRaw                  (struct)
make_episode_raw()
raw_to_entry()
spawn_episode_thumb_loading()
open_series_screen()
```

**Dependencies:** `fjord_api`, `slint`, `tokio`, `config::AppState`, `poster::*`

---

### `stats.rs`
**Lines to move:** ~972–1070

Stats overlay formatting only. Pure function — takes `StatsData`, sets window
properties. No state, no async.

```
update_stats_window()
```

**Dependencies:** `fjord_player::StatsData`, `slint::MainWindow`

---

### `playback.rs`
**Lines to move:** ~301–342, 344–377, 1072–1106, 1139–1497, 1863–1930

Video state, GL FBO management, playback helpers, rendering notifier wiring,
and the mpv event-poll timer. `start_playback` already takes all its dependencies
as explicit parameters so it moves cleanly. The rendering notifier and mpv timer
both close over `Arc` clones — extract as `wire_rendering_notifier` and
`wire_mpv_timer` functions that take those same values as arguments.

```
VideoState                  (struct + Default impl)
fmt_secs()
build_track_model()
create_fbo()               (unsafe)
delete_fbo()               (unsafe)
start_playback()
wire_rendering_notifier()   (sets up BeforeRendering/AfterRendering/Teardown)
wire_mpv_timer()            (16 ms poll: decoder log, tracks, seek bar, intro skip,
                             controls hide, playback finished + auto-advance trigger)
```

`wire_rendering_notifier` and `wire_mpv_timer` return the `slint::Timer` /
notifier handle; `main()` calls `std::mem::forget` on the timer as before.

**Dependencies:** `fjord_player`, `fjord_api`, `slint`, `gl`, `config::AppState`,
`stats::update_stats_window`

---

### `auth.rs`
**Lines to move:** ~1694–1788 (`on_do_login` callback body)

Login flow — parse server URL, call `fjord_api::authenticate`, persist config,
fetch initial library + home data, push to UI. Self-contained enough to extract
cleanly. Natural home for future multi-server or token-refresh logic.

```
do_login()                  (extracted from on_do_login closure)
```

**Dependencies:** `fjord_api`, `slint`, `tokio`, `url`, `config::*`, `home::*`,
`poster::*`, `movies::*`, `series::*`

---

### `browse.rs`
**Lines to move:** ~1790–1860 (filter, library search, nav callbacks)

Browse list and library grid search logic. Groups all the client-side filtering
callbacks together. Natural home when server-side search lands.

```
update_library_filter()     (local fn, extracted)
wire_browse()               (registers on_filter_changed, on_library_search_*,
                             on_nav_selected on the window)
```

**Dependencies:** `fjord_api`, `slint`, `config::AppState`

---

### `controls.rs`
**Lines to move:** ~2322–2530 (all player control callbacks)

The ~15 player control callbacks are currently wired inline in `main()`.
Extract into a single `wire_controls(window, video)` function to clean up
~200 lines from `main()`. Natural home for chapters, playback speed, etc.

```
wire_controls()             (registers on_pause_play_toggle, on_seek_*, on_stop_playback,
                             on_seek_to, on_skip_intro, on_select_sub/audio/video,
                             on_commit_panel_selection, on_volume_*, on_show_controls,
                             on_resume_player, on_mute_toggle, on_toggle_stats,
                             on_minimize_player)
```

**Dependencies:** `fjord_player`, `slint`, `playback::VideoState`

---

### `main.rs` (after split)
What remains:

```
slint::include_modules!()   (must stay here — generates MainWindow type)
is_unauthorized()
ss()
item_to_card_item()
items_to_model()
push_section_model()
to_slint_model()
display_names()
apply_settings_to_window()
read_settings_from_window()
fn main()                   (apply saved config on startup, wire all modules,
                             std::mem::forget on timers, window.run())
```

The model helpers (`ss`, `items_to_model`, etc.) are small and tightly coupled to
`MainWindow` — they stay in `main.rs` rather than creating a trivial `ui.rs`.
`apply_settings_to_window` / `read_settings_from_window` are pure wiring between
`AppState` and `MainWindow` properties — same rationale.

---

## Step-by-step execution order

Do these in order. After each step: `cargo build` must succeed before continuing.

1. **Create `config.rs`** — move `Config`, `AppState`, path helpers, item cache
   load/save, `ensure_device_id`, `non_empty`, `fmt_resume_label`, `default_*`.
   Add `mod config;` to `main.rs`.

2. **Create `stats.rs`** — move `update_stats_window`. Smallest module, safest
   to do early. Add `mod stats;`.

3. **Create `playback.rs`** — move `VideoState`, `fmt_secs`, `build_track_model`,
   `create_fbo`, `delete_fbo`, `start_playback`. Extract rendering notifier into
   `wire_rendering_notifier` and mpv poll timer into `wire_mpv_timer`. Add `mod playback;`.

4. **Create `home.rs`** — move `HomeData`, `home_cache_path`, `load_home_cache`,
   `save_home_cache`, `fetch_home_data`, `push_home_data`, `home_data_sections`.
   Extract not-watched timer into `wire_nw_timer`. Add `mod home;`.

5. **Create `poster.rs`** — move `fetch_poster_cached`, `fetch_backdrop_cached`,
   `decode_poster_buffer`, `spawn_poster_loading`, `spawn_series_poster_loading`.
   Add `mod poster;`.

6. **Create `movies.rs`** — move `spawn_movies_poster_loading`. Add `mod movies;`.

7. **Create `series.rs`** — move `EpisodeRaw`, `make_episode_raw`, `raw_to_entry`,
   `spawn_episode_thumb_loading`, `open_series_screen`. Add `mod series;`.

8. **Create `detail.rs`** — extract the detail page logic out of the `on_open_detail`
   closure into `detail::open_detail(...)`. Add `mod detail;`.

9. **Create `browse.rs`** — move `update_library_filter` and extract filter/search/nav
   callback bodies into `browse::wire_browse(window, state)`. Add `mod browse;`.

10. **Create `auth.rs`** — extract `on_do_login` body into `auth::do_login(...)`.
    Add `mod auth;`.

11. **Create `controls.rs`** — move all player control callbacks into
    `controls::wire_controls(window, video)`. Add `mod controls;`.

12. **Final `cargo build --release`** — confirm the release build is clean.

13. **Smoke-test** — run the app, log in, browse home/movies/TV, open detail,
    open a series, play an episode, check stats overlay, test player controls.

14. **Split `fjord-api/src/models.rs`** — create `models/` subdirectory with
    `mod.rs` (re-exports), `common.rs`, `movie.rs`, `series.rs`, `episode.rs`,
    `person.rs`. `cargo build` must pass with no changes to callers.

---

## `fjord-api` crate split

`fjord-api/src/models.rs` is one big file. Split it into per-type modules
as a separate step after the `fjord-app` split is done and merged.

```
fjord-api/src/models/
    mod.rs          re-exports everything (keeps external API unchanged)
    common.rs       shared types (UserData, MediaStream, PersonInfo, etc.)
    movie.rs        Movie-specific fields
    series.rs       Series, Season
    episode.rs      Episode-specific fields
    person.rs       PersonInfo, cast types
```

Add this as step 14 in the execution order after the smoke-test.

---

## Risks and mitigations

| Risk | Mitigation |
|---|---|
| Circular imports (series.rs calls poster.rs calls config.rs) | Draw the dependency arrows before creating files — config ← poster ← series is one-directional |
| `pub` scope creep | Default to `pub(crate)`. Only add `pub` when the compiler complains |
| Breaking the GL thread invariants in playback.rs | Move the structs only; leave the render notifier closure in main.rs |
| Slint `include_modules!()` types not visible from new modules | Import `crate::MainWindow` (or the generated type) via `use crate::*` in each module that needs it |
