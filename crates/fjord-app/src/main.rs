// ── fjord-app · main.rs ──────────────────────────────────────────────────────
//   model helpers        item_to_card_item, items_to_model — both now take `watchlist:
//                        &HashSet<String>` (2026-07-20, real bug fix: FjordState.jellyfin_watchlist_ids
//                        consulted directly at construction time, since a live model patch alone
//                        gets silently wiped by the next rebuild — see that field's own doc
//                        comment in config.rs); apply_cards_preserving_identity (mutate an
//                        existing model in place when ids/order match, so unrelated cards' poster Image
//                        elements survive a refresh instead of re-fading, Phase 96), push_section_model
//                        (takes HomeSection), show_toast (any-thread toast helper),
//                        set_server_url_ui (server-url + server-unencrypted together)
//   trim_last_grapheme   removes exactly one Unicode grapheme cluster (not scalar value) from the
//   grapheme_count/insert_at_grapheme/delete_before_grapheme/delete_at_grapheme/with_caret
//                        caret editing by grapheme for the hand-drawn text fields (2026-10-04)
//                        end of a string — extracted from the on-screen keyboard's own Backspace
//                        handler (2026-08-23) so every plain append/remove-from-the-end search/
//                        naming field (Browse, Discover, Library grid, PlaylistPicker naming — none
//                        of them a real LineEdit with cursor-position risk) shares the same
//                        emoji/accent-safe backspace on ordinary typing too, not just the on-screen
//                        keyboard's own ⌫ key
//   panic hook           writes "PANIC" + backtrace to fjord.log (timestamp + thread since 2026-10-08)
//   restrict_log_permissions  log folder 0700, fjord.log + rotated logs 0600 (best-effort,
//                        failures logged once; 2026-10-09 security review; unit-tested)
//   session_current      Arc::ptr_eq guard against FjordState.client — shared by
//                        spawn_screen_cache_refresh (above) and prewarm.rs::spawn_metadata_prewarm;
//                        see doc comment at definition
//   should_revalidate    rate-limits the 7 screen "revalidate on cache hit" functions (Collection/
//                        Detail/Series/Season/Artist/Person/Album) to once per 60s per item id
//                        (2026-07-31, same missing-guard bug class as discover's own
//                        seerr_admin_last_refresh cooldown, fixed the same day) — see doc comment
//                        at definition
//   profile::wire_idle_lock_timer  15s repeating slint::Timer (Bonfire Phase 4, inactivity
//                        auto-lock, 2026-08-29) — see its own doc comment in profile.rs for the
//                        full mechanism; wired here alongside the other 4 periodic timers.
//   on_handle_key/activity::FjordApplicationHandler  the two activity-reset sites for
//                        wire_idle_lock_timer's own idle clock — every keypress (on_handle_key)
//                        and, since the event-loop branch (2026-09-08), TRUE global mouse
//                        activity via a winit-level slint::BackendSelector hook (activity.rs) that
//                        observes every raw CursorMoved/MouseInput/MouseWheel before Slint's own
//                        hit-testing — replaces the old best-effort AppState.record-activity()
//                        TouchArea mechanism, which only ever saw movement over uncovered
//                        background, never over a MediaCard/FjordButton/NavItem sitting on top.
//   main                 entry point; log rotation (fjord.log → .old each start) + per-layer
//                        callbacks: one wire_* call per area — the code lives in that area's module
//                        (0.5.0 step 3); helpers that moved out are re-exported below the `mod` list
//                        filters (console + file both use Config.log_level, read directly off
//                        disk before the subscriber exists; RUST_LOG still overrides either —
//                        Settings→General row, applies on next launch); panic hook (writes to
//                        fjord.log); logs "fjord version: {FJORD_BUILD_ID}"; wires all AppState
//                        global callbacks
//     apply saved cfg    cold-start vs warm-start; sets show-connecting, calls spawn_auto_login
//                        — cached content is no longer shown before connectivity is confirmed
//                        (a full outage used to look identical to normal quiet operation)
//     login              on_do_login → auth::do_login (also starts websocket)
//     browse play        on_play_item (server-side search results)
//     home / library     on_item_play, on_open_library (lazy fetch: nav=1=TV, nav=2=Movies, nav=3=Collections, nav=4=Artists)
//     detail             on_play_detail, on_resume_detail, on_close_detail
//     collection         on_open_collection → collection::open_collection_screen
//     artist             on_open_artist → artist::open_artist_screen; on_close_artist;
//                        on_toggle_artist_fav; on_play_artist_all (fetches all album tracks, starts queue)
//     album              on_open_album → album::open_album_screen; on_close_album; on_play_album_track;
//                        on_toggle_album_fav; on_toggle_album_played
//     series             on_open_series, on_series_select_season (cache+generation guard), on_play_series_episode,
//                        on_toggle_series_played, on_toggle_series_fav
//     season             on_open_season_detail, on_close_season_detail, on_toggle_season_fav, on_toggle_season_played
//     person             on_open_person, on_open_discover_person (2026-08-13, TMDB cast member), on_close_person
//     Up Next banner     on_cancel_auto_advance (Skip), on_play_next_ep (Play Now)
//     player controls    wire_controls
//     context menu       wire_context_menu, wire_queue_callbacks
//     queue panel        on_open_queue_panel (mouse entry point for the music-bar ⋮ button —
//                        mirrors keys.rs's 'q' path, plus a keyboard focus re-grab, CR11-6)
//     audio devices      fetch_audio_devices (startup), on_audio_device_selected
//     system fonts       fetch_system_fonts (startup), on_font_family_selected — same pattern,
//                        for Settings → UI → Text font; settings-font-family (the value bound
//                        to MainWindow.font-family) is set synchronously from Config at launch,
//                        not left waiting on the async fc-list enumeration
//     yt-dlp detection   detect_yt_dlp (startup, same fetch-once-locally pattern as audio
//                        devices/fonts above) — gates Watch Trailer button visibility;
//                        trailer_ytdl_format maps Trailer Quality to an mpv ytdl-format string;
//                        on_play_trailer registered here (not discover.rs — see its own comment
//                        at the call site) since it needs VideoState
//     settings           on_settings_changed (also live-applies subtitle appearance via
//                        Player::set_sub_style when a player is active, no restart needed;
//                        also rebuilds seerr_client + push_seerr_status every save so
//                        toggling seerr-enabled live-hides/shows every seerr-connected-gated
//                        row instead of only taking effect after a restart, 2026-07-17);
//                        client-version set once at startup (FJORD_BUILD_ID), unlike
//                        server-name/server-version which are set per-login;
//                        on_settings_row_focused (mouse click path, → settings::row_focused);
//                        on_keybinding_collision_confirmed/_cancelled (2026-08-07, Key
//                        Bindings rebind-collision confirm — the only two places that ever
//                        call keys::apply_rebind for the collision path, keyboard and mouse
//                        both funnel through these same two callbacks)
//     spawn_seerr_settings_fetch  streaming region + display language + discover language +
//                        discover region (2026-07-18), one round trip (2026-07-17, extended
//                        from streaming-region-only); also captures the connected account's
//                        own id + MANAGE_REQUESTS bit (FjordState.seerr_user_id/seerr_is_admin,
//                        AppState.seerr-is-admin — piggybacks on the same /auth/me call, no new
//                        round trip) for the Discover context menu's ownership check +
//                        Approve/Decline gate (2026-07-18); detect_yt_dlp, fetch_audio_devices,
//                        fetch_system_fonts — same fetch-once-at-startup shape, gated on a live
//                        Seerr connection first
//     on_discover_region_selected  Settings → Integrations → Discover Region (2026-07-18,
//                        Watchlist + Release Calendar) — same GET-mutate-POST round trip as
//                        on_streaming_region_selected, writes UserGeneralSettings.discover_region
//     discover watchlist  discover::ensure_discover_watchlist called here too (2026-07-20,
//                        Watchlist row), alongside the existing nav==6 trigger in
//                        discover.rs::wire_discover — its own discover_watchlist_fetched guard
//                        makes calling it redundantly from both sites safe, whichever fires first
//                        wins; the Discover/dashboard Watchlist rows and the in-library star
//                        (patch_watchlist_on_jellyfin_models) both need it populated well before
//                        a user ever visits the Discover tab. Deliberately called AFTER the
//                        seerr-client block's own `s` MutexGuard is dropped, not from inside
//                        it — real bug fixed same day, live-reported "fjord do not even start":
//                        ensure_discover_watchlist synchronously locks `state` itself before
//                        spawning, and std::sync::Mutex isn't reentrant, so calling it while `s`
//                        was still held there self-deadlocked the whole app before window.run()
//                        was ever reached (see this function's own inline comment at the call
//                        site, and CLAUDE.md's Seerr integration section, for the full trace)
//     fullscreen         on_toggle_fullscreen, launch-fullscreen setting; the startup gate's
//                        own launch_fullscreen apply (2026-08-14 fix) is hoisted ABOVE the
//                        show_picker/auto-login split so a picker launch gets it too, not
//                        just plain auto-login
//     account/profile picker (2026-08-14, 2-tier redesign)  on_account_picker_select,
//                        on_account_picker_add_account, on_settings_add_account,
//                        on_profile_picker_back_to_accounts, on_default_account_selected —
//                        mirror the pre-existing profile-tier callbacks one tier up; see
//                        profile.rs's own header for what each does
//     reset_session_state  shared teardown between sign-out and Bonfire profile switching
//                        (Phase 1 step 3, 2026-08-09) — stops playback, aborts the
//                        websocket, clears every in-memory FjordState list/cache and closes
//                        every content-bearing screen (extended 2026-08-14 for the account
//                        picker's own show/source/prefill state; extended 2026-08-16, code
//                        review, to also clear show-profile-picker itself — previously only
//                        its sibling show-account-picker was cleared here, despite this
//                        function's own doc comment already claiming to cover "either tier's
//                        overlay"); extended again 2026-08-29 (Bonfire Phase 4 review) to also
//                        clear show-blocklist/show-playlist-picker — the identical "outgoing
//                        content still visible" gap, found while designing the idle-lock timer
//                        (an unattended background lock is far more likely to actually catch a
//                        user on one of these two screens than a deliberate sign-out click is);
//                        does NOT touch Config's auth/Seerr fields or decide what
//                        shows next, both genuinely caller-specific
//     sign-out           on_sign_out: removes the signed-out profile (+ its Bonfire
//                        sub-profiles) from Config.profiles, reset_session_state, then routes
//                        to the account picker if 2+ accounts remain, else plain Login
//                        (2026-08-16, code review — was always Login unconditionally, even
//                        with another valid account still known; resolved via AskUserQuestion)
//     retry connection   on_retry_connection (OfflineScreen's Retry button + Enter key) →
//                        re-invokes spawn_auto_login with fresh clones
// ─────────────────────────────────────────────────────────────────────────────
slint::include_modules!();

mod activity;
mod album;
mod artist;
mod auth;
mod blocklist;
mod bonfire_admin;
mod browse;
mod collection;
mod config;
mod context_menu;
mod controls;
mod detail;
mod discover;
mod display_sync;
mod dmabuf_plane;
mod hdr;
mod home;
mod keys;
mod movies;
mod music;
mod person;
mod pipewire_fix;
mod playback;
mod poster;
mod prewarm;
mod profile;
mod profile_edit;
mod season;
mod secrets;
mod seerr_auth;
mod series;
mod session;
mod settings;
mod startup;
mod stats;
mod text_field;
mod video_surface;
mod ws;

pub(crate) use discover::{detect_yt_dlp, trailer_ytdl_format};
pub(crate) use display_sync::spawn_display_sync_modes_fetch;
pub(crate) use movies::{spawn_library_fetch, spawn_movies_list_fetch};
pub(crate) use music::{push_queue_display, spawn_queue_poster_loading};
pub(crate) use prewarm::wire_prewarm_progress_timer;
pub(crate) use seerr_auth::spawn_seerr_settings_fetch;
pub(crate) use session::reset_session_state;
pub(crate) use settings::{
    apply_settings_to_window, fetch_audio_devices, fetch_system_fonts, read_settings_from_window,
    settings_diff, settings_snapshot,
};
pub(crate) use startup::{
    AMBIENT_REFRESH_LIMIT, push_cached_data, spawn_auto_login, spawn_jellyfin_admin_check,
    spawn_screen_cache_refresh, wire_screen_cache_save_timer,
};

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU32};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::Result;
use fjord_api::{JellyfinClient, models::MediaItem};
use slint::{Global, Model, ModelRc, SharedString, StandardListViewItem, VecModel};
use tracing::{debug, info, warn};
use url::Url;

use config::{
    FjordState, ensure_device_id, load_config, load_screen_caches, save_config, save_screen_caches,
    sub_color_hex,
};
use home::{
    HomeSection, fetch_home_data, fetch_movie_collections, home_data_sections, load_albums_cache,
    load_artists_cache, load_collections_cache, load_home_cache, load_movies_cache,
    load_playlists_cache, load_series_cache, push_home_data, push_home_data_preserving_posters,
    refresh_row_preserving_posters, run_poster_cache_cleanup, save_albums_cache,
    save_artists_cache, save_collections_cache, save_home_cache, save_movies_cache,
    save_playlists_cache, save_series_cache, wire_nw_timer,
};
use movies::{
    spawn_albums_poster_loading, spawn_artists_poster_loading, spawn_collections_poster_loading,
    spawn_movies_poster_loading, spawn_playlists_poster_loading,
};
use playback::{
    VideoState, do_stop_playback, quit_cleanup, start_playback, wire_mpv_timer,
    wire_rendering_notifier,
};
use poster::{spawn_poster_loading, spawn_series_poster_loading};
use series::{ep_to_card, open_series_screen, spawn_episode_thumb_loading};

pub(crate) fn is_unauthorized(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .map(|s| s.as_u16() == 401)
        .unwrap_or(false)
}

pub(crate) fn is_not_found(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .map(|s| s.as_u16() == 404)
        .unwrap_or(false)
}

/// Bonfire's own real developer-api.md (fetched live, 2026-08-29, after a
/// live report of hitting this): both `/plugins/profiles/switch` and
/// `/plugins/profiles/verify-pin` are "rate limited to 5 failed attempts in
/// 15 minutes" — an anti-brute-force lockout on the PIN, not a blanket
/// per-request limit, and NOT distinguishable from a wrong PIN by anything
/// but this status code (400 = wrong PIN/bad request, 429 = rate-limited).
/// Used by `switch_to_profile` to swap the raw `HTTP status client error
/// (429 Too Many Requests) for url (...)` text for something a user can
/// actually act on — a wire_idle_lock_timer-triggered unlock is exactly as
/// capable of tripping this as a manual switch is.
pub(crate) fn is_rate_limited(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .map(|s| s.as_u16() == 429)
        .unwrap_or(false)
}

/// True if `client` is still the session's live client. Multi-second (or, for
/// the opt-in prewarm, multi-minute) background sweeps write per-user data
/// (`MediaItem`s embed `UserData`: played/favorite) into shared `FjordState`
/// caches; without this guard, a sign-out (or a different account signing
/// back in on a shared HTPC) mid-sweep lets the old account's results keep
/// landing in the new session's caches for as long as the sweep keeps
/// running. Mirrors `ws.rs`'s identical guard (CR11-2) for the same class of
/// risk. Shared by `spawn_screen_cache_refresh` (below, runs on every login)
/// and `prewarm.rs::spawn_metadata_prewarm` (opt-in, user-triggered).
pub(crate) fn session_current(state: &Mutex<FjordState>, client: &Arc<JellyfinClient>) -> bool {
    state
        .lock()
        .unwrap()
        .client
        .as_ref()
        .is_some_and(|c| Arc::ptr_eq(c, client))
}

/// The Seerr-flavored twin of `session_current` (Bonfire Phase 1, step 8
/// audit, 2026-08-09) — Discover's own long-running fetches (landing rows,
/// watchlist, search) hold an `Arc<SeerrClient>`, not an
/// `Arc<JellyfinClient>`, so they need their own `Arc::ptr_eq` check against
/// `FjordState.seerr_client` rather than `session_current`'s Jellyfin one.
/// Same reasoning: `reset_session_state` clears `seerr_client` on both
/// sign-out and a Bonfire profile switch (a different Jellyfin user can have
/// a completely different, or no, Seerr connection), and Discover's fetches
/// are comparatively long (several sequential/parallel TMDB calls) with no
/// other per-fetch staleness guard of their own.
pub(crate) fn seerr_session_current(
    state: &Mutex<FjordState>,
    client: &Arc<fjord_seerr::SeerrClient>,
) -> bool {
    state
        .lock()
        .unwrap()
        .seerr_client
        .as_ref()
        .is_some_and(|c| Arc::ptr_eq(c, client))
}

// Rate-limits the 7 screen-open "revalidate on a cache hit" functions
// (collection.rs/detail.rs/series.rs/season.rs/artist.rs/person.rs/album.rs'
// spawn_X_revalidate) — same bug class, same fix shape as
// discover::refresh_seerr_admin_status's own cooldown (2026-07-31): each of
// these fired a full item-detail + list + N-poster refetch on EVERY open of
// an already-cached screen, no guard at all, so rapid back-and-forth between
// a couple of recently-viewed items (an ordinary browsing pattern, not an
// edge case) re-fired the whole fetch set every time. Jellyfin item ids are
// unique GUIDs, so one shared map (not one per screen type) is sufficient.
const REVALIDATE_COOLDOWN: Duration = Duration::from_secs(60);

pub(crate) fn should_revalidate(state: &Mutex<FjordState>, id: &str) -> bool {
    let mut s = state.lock().unwrap();
    if s.screen_revalidate_last_run
        .get(id)
        .is_some_and(|t| t.elapsed() < REVALIDATE_COOLDOWN)
    {
        return false;
    }
    s.screen_revalidate_last_run
        .insert(id.to_string(), Instant::now());
    true
}

// Self-healing for ghost items (cache-staleness fix S4): when a fetch 404s the
// item no longer exists on the server — remove it from every canonical vec and
// visible model, mark the list caches dirty, and tell the user. Safe to call
// from any thread.
pub(crate) fn purge_deleted_item(
    state: &Arc<Mutex<FjordState>>,
    ww: &slint::Weak<MainWindow>,
    id: &str,
) {
    {
        let mut s = state.lock().unwrap();
        s.all_movies.retain(|i| i.id != id);
        s.all_series.retain(|i| i.id != id);
        s.all_collections.retain(|i| i.id != id);
        s.all_artists.retain(|i| i.id != id);
        s.all_albums.retain(|i| i.id != id);
        s.filtered_items.retain(|i| i.id != id);
        s.movie_collections.remove(id);
        for eps in s.series_episode_cache.values_mut() {
            eps.retain(|e| e.id != id);
        }
        // Whatever list this ghost came from is stale — refresh on next grid open.
        s.movies_fetched = false;
        s.movie_posters_loaded = false;
        s.collections_fetched = false;
        s.artists_fetched = false;
        s.albums_fetched = false;
    }
    let id2 = id.to_string();
    let ww2 = ww.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww2.upgrade() {
            crate::context_menu::remove_item_from_all_models(&w, &id2);
        }
    });
    show_toast(ww.clone(), "Item was removed from the server".to_string());
}

/// Show a bottom-center error toast.  Safe to call from any thread or the Slint event loop.
/// The Slint Timer in main.slint auto-dismisses it after 4 s.
pub(crate) fn show_toast(ww: slint::Weak<MainWindow>, msg: String) {
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            let g = AppState::get(&w);
            g.set_toast_message(msg.as_str().into());
            g.set_toast_visible(true);
            debug!("show_toast: {msg:?}");
        } else {
            debug!("show_toast: window gone, dropped {msg:?}");
        }
    });
}

/// Strips embedded HTML markup out of a metadata field, converting basic
/// structure (paragraphs, list items, line breaks) into plain-text
/// equivalents rather than just deleting tags outright. Some metadata
/// providers — anime-focused ones especially, confirmed live via a
/// screenshot of a person's bio rendering raw `<p>`/`<strong>`/`<a href>`/
/// `<ul><li>` markup verbatim — return `Overview`/bio text as real HTML that
/// Jellyfin passes through unprocessed; Fjord never sanitized it anywhere.
/// No HTML-parsing crate was added for this — confirmed via `Cargo.toml`
/// that neither `regex` nor any html crate is a workspace dependency, and
/// the tag vocabulary actually seen in the wild (`p`, `br`, `strong`, `em`,
/// `a`, `ul`/`li`, `div`, `span`) is small and well-known enough that a
/// plain linear scan covers it without one.
pub(crate) fn strip_html_to_text(s: &str) -> String {
    if !s.contains('<') {
        // Fast path — no markup at all, the overwhelming common case.
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '<' {
            out.push(c);
            continue;
        }
        let mut tag = String::new();
        for c2 in chars.by_ref() {
            if c2 == '>' {
                break;
            }
            tag.push(c2);
        }
        let tag_lower = tag.to_ascii_lowercase();
        let closing = tag_lower.starts_with('/');
        let tag_name = tag_lower
            .trim_start_matches('/')
            .split_whitespace()
            .next()
            .unwrap_or("");
        match tag_name {
            "br" => out.push('\n'),
            "li" if !closing => {
                if !out.is_empty() && !out.ends_with('\n') {
                    out.push('\n');
                }
                out.push_str("• ");
            }
            "p" | "li" | "div" if closing && !out.ends_with('\n') => {
                out.push('\n');
            }
            _ => {} // strong/em/a/span/ul/etc — drop the tag, keep any inner text
        }
    }
    // Decode the handful of entities actually seen in practice — done after
    // tag-stripping, not before, since &lt;/&gt; represent literal escaped
    // characters in the source text, not real markup for the scan above to
    // reinterpret.
    let out = out
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&nbsp;", " ")
        .replace("&lt;", "<")
        .replace("&gt;", ">");
    // Collapse runs of 2+ blank lines down to a single blank line, and trim
    // trailing whitespace per line (tag-stripping above can leave some).
    let mut result = String::new();
    let mut blank_run = 0;
    for line in out.lines() {
        let line = line.trim_end();
        if line.is_empty() {
            blank_run += 1;
            if blank_run > 1 {
                continue;
            }
        } else {
            blank_run = 0;
        }
        if !result.is_empty() {
            result.push('\n');
        }
        result.push_str(line);
    }
    result.trim().to_string()
}

// ── model helpers ─────────────────────────────────────────────────────────────

/// `watchlist` is the resolved set of LOCAL Jellyfin ids currently on the
/// Seerr watchlist (`FjordState.jellyfin_watchlist_ids`) — consulted here,
/// at construction time, rather than relying solely on a live model patch
/// afterward, since a patch alone gets silently wiped by the next rebuild
/// (see `FjordState.jellyfin_watchlist_ids`'s own doc comment for the real
/// bug this fixes, 2026-07-20).
pub(crate) fn item_to_card_item(
    i: &MediaItem,
    watchlist: &std::collections::HashSet<String>,
) -> CardItem {
    CardItem {
        id: SharedString::from(i.id.as_str()),
        item_type: SharedString::from(i.item_type.as_str()),
        title: SharedString::from(i.card_title().as_str()),
        subtitle: SharedString::from(i.card_subtitle().as_str()),
        year: i.production_year.unwrap_or(0) as i32,
        has_played: i.user_data.played,
        is_favorite: i.user_data.is_favorite,
        resume_pct: i.resume_pct(),
        unplayed_count: i.user_data.unplayed_item_count,
        on_watchlist: watchlist.contains(&i.id),
        ..Default::default()
    }
}

pub(crate) fn items_to_model(
    items: &[MediaItem],
    watchlist: &std::collections::HashSet<String>,
) -> ModelRc<CardItem> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|i| item_to_card_item(i, watchlist))
            .collect::<Vec<_>>(),
    ))
}

/// Apply `fresh` cards to `old`'s model. If `fresh` has the same ids in the same
/// order as what's already there, mutate the EXISTING model row-by-row via
/// set_row_data instead of returning a new ModelRc — swapping the model instance
/// makes Slint destroy and recreate every delegate element (including each card's
/// poster Image), which re-triggers FadeInTrigger's fade-in even when nothing
/// about the card actually changed. Only a genuine membership/order difference
/// rebuilds a new model, which is correct there (a fade is expected). Shared by
/// every place that builds a fresh Vec<CardItem> and pushes it to a model —
/// poster.rs's home/series decode, movies.rs's library decode, home.rs's row
/// merges, context_menu.rs's WS delta-sync upserts (Phase 96 consolidation).
pub(crate) fn apply_cards_preserving_identity(
    old: &ModelRc<CardItem>,
    fresh: Vec<CardItem>,
) -> ModelRc<CardItem> {
    let old_rows: Vec<CardItem> = (0..old.row_count())
        .filter_map(|i| old.row_data(i))
        .collect();
    let same_shape = old_rows.len() == fresh.len()
        && old_rows
            .iter()
            .zip(fresh.iter())
            .all(|(a, b)| a.id.as_str() == b.id.as_str());
    if same_shape {
        tracing::debug!(
            "apply_cards_preserving_identity: {} row(s), same_shape=true",
            fresh.len()
        );
        for (i, card) in fresh.into_iter().enumerate() {
            old.set_row_data(i, card);
        }
        return old.clone();
    }
    // Diagnostic: pin down *why* same_shape failed — different length, or same
    // length but reordered/different ids. Left at debug (not removed) since this
    // exact log is what pinpointed Phases 96-99's library-grid flash bugs.
    let first_mismatch = old_rows
        .iter()
        .zip(fresh.iter())
        .position(|(a, b)| a.id.as_str() != b.id.as_str());
    tracing::debug!(
        "apply_cards_preserving_identity: old_len={} fresh_len={} same_shape=false first_mismatch_idx={:?}",
        old_rows.len(),
        fresh.len(),
        first_mismatch
    );
    ModelRc::new(VecModel::from(fresh))
}

pub(crate) fn push_section_model(window: &MainWindow, sec: HomeSection, model: ModelRc<CardItem>) {
    let g = AppState::get(window);
    match sec {
        HomeSection::ContinueWatching => g.set_continue_watching(model),
        HomeSection::NextUp => g.set_next_up(model),
        HomeSection::RecentlyAdded => g.set_recently_added(model),
        HomeSection::ContinueWatchingMovies => g.set_continue_watching_movies(model),
        HomeSection::RecentlyAddedMovies => g.set_recently_added_movies(model),
        HomeSection::NotWatchedMovies => g.set_not_watched_movies(model),
        HomeSection::ContinueWatchingTv => g.set_continue_watching_tv(model),
        HomeSection::RecentlyAddedTv => g.set_recently_added_tv(model),
        HomeSection::NotWatchedTv => g.set_not_watched_tv(model),
        HomeSection::RecentlyAddedCollections => g.set_recently_added_collections(model),
        HomeSection::UnwatchedCollections => g.set_unwatched_collections(model),
        HomeSection::RecentlyAddedAlbums => g.set_recently_added_albums(model),
        HomeSection::RecentlyPlayedAlbums => g.set_recently_played_albums(model),
        HomeSection::FavoriteMovies => g.set_favorite_movies(model),
        HomeSection::FavoriteSeries => g.set_favorite_series(model),
        HomeSection::FavoriteAlbums => g.set_favorite_albums(model),
        HomeSection::Playlists => g.set_music_playlists(model),
    }
}

/// Read-side counterpart to push_section_model — lets a poster-decode pass apply
/// via apply_cards_preserving_identity instead of always building a fresh model.
pub(crate) fn get_section_model(window: &MainWindow, sec: HomeSection) -> ModelRc<CardItem> {
    let g = AppState::get(window);
    match sec {
        HomeSection::ContinueWatching => g.get_continue_watching(),
        HomeSection::NextUp => g.get_next_up(),
        HomeSection::RecentlyAdded => g.get_recently_added(),
        HomeSection::ContinueWatchingMovies => g.get_continue_watching_movies(),
        HomeSection::RecentlyAddedMovies => g.get_recently_added_movies(),
        HomeSection::NotWatchedMovies => g.get_not_watched_movies(),
        HomeSection::ContinueWatchingTv => g.get_continue_watching_tv(),
        HomeSection::RecentlyAddedTv => g.get_recently_added_tv(),
        HomeSection::NotWatchedTv => g.get_not_watched_tv(),
        HomeSection::RecentlyAddedCollections => g.get_recently_added_collections(),
        HomeSection::UnwatchedCollections => g.get_unwatched_collections(),
        HomeSection::RecentlyAddedAlbums => g.get_recently_added_albums(),
        HomeSection::RecentlyPlayedAlbums => g.get_recently_played_albums(),
        HomeSection::FavoriteMovies => g.get_favorite_movies(),
        HomeSection::FavoriteSeries => g.get_favorite_series(),
        HomeSection::FavoriteAlbums => g.get_favorite_albums(),
        HomeSection::Playlists => g.get_music_playlists(),
    }
}

/// Wraps a future with `debug!` timing — user question, 2026-08-14 ("can we
/// make the login faster, what is it that make it take some time"). The
/// login/session-setup pipeline (`finish_session_setup`'s own 4-way join,
/// `fetch_home_data`'s 14-way join inside it) was already confirmed fully
/// parallel — nothing sequential to fix there — so the real answer to "what
/// makes it slow" is "whichever single request is slowest," which no log
/// anywhere currently identifies. Wrapping each branch with this makes the
/// next real login/session-setup show exactly which call dominates, instead
/// of only ever seeing the combined `tokio::join!` total.
pub(crate) async fn timed<T>(label: &str, fut: impl std::future::Future<Output = T>) -> T {
    let started = std::time::Instant::now();
    let r = fut.await;
    tracing::debug!(
        "timing: {label} took {:.3}s",
        started.elapsed().as_secs_f64()
    );
    r
}

pub(crate) fn to_slint_model(names: Vec<String>) -> ModelRc<StandardListViewItem> {
    let items: Vec<StandardListViewItem> = names
        .into_iter()
        .map(|name| {
            let mut e = StandardListViewItem::default();
            e.text = SharedString::from(name.as_str());
            e
        })
        .collect();
    ModelRc::new(VecModel::from(items))
}

pub(crate) fn display_names(items: &[MediaItem]) -> Vec<String> {
    items.iter().map(|i| i.display_name()).collect()
}

fn ss(s: &str) -> SharedString {
    SharedString::from(s)
}

/// Drop the last Unicode GRAPHEME CLUSTER from `s`, not the last `char`
/// (Unicode scalar value). A naive char-based trim never splits a
/// multi-byte character in half, but it DOES leave a dangling combining
/// mark behind for a decomposed accented character (e.g. NFD "café" =
/// 'c','a','f','e', COMBINING ACUTE ACCENT — one backspace removes only
/// the accent, leaving a bare 'e'), or half a flag emoji (a
/// regional-indicator pair) — both empirically reproduced with a throwaway
/// test before this was first fixed for the on-screen keyboard (code
/// review, 2026-08-22). unicode-segmentation's real UAX #29 grapheme-
/// cluster boundaries handle both correctly; it's already a direct
/// `fjord-app` dependency. Extracted (2026-08-23, full on-screen-keyboard
/// rollout) from `on_onscreen_keyboard_trim_last`'s own closure body so
/// Discover/Browse/PlaylistPicker's own native (non-on-screen-keyboard)
/// backspace handling can share the identical fix — those 3 fields are
/// always append/remove-from-the-end only (no cursor-position concept
/// exists for them, unlike a real `LineEdit`), so unlike Login's own field
/// this is safe to apply unconditionally, not just via the on-screen
/// keyboard's own dispatch path.
pub(crate) fn trim_last_grapheme(s: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let mut graphemes: Vec<&str> = s.graphemes(true).collect();
    graphemes.pop();
    graphemes.concat()
}

/// AppState.server-url together with its server-unencrypted flag (Settings
/// shows "not encrypted" for an http:// server — 2026-10-09 security review).
pub(crate) fn set_server_url_ui(g: &AppState, url: &str) {
    g.set_server_url(ss(url));
    g.set_server_unencrypted(url.trim().to_ascii_lowercase().starts_with("http://"));
}

// ── Text cursor for the hand-drawn search fields (2026-10-04) ────────────────
// Live-reported: in Discover search you couldn't move back to fix one letter,
// only delete everything after it. `cursor` counts grapheme clusters before
// the caret (same unit as trim_last_grapheme, so an accented letter or a flag
// is one step); out-of-range cursors are clamped to the end.
pub(crate) fn grapheme_count(s: &str) -> usize {
    use unicode_segmentation::UnicodeSegmentation;
    s.graphemes(true).count()
}

/// Inserts `text` at the caret; returns the new string and caret.
pub(crate) fn insert_at_grapheme(s: &str, cursor: usize, text: &str) -> (String, usize) {
    use unicode_segmentation::UnicodeSegmentation;
    let g: Vec<&str> = s.graphemes(true).collect();
    let c = cursor.min(g.len());
    let out = format!("{}{}{}", g[..c].concat(), text, g[c..].concat());
    // Count the caret in the result's own graphemes: `text` can merge with a
    // neighbour (e.g. a combining accent) instead of adding a whole step.
    let new_c = grapheme_count(&format!("{}{}", g[..c].concat(), text));
    (out, new_c)
}

/// Backspace: removes the grapheme before the caret.
pub(crate) fn delete_before_grapheme(s: &str, cursor: usize) -> (String, usize) {
    use unicode_segmentation::UnicodeSegmentation;
    let mut g: Vec<&str> = s.graphemes(true).collect();
    let c = cursor.min(g.len());
    if c == 0 {
        return (s.to_string(), 0);
    }
    g.remove(c - 1);
    (g.concat(), c - 1)
}

/// Delete: removes the grapheme after the caret.
pub(crate) fn delete_at_grapheme(s: &str, cursor: usize) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let mut g: Vec<&str> = s.graphemes(true).collect();
    if cursor < g.len() {
        g.remove(cursor);
    }
    g.concat()
}

/// The text with `caret` drawn at the cursor (the fields draw "▌").
pub(crate) fn with_caret(s: &str, cursor: usize, caret: &str) -> String {
    use unicode_segmentation::UnicodeSegmentation;
    let g: Vec<&str> = s.graphemes(true).collect();
    let c = cursor.min(g.len());
    format!("{}{}{}", g[..c].concat(), caret, g[c..].concat())
}

// Local-timezone log timestamps. tracing-subscriber's default timer is UTC;
// its own `LocalTime` (via the `time` crate) is unsound to call in a
// multi-threaded program on Unix (reads TZ/localtime data without the OS
// guaranteeing thread safety) and requires an explicit unsafe opt-in feature
// — not worth pulling in for a heavily-Tokio-threaded app like this one.
// `chrono::Local` (already a workspace dependency) doesn't have that
// restriction, so a small custom `FormatTime` impl is the simplest safe path.
struct LocalTimer;
impl tracing_subscriber::fmt::time::FormatTime for LocalTimer {
    fn format_time(&self, w: &mut tracing_subscriber::fmt::format::Writer<'_>) -> std::fmt::Result {
        write!(
            w,
            "{}",
            chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f%:z")
        )
    }
}

// ── close_login_screen ───────────────────────────────────────────────────────
// Real bug, code-review-confirmed 2026-08-22 (Bonfire Phase 3's on-screen
// keyboard): every one of LoginScreen's 5 real exit paths (auth.rs's two
// finish_session_setup success closures, profile.rs's open_profile_picker/
// open_account_picker/on_cancel_add_account) called `g.set_show_login(false)`
// directly with no guarantee the on-screen keyboard — if a mouse click had
// skipped its own Done key — was ever closed alongside it. keys.rs's
// show-onscreen-keyboard gate is checked before every other input tier, so a
// stray `true` surviving past LoginScreen permanently swallows almost all
// keyboard/remote input app-wide until sign-out or a profile switch. All 5
// sites already shared one line, so this is a genuine single choke point
// rather than 5 independent inline resets — the general fix this class of
// bug keeps needing, per this project's own "fix at the shared point when
// one naturally exists" precedent.
pub(crate) fn close_login_screen(g: &AppState) {
    g.set_show_login(false);
    g.set_show_onscreen_keyboard(false);
    g.set_onscreen_keyboard_target(ss(""));
    g.set_onscreen_keyboard_cursor(0);
}

/// How many previous sessions' logs to keep, on top of the current one
/// (2026-08-15, live-reported: "can we also make it so we save more than
/// one log" — a single `.old` generation genuinely wasn't enough during a
/// real multi-restart HTPC testing session that same day: content needed to
/// diagnose a live-reported bug got rotated away twice in a row before it
/// could be read, once each time the app was relaunched for an unrelated
/// test). 10 is a plain, generous-but-bounded round number — matches this
/// project's own original reasoning for rotating at all in the first place
/// (an unbounded file once reached 6.4 GB, Phase 62), just applied to N
/// generations instead of 1.
const LOG_GENERATIONS_KEPT: usize = 10;

/// Rotates `fjord.log` → `fjord.log.1` → `fjord.log.2` → ... → `fjord.log.N`
/// (deleted once past N) in `log_dir`, called once at the very top of every
/// launch, before anything is written to the new `fjord.log`. Generalizes
/// the original single `fjord.log` → `fjord.log.old` swap to N generations —
/// see `LOG_GENERATIONS_KEPT`'s own doc comment for why one generation
/// stopped being enough. Every step is best-effort (`let _ =`) — a rotation
/// failure (e.g. a stale generation the user has open in another program)
/// should never block startup; worst case is one generation not shifting
/// this run, not a crash.
/// Owner-only logs (2026-10-09 security review — they hold the server
/// address and user/device ids): folder 0700, `fjord.log` created 0600
/// before the appender opens it (appending keeps the mode), and every
/// rotated generation 0600. Best-effort — the HTPC's log folder is a link to
/// an NFS share — so failures are returned and logged once tracing is up.
fn restrict_log_permissions(log_dir: &std::path::Path, keep: usize) -> Vec<String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        let set = |path: &std::path::Path, mode: u32| {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))
                .err()
                .map(|e| format!("{}: {e}", path.display()))
        };
        let mut errors: Vec<String> = Vec::new();
        errors.extend(set(log_dir, 0o700));
        let current = log_dir.join("fjord.log");
        if let Err(e) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .mode(0o600)
            .open(&current)
        {
            errors.push(format!("{}: {e}", current.display()));
        }
        errors.extend(set(&current, 0o600));
        for n in 1..=keep {
            let rotated = log_dir.join(format!("fjord.log.{n}"));
            if rotated.exists() {
                errors.extend(set(&rotated, 0o600));
            }
        }
        errors
    }
    #[cfg(not(unix))]
    {
        let _ = (log_dir, keep);
        Vec::new()
    }
}

fn rotate_logs(log_dir: &std::path::Path, keep: usize) {
    let gen_path = |n: usize| log_dir.join(format!("fjord.log.{n}"));
    let _ = std::fs::remove_file(gen_path(keep));
    for n in (1..keep).rev() {
        let _ = std::fs::rename(gen_path(n), gen_path(n + 1));
    }
    let current = log_dir.join("fjord.log");
    if current.exists() {
        let _ = std::fs::rename(&current, gen_path(1));
    }
}

fn main() -> Result<()> {
    let cache_dir = std::env::var("XDG_CACHE_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache")
        })
        .join("fjord");
    // Logs move into their own logs/ subdirectory (2026-08-15, same
    // live-reported request as LOG_GENERATIONS_KEPT above — "move the logs
    // to .cache/fjord/logs") rather than sitting flat alongside posters/,
    // discover_posters/, profiles/, etc. One-time best-effort cleanup of the
    // old flat fjord.log/fjord.log.old (pre-move location) — not a real
    // migration (nothing in either is worth preserving once superseded by
    // the new rotation scheme), just avoiding two stale, orphaned copies
    // sitting around forever.
    let _ = std::fs::remove_file(cache_dir.join("fjord.log"));
    let _ = std::fs::remove_file(cache_dir.join("fjord.log.old"));
    let log_dir = cache_dir.join("logs");
    let _ = std::fs::create_dir_all(&log_dir);
    rotate_logs(&log_dir, LOG_GENERATIONS_KEPT);
    let log_permission_errors = restrict_log_permissions(&log_dir, LOG_GENERATIONS_KEPT);
    let log_path = log_dir.join("fjord.log");
    let file_appender = tracing_appender::rolling::never(&log_dir, "fjord.log");
    let (file_writer, _guard) = tracing_appender::non_blocking(file_appender);
    use tracing_subscriber::{EnvFilter, Layer, layer::SubscriberExt, util::SubscriberInitExt};
    // User's Settings→General log-level choice (default "info"), read directly from
    // disk before the subscriber exists — the full config load happens later once
    // `state`/`window` exist, this is just a cheap early peek. Applies on next
    // launch, not live. RUST_LOG still wins over this when set (dev override) —
    // the file used to grow without bound before Phase 62's per-launch rotation,
    // so a debug-level file is now bounded to one session's worth.
    let user_level = load_config()
        .map(|c| c.device.log_level)
        .unwrap_or_default();
    let level_str = match user_level.as_str() {
        "error" | "warn" | "debug" => user_level.as_str(),
        _ => "info",
    };
    let console_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "warn,fjord_app={level_str},fjord_player={level_str},fjord_api={level_str}"
        ))
    });
    let file_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "warn,fjord_app={level_str},fjord_player={level_str},fjord_api={level_str}"
        ))
    });
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_timer(LocalTimer)
                .with_filter(console_filter),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_timer(LocalTimer)
                .with_writer(file_writer)
                .with_filter(file_filter),
        )
        .init();
    info!("log file: {}", log_path.display());
    info!(
        "fjord version: {} ({})",
        env!("CARGO_PKG_VERSION"),
        env!("FJORD_BUILD_ID")
    );
    if !log_permission_errors.is_empty() {
        warn!(
            "log permissions not tightened: {}",
            log_permission_errors.join("; ")
        );
    }

    // Panic hook — writes directly to the log file so Slint "Recursion detected"
    // panics (which would otherwise SIGABRT silently) appear in fjord.log.
    let panic_log = log_dir.join("fjord.log");
    let default_hook = std::panic::take_hook();
    // 2026-10-08: timestamp + thread in the header, so a panic lines up with
    // the log lines around it (two "Recursion detected" panics on the HTPC
    // had neither).
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let when = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S%.6f%:z");
        let thread = std::thread::current()
            .name()
            .unwrap_or("unnamed")
            .to_string();
        let msg = format!("{when} PANIC (thread {thread}): {info}\nBacktrace:\n{bt}\n");
        eprintln!("{msg}");
        let mut opts = std::fs::OpenOptions::new();
        opts.create(true).append(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        if let Ok(mut f) = opts.open(&panic_log) {
            use std::io::Write;
            let _ = f.write_all(msg.as_bytes());
        }
        default_hook(info);
    }));

    let rt = tokio::runtime::Runtime::new()?;

    // event-loop branch (2026-09-08) — true global mouse-activity tap,
    // replacing the old best-effort AppState.record-activity() TouchArea
    // mechanism. Must run before MainWindow::new(): this is what actually
    // selects/builds the winit backend with our custom handler attached.
    // Self-contained (activity::ActivityClock has no dependency on
    // FjordState/VideoState, which aren't constructed until after this),
    // and best-effort itself, matching slint::set_xdg_app_id's own
    // established pattern a few lines below: on any failure,
    // i-slint-backend-selector's select_internal() (confirmed directly
    // against its own source) returns before ever calling
    // i_slint_core::platform::set_platform, so nothing has been partially
    // applied — Slint's own implicit lazy default-platform init runs
    // completely normally the moment MainWindow::new() needs one.
    let activity_clock = activity::ActivityClock::new();
    if let Err(e) = slint::BackendSelector::new()
        .with_winit_custom_application_handler(activity::FjordApplicationHandler {
            clock: activity_clock.clone(),
            hdr_handles_captured: false,
        })
        .select()
    {
        tracing::warn!(
            "couldn't install global mouse-activity tap, falling back to default backend init: {e}"
        );
    }

    let window = MainWindow::new()?;

    // Real bug, live-reported 2026-09-04 ("nothing showed up so i did
    // start it again" — a double-click on the desktop/taskbar launcher
    // spawned two full, independent processes, confirmed from the HTPC
    // log). Root cause, verified directly against Slint 1.16.1's and
    // winit 0.30.13's own vendored source: Fjord never set a Wayland/X11
    // app_id anywhere — `i-slint-backend-winit`'s own `ensure_window()`
    // only calls `window_attributes.with_name(...)` when
    // `WindowInner::xdg_app_id()` returns `Some` (there is NO fallback to
    // the executable's own basename), so without this call winit's
    // Wayland backend never calls `window.set_app_id(...)` at all. With
    // no app_id, KDE's Task Manager has nothing to match a running Fjord
    // window against `fjord.desktop` — clicking the launcher/taskbar icon
    // while Fjord is already running can never recognize that and just
    // runs `Exec=fjord` fresh every time, structurally, regardless of
    // timing. This is a materially better fix than a custom single-
    // instance guard (investigated and explicitly rejected, per direct
    // user decision — see this commit's own message/CLAUDE.md): a
    // background process can never truly force-focus a window on Wayland
    // (winit's own `focus_window()` is a literal no-op there), but a real
    // click on an existing KDE taskbar entry IS a legitimate, compositor-
    // mediated interaction, which genuinely can raise+focus a window —
    // this fix just lets that already-correct KDE mechanism actually see
    // Fjord's window at all. Must be set before the window is shown (per
    // `set_xdg_app_id`'s own doc comment) — `MainWindow::new()` alone
    // doesn't show it, only `window.run()` further down does, so this is
    // safely placed right after construction. `pkg/fjord.desktop` and
    // `pkg/fjord-x11.desktop` both declare `StartupWMClass=fjord` to
    // match this exact value regardless of which launcher started it.
    // Defensive: a failure here (shouldn't realistically happen on this
    // Linux-only target) must never be a reason Fjord fails to start.
    if let Err(e) = slint::set_xdg_app_id("fjord") {
        tracing::warn!("couldn't set Wayland/X11 app id: {e}");
    }

    let state = Arc::new(Mutex::new(FjordState::new()));
    let video = Arc::new(Mutex::new(VideoState::default()));

    // Fjord's own version — compile-time constants, no login/network round
    // trip needed, so this is set once here rather than alongside server-name/
    // server-version (which only become known after auth). CARGO_PKG_VERSION
    // (from Cargo.toml's workspace.package.version, bumped manually per
    // CHANGELOG.md release) for a human-readable sense of progress, plus
    // FJORD_BUILD_ID (build.rs, changes every commit) for exact-commit HTPC
    // log triage — see Cargo.toml's version comment for the full reasoning.
    AppState::get(&window).set_client_version(
        format!("{} ({})", env!("CARGO_PKG_VERSION"), env!("FJORD_BUILD_ID")).into(),
    );

    // Shared flag: show_controls() sets it lock-free; the mpv timer reads it
    // while already holding the video lock and resets controls_idle_ticks.
    // This avoids the UI thread blocking on the video mutex during mouse movement.
    let controls_show = Arc::new(AtomicBool::new(false));
    let seek_suppress = Arc::new(AtomicU32::new(0));

    wire_rendering_notifier(&window, Arc::clone(&video));
    let mpv_timer = wire_mpv_timer(
        window.as_weak(),
        Arc::clone(&video),
        Arc::clone(&state),
        rt.handle().clone(),
        Arc::clone(&controls_show),
        Arc::clone(&seek_suppress),
    );
    std::mem::forget(mpv_timer);

    let nw_timer = wire_nw_timer(
        window.as_weak(),
        Arc::clone(&video),
        Arc::clone(&state),
        rt.handle().clone(),
    );
    std::mem::forget(nw_timer);

    let screen_cache_save_timer =
        wire_screen_cache_save_timer(Arc::clone(&state), rt.handle().clone());
    std::mem::forget(screen_cache_save_timer);

    let prewarm_progress_timer = wire_prewarm_progress_timer(window.as_weak(), Arc::clone(&state));
    std::mem::forget(prewarm_progress_timer);

    // Bonfire Phase 4 (inactivity auto-lock, 2026-08-29).
    let idle_lock_timer = profile::wire_idle_lock_timer(
        window.as_weak(),
        Arc::clone(&state),
        Arc::clone(&video),
        rt.handle().clone(),
        activity_clock.clone(),
    );
    std::mem::forget(idle_lock_timer);

    // ── random logo index — pick from available icons at startup ─────────────
    {
        use std::time::{SystemTime, UNIX_EPOCH};
        const LOGOS: [i32; 6] = [1, 2, 4, 5, 9, 10];
        let n = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .subsec_nanos() as usize;
        AppState::get(&window).set_app_logo_idx(LOGOS[n % LOGOS.len()]);
    }

    // ── apply saved config ────────────────────────────────────────────────────
    if let Some(mut cfg) = load_config() {
        ensure_device_id(&mut cfg);
        // If IRQ scheduling is on AND the device is PipeWire (so the file should
        // exist) but the file is missing, sync down to false so the UI matches reality.
        // Skip this when a direct ALSA device is selected — the file is intentionally
        // absent then and alsa_irq_scheduling should be preserved for when the user
        // switches back to a PipeWire device.
        if cfg.device.audio_spdif
            && cfg.device.alsa_irq_scheduling
            && pipewire_fix::is_pipewire_device(if cfg.device.audio_device_passthrough.is_empty() {
                &cfg.device.audio_device
            } else {
                &cfg.device.audio_device_passthrough
            })
            && !pipewire_fix::wireplumber_config_exists()
        {
            cfg.device.alsa_irq_scheduling = false;
            save_config(&cfg);
        }
        {
            let mut s = state.lock().unwrap();
            s.seerr_client = seerr_auth::build_seerr_client(cfg.active());
            s.config = cfg;
        }
        {
            let s = state.lock().unwrap();
            if let Some(client) = s.seerr_client.clone() {
                if let Ok(base_url) = Url::parse(&s.config.active().seerr_url) {
                    seerr_auth::spawn_refresh_seerr_version(
                        base_url,
                        window.as_weak(),
                        rt.handle(),
                    );
                }
                spawn_seerr_settings_fetch(
                    client,
                    Arc::clone(&state),
                    window.as_weak(),
                    rt.handle().clone(),
                );
            }
        }
        // Also triggers the Home/Movies/TV dashboard Watchlist rows (2026-07-20)
        // — needs to fire at startup, not just on first Discover-tab arrival
        // (nav==6's own call), since Home is the very first screen shown after
        // login. Its own discover_watchlist_fetched guard makes calling it
        // redundantly alongside that nav==6 trigger safe. Deliberately called
        // AFTER the block above's `s` lock guard is dropped, not inside it —
        // ensure_discover_watchlist synchronously locks `state` itself before
        // spawning (to check discover_watchlist_fetched and clone the client),
        // and std::sync::Mutex isn't reentrant: calling it while `s` was still
        // held above self-deadlocked the whole app before window.run() was
        // ever reached (real bug, live-reported "fjord do not even start" —
        // the process hung forever with no window, confirmed via fjord.log
        // stopping right after the async-spawned seerr debug line, and via
        // /proc/<pid>/wchan showing futex_do_wait on every launch attempt).
        // It has its own internal seerr_client presence check, so it doesn't
        // need to be nested inside the `if let Some(client) = ...` above.
        discover::ensure_discover_watchlist(
            Arc::clone(&state),
            window.as_weak(),
            rt.handle().clone(),
        );
        apply_settings_to_window(&window, &state.lock().unwrap());

        // Bonfire Phase 1, step 6 (2026-08-09): with 2+ known profiles, the
        // launch policy decides whether to ask which one via the picker
        // instead of silently resuming — see should_show_picker_at_startup's
        // own doc comment. With 0 or 1 profile (every existing single-
        // profile install, today) this is unconditionally AutoLogin and the
        // auto-login flow below is byte-for-byte what it always was.
        // Returns a 3-way StartupGate, not a plain bool, since 2026-08-14 —
        // a PIN-protected "Remember Last"/"Default Profile" target now
        // needs its own picker-with-PIN-already-open path instead of either
        // silently skipping the PIN (the real bug this fixed) or falling
        // back to an untargeted full picker.
        let gate = {
            let mut s = state.lock().unwrap();
            profile::should_show_picker_at_startup(&mut s.config)
        };

        // Launch Fullscreen is device-scoped (DeviceConfig, not
        // ProfileSettings) — applies before either branch below, not just
        // inside the auto-login `else`. Real bug, live-reported 2026-08-14
        // ("it shuld also respect the fullscreen toggle in the config"):
        // this used to sit inside the `else` block only, so any install
        // that hits the picker (2+ profiles, launch policy Always Ask or
        // Remember Last with no valid resumable profile) launched windowed
        // regardless of the setting — the picker itself, and whichever
        // profile the user then picks, both need the window already
        // fullscreen by the time they show, not just the eventual dashboard.
        if state.lock().unwrap().config.device.launch_fullscreen {
            window.window().set_fullscreen(true);
        }

        match gate {
            profile::StartupGate::ShowAccountPicker => {
                profile::open_account_picker(&state, &window, false);
                // Real bug, live-reported 2026-09-01 — see
                // sync_all_known_accounts_in_background's own doc comment for
                // the full story: a change made entirely outside Fjord (here,
                // kicking an account via Jellyfin's own web UI) has no way to
                // reach the cold-start picker before this call, since no
                // authenticated session exists yet at this exact point. Fired
                // AFTER the picker opens (matches the sidebar's own "Switch
                // Profile"/"Switch Account" precedent) — instant open from
                // cached data, self-corrects a moment later if anything changed.
                profile::sync_all_known_accounts_in_background(&state, &window, rt.handle());
            }
            profile::StartupGate::ShowProfilePicker(account_root_id) => {
                profile::open_profile_picker(&state, &window, false, false, &account_root_id);
                profile::sync_all_known_accounts_in_background(&state, &window, rt.handle());
            }
            profile::StartupGate::ShowProfilePickerPin(account_root_id, target_user_id) => {
                profile::open_profile_picker_with_pin(
                    &state,
                    &window,
                    &account_root_id,
                    &target_user_id,
                );
                profile::sync_all_known_accounts_in_background(&state, &window, rt.handle());
            }
            profile::StartupGate::RequireLogin(server_url, username) => {
                let g = AppState::get(&window);
                g.set_login_server_prefill(ss(&server_url));
                g.set_login_username_prefill(ss(&username));
                g.set_login_append_mode(false);
                g.set_login_append_source(ss(""));
                // login-remember reflects this account's own already-known
                // false value (code review 2026-08-16, resolved via
                // AskUserQuestion) — a plain re-login without touching the
                // checkbox keeps remember_login=false, rather than silently
                // flipping it back to the default true. Mirrors
                // profile::require_login_for_account's identical fix for the
                // mid-session (picker-driven) RequireLogin path.
                g.set_login_remember(false);
                g.set_show_login(true);
                // Deferred (2026-08-15) — this arm runs synchronously, before
                // window.run() has started the event loop, same as the picker
                // arms right above it; see profile::grab_focus_deferred's own
                // doc comment for the full reasoning.
                profile::grab_focus_deferred(&window);
            }
            profile::StartupGate::AutoLogin => {
                let s = state.lock().unwrap();
                let server_url_str = s.config.active().server_url.clone();
                let user_id = s.config.active().user_id.clone();
                let token = s.config.active().token.clone();
                let device_id = s.config.device.device_id.clone();
                drop(s);

                if let Ok(server_url) = Url::parse(&server_url_str) {
                    let Ok(raw_client) =
                        JellyfinClient::new(server_url.clone(), user_id, token, device_id)
                    else {
                        tracing::error!("failed to build HTTP client — skipping auto-login");
                        return Ok(());
                    };
                    let client = Arc::new(raw_client);
                    state.lock().unwrap().client = Some(Arc::clone(&client));
                    set_server_url_ui(&AppState::get(&window), &server_url_str);

                    // Startup connectivity gate: show a plain connecting state instead
                    // of pushing cached content until the saved session is confirmed
                    // reachable — a full outage should be visibly different from
                    // normal quiet operation, not hidden behind a stale dashboard.
                    // show-login must be explicitly cleared here too — it defaults to
                    // true, and keys.rs's handle_key checks it before show-connecting/
                    // show-offline, so leaving it at the default would silently eat
                    // every key on both new screens (only the 401 branch sets it back
                    // to true).
                    {
                        let g = AppState::get(&window);
                        g.set_show_login(false);
                        g.set_show_connecting(true);
                    }

                    spawn_auto_login(
                        Arc::clone(&client),
                        Arc::clone(&state),
                        window.as_weak(),
                        rt.handle().clone(),
                    );

                    let client_retry = Arc::clone(&client);
                    let state_retry = Arc::clone(&state);
                    let ww_retry = window.as_weak();
                    let rt_retry = rt.handle().clone();
                    AppState::get(&window).on_retry_connection(move || {
                        if let Some(w) = ww_retry.upgrade() {
                            let g = AppState::get(&w);
                            g.set_show_offline(false);
                            g.set_show_login(false);
                            g.set_show_connecting(true);
                        }
                        spawn_auto_login(
                            Arc::clone(&client_retry),
                            Arc::clone(&state_retry),
                            ww_retry.clone(),
                            rt_retry.clone(),
                        );
                    });
                }
            }
        }
    }

    auth::wire_login(&window, &state, &rt);

    profile::wire_pickers(&window, &state, &video, &rt);

    profile_edit::wire_profile_edit(&window, &state, &rt);

    profile::wire_bonfire_group(&window, &state, &rt);

    bonfire_admin::wire_bonfire_admin(&window, &state, &rt);

    // ── filter / library search / nav ─────────────────────────────────────────
    browse::wire_browse(&window, Arc::clone(&state), rt.handle().clone());

    browse::wire_play_item(&window, &state, &video, &rt);

    home::wire_item_play(&window, &state, &video, &rt);

    movies::wire_library(&window, &state, &rt);

    detail::wire_detail(&window, &state, &rt);
    collection::wire_collection(&window, &state, &rt);
    artist::wire_artist(&window, &state, &video, &rt);
    album::wire_album(&window, &state, &video, &rt);
    music::wire_music_bar(&window, &state, &video, &rt);
    album::wire_album_play_all(&window, &state, &video, &rt);
    detail::wire_detail_play(&window, &state, &video, &rt);

    series::wire_series(&window, &state, &video, &rt);

    season::wire_season(&window, &state, &rt);

    person::wire_person(&window, &state, &rt);

    season::wire_season_toggles(&window, &state, &rt);

    controls::wire_up_next(&window, &state, &video, &rt);

    // ── player controls ───────────────────────────────────────────────────────
    controls::wire_controls(
        &window,
        Arc::clone(&video),
        Arc::clone(&state),
        Arc::clone(&controls_show),
        Arc::clone(&seek_suppress),
        rt.handle().clone(),
    );

    // ── context menu + queue ──────────────────────────────────────────────────
    context_menu::wire_context_menu(
        &window,
        Arc::clone(&state),
        Arc::clone(&video),
        rt.handle().clone(),
    );
    context_menu::wire_queue_callbacks(
        &window,
        Arc::clone(&state),
        Arc::clone(&video),
        rt.handle().clone(),
    );
    context_menu::wire_playlist_picker(&window, Arc::clone(&state), rt.handle().clone());

    // ── Seerr integration ──────────────────────────────────────────────────────
    seerr_auth::wire_connect_seerr(&window, Arc::clone(&state), rt.handle().clone());
    discover::wire_discover(&window, Arc::clone(&state), rt.handle().clone());

    music::wire_queue(&window, &state, &video, &rt);

    detail::wire_detail_toggles(&window, &state, &rt);
    series::wire_series_toggles(&window, &state, &rt);
    collection::wire_collection_toggles(&window, &state, &rt);

    blocklist::wire_blocklist(&window, &state, &rt);

    settings::wire_device_lists(&window, &state, &rt);

    display_sync::wire_display_sync(&window, &state, &rt);

    settings::wire_profile_defaults(&window, &state);

    settings::wire_regions(&window, &state, &rt);

    discover::wire_trailers(&window, &state, &video, &rt);

    settings::wire_settings_changed(&window, &state, &video, &rt);

    // ── fullscreen toggle ────────────────────────────────────────────────────
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_toggle_fullscreen(move || {
            if let Some(w) = window_weak.upgrade() {
                let fs = w.window().is_fullscreen();
                w.window().set_fullscreen(!fs);
            }
        });
    }

    auth::wire_sign_out(&window, &state, &video, &rt);

    AppState::get(&window).on_quit(|| {
        slint::quit_event_loop().ok();
    });

    prewarm::wire_prewarm(&window, &state, &rt);

    keys::wire_key_dispatch(&window, &state, &video, &rt, &activity_clock);

    keys::wire_keybindings(&window, &state);

    keys::push_keybinding_rows(&window, &state);

    // Re-grab keyboard focus after any mouse interaction steals it (e.g. ComboBox, CheckBox)
    {
        let ww = window.as_weak();
        AppState::get(&window).on_refocus(move || {
            if let Some(w) = ww.upgrade() {
                w.invoke_grab_keyboard_focus();
            }
        });
    }

    // On-screen alphanumeric keyboard (Bonfire Phase 3, 2026-08-22) — two
    // small, pure, generic string utilities Slint's own expression language
    // can't do on its own (no `.length`/`.substring()` on `string`, only
    // `.character-count()`/`.to-uppercase()`/`.to-lowercase()`/`.is-empty()`,
    // confirmed against the real Slint 1.16.1 compiler source). No screen or
    // field knowledge here — reusable by every future screen this keyboard
    // gets wired into, not just Login.
    AppState::get(&window).on_onscreen_keyboard_trim_last(
        |s: slint::SharedString| -> slint::SharedString { trim_last_grapheme(&s).into() },
    );
    AppState::get(&window).on_onscreen_keyboard_byte_len(|s: slint::SharedString| -> i32 {
        // Real UTF-8 byte length — LineEdit::set-selection-offsets operates
        // on byte offsets (confirmed against the core TextInput
        // implementation's own cursor_position_byte_offset tracking), not
        // Slint's own .character-count()'s Unicode-scalar count, which would
        // be wrong for any non-ASCII text.
        s.len() as i32
    });
    // Caret editing for the drawn text fields + on-screen-keyboard ◀ ▶
    // (2026-10-05) — see text_field.rs.
    text_field::wire(&window);

    window.invoke_grab_keyboard_focus();
    window.run()?;
    // Send stop report and release screensaver inhibitor if a video was playing when the user quit.
    quit_cleanup(&video, &rt, &state);
    Ok(())
}

#[cfg(all(test, unix))]
mod log_permission_tests {
    use super::restrict_log_permissions;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn logs_end_up_owner_only() {
        let dir = std::env::temp_dir().join(format!("fjord-logtest-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = dir.join("fjord.log.1");
        std::fs::write(&old, b"x").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(restrict_log_permissions(&dir, 10).is_empty());
        let mode = |p: &std::path::Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("fjord.log")), 0o600);
        assert_eq!(mode(&old), 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod settings_diff_tests {
    use super::settings_diff;
    use serde_json::json;

    #[test]
    fn names_changes_and_hides_text_values() {
        let before = [
            json!({"separate_video_surface": true, "cache_secs": 60, "hwdec": "auto"}),
            json!({"seerr_key": "a"}),
        ];
        let after = [
            json!({"separate_video_surface": false, "cache_secs": 60, "hwdec": "nvdec"}),
            json!({"seerr_key": "b"}),
        ];
        let d = settings_diff(&before, &after);
        // serde_json orders keys alphabetically.
        assert_eq!(
            d,
            vec![
                "hwdec (changed)",
                "separate_video_surface: true → false",
                "seerr_key (changed)"
            ]
        );
        assert!(settings_diff(&before, &before).is_empty());
    }
}

#[cfg(test)]
mod strip_html_tests {
    use super::strip_html_to_text;

    #[test]
    fn plain_text_passes_through_unchanged() {
        assert_eq!(
            strip_html_to_text("A regular overview, no markup."),
            "A regular overview, no markup."
        );
    }

    #[test]
    fn anime_provider_style_bio_is_stripped() {
        // Real shape from the live-reported bug (Daisuke Ono's bio) —
        // <p>/<strong>/<a href>/<ul><li> all present in one field.
        let raw = "<p><strong>Height:</strong> 174 cm (5'8.5\")</p>\
                   <p>Follow him on <a href=\"https://twitter.com/example\">Twitter</a>.</p>\
                   <p><strong>Non-Anime Roles</strong></p>\
                   <ul><li>Role One</li><li>Role Two</li></ul>";
        let out = strip_html_to_text(raw);
        assert!(!out.contains('<'), "no raw tags should remain: {out:?}");
        assert!(out.contains("Height: 174 cm (5'8.5\")"));
        assert!(out.contains("Follow him on Twitter."));
        assert!(out.contains("• Role One"));
        assert!(out.contains("• Role Two"));
    }

    #[test]
    fn br_becomes_newline_and_entities_decode() {
        let out = strip_html_to_text("Line one<br>Line two &amp; more &quot;quoted&quot;");
        assert_eq!(out, "Line one\nLine two & more \"quoted\"");
    }

    #[test]
    fn blank_lines_from_stripped_tags_collapse() {
        let out = strip_html_to_text("<p>First</p><p>Second</p>");
        assert_eq!(out, "First\nSecond");
    }
}

#[cfg(test)]
mod text_cursor_tests {
    use super::*;

    #[test]
    fn insert_moves_the_caret() {
        assert_eq!(insert_at_grapheme("helo", 3, "l"), ("hello".to_string(), 4));
        assert_eq!(insert_at_grapheme("", 0, "a"), ("a".to_string(), 1));
        assert_eq!(insert_at_grapheme("ab", 99, "c"), ("abc".to_string(), 3));
        // A combining accent merges with the letter before it.
        assert_eq!(
            insert_at_grapheme("cafe", 4, "\u{301}"),
            ("cafe\u{301}".to_string(), 4)
        );
    }

    #[test]
    fn backspace_and_delete_work_by_grapheme() {
        assert_eq!(
            delete_before_grapheme("hexllo", 3),
            ("hello".to_string(), 2)
        );
        assert_eq!(delete_before_grapheme("abc", 0), ("abc".to_string(), 0));
        assert_eq!(
            delete_before_grapheme("cafe\u{301}", 4),
            ("caf".to_string(), 3)
        );
        assert_eq!(delete_at_grapheme("hexllo", 2), "hello");
        assert_eq!(delete_at_grapheme("abc", 3), "abc");
        assert_eq!(grapheme_count("cafe\u{301}"), 4);
    }

    #[test]
    fn caret_is_drawn_at_the_cursor() {
        assert_eq!(with_caret("hello", 2, "▌"), "he▌llo");
        assert_eq!(with_caret("", 0, "▌"), "▌");
        assert_eq!(with_caret("ab", 9, "▌"), "ab▌");
    }
}
