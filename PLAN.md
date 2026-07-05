# Fjord — Development Plan

## Goal

A native Jellyfin frontend for Linux built with Rust and Slint. Uses the mpv render API so mpv renders directly into an OpenGL FBO, enabling `report_swap()` for vsync feedback — the approach that avoids choppy playback on NVIDIA legacy Wayland drivers.

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
| 13 — EAC3 passthrough diagnosis (2026-06-20) | Root cause (#39): tsched software timer caused PipeWire RT thread to miss 21.3 ms deadline at 192 kHz IEC61937 rates under GPU load. Fix: `api.alsa.disable-tsched=true` (hardware IRQ wakeups) + `suspend-timeout-seconds=2`. Now a Settings toggle. Do not use `api.alsa.headroom` — shifts audio timeline, causes frame drops under `display-vdrop`. |
| 14 — Settings: SPDIF per-format toggles, HDR passthrough row, virtual rows (2026-06-21) | Per-format SPDIF toggles (AC3/EAC3/DTS/DTS-HD/TrueHD) replace single passthrough switch. Tone-mapping row hidden when HDR passthrough on. Video-latency-hacks row hidden unless display-resample active. Cross-section passthrough+display-resample conflict warning. |
| 15 — Audio output device selector (2026-06-21) | Dropdown in Settings → Audio populated from `mpv --audio-device=help`. Device stored in `Config.audio_device`; applied to mpv at playback start. Content-driven popup width; keyboard nav fixed. |
| 16 — PipeWire IRQ scheduling toggle (2026-06-21) | Settings → Audio toggle (visible when SPDIF on + PipeWire/auto device). Writes/deletes `~/.config/wireplumber/wireplumber.conf.d/fjord-alsa-irq.conf` and restarts WirePlumber on change. Config persists after exit; syncs down to false on startup if file missing. |
| 17 — Intro Skipper API v2 + generalized skip segments (2026-06-21) | Migrated from two old endpoints to single `GET /Episode/{id}/Timestamps` returning all 5 segment types. `EpisodeTimestamps` model with `Segment { start, end }` for Introduction, Credits, Recap, Preview, Commercial. Single generic skip overlay replaces `show-skip-intro`. Timer checks segments in priority order (Intro → Recap → Preview → Commercial). Up Next banner: `next-ep-banner-focused` (0=Play Now, 1=Skip); Left/Right toggle, Enter activates. |
| 18 — Per-segment skip modes + configurable timers (2026-06-21) | Each of Intro/Recap/Preview/Commercial has 4 modes: `always-skip`, `ask`, `ask-timed` (auto-skips on countdown expiry), `never-skip`. Credits has 3 modes. All 10 new `Config` fields persisted in JSON. Settings → Player section extended with rows 4–13 + INTRO SKIPPER / CREDITS section headers. `VideoState` extended with `skip_segment_handled`, `skip_timed_shown_at`, `skip_timed_prompt_secs`. |
| 19 — Movie detail enrichment: cast, collection, similar (2026-06-21) | Director, writer, tagline, studio; `BackdropHero`/`PosterBlock`/`MetaLine` atoms extracted to `widgets.slint`. `CastRow` atom with async portrait fetch. `fetch_movie_collections` background task builds BoxSet membership map; collection `SectionRow`. `get_similar_items` "More Like This" `SectionRow`. |
| 20 — CR5: post-enrichment bug fixes (2026-06-21) | `fetch_movie_collections` now spawned in auto-login path. Portrait index mismatch fixed. `SectionRow.item-play` passes `item_type`. CastRow focus ring visibility fix. Back from detail resets stale collection/similar models. BoxSet items sorted by ProductionYear. |
| 21 — Detail page keyboard nav + stop-returns-to-detail (2026-06-21) | `open_detail` calls `invoke_grab_keyboard_focus()`. `VideoState.from_detail` flag restores detail page on stop/natural-end. `on_minimize_player` clears flags. `main.slint` hides DetailPage when `is_playing`. |
| 22 — Series season episode cache + race fix (2026-06-22) | Season episodes cached in `FjordState.series_episode_cache`; cleared on series switch. `series_season_generation` counter prevents stale async results from rapid tab navigation. |
| 23 — Series detail UX polish (2026-06-22) | "✓ Watched" button on series detail. `PosterBlock` extended with played/resume/unplayed badges. Season row focus indicator (accent bottom border). C key on Next Up card. Default focus on episode row; Next Up steals focus when data arrives. |
| 24 — Back button + series/detail header keyboard nav (2026-06-22) | `series-focused-btn` (-1=not in header, 0=Back, 1=♥, 2=✓). Detail `detail-focused-btn = -1` = Back. Season `season-focused-back`. All Back buttons gain `kbd-focused` ring. |
| 25 — Crash fix: series screen "Recursion detected" (2026-06-23) | `kb-x` in season tabs replaced `self.width` with `root.width` to break layout cache re-entrancy cycle. |
| 26 — UI polish: backdrop header, icon circle buttons, ends-at, load-then-show (2026-06-23) | "Ends HH:MM" below action buttons. `IconCircleButton` component (38 px circle). Backdrop fills header block height. `open_detail`/`open_series_screen` defer show until `spawn_main` completes; loading overlay with spinner + progress bar. |
| 27 — UI polish: icon centering, spinner size, portrait preload, progress bar (2026-06-23) | `IconCircleButton` text centred (explicit width/height), font-size 20 px. Spinner dots 14 px. Cast portraits fetched before page shown (no trickle-in). `app-loading-progress` property; 240 px animated progress bar in loading overlay. |
| 28 — Person detail screen (2026-06-23) | Enter on any cast member opens PersonScreen: portrait + bio + filmography SectionRow. `AppMode::Person` (priority above Detail). `get_person_filmography` API endpoint. `CastRow.item-selected` callback wired from detail/series/season screens. Mouse click on cast card also opens person. `close-person` Back button + keyboard Back. Loading overlay (spinner + progress bar) matching deferred show pattern of detail/series screens. |
| 29 — Player: minimal keyboard-pause bar (2026-06-23) | Space-to-pause no longer reveals the full controls bar. Instead shows a slim 52 px minimal bar (seek progress + current/total time + "Ends at") via `pause-bar-visible` flag. Space-to-resume immediately hides both the minimal bar and the full controls (even if the full bar was open from mouse). Mouse click resume also clears `pause-bar-visible`. |
| 30 — Player: seek accumulation OSD (2026-06-23) | Keyboard Left/Right accumulate into a debounced seek instead of seeking immediately. OSD shows direction + total delta + target time ("▶▶ +20s → 1:23:45"). Seek executes ~480 ms after the last key press. Rapid presses add up. Mouse button seeks remain immediate. |
| 31 — Mini-player bar redesign (2026-06-24) | Replaced sidebar "Now Playing" card with `MiniPlayerBar`: full-width bar docked at top, window-aware (all screens offset by `bar-h`). Mode 3 (!video-behind-ui): 108px with live thumbnail + title + buttons. Mode 2 (video-behind-ui): 56px compact bar (no thumbnail, video fills window) with NOW PLAYING label + title + FjordButton Resume/Stop. Video-behind-UI uses dual video layers in main.slint: layer 1 (z=below AppShell) shows video through transparent screen roots on dashboard/library; layer 2 (z=above AppShell, only when overlay open) prevents library cards from ghosting through detail/series/season/person screens. Dim overlay #00000044 (~27%). `float-card-focused` (-1/0/1); `focus_bar_on_up` called at end of every mode arm — Up returns `false` from all screens when at topmost position (Back button in detail/season, header in person, no-prev-section in dashboard, search header in library) so the bar is reachable via Up from any screen. |
| 32 — Code review CR6 (2026-06-24) | 13 issues from full-codebase review. CR6-1: sign-out now clears episode/collection caches, `movies_fetched`, and all overlay AppState flags. CR6-2+12: consolidated XDG path helpers (`xdg_config_base`/`xdg_cache_base`) in `config.rs`; `$HOME` unset logs a `tracing::error!` instead of silently using a relative path. CR6-3: removed `next_ep_pending = None` from countdown task's `!still_playing` branch — natural-end path owns that field exclusively (race nearly guaranteed with fallback 30s trigger + 30s countdown). CR6-4: `get_all_movies` and `get_all_series` now paginate via shared `get_all_paged()` helper (parallel 1000-item pages); Jellyfin's `MaxPageSize` can no longer silently truncate large libraries. CR6-5: season screen deferred to match detail/series — portraits fetched in parallel before page is shown (no trickle-in). CR6-6: `spawn_collection` retry loop bails early when detail page moves on, releasing `Arc<JellyfinClient>` promptly. CR6-7: `person.rs` Left/Right return `false` in header row (were dead keys). CR6-8: removed duplicate `HomeData.recently_added` field (clone of `recently_added_tv`). CR6-9: `invoke_from_event_loop().ok()` → `let _ =`. CR6-10: `on_resume_player` resets `float_card_focused = -1`. CR6-11+13: extracted `push_decoded_section` and `push_decoded_series` helpers, eliminating ~130 lines of duplicated decode+push logic in `poster.rs`. |
| 33 — Code review CR7 (2026-06-24) | 15 findings from full-codebase review (Phases 21–32). CR7-1: always-skip auto-advance fallback when EOF beats next-up fetch (captures `playing_series_id` pre-teardown, spawns fresh `get_next_up_for_series` if `next_ep_pending` is None). CR7-4: Next Up episode `is_favorite` hardcoded false — now reads `user_data.is_favorite`. CR7-5: `context_play_from_start` used `next.series_id` (Jellyfin can omit it) instead of known `id`. CR7-6: TOCTOU between generation check and `next_ep_pending` write merged into single lock scope. CR7-7: context menu `focused` not reset when item marked played — Enter would invoke Resume on fully-played item. CR7-8/9: Left from ♥ (btn=1) in season and series header did nothing (`> 1` → `>= 1`). CR7-10: `update_series_unplayed_count` now also updates `AppState.series_unplayed_count` when the series screen is open. CR7-11/12: loading-progress 50% invokes in season and person screens now guarded by stale season-ID / person-ID check. CR7-13: `spawn_series_poster_loading` now deduplicates IDs before spawning (prevents premature push on duplicate server entries). CR7-14: sign-out clears `last_nw_mov_refresh` / `last_nw_tv_refresh`. CR7-15: CLAUDE.md updated — AppMode variant count 8→10, added `person::handle_key` + `season::handle_key` to dispatch table, ResumePlayer exclusion list updated. |
| 34 — Sort, filter, and alphabet scrubber in library grid (2026-06-25) | Client-side sort (Name A-Z/Z-A, Year↓/↑, Random via LCG pseudo-shuffle) and Unwatched/Favorites filters applied to cached `all_movies`/`all_series`. Sort persisted per library type in `Config.library_movies_sort`/`library_series_sort`. `refresh_library_display` central function (browse.rs) rebuilds `library-display` and `library-alpha-offsets` (27-element vec, A-Z+#, each -1 when letter has no items). Sort bar UI (40 px strip) with 5 sort pills + 2 filter toggles. Alphabet scrubber: right-edge 22 px strip, visible only when sort=A-Z and no query; click or A-Z key jumps grid. Full arrow-key nav chain: grid → search → sort bar → Back button → mini-player (each layer reachable via Up). Sort pills auto-apply on Left/Right navigation (no Enter needed); filter toggles (Unwatched/Favorites) still require Enter and keep the bar focused. Focus indicator: 2 px border (accent when inactive, white ring when on the active pill) + surface-overlay background so cursor is always visible. Down from sort bar moves to search field (not grid). `library-back-focused` state: Back button gets kbd-focused ring; Enter/Esc closes library; Down returns to sort bar. Loading overlay and LoadingSpinner improved: 20 px dots (was 14 px), 260 ms cadence, 75% dim (was 53%), 6 px progress bar (was 4 px, 320 px wide). |
| 35 — Error toast notifications (2026-06-25) | `toast-message: string` + `toast-visible: bool` added to `AppState`. `ToastNotification` component in `widgets.slint`: bottom-center pill, dark semi-transparent background, 4 px red left accent stripe, word-wrap text. Positioned and sized by caller (main.slint). Z-order above ContextMenu (last element in MainWindow). `show_toast(ww, msg)` helper in `main.rs`: any-thread safe via `invoke_from_event_loop`. Auto-dismiss: local `_toast-vis` mirror property + `changed _toast-vis` fires `toast-timer` (4 s Slint Timer, self-stops after trigger). Wired into: `start_playback` Player::new failure ("Couldn't start playback — check your server connection"), `context-mark-played` failure ("Couldn't update watch status"), `context-toggle-fav` failure ("Couldn't update favourite"). |
| 41 — WebSocket real-time events (2026-06-25) | `JellyfinClient::ws_url()` builds `ws[s]://host/socket?api_key=…&deviceId=…`. `ws.rs` module (tokio-tungstenite): persistent reconnect loop with exponential backoff (1 s → 60 s). **LibraryChanged** → debounced 5 s refresh of home dashboard rows (re-runs `fetch_home_data`, updates `AppState` models + poster cache, saves `home.json`). **UserDataChanged** → `update_card_in_all_models` patches played/fav on every visible card immediately (also updates Rust-side vecs via `update_item_user_state`). **ForceKeepAlive/KeepAlive** → sends `{"MessageType":"KeepAlive"}` response. `FjordState.ws_abort: Option<AbortHandle>` stores the task handle; `abort()` called on sign-out. WS started after both auto-login and manual login. |
| 37 — Chapter navigation (2026-06-25) | `Player::get_chapters()` reads `chapter-list/{N}/time` + `chapter-list/{N}/title` after 2 s. `Player::chapter_step(±1)` uses `add chapter`. `VideoState.chapters: Vec<(f64,String)>` (retries up to 30 ticks if count=0). **Seek bar tick marks**: `AppState.chapter-marks: [float]` (normalised 0–1); rendered as 2 px semi-transparent white rectangles inside `seek-track`. **Keys**: `,` = prev chapter, `.` = next chapter (`NextChapter`/`PrevChapter` actions in player map; excluded from shows_controls). **Chapter OSD**: `chapter-osd-text` + `chapter-osd-visible` on AppState; 36 px top-left pill shows "▸ Chapter Name" for ~2 s (`chapter_osd_ticks` countdown in 16 ms timer). OSD name computed immediately from `vs.chapters` + current position without waiting for mpv event. |
| 38 — Sub/audio delay adjustment (2026-06-25) | `Player::adjust_sub_delay(ms)` / `adjust_audio_delay(ms)` call `add sub-delay`/`add audio-delay` and return the new value. **Keys**: `z`/`Z` nudge sub-delay ±100 ms, `x`/`X` nudge audio-delay ±100 ms (matches mpv defaults; remappable; 4 new `Action` variants). **Delay OSD**: `delay-osd-text` + `delay-osd-visible` on AppState; `delay_osd_ticks` countdown in 16 ms timer (~2 s); pill at y:68 px (below chapter OSD at y:24 px to avoid overlap). `fmt_delay_ms(label, secs)` helper in `controls.rs`. Reset cleared on `reset_playback_ui`; no persistence (mpv state resets with each new Player). |
| 39 — Ratings and genres on detail/series pages (2026-06-25) | `detail-rating-label: string` ("★ 7.4") and `detail-genres: string` ("Drama, Crime") added to `AppState`; same for series (`series-rating-label`, `series-genres`). Populated in `detail.rs` and `series.rs` `spawn_main` from `MediaItem.community_rating` + `genres`. `MetaLine` widget renders rating in gold (`#f5c518`) after the year/runtime chip. Genres rendered as plain text line below MetaLine in both `detail.slint` and `series.slint`. |
| 40 — Collections library screen (2026-06-25) | New sidebar nav order: Home(0), TV Shows(1), Movies(2), Collections(3), Music/placeholder(4), Browse All(5). Nav references updated across `layout.slint`, `main.slint`, `app_state.slint`, `browse.rs`, `keys.rs`, `home.rs`. `AppMode::Collection` added between Series and Player. `library-has-filters: bool` property (false for nav=3) hides Unwatched/Favorites filter toggles and caps sort-bar cursor. `library_collections_sort` persisted in `Config`. `FjordState.all_collections`/`collections_fetched` for lazy fetch. `on_open_library(3)` fetches BoxSets once per session via `get_all_boxsets()`. `CollectionScreen` (new `collection.slint` + `collection.rs`): backdrop hero + Back button + member poster grid. `on_open_collection` wired in `main.rs`: fetches BoxSet items + all posters + BoxSet poster/backdrop in parallel, defers `show-collection` until all data ready. `handle_key` covers grid nav, Enter→detail, C→context-menu, Back-button focus. |
| 42 — Code review CR8 (2026-06-25) | 6 findings from Phase 40 review. CR8-1: `open_collection_screen` now returns early on `get_boxset_items` error and calls `show_toast` ("Couldn't load collection…") instead of showing a blank screen. CR8-2: `layout.slint` double-click handlers for TV/Movies/Collections now call `open-library(nav)` — previously skipped, preventing lazy network fetch, sort restore, and `library-has-filters` initialisation. CR8-3: sign-out block now calls `set_show_collection(false)` and `set_all_collections(empty_model)` — previously `show-collection` could stay true after sign-out, routing keyboard events to the collection handler with a null client. CR8-4: stale-request guard added to `invoke_from_event_loop` closure in `open_collection_screen` — aborts if `collection-id` no longer matches (rapid double-open). CR8-5: `items_to_cards` now copies `series_id` from `MediaItem` — `update_card_in_all_models` can now match episode cards in `collection-items` by series. CR8-6: backdrop fetch in `open_collection_screen` is now conditional on `backdrop_image_tags` (fetches `get_item_detail` in the join, skips the backdrop HTTP call for BoxSets that have none). |
| 43 — Collections dashboard (2026-06-26) | `CollectionsDashboard` component (new, in `home.slint`): two `SectionRow` rows — "Recently Added" and "Unwatched" — shown when `active-nav == 3 && !show-library`. Two new Jellyfin API methods in `fjord-api/client.rs`: `get_recently_added_collections` (15 newest BoxSets by DateCreated) and `get_unwatched_collections` (15 unplayed BoxSets, random order). `HomeData` extended with `recently_added_collections` + `unwatched_collections`; both fetched in parallel in `fetch_home_data` and set in `push_home_data`. `home_data_sections` array extended from 9→11 elements (indices 9+10); `spawn_poster_loading` updated to handle 11 sections; `push_section_model` cases 9→`recently-added-collections`, 10→`unwatched-collections`. `on_item_play` in `main.rs` now routes BoxSet IDs (found in `all_collections`) to `collection::open_collection_screen` before attempting playback. Sign-out clears both new AppState properties. |
| 44 — Collections library warm-start cache (2026-06-26) | `load/save_collections_cache` added to `home.rs` (mirrors movies/series pattern). On warm start, `collections.json` is loaded into `FjordState.all_collections` + `AppState.all_collections`, `collections_fetched` set true, and poster loading started — so the Collections grid shows content immediately on the first open instead of appearing blank while the network fetch completes. `save_collections_cache` called after each successful `get_all_boxsets()` fetch in `on_open_library(3)`. CLAUDE.md disk-caches section updated. |
| 43c — CR9: Collections dashboard cleanup (2026-06-26) | **Cleanup**: `spawn_movies_poster_loading` and `spawn_collections_poster_loading` collapsed into a single `spawn_library_poster_loading(…, LibraryKind)` + shared `push_library_cards` helper, with thin public wrappers preserved for call sites. `LibraryKind` enum encodes the three things that differ: `item_type`, `active_nav` guard, and which AppState setter to call. **Altitude**: `push_section_model` now takes `HomeSection` (named enum with `#[repr(usize)]`) instead of `usize`. `home_data_sections` returns `[(HomeSection, Vec<MediaItem>); 11]`; `spawn_poster_loading` accepts the same type and builds a `[HomeSection; 11]` array to pass through `push_decoded_section`. `wire_nw_timer` uses `HomeSection::NotWatchedMovies as usize` and `HomeSection::NotWatchedTv as usize` for array indexing — silently wrong integer insertions are no longer possible. `HomeSection::empty_array()` provides the zero-filled base for partial fills. **Convention**: `app_state.slint` imports moved to after the `// ──` header block as required by CLAUDE.md. |
| 43b — CR9: Collections dashboard bug fixes (2026-06-26) | **Critical**: `on_item_play` BoxSet routing now falls back to the always-populated dashboard models (`recently-added-collections` / `unwatched-collections`) when `all_collections` is empty (only populated after the library grid opens). **High**: `spawn_collections_poster_loading` `set_library_display` guard now requires `active-nav == 3` — previously could overwrite the movies grid if the user switched tabs during an async poster fetch. Same guard added to `spawn_movies_poster_loading` (`active-nav == 2`). **Medium**: `remove_from_dynamic_rows` now also filters `unwatched_collections` — played BoxSets were previously left in the Unwatched row with a ✓ badge. **Low**: `spawn_collections_poster_loading` now calls `set_all_collections(empty)` immediately for empty BoxSet lists (the while-let JoinSet loop was skipped, leaving a stale model). |
| 42 — Poster / backdrop cache cleanup (2026-06-26) | `run_poster_cache_cleanup(movie_ids, series_ids, collection_ids)` added to `home.rs`. Spawned as a background Tokio task after every auto-login. Builds a known-ID set from `all_movies ∪ all_series ∪ all_collections`, walks `~/.cache/fjord/posters/` and `~/.cache/fjord/backdrops/`, deletes files whose name (= item ID) is not in the set. Two guards: (1) skip if combined ID set is empty (handles network error / first run); (2) 24 h minimum interval via `~/.cache/fjord/last_cleanup` (ASCII Unix timestamp). `poster_cache_dir()` and `backdrop_cache_dir()` helpers added to `config.rs` alongside existing `poster_cache_path(id)` / `backdrop_cache_path(id)`. Portrait/season/episode cache files that fall outside the known ID set are re-fetched transparently on next access. |

| 45 — Artist library screen (2026-06-26) | Music sidebar Enter → artist library grid → ArtistScreen → AlbumScreen. `get_album_artists()` + `get_artist_albums()` API methods. `LibraryKind::Artists` + `spawn_artists_poster_loading()`. `FjordState.all_artists`/`artists_fetched`; `artists.json` warm-start cache. `library_artists_sort` in `Config`. `AppMode::Artist` (between Series and Collection in priority). `artist.slint` ArtistScreen: circular portrait + meta + album grid. `artist.rs` `open_artist_screen` (parallel albums + portrait fetch, semaphore(8) poster load, gen-guarded). `on_open_detail` routes `MusicArtist` to `artist::open_artist_screen`. `run_poster_cache_cleanup` extended with `artist_ids` parameter. `library-has-filters` false for nav=4. Main.slint: nav=4 + `show-library` → `LibraryGrid` "All Artists"; `ArtistScreen` overlay between SeasonScreen and CollectionScreen; second video layer condition includes `show-artist`. |
| 47 — Playback queue (2026-06-27) | Context menu gains "Play Next" (row 2, insert at front) and "Add to Queue" (row 3, append to back) for any item — movies, episodes, album tracks. `QueueItem { id, item_type, series_id, title }` struct in `playback.rs`. `VideoState.queue: VecDeque<QueueItem>`. Natural-end path in `wire_mpv_timer` pops the queue when `!had_series` and `next_ep_pending` is None. `wire_queue_callbacks` in `context_menu.rs` wires the two callbacks; title is looked up from `FjordState` models (all_movies/all_series/all_albums/all_artists/series_episode_cache) at enqueue time — no Slint callback signature changes. `queue-count: int` in AppState; set on enqueue and after each pop. Cleared on sign-out. Series Up Next banner unchanged (series auto-advance takes priority over queue). |
| 46 — Music library Artists/Albums view toggle (2026-06-27) | Sort bar on the Music library grid (nav=4) gains [Artists] / [Albums] view toggle pills at cursor positions 5 and 6. View pills require Enter to apply (like filter toggles) — cursor navigates freely without switching the view, so the user can move from Albums to the sort pills without accidentally resetting the view. `library-music-view: int` (0=Artists, 1=Albums) in `AppState`; radio-button semantics (always one selected, both-unselected is impossible). `library_music_view: u8` persisted in `Config`. `get_all_albums()` API method (all MusicAlbums, SortBy=SortName). `LibraryKind::Albums` variant added; `matches_library_display()` method guards against Artists/Albums overwriting each other's grid on async completion. `spawn_albums_poster_loading()` wrapper. `FjordState.all_albums`/`albums_fetched`; `albums.json` warm-start cache; `load/save_albums_cache` in `home.rs`. `library_albums_sort: u8` persisted in `Config`. `on_open_library(4)` fetches artists and albums in parallel (each lazy, once per session); restores `library_music_view` and correct sort from config. `on_library_music_view_changed` persists config, restores sort, and calls `refresh_library_display`. Sign-out clears `all_albums`, `albums_fetched`, `set_all_albums(empty)`, `set_library_music_view(0)`. `run_poster_cache_cleanup` extended with `album_ids` parameter. |
| 48 — Alphabet scrubber keyboard nav + Music sort bar redesign (2026-06-27) | Removed A-Z letter-key shortcuts from `dispatch_library` (were intercepting `C`=context-menu and other single-letter actions). Replaced with a proper 5th library focus state: `library-scrubber-focused: bool` + `library-scrubber-cursor: int` (0=A..26=#) in `AppState`. Entry: sort bar Right past last pill when sort=A-Z and no query. In scrubber: Up/Down navigate A-Z/#, Enter jumps grid + returns to grid focus, Back/Left returns to sort bar. Scrubber cell for focused letter renders with `accent-muted` background + accent bold text. All sort pills now require Enter to apply (was auto-apply on navigate); cursor starts on the active sort pill when entering the sort bar (`sort_bar_init_cursor` helper). Music sort bar layout redesigned: [Artists(0)][Albums(1)] at LEFT (reached by pressing Left from sort pills) | Sort: [A→Z(2)][Z→A(3)][Year↓(4)][Year↑(5)][Shuffle(6)] | [Favorites(7)] at RIGHT — Music max cursor now 7. `pill-focused` in UI uses `si + (nav==4 ? 2 : 0)` offset. |

---

## Pending

---

### 🔴 Phase 53 — Code review CR10 (2026-07-06) — fix one at a time

Full-codebase review (all three crates + Slint UI). Findings ordered by severity; check off as fixed.

**Critical — crashes and data bugs**

- [x] **CR10-1** *(fixed 2026-07-06)* `Handle::current()` panics in three Slint callbacks — `main.rs` `on_toggle_artist_fav` / `on_toggle_album_fav` / `on_toggle_album_played` called `tokio::runtime::Handle::current()` on the Slint event-loop thread, which never enters the Tokio runtime. Pressing ♥ on the artist screen or ♥/✓ on the album screen panicked. Fixed by capturing `rt.handle().clone()` into each closure (like every other callback) and passing the captured handle to `refresh_favorites`.
- [x] **CR10-2** *(fixed 2026-07-06)* Stale playlist resurrected old music after unrelated playback — `start_playback` never cleared `vs.playlist`/`vs.queue`, so a movie's natural-end (or a single-track play) resumed a leftover album. Fixed with `VideoState.keep_playlist: bool` (same pattern as `from_detail`): playlist-driven callers (Play All album/artist, prev/next track, queue jump, natural-end advance) set it right before `start_playback`; `start_playback` consumes it and, when false, wipes playlist+queue+shuffle_order and resets queue-count/panel/display. `do_stop_playback` (user stop) now also clears playlist+queue and resets the queue UI.
- [x] **CR10-3** *(fixed 2026-07-06)* API token was written to `fjord.log` — `direct_play_url` embeds `api_key=<token>` and the full URL was logged at info level in both `playback.rs` (`start_playback`) and `mpv.rs` (`Player::new`). Added `fjord_player::redact_api_key(url)` (replaces the `api_key=` value with `REDACTED`) and applied it to both log lines.
- [x] **CR10-4** *(fixed 2026-07-06)* Quit keybinding destroyed by queue-panel binding — in `default_normal_map`, `q`/`Q` → `OpenQueuePanel` (Phase 51) overwrote `q`/`Q` → `Quit` in the same HashMap; fresh installs / reset-to-defaults had no Quit key at all. Resolution: Quit's default is now **Ctrl+Q** (desktop convention); plain `q`/`Q` stays with the queue panel as Phase 51 intended. The loading-overlay quit bypass in `handle_key` updated to require Ctrl. Sidebar Quit item unchanged for remote/HTPC use. CLAUDE.md shortcut docs updated. Note: users with a pre-Phase-51 `keybindings.json` keep whatever they had (file replaces defaults).

**High — broken features**

- [ ] **CR10-5** Shift+letter bindings never match — `main.slint:113` passes `event.modifiers.shift` into the KeyCombo lookup, but all uppercase defaults are stored with `shift: false` (`KeyCombo::plain("Z")`). Shift+z produces `{key:"Z", shift:true}` → no match → `Z`/`X` (sub/audio delay **decrease**) are dead from the keyboard. Verify at runtime, then normalize the lookup (retry with `shift=false` for single printable chars).
- [ ] **CR10-6** Queue panel never shows "Add to Queue" items — `push_queue_display` (`main.rs:159`) renders only `vs.playlist`; `on_queue_add_item` pushes to `vs.queue`. Also `queue-count` means "context queue length" in the add callbacks but "remaining playlist tracks" in the advance path (`playback.rs:1812`). Render both collections; unify the count.
- [ ] **CR10-7** Enqueuing a Series card creates an unplayable queue item — context-menu Play Next / Add to Queue store `item_type` verbatim; dequeue calls `direct_play_url(series_id)` which can't stream. Resolve series to next-up episode at enqueue (like `on_item_play`) or hide those rows for Series.
- [ ] **CR10-8** "Play Next" broken under shuffle after a few tracks — `context_menu.rs:444` inserts at `shuffle_order` slot 1, but the current item is only at shuffle position 0 right after toggling. Once playback advances to position *k*, the insert lands behind the cursor and never plays. Insert at (current shuffle position + 1).
- [ ] **CR10-9** Wrong nav guard in `push_decoded_series` — `poster.rs:140` refreshes the library grid on `active-nav == 2` (Movies) but series belong to nav 1 (TV). TV grid shows poster-less cards after cold open; Movies grid gets a spurious refresh that re-shuffles sort=Random under the user. Change to `== 1`.
- [ ] **CR10-10** `update_item_user_state` skips music — `config.rs:331` patches movies/series/collections/filtered but not `all_albums`, `all_artists`, or `series_episode_cache`. Fav/played toggles on music update Slint models but not canonical vecs, so later model rebuilds revert the badges.
- [ ] **CR10-11** WebSocket task can die permanently on a byte-slice panic — `ws.rs:117` does `&text[..len.min(120)]`, which panics mid-UTF-8-char; `fjord_app` runs at debug level so it evaluates for every non-JSON message, and a panic kills the whole `ws_loop` (reconnect loop included). Use `chars().take(120)` like `client.rs:412`.
- [ ] **CR10-12** Sign-out deletes the DeviceId — `main.rs:2259` removes `config.json` wholesale, so next login generates a new device id (the exact multi-session token-invalidation scenario CLAUDE.md calls critical). Clear auth fields but keep `device_id` and settings.
- [ ] **CR10-13** Up-Next marks the current episode played ~30 s early — `playback.rs:1636-1639` fires `mark_played` as soon as the banner triggers, even in "ask" mode; if the user hits Skip and stops, the episode stays marked played and the resume point is lost. Only mark played on actual advance (or filter the next-up query differently).

**Medium**

- [ ] **CR10-14** Reverse-proxy subpath servers unsupported — every endpoint (incl. `authenticate`) uses `Url::join("/Users/…")` with a leading slash, discarding any base path (`https://host/jellyfin` → `https://host/Users/…`). Join relative paths, or document the limitation.
- [ ] **CR10-15** mpv `poll()` treats any error event as end-of-file — `mpv.rs:219` returns `Finished` on `Some(Err(_))`; a transient mpv error event tears down playback.
- [ ] **CR10-16** Quit can hang up to 30 s — `quit_cleanup` does `rt.block_on` on the stop report with the client's 30 s timeout; unreachable server stalls app exit. Bound the wait (e.g. `tokio::time::timeout(3 s)`).
- [ ] **CR10-17** `queue_remove` of the playing row shifts `is_current` onto the next row while the removed track keeps playing (`main.rs:1873`).
- [ ] **CR10-18** `find_title_in_state` can't resolve episodes queued from home rows (only `series_episode_cache` is searched) → raw GUID shown in the queue panel (`context_menu.rs:381`).
- [ ] **CR10-19** series.rs episode-row `Down` returns `true` even with no cast/similar row below — music bar unreachable via Down from the series screen (album.rs returns `false` correctly). Same check for `season.rs` episode row.
- [ ] **CR10-20** `series.rs spawn_main` writes `series_open_id`/`series_episode_cache` into `FjordState` without a generation guard — rapid open A → open B can leave A's state behind B's UI.
- [ ] **CR10-21** `MpvRenderCtx` callback teardown race — `set_update_callback`/`Drop` (`mpv.rs:584-615`) free `cb_data` immediately after clearing the callback; mpv doesn't document synchronization with an in-flight callback on its thread. Theoretical use-after-free in unsafe code.

**Low — polish and doc drift**

- [ ] **CR10-22** `get_episode_timestamps` doc comment claims "errors on other HTTP failures" but returns `Ok(None)` (`client.rs:410-414`).
- [ ] **CR10-23** Progress reports always send `IsPaused: false` (`client.rs:327`) — Jellyfin dashboard shows paused sessions as playing.
- [ ] **CR10-24** Case-sensitive client-side re-sort in `get_all_items`/`get_all_paged` (`a.name.cmp`) disagrees with server `SortName` ordering — lowercase-first titles sort after Z.
- [ ] **CR10-25** CLAUDE.md drift: album screen docs still describe a ♥/✓ two-button row; the ✓ Watched button was removed (`album.rs` handles ♥ only).

**Reviewed clean**: `controls.rs`, `settings.rs`, `detail.rs`, `season.rs`, `artist.rs`, `collection.rs`, `person.rs`, `stats.rs`, `pipewire_fix.rs`, `movies.rs`, `browse.rs`, `auth.rs` (both crates), models (tested), and the Phase 50–52 Slint widgets (MusicPlayerBar / QueuePanel / LyricsView follow the documented `kb-y` Flickable pattern correctly).

---

### 🟡 Phase 47 — Queue management *(partially done)*

**Done**: context menu "Play Next" / "Add to Queue", `VecDeque<QueueItem>` backend, `queue-count`, auto-advance on natural end.

**Missing** (completed by Phase 50 + 51): queue viewer, reorder, remove, Prev/Next track, Shuffle, Repeat — the features that make it actually manageable.

---

### 🟢 Phase 49 — Bottom player bar + Music player bar *(done)*

Moved video mini-player from top to bottom. Introduced `MusicPlayerBar` (72 px, bottom) for audio-only playback. Audio items (`item_type == "Audio"`) set `is-audio-playing = true` instead of `is-playing = true` — no fullscreen player. Music bar shows album art (60×60), title, artist (click → open-album), ⏸/▶, ⏹, progress bar, elapsed/total. `start_playback` gains `audio_meta: Option<(artist, album_art_id)>` parameter. `reset_playback_ui` clears `is-audio-playing`. `play-album-all` callback enqueues all remaining tracks and starts track 0. `MiniPlayerBar` and all overlay screens now bottom-padded via `total-bar-h = bar-h + music-bar-h`.

**Deferred to Phase 50**: Prev/Next, Shuffle, Repeat, volume slider, ♥, `⋮` queue (right zone placeholder only). `is_audio_only` MediaStreams detection (currently keyed off `item_type == "Audio"`). Keyboard nav in music bar (`[`/`]`/`Space`/`K`/`S`). Artist navigation from music bar.

---

### ✅ Phase 50 — Playlist backend + Prev/Next/Shuffle/Repeat (2026-06-28)

Replace the queue backend so playback order is fully controllable.

**Backend overhaul**: `VecDeque<QueueItem>` → two-collection model. `playlist: Vec<QueueItem>` (the ordered album/artist track list; `playlist_index: usize` marks current) + `queue: Vec<QueueItem>` (context-menu enqueued items, plays after playlist). Natural-end advance checks playlist first, then falls back to queue. Context menu "Play Next" inserts at `playlist_index + 1`; "Add to Queue" appends to `queue`. `on_play_album_all` and `on_play_artist_all` now populate `vs.playlist` (not queue).

**`QueueItem` gains**: `audio_meta: Option<(String, String)>` — `(artist, album_art_id)` needed by music bar; stored per-item so playlist advance passes correct metadata to `start_playback`.

**`RepeatMode` enum** in `playback.rs` (Off/All/One). `playlist_next`/`playlist_prev` public helpers implement advance logic.

**Shuffle**: `shuffle: bool` + `shuffle_order: Vec<usize>` (LCG Fisher-Yates, no rand crate). Toggling moves current item to position 0 in shuffle order. `playlist_next` follows shuffle_order.

**Previous track**: pos < 2 s → `playlist_prev` → go back; pos ≥ 2 s → `seek_to(0.0)` (restart current). Mirrors Spotify.

**New `Action` variants**: `PrevTrack` (`[`), `NextTrack` (`]`), `ToggleShuffle`, `CycleRepeat` — all remappable. Global pre-dispatch fires when `is_audio_playing` from any non-ContextMenu mode.

**Music bar right zone**: ⏮ Prev / ⏭ Next / ⇌ Shuffle / ↺ Repeat buttons (slots 4-7) with focus borders and active-state accent colouring. `queue-shuffle` / `queue-repeat-mode` AppState properties drive visual feedback.

**`wire_mpv_timer` natural-end** respects RepeatMode::One (replay) / All (wrap) / Off (stop).

---

### ✅ Phase 51 — Queue viewer panel (2026-06-28)

A right-side overlay panel showing the full playback playlist.

**Entry points**: `Q` key in the video player or when `is_audio_playing` (any non-ContextMenu mode); ⋮ button (slot 8) in the music bar right zone (keyboard + mouse). `AppMode::QueuePanel` takes priority (after ContextMenu) in `active_mode()`.

**Panel**: 400px wide, dim overlay behind it (click outside → close). Header: "Queue — N of M" + "Clear All" button. Scrollable Flickable list: 40×40 album art thumbnail (♫ placeholder when not loaded) + index number · title · artist. Currently playing item: `#ffffff14` accent background + bold accent-coloured title + left stripe. Focused item: `surface-overlay` bg + left accent stripe. `kb-y` binding auto-scrolls to keep cursor centred.

**Keyboard**: Up/Down navigate `queue-panel-cursor`; Enter → `queue-jump(cursor)` → `start_playback` for that item, closes panel; Delete → `queue-remove(cursor)` splices item from playlist, clamps cursor; Back/Q → close.

**Jump-to-item**: `on_queue_jump(idx)` sets `playlist_index = idx`, closes panel, calls `start_playback`.

**Remove**: `on_queue_remove(idx)` splices from `playlist`, adjusts `playlist_index`, rebuilds `shuffle_order`, calls `push_queue_display`.

**Clear**: `on_queue_clear()` clears both `playlist` and `queue`, closes panel.

**`push_queue_display(vs, g)`** (in main.rs): rebuilds `AppState.queue-items: [QueueEntry]` from `vs.playlist` with `poster-id` set (album_art_id for audio, item id for video) and `has_poster: false`. Called after every mutation. `spawn_queue_poster_loading(client, ww, rt)` runs concurrently (Semaphore(8)) to fetch each row's art and fill `has_poster + poster` via `model.set_row_data(i, row)`, guarded by `poster-id` match to discard stale responses.

---

### ✅ Phase 52 — Lyrics (2026-06-28)

API: `GET /Audio/{itemId}/Lyrics` (Jellyfin 10.9+). Returns `{ Lyrics: [{ Start: int (ms), Text: string }] }` or 404 when absent/older server. Fetched in background at playback start for audio items via `client.get_lyrics(item_id)`. Result stored in `VideoState.lyrics: Option<Vec<(u64, String)>>` and pushed to `AppState.lyrics-lines: [LyricEntry]`.

**Lyrics view** (`LyricsView` in `widgets.slint`): full-window dark overlay (`#000000cc`), centred column 600px max. `Flickable` with `kb-y` auto-scroll keeps active line centred. Active line rendered in accent (16px bold), ±1 line in `text-muted`, others in `text-subtle`. When `start_ms == 0` for all lines (unsynced), all lines show static with no active-line tracking. Click anywhere to dismiss.

**Toggle**: ♪ button (slot 9) in music bar right zone — only rendered when `lyrics-available = true`; `L`/`l` key in normal map also toggles. `toggle-lyrics` callback in `AppState` (wired in `main.rs`). `show-lyrics` cleared on playback stop and new track start.

**Active-line tracking**: every 30 timer ticks (~500 ms), compare `pos * 1000 ms` against `lyrics[i].start_ms`; `rposition` gives the last line whose timestamp ≤ current position → `lyrics-active-idx`. Only runs when `show-lyrics && is-audio-playing`.

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
- **Vulkan rendering path** — second render backend alongside the current OpenGL path. Requires: Slint WGPU backend, `MpvRenderCtx` initialized with `MPV_RENDER_API_TYPE_VULKAN`, Vulkan FBO management replacing the current `gl::*` code. Enables true zero-copy decode on AMD (`hwdec=vulkan`, no CPU roundtrip). Legacy NVIDIA hardware needs OpenGL; selection persists in Config as `gpu_renderer: "opengl" | "vulkan"` and takes effect on next restart.
- Gamepad / remote control — d-pad maps to arrow keys; formal evdev/udev support deferred
- **Dashboard row reorder** — drag-to-reorder; part of the future theming/layout customisation update
- **Trickplay** — seek bar scrub thumbnail popup. Requires: fetch Jellyfin trickplay manifest (`GET /Videos/{id}/Trickplay/{width}/tiles`), parse tile sheet dimensions (tile size, columns, rows, interval), cache tile images per video, render a thumbnail above the seek bar while scrubbing (position computed from `seek-hover-pos`). Deferred because it's a separate subsystem from chapter nav and the API surface needs more investigation.
- **Multi-account / multi-server support** — currently Fjord stores one server URL + one user session in `config.json`. To support multiple accounts: `Config` would need a `Vec<ServerProfile>` (each holding server URL, device ID, username, token) with an `active_profile: usize` index; the login screen would gain a server-picker step; sign-out would become "switch profile" rather than "clear everything"; the `FjordState` runtime fields (`all_movies`, `all_series`, caches, etc.) would be cleared and repopulated whenever the active profile changes. CR6-1 (sign-out cleanup) is a prerequisite — it establishes the correct invariant that switching users produces a clean slate, which multi-account support then relies on.
