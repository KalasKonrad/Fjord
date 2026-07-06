# Fjord — Claude Code Context

Fjord is a Jellyfin media frontend built in Rust with Slint as the GUI toolkit and libmpv for video playback. It is built by KalasKonrad as a personal project, partly as a learning exercise in Rust and partly to solve a real problem: every existing Flutter-based Jellyfin frontend (Fladder, Jellyflix) uses media_kit which embeds mpv into a Flutter texture. That path never calls `mpv_render_context_report_swap()`, so mpv has no vsync feedback and playback is choppy on NVIDIA legacy drivers on Wayland. Fjord fixes this by using the mpv render API so mpv renders into an OpenGL FBO that Slint composites, with `report_swap()` called after every frame.

## Project structure

```
Fjord/
├── Cargo.toml                  workspace root
├── PLAN.md                     development roadmap
├── JELLYFIN.md                 Jellyfin API reference (endpoints, params, WebSocket events, caveats)
├── SLINT.md                    Slint best practices and gotchas for Fjord
├── README.md                   public-facing project description
├── crates/
│   ├── fjord-api/              Jellyfin REST API client
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── auth.rs         authenticate() — POST /Users/AuthenticateByName
│   │       ├── client.rs       JellyfinClient struct, all API calls; ws_url() builds ws[s]:// event URL
│   │       └── models/         serde types for Jellyfin responses
│   │           ├── mod.rs      re-exports all model types
│   │           ├── auth.rs     AuthResponse, UserDto
│   │           ├── intro.rs    Segment, EpisodeTimestamps (Intro Skipper plugin)
│   │           └── media.rs    MediaItem, UserData, StudioInfo, ItemsResponse
│   ├── fjord-player/           libmpv wrapper
│   │   └── src/
│   │       ├── lib.rs
│   │       └── mpv.rs          Player struct, MpvRenderCtx, FBO rendering
│   └── fjord-app/              Slint UI + main binary
│       ├── build.rs            compiles .slint files
│       ├── src/
│       │   ├── main.rs         entry point: apply saved config, wire modules, window.run()
│       │   ├── config.rs       Config (persisted JSON, all settings+auth), FjordState (holds config: Config + runtime state + movie_collections map), path helpers, load/save
│       │   ├── home.rs         HomeData, fetch_home_data, push_home_data, home cache, fetch_movie_collections
│       │   ├── poster.rs       fetch_poster_cached, decode_poster_buffer, spawn_poster_loading
│       │   ├── movies.rs       spawn_movies_poster_loading, spawn_collections_poster_loading, movie/collection library grid logic
│       │   ├── series.rs       ep_to_card, open_series_screen, spawn_episode_thumb_loading, SeriesCtx
│       │   ├── season.rs       open_season_screen, handle_key (season detail screen)
│       │   ├── detail.rs       open_detail, detail page fetch + metadata + cast photos + collection row + similar row
│       │   ├── collection.rs   open_collection_screen, handle_key (BoxSet member grid screen)
│       │   ├── album.rs        open_album_screen, handle_key (MusicAlbum tracklist screen); play-album-all callback
│       │   ├── artist.rs       open_artist_screen, handle_key (MusicArtist portrait + album grid screen)
│       │   ├── playback.rs     VideoState, fmt_secs, fmt_ends_at, build_track_model, GL FBO helpers
│       │   ├── stats.rs        update_stats_window, all stats formatting helpers; sets audio-passthrough-active
│       │   ├── browse.rs       update_library_filter, populate_browse_async (off-thread), browse list + library search callback wiring
│       │   ├── auth.rs         do_login, initial library fetch after authentication
│       │   ├── controls.rs     wire_controls: all player control callback registrations
│       │   ├── context_menu.rs wire_context_menu, wire_queue_callbacks, update_card_in_all_models
│       │   ├── keys.rs         Action enum, KeyCombo, Keybindings, AppMode, active_mode(),
│       │   │                   handle_key (main dispatcher), handle_global_shortcuts,
│       │   │                   dispatch_player, dispatch_library, dispatch_dashboard
│       │   ├── settings.rs     dispatch_settings, apply_dropdown_selection; section/row index constants
│       │   ├── pipewire_fix.rs is_pipewire_device, apply_alsa_irq_scheduling (WirePlumber config)
│       │   └── ws.rs           start_websocket (reconnect loop + event routing: LibraryChanged, UserDataChanged, KeepAlive)
│       └── ui/
│           ├── main.slint      MainWindow: keyboard handler, sync-layout, export { AppState }; loading overlay (spinner + progress bar)
│           ├── app_state.slint global AppState singleton — all shared UI state + callbacks
│           ├── theme.slint     color palette, spacing tokens, HomeItem / CardItem / TrackItem structs
│           ├── layout.slint    AppShell: sidebar (random logo, nav items) + content area
│           ├── home.slint      HomeScreen, DashboardScreen, MusicDashboard, CollectionsDashboard, LibraryGrid, SectionRow components
│           ├── detail.slint    DetailPage component (backdrop extends to header block; PosterBlock, MetaLine, CastRow atoms; tagline, director, writer, studio; ♥/✓ IconCircleButton; ends-at below buttons; SectionRow for "More Like This")
│           ├── series.slint    SeriesScreen component
│           ├── season.slint    SeasonScreen component (season detail: backdrop/poster/meta/episodes/cast)
│           ├── collection.slint  CollectionScreen component (BoxSet detail: backdrop hero + Back button + member poster grid)
│           ├── album.slint       AlbumScreen component (MusicAlbum detail: cover art + metadata + scrollable tracklist + ▶ Play All + ♥ button)
│           ├── artist.slint      ArtistScreen component (MusicArtist: circular portrait + meta + album card grid)
│           ├── player.slint    PlayerOverlay component
│           ├── settings.slint  SettingsPage two-pane layout (section list + rows)
│           ├── browse.slint    BrowseScreen component
│           ├── login.slint     LoginScreen component
│           ├── context_menu.slint ContextMenu overlay component
│           └── widgets.slint   FjordButton, NavItem, BrowseItem, MediaCard, LoadingSpinner, ToggleSwitch,
│                               SectionHeader, SettingsDropdown, SettingsRow, StatRow,
│                               IconCircleButton (38px circle; gray=inactive, accent=active; ♥/✓ icons; font-size 20px centred),
│                               BackdropHero, PosterBlock, MetaLine (shared detail-page atoms),
│                               MusicPlayerBar (72px full-width bottom bar; is-audio-playing; left: art+title+artist; centre: ⏸/▶+⏹+progress+time; right: ⏮Prev(slot4)/⏭Next(slot5)/⇌Shuffle(slot6)/↺Repeat(slot7)/⋮Queue(slot8) — accent when active, focus-border when music-bar-focused matches),
                              QueuePanel (right-side overlay 400px; dim backdrop + click-outside-to-close; header "Queue N of M" + Clear All; scrollable Flickable list; playlist rows first, then context-menu queue rows (is-queued: "· Queued" subtitle, subtle colour); index+title+artist per row; current item accented bg+bold; focused row left accent stripe + surface-overlay bg; kb-y auto-scroll),
│                               FloatingMiniPlayer (320×90 px bottom-right overlay; live video + title + Resume/Stop),
│                               ToastNotification (bottom-center error pill; red left stripe; positioned+sized by caller)
```

### `fjord-app/src/` module responsibilities

Each module owns one concern. `main.rs` wires all modules together: rendering
notifier, mpv event-poll timer, not-watched refresh timer, applying saved config
on startup, and `AppState::get(&window).on_*()` callback registrations.
`slint::include_modules!()` must stay in `main.rs` — it generates `MainWindow`
and `AppState` (the Slint global) as `crate::MainWindow` and `crate::AppState`.
Every module that accesses the global imports `use slint::Global;` and uses
`AppState::get(&window).set_X()` / `.get_X()` / `.on_X()`.

`show_toast(ww: Weak<MainWindow>, msg: String)` in `main.rs` is the canonical way to surface errors to the user. It is safe to call from any thread or from the Slint event loop. It sets `toast-message` + `toast-visible` via `invoke_from_event_loop`; a Slint `Timer` in `main.slint` auto-dismisses after 4 s.

| Module | Owns |
|---|---|
| `config.rs` | `Config` (persisted JSON: auth + all settings, including `sub_enabled`, `sub_lang`, `sub_lang2`, `library_movies_sort`, `library_series_sort`, `library_collections_sort`, `library_artists_sort` (0=Name A-Z…4=Random), `library_albums_sort`, `library_music_view` (0=Artists, 1=Albums)), `FjordState` (runtime app state: `config: Config` is the canonical settings copy + auth; client, library vecs, keybindings, `movie_collections: HashMap<movie_id,(boxset_id,boxset_name)>`, `all_collections: Vec<MediaItem>`, `collections_fetched: bool`, `all_artists: Vec<MediaItem>`, `artists_fetched: bool`, `all_albums: Vec<MediaItem>`, `albums_fetched: bool`, `series_episode_cache: HashMap<season_id, Vec<MediaItem>>` (in-memory cache, cleared on series switch), `series_season_generation: u64` (stale-fetch guard for rapid season tab navigation)), XDG path helpers, `load/save_config`, `ensure_device_id`, `load/save_keybindings`. Adding a setting: add to `Config` only — `FjordState.config` is the single copy, saved directly in `on_settings_changed`. |
| `home.rs` | `HomeSection` enum (`#[repr(usize)]`, 16 variants, `empty_array()`), `HomeData` (incl. `recently_added_collections`, `unwatched_collections`, `recently_added_albums`, `recently_played_albums`, `favorite_movies`, `favorite_series`, `favorite_albums`), home/movies/series/collections/artists/albums cache, `fetch_home_data`, `push_home_data`, `home_data_sections` (returns `[(HomeSection, Vec<MediaItem>); 16]`), `load/save_movies_cache`, `load/save_series_cache`, `load/save_collections_cache`, `load/save_artists_cache`, `load/save_albums_cache`, `fetch_movie_collections` (background BoxSet membership map), `run_poster_cache_cleanup(movie_ids, series_ids, collection_ids, artist_ids, album_ids)` (24 h guarded orphan cleanup for posters/ + backdrops/) |
| `poster.rs` | `fetch_poster_cached`, `fetch_backdrop_cached`, `decode_poster_buffer`, `spawn_poster_loading`, `spawn_series_poster_loading` |
| `movies.rs` | `spawn_movies_poster_loading`, `spawn_collections_poster_loading`, `spawn_artists_poster_loading`, `spawn_albums_poster_loading` — thin wrappers around `spawn_library_poster_loading(…, LibraryKind)`. `LibraryKind` enum (`Movies`/`Collections`/`Artists`/`Albums`) encodes item-type, `active-nav` guard, and AppState setter. `Albums` and `Artists` share `active-nav == 4` but `matches_library_display()` guards each against overwriting the other when `library-music-view` doesn't match. Guards require matching `active-nav` (2/3/4) before overwriting `library-display`. |
| `series.rs` | `ep_to_card` (MediaItem→CardItem, title = episode name, subtitle "S1:E3"); `spawn_episode_thumb_loading` (parallel thumb fetch → `series-episode-cards`); `SeriesCtx` (shared context for 3 parallel background tasks): `spawn_main` (detail+poster+seasons in parallel, backdrop, first-eps; emits `app-loading-progress=0.5`; fetches ALL cast portraits in parallel before showing page (no trickle-in); single `invoke_from_event_loop` sets cast+metadata+poster+backdrop+episodes then sets `app-content-loading=false` + `show_series=true`); `spawn_next_up` (`get_next_up_for_series` → series-has-next-up + thumb), `spawn_similar` (`get_similar_items` → series-similar SectionRow); `open_series_screen` (resets all AppState series props, sets `app-content-loading=true` + `app-loading-progress=0`, defers `show_series` until spawn_main completes, spawns all 3 tasks); `handle_key` (season row: L/R cycle tabs, Enter/I opens season detail, Down enters episode row; episode row: L/R nav, Up→season row, Enter plays, I→detail, C→ctx-menu) |
| `season.rs` | `open_season_screen` (reset AppState season props, pre-fill title from series-seasons model, spawn async fetch for season detail + poster/backdrop + cast portraits); `handle_key` (episode row default; Down→cast row if present; Up returns to episodes; Enter plays; I opens episode detail; C opens ctx-menu; Back closes) |
| `detail.rs` | Detail page: parallel metadata+poster fetch (tokio::join!), backdrop; emits `app-loading-progress=0.5`; fetches ALL cast portraits in parallel before showing page (no trickle-in); single `invoke_from_event_loop` sets all data then shows page; `detail-ends-at` ("Ends HH:MM", below buttons); collection `SectionRow`; "More Like This" `SectionRow`; `open_detail` sets `app-content-loading=true` + `app-loading-progress=0`, defers `show_detail` until spawn_main completes |
| `collection.rs` | `open_collection_screen(id, title, state, ww, rt)`: increments `collection-open-gen`, resets AppState collection props, sets `app-content-loading=true`; spawns async fetch of BoxSet items + BoxSet poster + item-detail in parallel; backdrop only when `backdrop_image_tags` non-empty; on `get_boxset_items` error clears loading (gen-guarded) and shows toast; `fetch_card_posters` then single `invoke_from_event_loop` (guarded by `collection-open-gen` — handles same-ID re-opens) sets all data then shows page (`show-collection=true`); `handle_key`: grid nav (Up/Down/Left/Right, Enter→`open-detail`, C→context-menu), Back-button focus (Up from row 0), Back→close |
| `album.rs` | `open_album_screen(id, title, state, ww, rt)`: increments `album-open-gen`; spawns async fetch of tracks + album poster + item-detail in parallel; sets album-artist/meta/overview/is-favorite/has-played from detail; single `invoke_from_event_loop` (gen-guarded) sets TrackItem model + shows page (`show-album=true`); `handle_key`: Back-button focus (Up from track 0 → button row → Back); ▶/♥ button row (album-btn-focused: 0=▶ Play All, 1=♥; Left/Right move, Enter activates — the ✓ Watched button was removed in 95c3829); track list (Up/Down nav, Enter→`play-album-track`, Back closes). `play-album-all` (wired in `main.rs`): populates `vs.playlist` with all tracks, starts track 0 with `audio_meta=(artist,album_id)`. |
| `artist.rs` | `open_artist_screen(id, title, state, ww, rt)`: increments `artist-open-gen`; spawns async fetch of `get_artist_albums` + artist portrait in parallel; fetches all album posters with Semaphore(8); single `invoke_from_event_loop` (gen-guarded) sets album `CardItem` model + portrait + shows page (`show-artist=true`); `handle_key`: Back-button focus (Up from row 0 in album grid), album grid nav (L/R/U/D/Enter→`open-album`, C→context-menu, Back→close) |
| `playback.rs` | `RepeatMode` enum (Off=0/All=1/One=2). `QueueItem { id, item_type, series_id, title, audio_meta: Option<(artist, album_art_id)> }` — `audio_meta` carries music-bar metadata per item. `VideoState` (incl. `chapters: Vec<(f64,String)>`, `chapters_loaded`, `chapter_load_attempts`, `chapter_osd_ticks`, `delay_osd_ticks`, `playlist: Vec<QueueItem>` (ordered album/artist track list), `playlist_index: usize` (current position), `shuffle: bool`, `shuffle_order: Vec<usize>` (LCG Fisher-Yates permutation), `repeat_mode: RepeatMode`, `queue: Vec<QueueItem>` (context-menu enqueue, plays after playlist), `current_is_audio: bool` (set in `start_playback`; gates natural-end advance by media class), `lyrics: Option<Vec<(u64, String)>>` (ms, text), `lyrics_available: bool`). Two-collection model: `playlist` is the full album/artist track list; `queue` is context-menu overflow. **Queue persistence (Phase 56)**: `start_playback` never wipes playlist/queue — playing music while items are queued means "insert at the top of the queue" (new item plays now, upcoming items continue after); Play All rebuilds the playlist but keeps `vs.queue`; `do_stop_playback` (user stop) KEEPS playlist+queue (panel stays reachable via `q` while idle; Clear All or sign-out empties it). Natural-end: RepeatMode::One replays current, ::All wraps to 0, ::Off advances — playlist first, then queue; advance is **class-gated** (audio→audio, video→video via `current_is_audio` vs queue-head `item_type`) so a movie ending never auto-starts queued music. Helpers: `playlist_prev(vs)` (pos < 2 s → go back, else None → caller seeks to 0), `playlist_next(vs)` (respects shuffle_order + RepeatMode, falls back to queue), `toggle_shuffle(vs)` (LCG Fisher-Yates, current item at position 0 in shuffle order), `shuffle_indices(n)`. `start_playback(url, id, item_type, title, config, client, series_id, audio_meta, video, ww, rt)` — `audio_meta` drives music bar; when `item_type=="Audio"` sets `is-audio-playing=true`; fetches album art + lyrics (Jellyfin 10.9+) in background (generation-guarded). `fmt_secs`, `fmt_ends_at`, `build_track_model` (title→lang→codec; `external_filename` fallback), GL FBO helpers, `wire_rendering_notifier`, `wire_mpv_timer` (chapter polling; OSD countdowns; music-bar pos/elapsed/total; lyrics-active-idx updated every ~500 ms when `show-lyrics && is-audio-playing`; playlist/queue advance on natural end respects RepeatMode), `reset_playback_ui` (clears chapter-marks/entries/OSD/delays/`is-audio-playing`/lyrics state) |
| `stats.rs` | `update_stats_window` and all stats string formatting; also sets `audio-passthrough-active` (checked every 500 ms via `audio-out-params/format`) |
| `browse.rs` | `refresh_library_display` (central: sort+filter+search → `library-display` + `library-alpha-offsets`), `build_alpha_offsets`, `pseudo_shuffle` (LCG Fisher-Yates), `update_library_filter`, `populate_browse_async` (snapshots data on UI thread, filters off-thread via Tokio, pushes back via `invoke_from_event_loop`; `AtomicU64` gen counter drops stale results), browse list + library search + sort + alpha-jump callback wiring |
| `auth.rs` | Login flow: authenticate, persist config, fetch initial library + home data |
| `controls.rs` | `wire_controls`: registers all player control `AppState::get(window).on_*()` callbacks; `chapter_osd_name()` helper (computes target chapter name from `vs.chapters` + position + delta, avoiding mpv async timing); wires `on_chapter_prev`/`on_chapter_next`; `on_chapter_jump(idx)` (seeks to `vs.chapters[idx].0`, also called from `commit_panel_selection` panel=4); `fmt_delay_ms(label, secs)` helper; wires `on_sub_delay_inc`/`on_sub_delay_dec`/`on_audio_delay_inc`/`on_audio_delay_dec` (each also updates `sub-delay-ms`/`audio-delay-ms` for Sync panel display) |
| `context_menu.rs` | `wire_context_menu`: open-context-menu / browse / series-ep callbacks, context-mark-played, context-toggle-fav, context-play-from-start; `wire_queue_callbacks(…, rt_handle)`: wires `queue-add-item` / `queue-play-next-item` via shared `queue_from_context_menu` + `enqueue_item` (play-next: if `!vs.playlist.is_empty()` inserts at `playlist_index + 1` and updates shuffle_order, else `vs.queue.insert(0, item)`; add: `vs.queue.push`); Series cards are resolved to their next unwatched episode asynchronously before enqueue (CR10-7; toast when fully watched or fetch fails); MusicAlbum/MusicArtist cards are expanded to their Audio tracks (artist → albums → tracks; Play Next preserves album order via reverse insertion; BoxSet toasts "can't be queued"); both call `crate::push_queue_display` after mutation (which renders playlist + queue rows and owns `queue-count` = `playback::upcoming_count`: playlist tracks after current + queued items; is-current requires the playing `vs.item_id` to match the row, not just `playlist_index`) — row title comes from `AppState.context-menu-title` (set by **every** context-menu open site from the card/track it opened on: SectionRow right-click, library/collection/artist grids, all keyboard C paths, browse); `find_title_in_state` scans all_movies/all_series/all_albums/all_artists/series_episode_cache as fallback only; `update_card_in_all_models` patches played/fav across every Slint model (see list below); `remove_from_dynamic_rows` filters Next Up/Continue Watching/Not Watched/`unwatched_collections` when an item is marked played |
| `keys.rs` | `Action` enum (~47 semantic actions, incl. `NextChapter`/`PrevChapter`, `SubDelayIncrease`/`SubDelayDecrease`/`AudioDelayIncrease`/`AudioDelayDecrease`, `PrevTrack`(`[`)/`NextTrack`(`]`)/`ToggleShuffle`/`CycleRepeat`, `OpenQueuePanel`(`q`/`Q`)/`DeleteItem`(Delete)), `KeyCombo`, `Keybindings` (normal + player maps); `AppMode` (14 variants incl. `QueuePanel`); `active_mode()` derives `AppMode` from `AppState` flags — `QueuePanel` checked after ContextMenu, before Person/Detail/etc.; `handle_key()` main dispatcher: `OpenQueuePanel` pre-dispatch fires from any non-ContextMenu/Player mode when `is_audio_playing` OR `queue-count > 0` (idle queue stays reachable; empty queue → "Queue is empty" toast; idle open puts cursor on row 0); then PrevTrack/NextTrack/ToggleShuffle/CycleRepeat pre-dispatch when `is_audio_playing` from any non-ContextMenu mode; then global `Quit` + `Fullscreen` pre-dispatches (Ctrl+Q and F/F11 work from any mode; Fullscreen skips Player, whose own arm also reveals the controls bar — the per-screen Quit/Fullscreen arms are legacy and unreachable); then `ResumePlayer` pre-match; then `match mode` routes to per-module handlers; `handle_key_queue_panel` (Up/Down/Enter/Delete/Back in queue panel); `handle_global_shortcuts` (F/Ctrl+Q/B/1/2/3/S); `dispatch_player` (incl. OpenQueuePanel), `dispatch_library`, `dispatch_dashboard`; music-bar focus slots 0=art/title 1=⏸/▶ 2=⏹ 3=timeline 4=⏮Prev 5=⏭Next 6=⇌Shuffle 7=↺Repeat 8=⋮Queue; `default_keybindings`, `remappable_actions`, `key_display_name`, `action_key_labels` |
| `person.rs` | `open_person_screen` (show-person=true, fetch portrait+bio+filmography in parallel); `handle_key` (header/bio ↔ filmography row; Enter on card→open-detail; Enter on Back→close) |
| `settings.rs` | `dispatch_settings`, `apply_dropdown_selection`; section constants (`SECTION_GENERAL/VIDEO/AUDIO/PLAYER_CFG/KEYBINDINGS`) and per-section row index constants (`GEN_*`, `VID_*`, `AUD_*`, `PLY_*`) |
| `pipewire_fix.rs` | `is_pipewire_device` (true for `""` / `pipewire` / `pipewire/*`), `apply_alsa_irq_scheduling` (writes/deletes `~/.config/wireplumber/wireplumber.conf.d/fjord-alsa-irq.conf` and restarts WirePlumber) |
| `ws.rs` | `start_websocket` → spawns reconnect loop, returns `AbortHandle` (stored in `FjordState.ws_abort`, aborted on sign-out). Connects to `ws[s]://host/socket?api_key=…&deviceId=…`. Handles: `LibraryChanged` (parses `ItemsAdded`/`ItemsUpdated`/`ItemsRemoved`: clears all `*_fetched` flags so the next grid open re-fetches, refreshes the currently open grid via `spawn_library_fetch`, purges removed ids from FjordState vecs + every visible model (`remove_item_from_all_models`) + their poster/backdrop cache files; then a 5 s debounced refresh of home rows **and** the series list), `UserDataChanged` (patches played/fav via `update_card_in_all_models`), `ForceKeepAlive`/`KeepAlive` (responds with `{"MessageType":"KeepAlive"}`). Reconnects with exponential backoff 1 s → 60 s. |
| `home.rs` (timer) | `wire_nw_timer`: 30 s not-watched refresh poll |

## Key design decisions

### mpv render API (not X11 embedding)
mpv uses `vo=libmpv` and `mpv_render_context`. It never opens its own window. Each frame:
1. Slint's `BeforeRendering` notifier fires on the GL thread
2. mpv renders into the back FBO (`mpv_render_context_render`)
3. The FBO texture is exposed to Slint as a `BorrowedOpenGLTexture`
4. Slint's `AfterRendering` notifier calls `report_swap()` for vsync feedback

**Double-buffer FBO:** Two GL texture/FBO pairs alternate each frame. Single-buffer caused Slint to skip re-renders because the texture ID was unchanged (Slint's change detection). Alternating IDs force a re-render every frame.

**Drop ordering:** `MpvRenderCtx` must be dropped before `Player`. This is enforced in `VideoState` and in the rendering teardown path.

### Four playback modes
1. **Fullscreen player** (`is-playing = true`): covers the full window, shows controls bar + inline stats overlay.
2. **Video behind UI** (`has-background-player + video-behind-ui = true`): video fills the full window (dim overlay `#00000044`); all overlay screens (Detail/Series/Season/Person) have transparent root backgrounds and transparent inner header Rectangles so the video shows through everywhere; the 56px `MiniPlayerBar` is docked at the **bottom** above the music bar (if any).
3. **Mini-player bar** (`has-background-player && !is-playing`): full-width `MiniPlayerBar` docked at the **bottom** of the window. Mode 3 (!video-behind-ui): 108px — live video thumbnail (192×108 px) + title + Resume/Stop buttons. Mode 2 (video-behind-ui): 56px compact bar — NOW PLAYING label + title + FjordButton Resume/Stop. `bar-h` is 108px (mode 3) or 56px (mode 2) or 0px when player is off. `float-card-focused` (-1=none, 0=Resume, 1=Stop); `focus_bar_on_up` still used for keyboard access. Down/Back unfocuses.
4. **Music bar** (`is-audio-playing = true`): full-width `MusicPlayerBar` (72 px) docked at the very bottom. Shown when audio-only items play (`item_type == "Audio"`). `is-playing` is **not** set — no fullscreen player opens. Left zone: 60×60 album art + title + artist (click → `music-bar-open-album`). Centre: ⏸/▶ (slot 1) + ⏹ (slot 2) + progress bar (slot 3) + elapsed/total. Right zone: ⏮ Prev (slot 4) + ⏭ Next (slot 5) + ⇌ Shuffle (slot 6) + ↺ Repeat (slot 7) + ⋮ Queue (slot 8) + ♪ Lyrics (slot 9, only rendered when `lyrics-available`). Shuffle button accent when `queue-shuffle`; Repeat button accent when `queue-repeat-mode > 0`, shows "↺¹" for One. Lyrics button accent when `show-lyrics`. `music-bar-play-pause` delegates to `pause_play_toggle`; `music-bar-stop` calls `do_stop_playback`. Keyboard: `[`/`]` → PrevTrack/NextTrack (global, any mode when `is_audio_playing`); `q`/`Q` → OpenQueuePanel (opens QueuePanel overlay; also works while idle whenever `queue-count > 0`); `L`/`l` → ToggleLyrics (global when `is_audio_playing`); focus slots navigate with arrow keys (Down→timeline, Up→unfocus from non-timeline); slot 8 (⋮) Right → 9 (♪) only when `lyrics-available`; slot 9 Right absorbed.

**Bar layout**: content fills `y=0 height=parent.height-total-bar-h`; `total-bar-h = bar-h + music-bar-h` (72px when music bar active, else 0px). `MiniPlayerBar` sits at `y=parent.height-bar-h-music-bar-h`; `MusicPlayerBar` at `y=parent.height-music-bar-h`. All overlay screens and the loading overlay use the same `total-bar-h` padding.

The "Video in background" setting (persisted) controls whether Back during playback enters mode 2 or mode 3.

### Dashboards and library grid

There are four dashboard screens (horizontal `SectionRow` card rows) and one library grid:

- **Home dashboard** (`HomeScreen`, `active-nav == 0`, up to 5 rows): Continue Watching, Next Up, Recently Added Shows (`Series`), Recently Added Movies, Recently Added Music (albums). Shows movies, series, and albums.
- **TV Shows dashboard** (`DashboardScreen`, `active-nav == 1`, up to 5 rows): Continue Watching TV, Next Up, Recently Added Shows (`Series`), Not Watched Shows (`Series`), Favorite Series.
- **Movies dashboard** (`DashboardScreen`, `active-nav == 2`, up to 4 rows): Continue Watching Movies, Recently Added Movies, Not Watched Movies, Favorite Movies.
- **Collections dashboard** (`CollectionsDashboard`, `active-nav == 3`, up to 2 rows): Recently Added collections, Unwatched collections. Fetched as part of `fetch_home_data` (sections 9+10 in the 16-element poster-loading array). Enter on a card calls `on_item_play` → `open_collection_screen`. `on_item_play` checks `FjordState.all_collections` first (lazy, only populated when the library grid is opened), then falls back to the always-present dashboard Slint models (`recently-added-collections` / `unwatched-collections`) to obtain the BoxSet id+name — so the card works immediately on startup before the library grid is ever opened. Double-click the Collections sidebar item (or press Enter from the dashboard) opens the `LibraryGrid` with all BoxSets.
- **Music dashboard** (`MusicDashboard`, `active-nav == 4 && !show-library`, up to 3 rows): Recently Added Albums, Recently Played Albums, Favorite Albums. Fetched as part of `fetch_home_data` (sections 11+12+15 in the 16-element array). Enter on a card routes via `on_item_play` → fetches item detail → if `item_type == "MusicAlbum"` opens `album::open_album_screen`. Individual `Audio` tracks play via `start_playback`; album art is fetched in background (generation-guarded) and shown in the music bar.
- **Library grid** (`LibraryGrid`, `show-library == true`): full poster grid of every item in a category. Opened by pressing Enter from the Movies, TV, or Music sidebar tab, or by double-clicking Collections. Separate concept from the dashboards — do not call this a "dashboard." For Music (nav=4) the sort bar layout is [Artists(0)][Albums(1)] | Sort: [A→Z(2)][Z→A(3)][Year↓(4)][Year↑(5)][Shuffle(6)] | [Favorites(7)]. View pills are at cursor 0-1 (left of sort pills, reached by pressing Left); Favorites is at cursor 7 (right of sort pills). `library-has-filters=false` for Music — the Favorites pill is rendered separately. All pills require Enter to apply; cursor starts on the active sort pill (at offset 2 for Music). Toggle pills require Enter to apply — cursor navigates freely without switching view. `library-music-view` (0=Artists, 1=Albums) persisted in `Config.library_music_view`; always one selected (radio semantics). Artists view: Enter on a card opens `ArtistScreen`. Albums view: Enter on a card opens `AlbumScreen` directly.

Episode cards in dashboard rows display the series poster (`series_id` used as the fetch key), not the episode thumbnail. `spawn_poster_loading` carries a `poster_id` field alongside `item_id` in its metadata tuple for exactly this reason.

Not Watched rows use `SortBy=Random` so each fetch returns a different selection. A 30-second polling timer (`timer_nw`) refreshes the Not Watched row when the relevant tab is visible, no playback is active, and 10 minutes have elapsed since the last refresh. Timestamps `last_nw_mov_refresh` / `last_nw_tv_refresh` in `AppState` track this independently per tab.

Poster images are cached to `~/.cache/fjord/posters/` and decoded off the UI thread — JPEG decode runs on a Tokio worker producing `SharedPixelBuffer<Rgba8Pixel>` (which is `Send`), then `Image::from_rgba8` is called inside `invoke_from_event_loop` because `slint::Image` is `!Send`.

`HomeItem` (defined in `theme.slint`) carries `has-played: bool`, `resume-pct: float`, `unplayed-count: int`, and `is-favorite: bool` — populated from `UserData`. Cards use two Jellyfin-style text rows via `CardItem.title`/`CardItem.subtitle` (`MediaItem::card_title()`/`card_subtitle()`): episodes show series name + "S1:E3 - Title", series show "2019 - Present" (Status/EndDate), albums show the artist, movies the year; inside series/season screens episode cards show episode name + "S1:E3". `MediaCard` renders:
- **✓ badge** (top-right, accent circle, bold) when `has-played`
- **progress bar** (bottom) when `resume-pct > 0 && !has-played`
- **unplayed-count pill** (top-right, accent circle, bold) when `unplayed-count > 0 && !has-played` (series posters only)
- **♥ badge** (top-left, accent circle) when `is-favorite`

Card dimensions are computed by breakpoint pure functions (`dash-card-w`, `dash-card-h`, `grid-cols`) that live on `MainWindow` because they reference `self.width`. A `sync-layout()` function pushes the results to `AppState.dash-cw`, `AppState.dash-ch`, and `AppState.library-cols` on `init` and `changed width` so all screens see the current sizes.

### Disk caches
- `~/.cache/fjord/home.json` — home row data. Shown from cache immediately on warm start, always refreshed in the background.
- `~/.cache/fjord/movies.json` — full movie list (`Vec<MediaItem>`). Populated after first network fetch; on warm start loaded for instant display **without** setting `movies_fetched` — the first grid open each session still does a background network refresh (the flag only guards against multiple refreshes within one session).
- `~/.cache/fjord/series.json` — full series list. Populated at login/auto-login and on every background refresh. Loaded on warm start so Browse All and the TV grid are instant.
- `~/.cache/fjord/collections.json` — full BoxSet list (`Vec<MediaItem>`). Populated on first Collections library grid open; on warm start loaded immediately so the Collections grid shows content at once. Refreshed once per session on grid open (`collections_fetched` flag guards re-fetch).
- `~/.cache/fjord/artists.json` — full album-artist list (`Vec<MediaItem>`). Populated on first Music library grid open; on warm start loaded immediately. Refreshed once per session (`artists_fetched` flag guards re-fetch).
- `~/.cache/fjord/albums.json` — full MusicAlbum list (`Vec<MediaItem>`). Populated on first Music library grid open (Albums view); on warm start loaded immediately. Refreshed once per session (`albums_fetched` flag guards re-fetch).
- `~/.cache/fjord/posters/<id>` — raw poster bytes, one file per item, plus a `<id>.tag` sidecar holding the server's `ImageTags.Primary` hash. Grid/row loaders pass the item's current tag to `fetch_poster_cached_tagged`; a mismatch re-downloads the image (replaced artwork propagates). Callers that don't know the tag (screen-open fetches that run parallel to the detail fetch, person portraits, queue thumbnails) use the untagged path and serve whatever is on disk. A failed re-download falls back to the stale copy.
- `~/.cache/fjord/backdrops/<id>` — raw backdrop bytes + `.tag` sidecar (`BackdropImageTags[0]`), same revalidation rules.
- `~/.cache/fjord/last_cleanup` — Unix timestamp (ASCII) of the last poster/backdrop cache cleanup run.

**Poster cache cleanup** (`run_poster_cache_cleanup` in `home.rs`): spawned as a background task after each auto-login. Takes `(movie_ids, series_ids, collection_ids, artist_ids, album_ids)`. Builds a known-ID set from `all_movies ∪ all_series ∪ all_collections ∪ all_artists ∪ all_albums`, then walks `posters/` and `backdrops/` deleting any file whose name (= item ID, after stripping `.tag`/`.tmp` suffixes) is not in the set. Guarded by: (1) combined ID set non-empty (skips on network error / first run); (2) 24 h minimum interval via `last_cleanup`. Portrait/season/episode cache files that fall outside the known set are re-fetched on next access.

On a warm start (valid saved session + fresh cache) the window opens in the logged-in state with content visible on the first frame — no loading flash.

### Keyboard navigation
A global zero-size `FocusScope` (`fs`) captures all keyboard input. `invoke_grab_keyboard_focus()` is called from Rust at startup **and after every login** (manual + auto-login) to give `fs` focus — without the post-login call, all keyboard navigation is dead until restart.

All keyboard input flows into `keys::handle_key()` in Rust (called from Slint's `key-pressed` handler). `active_mode()` derives the current `AppMode` (13 variants) from `AppState` flags — screen priority is encoded once in this function, not scattered across conditionals. A `match mode { ... }` then routes to per-module handlers: `context_menu::handle_key`, `person::handle_key`, `season::handle_key`, `series::handle_key`, `detail::handle_key`, `artist::handle_key`, `collection::handle_key`, `album::handle_key`, `dispatch_player`, `dispatch_library`, `browse::handle_key`, `dispatch_settings`, `dispatch_dashboard`. A pre-match check for `ResumePlayer` fires globally for all modes except Player/Person/Season/Series/Detail/Artist/Collection/Album/ContextMenu. `handle_global_shortcuts` (F/Ctrl+Q/B/1/2/3/S) is called as a fallback from both Dashboard and Settings arms. The contract is uniform: **Enter/Right enter**, **Backspace/Escape go back**, **Up/Down navigate rows/items**, **Left/Right navigate within a row or cycle a combobox**.

All keyboard state lives in the `AppState` global singleton. Key nav state:
- **`-1` = sidebar**: Up/Down cycle nav tabs (0 Home → 1 TV Shows → 2 Movies → 3 Collections → 4 Music → 5 Browse All → 10 Settings → 11 Quit → wrap); arrowing to nav=5 opens `show-browse` immediately; Right/Enter enters the content grid or library; `settings-focused` is reset to -1 when `active-nav` changes and also when `B` opens browse.
- **`≥ 0` = content grid**: focused-section is the row index, `focused-card` is the column. Up/Down move between rows (Up at row 0 stays in content); Left/Right move between cards; Enter plays; I opens detail/series screen.
- **Browse list** (`show-browse = true`, `active-nav == 5`): opens in sidebar mode (`current-item = -1`). Up/Down navigate the sidebar; Right or Enter enters the list (`current-item = 0`). In list mode: Up/Down navigate items; Up at item 0 focuses the search bar (`browse-header-focused = true`); Left returns to sidebar; `/` also jumps to search. Search bar focused (`browse-header-focused = true`): typing filters client-side; Backspace deletes (empty → back to list); Down/Enter moves into results; Escape clears query and unfocuses. Backspace/Escape in list/sidebar mode closes browse and resets `active-nav = 0` when exiting via the Browse All sidebar entry. `B` shortcut also opens browse without changing `active-nav`.
- **Library grid** (`show-library = true`): 2D arrow nav across the poster grid; Enter opens detail; Backspace/Escape closes. Layout: top bar (54px) → sort bar (40px) → search field (52px) → poster grid with alphabet scrubber on right edge. Five focus states, navigated via Up/Down in visual layout order: **(1) grid mode** (`library-header-focused = false`, `library-sort-focused = false`, `library-back-focused = false`, `library-scrubber-focused = false`) — arrow keys navigate posters, Up at row 0 focuses the search field, `/` also jumps to the search field, `Tab` also toggles the sort bar; **(2) search field focused** (`library-header-focused = true`) — letters type into query immediately, Backspace deletes (empty → back to grid), Down/Enter moves into results, Escape clears query and returns to grid, Up moves to sort bar; **(3) sort bar focused** (`library-sort-focused = true`) — Left/Right navigate cursor (0–4=sort pills, 5=Unwatched, 6=Favorites); Left/Right move the cursor only (no immediate apply); Enter applies the focused option for all pill types (sort 0-4, filter toggles 5-6, music view 5-6) and exits the sort bar; on entry the cursor is set to the current active sort; Right past the last cursor position when sort=A-Z and no query enters the alphabet scrubber; `Tab`/Esc exits, Down moves to search field, Up moves to Back button; focused pill shows 2px border (white when active, accent when inactive) and `surface-overlay` background so the cursor is always visible even on the currently-active pill; **(4) alphabet scrubber** (`library-scrubber-focused = true`, `library-scrubber-cursor: int` 0=A..26=#) — Up/Down navigate letters; Enter jumps the grid to that letter's first item and returns to grid mode; Back/Left returns to sort bar; only reachable when sort=A-Z and no query; focused cell renders with `accent-muted` background + accent bold text; **(5) Back button focused** (`library-back-focused = true`) — Enter or Back/Esc closes the library, Down returns to sort bar, Up reaches the mini-player bar (via `focus_bar_on_up`). Sort (0=Name A-Z, 1=Name Z-A, 2=Year↓, 3=Year↑, 4=Random) persisted per library type in `Config.library_movies_sort`/`library_series_sort`; filters reset to false on open. `refresh_library_display` in `browse.rs` is the single function that rebuilds `library-display` + `library-alpha-offsets` from current sort/filter/query. Alphabet scrubber: right-edge 22px strip, 27 cells (A-Z + #), visible only when sort=Name A-Z and no query; click calls `AppState.library-jump-to-letter(idx)` → reads `library-alpha-offsets[idx]` → sets `library-focused`.
- **Series screen** (`show-series = true`): Full-page scrollable layout: backdrop → header (poster/title/meta/genres/overview) → Next Up SectionRow → season tabs → episode SectionRow → CastRow → More Like This SectionRow. Two nav states tracked by `series-in-season-row`: (1) **season row** (`true`) — Left/Right cycle tabs (each change also loads episodes), Down enters episode row, Enter or I opens season detail (`show-season`); (2) **episode row** (`false`) — Left/Right navigate cards in the horizontal SectionRow (`series-focused-ep`), Up returns to season row, Enter plays focused episode, I opens episode detail, C opens context menu. Backspace/Escape closes series screen and returns to previous screen.
- **Season detail screen** (`show-season = true`, overlays series screen): Full-page scrollable layout: backdrop → header (poster/title/meta/overview) → episode SectionRow (reuses `series-episode-cards` + `season-focused-ep`) → CastRow (`season-cast`, season-specific people). Episode row (default focus) — Left/Right navigate, Down→CastRow if present; CastRow — Left/Right navigate, Up→episodes. Enter plays focused episode, I opens episode detail, C opens context menu. Double-click on a season tab also opens season detail. Back closes and returns to series screen.
- **Artist screen** (`show-artist = true`): Full-page layout: circular portrait (160 px) + name/meta header + ▶/♥ buttons + collapsible bio + album card grid. Nav states: (1) **Back button** (`artist-back-focused`) — Enter/Back closes, Down → button row; (2) **▶/♥/bio row** (`artist-btn-focused`: 0=▶ Play All, 1=♥, 2=bio) — Left/Right between 0-1, Enter activates (bio toggles expand), Up → Back (bio → ▶), Down → bio then grid; (3) **album grid** — arrows navigate (`artist-focused`), Up at row 0 → bio (or button row), Enter → `open-album`, C → context menu, Back → closes. Mouse: single click opens (focus synced) — grid-wide convention. Opened from the Music library grid (Enter on a MusicArtist) or via `on_open_detail(MusicArtist)`. `artist-open-gen` guards stale async responses.
- **Collection screen** (`show-collection = true`): Full-page scrollable layout: backdrop hero + Back button + member poster grid. Two nav states: (1) **Back button focused** (`collection-back-focused = true`) — Enter/Back closes screen, Down enters grid, Up → mini-player bar; (2) **grid mode** — Left/Right/Up/Down navigate cards (`collection-focused`), Up at row 0 → Back button, Enter → `open-detail`, C → context menu, Back → closes screen. Opened from the Collections library grid (Enter on a BoxSet) or via `on_open_collection` callback.
- **Album screen** (`show-album = true`): Full-page layout: top bar (← Back + title) + header block (cover art + metadata + ♥ button) + scrollable track list. Three nav states tracked by `album-back-focused` / `album-btn-focused`: (1) **Back button focused** (`album-back-focused = true`) — Enter/Back closes screen, Down → ♥ row, Up → mini-player bar; (2) **▶/♥/bio row** (`album-btn-focused`: 0=▶ Play All, 1=♥, 2=bio; the ✓ Watched button was removed) — Left/Right between 0-1, Enter activates (bio toggles expand), Up → Back button (bio → ▶), Down → bio then track list, Back closes; (3) **track list** (default) — Up/Down navigate `album-focused-track`, Up at track 0 → bio (or button row), Enter plays via `play-album-track`, Back closes. Mouse: click → focus row, double-click → play. Opened via `on_item_play` (MusicAlbum type) or `on_open_album` callback.
- **Detail page** (`show-detail = true`): Up/Down scroll the overview; Left/Right cycle the focused button: Play (0) → Resume (1, only if `detail-can-resume`) → Series (2, only if `detail-series-id` non-empty, Episodes only); Enter/Space activates the focused button — Play calls `play-detail`, Resume calls `resume-detail`, Series closes the detail page and opens the series screen via `open-series(detail-series-id)`; R resumes (if available); Backspace/Escape or the Back button closes and resets `detail-scroll`. **Important:** Rust code that closes the detail page (e.g. `on_play_detail`, `on_resume_detail`) must also reset `detail-scroll = 0` before calling `set_show_detail(false)`; otherwise the next detail open starts scrolled.
- **Settings** (`active-nav == 10`): two-pane layout. `settings-section: int` (-1 = app sidebar, ≥0 = selected section in the left pane). `settings-focused: int` (-1 = left pane focus, ≥0 = focused row in right pane). Left pane: Up/Down navigate sections (General=0, Video=1, Audio=2, Player=3, Key Bindings=4); Right/Enter enters the right pane (`settings-focused = 0`). Right pane: Up/Down move through rows; Left/Right cycle combobox values; **Enter on a dropdown row opens a popup list** (`settings-dropdown-open = true`, cursor set to current value's index) — Up/Down move the cursor, Enter confirms selection, Escape/Left closes without change; Enter on a toggle row toggles it; Left/Backspace returns to left pane (`settings-focused = -1`); Backspace/Escape from left pane exits settings (`settings-section = -1`). Row hover highlighting uses `SettingsRow` component (TouchArea lower z, `@children` higher z). `SettingsDropdown` has `kb-open` / `kb-cursor` properties; the popup highlights the cursor item and scrolls to keep it centred (max height 320px with Flickable). Section rows: **Video** hwdec(0), vf(1), deinterlace(2), video-sync(3), interpolation(4), tscale(5 virtual, hidden when interpolation off), target-colorspace(6), tone-mapping(7 virtual, hidden when HDR passthrough on), opengl-early-flush(8), video-latency-hacks(9 virtual, hidden when video-sync≠display-resample). **Audio** audio-device(0), SPDIF(1), AC3(2 hidden when SPDIF off), EAC3(3 hidden when SPDIF off), DTS(4 hidden when SPDIF off), DTS-HD(5 hidden when SPDIF off), TrueHD(6 hidden when SPDIF off), alsa-irq-scheduling(7 virtual, hidden when SPDIF off OR non-PipeWire device), audio-lang(8). The audio-device dropdown is dynamic (populated at startup via `mpv --no-config --audio-device=help`) and uses a special path in `dispatch_settings` / `apply_dropdown_selection` — `AppState.audio-device-selected(desc)` callback maps description → mpv name via `FjordState.audio_devices`. `AppState.settings-device-is-pipewire` (bool, set by Rust) gates the IRQ row; `pipewire_fix.rs` implements `is_pipewire_device()` and `apply_alsa_irq_scheduling(bool)` (writes/deletes `~/.config/wireplumber/wireplumber.conf.d/fjord-alsa-irq.conf` and restarts WirePlumber via `systemctl --user restart wireplumber`; config persists after Fjord exits and is only changed when the toggle changes state). **Player** sub-enabled(0), sub-lang(1, hidden + indented when sub-enabled is off), sub-lang2(2, hidden + indented when sub-enabled is off), cache-mb(3); **INTRO SKIPPER** intro-mode(4), intro-secs(5 virtual, shown when intro=ask-timed), recap-mode(6), recap-secs(7 virtual), preview-mode(8), preview-secs(9 virtual), commercial-mode(10), commercial-secs(11 virtual); **CREDITS** credits-mode(12), credits-secs(13 virtual, shown when credits=ask). Skip modes for Intro/Recap/Preview/Commercial: `always-skip` (immediate seek, no overlay), `ask` (single "Skip →" button), `ask-timed` (two-button overlay "Skip" + "Don't Skip" + per-segment countdown — auto-skips when timer runs out; Back/Esc dismisses), `never-skip` (no overlay). Credits modes: `always-skip` (auto-advance immediately), `ask` (show Up Next banner with configurable countdown), `never-skip` (no banner). Cross-section conflict "⚠ passthrough + display-resample" shown in both Video (below video-sync) and Audio (below SPDIF rows); only shown when master SPDIF toggle is on and at least one format is enabled. (GPU API row removed — had no effect with `vo=libmpv` + OpenGL context.)
- **Mini-player bar** (`has-background-player && !is-playing`): Up from any screen reaches it (`float-card-focused = 0`); Left/Right toggle between 0 (Resume) and 1 (Stop); Enter activates; Down/Back unfocuses. Pre-dispatch check in `handle_key` intercepts these keys before the underlying screen. `reset_playback_ui` clears `float-card-focused = -1`. The bar is reached via `focus_bar_on_up` (called at end of every mode arm) — for this to work every screen handler must return `false` for Up when at the topmost position: Detail (`row=0, focused_btn=-1` Back button → `return false`), Season (`btn=0` Back button → `return false`), Person (header, `in_film=false` → `false`), Dashboard (content grid, no prev section → `return false`), Library search header (`key::UP` → focus bar and return true). Series Back button already returns `false` via `_ => false` catch-all.
- **Player** (`is-playing = true`): `dispatch_player` checks overlays in priority order — **(1) ask-timed overlay** (`show-skip-timed`): Left/Right toggle `skip-timed-focused` (0=Skip, 1=Don't Skip), Enter activates focused button, Back/Esc dismisses (sets `skip_segment_handled = true`, hides overlay); **(2) ask-mode skip segment** (`show-skip-segment`): Enter skips; **(3) Up Next banner** (`show-next-ep-banner`): Left/Right toggle `next-ep-banner-focused` (0=Play Now, 1=Skip), Enter activates. All three take priority over all other player keys. Space/K/P pause (blocked while seek bar is held — `seek-dragging` is true during drag, `dispatch_player` eats the event so mpv isn't toggled while the bar shows the frozen drag position); Left/Right seek ±10s (Shift ±30s); Up/Down volume; `,`/`.` prev/next chapter (`PrevChapter`/`NextChapter` actions — do **not** reveal controls bar); S/A/V open track panels; Up/Down in panel navigates tracks; Enter commits selection; M mute; I toggles stats overlay only (does **not** show the controls bar — the player-mode key handler skips `invoke_show_controls()` for `Action::ToggleStats`); F/F11 fullscreen; 0–9 seek to %; Backspace minimizes (or closes open panel first); Escape stops (or closes open panel first). **Mouse-accessible panels**: controls bar right side: **Ch ▾** (panel-id=4, conditional on `chapter-entries.length > 0`) opens chapter list using TrackPanel — clicking seeks to that chapter, active chapter highlighted via `current-chapter`; **Sync ▾** (panel-id=5) opens a custom panel with −100ms/+100ms buttons for Sub and Audio delay, displaying live `sub-delay-ms`/`audio-delay-ms` values updated by the delay callbacks. Volume Up/Down shows a top-center toast overlay (~1.5 s, auto-hides); when SPDIF passthrough is active (`audio-passthrough-active`) the overlay shows "Vol · passthrough" and `adjust_volume` is skipped. The controls bar shows title, seek track, `HH:MM:SS / HH:MM:SS` elapsed/total, and **"Ends HH:MM"** (`playback-ends-at`, local wall-clock time, updated every ~500 ms, cleared on stop). Track panels are `min(parent.width - 32px, 400px)` wide with `wrap: word-wrap`; labels are ordered **title → lang → codec** (external subtitle files fall back to base filename as title). The stats overlay (`stats-visible`) is a 420 px panel top-right with three sections: **VIDEO** (IN/OUT/COLOR/HWDEC), **AUDIO** (IN/OUT), **SYNC** (DISPLAY/VSYNC/A/V/SPEED/DROP/BITRATE/CACHE). Values use `wrap: word-wrap` so long codec/format strings never elide. **Chapter navigation**: after 2 s of playback, `chapter-list` is polled (up to 30 ticks); normalised positions pushed to `AppState.chapter-marks: [float]`, rendered as 2 px semi-transparent tick marks on the seek bar. `,`/`.` call `Player::chapter_step(-1/+1)`; `chapter_osd_name()` computes the target chapter name immediately from `vs.chapters` + current position; a 36 px top-left OSD pill (`chapter-osd-text`, `chapter-osd-visible`) shows "▸ Name" for ~2 s via a `chapter_osd_ticks` countdown in the 16 ms timer. **Sub/audio delay**: `z`/`Z` nudge `sub-delay` ±100 ms; `x`/`X` nudge `audio-delay` ±100 ms (remappable; 4 new `Action` variants). A 36 px OSD pill (`delay-osd-text`, `delay-osd-visible`) shows e.g. "⏱ Sub delay: +200 ms" at y:68 px (below chapter OSD at y:24 px) for ~2 s via `delay_osd_ticks` countdown. No persistence — resets when playback stops.

**Hold vs tap Left:** At `focused-card == 0`, a single tap Left exits to the sidebar; this uses `!event.repeat` as a best-effort guard. `event.repeat` is unreliable in Slint (see Slint gotchas), so this distinction may not always hold — but the worst case is landing in the sidebar, which is harmless.

Shortcuts active at dashboard/browse level: `1`/`2`/`3` jump to Home/Movies/TV (also resets `settings-focused`); `S` to Settings; `B` opens the browse list; `F`/`F11` toggles fullscreen; `Ctrl+Q` quits from any mode (global pre-dispatch in `handle_key`; plain `q`/`Q` belongs to the queue panel since Phase 51); `R` resumes background player.

### Context menu
Triggered by `C` key on any focused card or right-click on any `MediaCard`. State lives in `AppState`:
- `context-menu-item-id`, `context-menu-item-type`, `context-menu-has-played`, `context-menu-is-favorite`, `context-menu-resume-pct`, `context-menu-focused: int`

Menu rows (in order): **Resume** (row 0, conditional: `resume-pct > 0 && !has-played`), **Play from Start** (row 1), **Play Next** (row 2, insert at front of queue), **Add to Queue** (row 3, append to back), **Mark Played/Unplayed** (row 4), **Add/Remove Favourite** (row 5), **View Details** (row 6). Initial focus lands on row 0 when Resume is available, otherwise row 1. Up/Down loop — pressing Up from the top row wraps to row 6; Down from row 6 wraps to the top row. The min row for looping is 0 when Resume is shown, 1 otherwise.

`wire_context_menu` in `context_menu.rs` registers `on_open_context_menu` (from card data), `on_open_context_menu_browse` (resolves browse index → `filtered_items`), and `on_open_context_menu_series_ep` (episode C-key). All three set `context-menu-focused` to 0 or 1 depending on resume availability. `on_context_play_from_start` checks `all_series`: for series it calls `get_next_up_for_series` (falling back to series screen); for movies/episodes it plays from position 0. `update_card_in_all_models` patches `has-played` / `is-favorite` across every `CardItem` model after a successful API toggle. **Every model that holds `CardItem` rows must be listed here — missing a model means the badge never updates on that row without a restart.** Current list: `continue-watching`, `next-up`, `recently-added`, `recently-added-movies`, `continue-watching-movies`, `not-watched-movies`, `continue-watching-tv`, `recently-added-tv`, `not-watched-tv`, `recently-added-collections`, `unwatched-collections`, `recently-added-albums`, `recently-played-albums`, `favorite-movies`, `favorite-series`, `favorite-albums`, `all-movies`, `all-series`, `library-display`, `series-episode-cards`, `series-next-up-cards`, `collection-items`, `detail-similar`, `detail-collection`, `series-similar`, `person-filmography`.

### Sidebar logo
The sidebar header shows a randomly selected icon from the kept pool: `fjord_01`, `fjord_02`, `fjord_04`, `fjord_05`, `fjord_09`, `fjord_10`. The index is picked at startup via `LOGOS[subsec_nanos % LOGOS.len()]` (array `[1,2,4,5,9,10]`) and stored in `AppState.app-logo-idx`. All 6 SVGs are embedded at compile time via a `@image-url()` ternary chain in `layout.slint`. Icon 01 has a transparent background with white FJORD text (evenodd fill-rule for O/D/R letter holes). Icons 02/04/10 have intentional dark rounded-square backgrounds — white corner-fill paths were removed and a `<clipPath>` with a rounded `<rect>` (rx=234/222/176 respectively) wraps all content to make the corners transparent. Icons 05/09 have fully transparent backgrounds. The random selection stays until a permanent icon is chosen.

### Subtitle auto-select
At playback start, if `settings-sub-enabled` is false → force track 0 (off). If a language preference is set, `sub_lang_code()` maps display names ("English" → "en") and tries `sub_lang` then `sub_lang2` by `lang.starts_with(code)`. If no match, mpv's default selection is left unchanged. External subtitle tracks use `track-list/N/external-filename` (base filename) as the label fallback when `title` is empty.

### Fullscreen
`window.window().set_fullscreen(bool)` / `is_fullscreen()` used directly. Toggle is wired to `on_toggle_fullscreen` callback (called by `F`/`F11` key). The "Launch in fullscreen" setting applies the flag before `window.run()` and also immediately when the checkbox is toggled.

### Session identity (DeviceId)

`JellyfinClient` carries a `device_id: String` field used in the `Authorization` header (`DeviceId="…"`). The internal `reqwest::Client` is built with a **30-second request timeout** so a server that accepts the TCP connection but stops responding never hangs the auto-login task or API calls indefinitely. On first run, `ensure_device_id()` reads `/proc/sys/kernel/random/uuid`, saves it to `~/.config/fjord/config.json`, and uses it for the lifetime of the install. This is critical: if two machines share the same DeviceId, Jellyfin invalidates one machine's token when the other authenticates, causing 401 errors on all API calls.

Sign-out clears only the auth fields (`server_url`/`user_id`/`token`) and re-saves config.json — `device_id` and all settings persist across sign-out.

On startup, after loading a saved session, `check_auth()` does a cheap `GET /Users/{id}/Items?Limit=0&Recursive=true` probe. On 401 the login screen is shown; any other error is ignored and the app proceeds (transient network issue). Passwords are never stored — Jellyfin tokens don't expire under normal use.

### Workspace crates
- `fjord-api`: no UI, no mpv. Pure async HTTP + JSON. Testable in isolation.
- `fjord-player`: no UI, no HTTP. Just libmpv bindings + render context.
- `fjord-app`: thin wiring layer. Imports the other two, drives the Slint event loop.

### Episode auto-advance
Behaviour depends on `Config.skip_credits_mode`:
- **`never-skip`**: no banner, no auto-advance — `banner_trigger` is never fired.
- **`always-skip`**: auto-advance immediately when credits position is reached (no banner shown); `start_playback` called directly from the timer via `invoke_from_event_loop`.
- **`ask`** (default): Up Next banner fires *during* playback at `VideoState.credits_start` (from the Intro Skipper `/Timestamps` response — `credits.start`) or when `duration >= 60 s AND duration - position <= 30 s` (fallback). `next_ep_banner_shown` flag prevents it firing more than once per episode. A configurable countdown (`Config.skip_credits_secs`, default 30 s, stored as `banner_trigger.2`) counts down; the banner auto-advances when it reaches zero. "Play Now" calls `on_play_next_ep`; "Skip" calls `cancel-auto-advance` which sets `next_ep_pending = None` and exits the countdown task without playing. Keyboard: `next-ep-banner-focused` (0=Play Now, 1=Skip), Left/Right toggle, Enter activates.

`banner_trigger` type is `Option<(String, Option<Arc<JellyfinClient>>, u32, bool)>` — the `u32` is countdown seconds (0 for always-skip, so loop body never executes), `bool` is `show_banner` (false for always-skip suppresses UI updates).

`next_ep_pending` lives in `VideoState` (not `FjordState`) so it is cleared atomically when a new video starts, preventing stale pending state bleeding across sessions.

Every `start_playback` call site must pass `series_id` so auto-advance works for plays from any screen. Audio call sites should pass `audio_meta = Some((artist, album_art_id))` so the music bar shows correct metadata; non-audio call sites pass `None`.

### Intro Skipper plugin
When starting playback of an Episode, `start_playback` spawns one background task:
- **All segments**: `client.get_episode_timestamps(item_id)` (`GET /Episode/{id}/Timestamps`). Returns `EpisodeTimestamps { introduction, credits, recap, preview, commercial }` where each is a `Segment { start: f64, end: f64 }`. Valid when `end > 0.0`. On success the matching `VideoState` fields are populated: `intro_timestamps`, `recap_timestamps`, `preview_timestamps`, `commercial_timestamps`, and `credits_start = Some(ts.credits.start)`.

Returns `None` gracefully when the plugin is absent (404).

The 16 ms timer checks the current playback position against each segment in priority order (Intro → Recap → Preview → Commercial). At most one segment is active at a time. Behaviour per segment depends on the configured mode (`settings-skip-*-mode` from `AppState`):
- **`always-skip`**: seek to segment end immediately, set `skip_segment_handled = true`, hide overlays.
- **`ask`**: show `show-skip-segment` (single "Skip →" button). Enter in `dispatch_player` calls `invoke_skip_segment()`.
- **`ask-timed`**: on first tick set `skip_timed_shown_at = Some(Instant::now())`. Each tick compute `remaining = prompt_secs - elapsed`. Set `show-skip-timed = true`, update countdown label. When remaining ≤ 0: seek to segment end, set `skip_segment_handled = true`, hide overlays. User can "Don't Skip" via the overlay button or Esc (calls `invoke_dismiss_skip_timed()`, sets `skip_segment_handled = true`, hides overlay — suppresses re-show while still in segment).
- **`never-skip`**: hide all overlays, do nothing.

`VideoState` fields: `skip_segment_handled: bool` — set after seek or dismiss, reset to false when position exits the segment; `skip_timed_shown_at: Option<Instant>` — when the timed overlay first appeared; `skip_timed_prompt_secs: u32` — snapshot of per-segment secs at overlay start. `show-skip-timed`, `skip-timed-label`, `skip-timed-secs`, `skip-timed-focused` in `AppState` drive the UI.

**Stale-response guard:** `VideoState.playback_generation` is a `u64` counter incremented at the top of every `start_playback` call. Each spawned task captures the current generation and discards its result if `vs.playback_generation` no longer matches when the response arrives. This prevents a slow network response for episode A from overwriting episode B's `intro_timestamps` or `credits_start` after a fast episode skip.

### Async strategy
Tokio for all async. The Slint event loop runs on the main thread. Background tasks (API calls, poster fetching) use `tokio::spawn`. Communication back to the UI uses `slint::invoke_from_event_loop` or channels.

## Build

```bash
cargo build                     # debug build
cargo build --release           # release
cargo run -p fjord-app          # run the app
```

Requires `mpv` and `libmpv` to be installed (`pacman -S mpv`).

## Dependencies (key ones)

| Crate | Purpose |
|-------|---------|
| `slint` | GUI framework |
| `slint-build` | build.rs compiler for .slint files |
| `libmpv2` | libmpv bindings |
| `reqwest` | HTTP client for Jellyfin API |
| `serde` / `serde_json` | JSON serialization |
| `tokio` | async runtime |
| `image` | JPEG/PNG decode for poster thumbnails |
| `gl` / `euclid` | OpenGL FBO management for mpv render API |
| `anyhow` / `thiserror` | error handling |

## What is Jellyfin

Jellyfin is an open-source media server. It exposes a REST API for browsing libraries (movies, TV shows, music) and getting playback URLs. Auth is username+password → returns an API token that goes in every subsequent request header as `X-Emby-Token` (Jellyfin kept the Emby header name).

Key API endpoints used:
- `POST /Users/AuthenticateByName` — login
- `GET /Users/{userId}/Items` — browse/search items
- `GET /Users/{userId}/Items/{itemId}` — item detail (overview, cast, backdrop tags, etc.)
- `GET /Items/{itemId}/Images/Primary` — poster image
- `GET /Items/{itemId}/Images/Backdrop/0` — backdrop image
- `GET /Users/{userId}/Items?Filters=IsResumable` — continue watching
- `GET /Users/{userId}/Items/Latest?IncludeItemTypes=…&GroupItems=true` — "Latest Media" rows (new episodes grouped into series, played incl.; bare array response)
- `GET /Shows/NextUp` — next unwatched episode per series (home row)
- `GET /Shows/NextUp?SeriesId=…` — next episode for a specific series (auto-advance)
- `GET /Shows/{seriesId}/Seasons` — season list
- `GET /Shows/{seriesId}/Episodes?seasonId=…` — episode list for a season
- `GET /Videos/{itemId}/stream?static=true&api_key=…` — direct-play URL
- `POST /Sessions/Playing` — report playback started
- `POST /Sessions/Playing/Progress` — report position
- `POST /Sessions/Playing/Stopped` — report stopped
- `GET /Episode/{itemId}/Timestamps` — all skippable segments (Introduction, Recap, Preview, Commercial, Credits) in one call (Intro Skipper plugin v2+, optional)
- `GET /Items/{itemId}/Similar?userId=…&Limit=12&Fields=…` — similar items (same type, movies or series)
- `GET /Users/{userId}/Items?IncludeItemTypes=BoxSet&Recursive=true&Fields=Id,Name` — all BoxSets (collection map build)
- `GET /Users/{userId}/Items?ParentId={boxsetId}&Fields=ProductionYear,UserData` — items in a BoxSet (collection row)

## Development workflow

1. **Implement** the feature or fix.
2. **Update PLAN.md** — check off completed items, add any new ones discovered.
3. **Update TOC headers** in every modified `.rs` / `.slint` file — symbols added/removed *and* behaviour changes.
4. **Commit and push** — always push immediately after committing (`git push`). The HTPC only sees what's on GitHub, so an unpushed commit is the same as no commit from the HTPC's perspective.
5. **Test on HTPC** — SSH in and run `makepkg -si` in the repo root. The PKGBUILD pulls from GitHub and does a native `cargo build --release --locked`.

## Testing setup

Two machines:
- **Dev machine** (this repo): AMD GPU, Wayland, Vulkan. Used for development.
- **HTPC**: NVIDIA legacy GPU, Wayland/EGL. The primary target. Logs land in `/home/htpc/.cache/fjord/fjord.log`.

Deploy workflow: push to GitHub → on the HTPC run `makepkg -si` with the `PKGBUILD` at the repo root. The PKGBUILD pulls from `https://github.com/KalasKonrad/Fjord.git` and does a native `cargo build --release --locked`, installing the binary to `/usr/bin/fjord`.

The HTPC is the harder target — it is what motivated the render API design in the first place.

## Known platform issues

### NVIDIA legacy Wayland: NVDEC stride corruption
**Symptom:** Diagonal stripe artifact (raw YUV scan lines) when using hardware decoding (`nvdec`, `nvdec-copy`). Software decoding is clean.

**Root cause:** NVDEC aligns decoded frame rows to 256-byte boundaries (e.g., a 1920-pixel-wide video gets a 2048-byte stride). mpv uploads via `glTexSubImage2D` with `GL_UNPACK_ROW_LENGTH=2048`. The NVIDIA legacy EGL driver silently ignores `GL_UNPACK_ROW_LENGTH`, so GL reads each row 128 bytes too tight — each successive row is offset from the previous, producing the diagonal slant.

**Fix:** Set `vf=format=yuv420p` in Settings → Video. This adds a software format conversion step after NVDEC decodes the frame, producing tight-packed yuv420p output so `GL_UNPACK_ROW_LENGTH` is never needed. For 10-bit HDR use `format=yuv420p10le`. The `auto` option detects the active hwdec and bit depth at runtime and picks the right format. `hwdec-image-format` was tried first but has no effect on NVIDIA legacy EGL.

**AMD Vulkan:** `vulkan-copy` works correctly with no stride workaround needed.

### PlayerConfig fields (fjord-player/src/mpv.rs)
All fields are logged at playback start so the log shows exactly what options were active. Key fields:
- `hwdec` — decoder selection (`auto`, `nvdec-copy`, `vulkan-copy`, etc.)
- `vf` — video filter string. Use `format=yuv420p` (or `auto`) for NVIDIA legacy stride fix.
- `video_sync` — `audio` (default), `display-resample` (locks to display refresh via `report_swap()`), `display-vdrop`, `display-adrop`, or `desync` (no A/V correction — debug option for isolating #39 passthrough dropout).
- `opengl_early_flush` — flush GL after each frame; may help with EGL pipeline ordering on NVIDIA.
- `video_latency_hacks` — compensates for imprecise Wayland vsync timestamps on NVIDIA 5xx legacy.

## Known Slint gotchas

These have each caused real bugs in this codebase:

**`Flickable` is the only reliable keyboard-scrollable container.** `ScrollView` ignores declarative `viewport-y` bindings (it manages its own scroll internally). `ListView` also writes to `viewport-y` from its own scroll handler, silently overwriting any binding you set. The correct pattern for any keyboard-driven scrollable list is `Flickable { viewport-height: ...; VerticalLayout { for ... } }` with `viewport-y` bound to a `clamp(...)` expression that tracks the focused index.

**Do not self-reference a `Flickable`'s own layout properties in its `viewport-y` binding.** Writing `viewport-y: clamp(... flk.height ... flk.viewport-height ...)` creates a binding whose dependencies Slint may not reliably track — `flk.height` and `flk.viewport-height` are layout-managed and may not trigger binding re-evaluation when `player-panel-cursor` changes. Instead, reference `parent.height` (the outer Rectangle's height) and the content layout's `preferred-height` directly: `clamp(-(cursor * 34px) + parent.height / 2 - 17px, min(0px, parent.height - list.preferred-height), 0px)`. This is what fixed the track panel scroll bug (#22).

**A `viewport-y` binding on a `Flickable` blocks native mouse-wheel scrolling.** When `viewport-y` is bound to an expression, the Flickable's internal scroll handler can't write to it (the binding overrides any assignment on the next frame), so mouse-wheel does nothing. When both keyboard nav and mouse-wheel scroll are needed: remove the `viewport-y` binding; on the outer Rectangle declare `property <length> kb-y: clamp(...)` and `changed kb-y => { fl.viewport-y = kb-y; }` for keyboard nav; the Flickable then handles mouse-wheel natively. Also fix any `fl.height` / `self.viewport-height` self-references in the old binding expression — use the outer container's `self.height` and the content layout's `preferred-height` instead. This is the pattern applied to all scrollable Flickables in the codebase (player panels, detail, series, home/movies/TV dashboards, library grid, settings right pane, browse list). Note: the browse list previously used `interactive: false` which blocks mouse-wheel regardless of bindings; changing it to `interactive: true` re-enables native scroll while child `TouchArea` clicks still fire normally (Slint distinguishes drag from click).

**Plain `Rectangle` children are horizontally centred by default.** If you need a fill bar or overlay anchored to the left edge, you must set `x: 0` explicitly. Omitting it centres the element and produces the "progress bar starts from the middle" bug.

**`KeyEvent.repeat` is unreliable — never use it to guard state transitions.** In practice `repeat` can be `false` for auto-repeated key events (confirmed on desktop Wayland, not just wireless keyboards). A guard like `if !event.repeat { close_screen() }` will fire on every spurious non-repeat event during a hold, chaining through screens unexpectedly. The correct pattern is to let the state machine be the guard: once the transition fires (e.g. `show-browse = false`), the outer `if AppState.show-browse` condition stops subsequent events from re-firing it. For search fields specifically: Backspace should only delete characters; use Escape as the dedicated "exit search" key. Never use `!event.repeat` to gate a backspace-exits-search path — a held Backspace will empty the query and then bleed into the close-screen handler.

**Slint ternary short-circuits dependency tracking.** If a property binding uses `condition ? A : B` and `B` contains a reactive property (e.g. `has-hover`), Slint only tracks `B`'s dependencies when the else-branch is actually evaluated. If the condition is initially true, `has-hover` is never read and hover changes never trigger a repaint. Fix: read the property unconditionally first using a block expression — `background: { let hov = ta.has-hover; cond ? Theme.accent : (hov ? Theme.surface : transparent) };`. This was the root cause of settings left-pane hover not working.

**`invoke_from_event_loop` closures must be `'static + Send`.** Capture owned values (`String`, `Arc<…>`) not references. This is the correct pattern for communicating from Tokio tasks back to Slint UI state.

**`TouchArea.moved` fires only during drag (button held), not plain cursor movement.** To react to mouse movement without a button press, use `changed mouse-x => { ... }` and `changed mouse-y => { ... }` callbacks. This is how the player controls overlay auto-show is implemented.

**`opacity: 0` elements remain fully hit-testable.** Setting `opacity: 0` makes an element invisible but it still participates in hit-testing and determines the mouse cursor shape — only `visible: false` removes it from event handling. The player controls bar fades via `opacity`, so its child `TouchArea`s were silently overriding `mouse-cursor: none` on the element beneath them. The fix is a full-size cursor-hider `TouchArea` declared last (highest z-order) with `enabled: !root.controls-visible` and `mouse-cursor: MouseCursor.none`. When `enabled: false`, a `TouchArea` passes events through to elements below it.

**`self.width` / `self.height` inside a conditional element reads from the parent layout cache — do not use it in ChangeTracker properties.** When a `changed prop => { ... }` ChangeTracker initialises it immediately reads the tracked property. If that property reads `self.width` or `self.height`, and `self` is an element inside a VerticalLayout/HorizontalLayout that is a conditional (`if cond: Element { ... }`), then `self.width`/`self.height` is derived from the parent layout's cache. If that layout cache is currently being evaluated — e.g. because a `kb-y` anchor position triggered it — and the layout cache called `ensure_updated()` on this very conditional (which then ran `user_init` → ChangeTracker → reads `self.width` → reads the layout cache) — Slint detects recursion and panics. **Fix:** replace `self.width`/`self.height` with `root.width`/`root.height` (the component root's size, set by the outer parent, never derived from the internal layout cache). Both values are always equal when the conditional element fills the full component width/height, which is the normal case. This was the root cause of the series screen "Recursion detected" crash — season tabs `kb-x` read `self.width`, which came from the content VerticalLayout's layout cache that was in the middle of calling `ensure_updated()` on the season tabs conditional.

## Style

- Standard Rust formatting (`cargo fmt`)
- Errors: use `anyhow::Result` at the top level, `thiserror` for library error types
- No `unwrap()` in library code — propagate errors
- Keep `fjord-api` and `fjord-player` free of Slint imports
- Every `.rs` and `.slint` source file opens with a `// ── <crate> · <filename> ──` header block listing its major symbols/sections (one line each). Longer files additionally carry `// ──` inline section markers immediately before major functions and visual blocks. The header is the first thing in the file, before any `use` statements or declarations. Update the header whenever symbols are added, removed, or their behaviour changes — not just when the name changes.
