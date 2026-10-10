// ── fjord-app · main.rs ──────────────────────────────────────────────────────
//   error helpers        is_unauthorized, is_not_found, is_rate_limited
//   session guards       session_current / seerr_session_current (Arc::ptr_eq against the live
//                        client — async results that land after a profile switch are dropped)
//   should_revalidate    once-per-60 s (REVALIDATE_COOLDOWN) per item id for the screens'
//                        "revalidate on cache hit" fetches
//   purge_deleted_item   drops an item the server no longer has from every cache and model
//   show_toast           toast from any thread
//   strip_html_to_text   server HTML (overviews) → plain text (unit-tested)
//   model helpers        item_to_card_item / items_to_model (watchlist set consulted at
//                        construction), apply_cards_preserving_identity (patch rows in place when
//                        ids/order match — no delegate rebuild, no poster flash),
//                        push_section_model / get_section_model (HomeSection), to_slint_model,
//                        display_names, timed (logs `timing:` lines)
//   text helpers         trim_last_grapheme; grapheme_count / insert_at_grapheme /
//                        delete_before_grapheme / delete_at_grapheme / with_caret — caret editing by
//                        grapheme cluster for the hand-drawn text fields (unit-tested)
//   set_server_url_ui    server-url + server-unencrypted together
//   close_login_screen   the one close path for the login screen (also closes the on-screen keyboard)
//   logging              LocalTimer; LOG_GENERATIONS_KEPT, rotate_logs, restrict_log_permissions
//                        (folder 0700, files 0600, best-effort; unit-tested)
//   main                 logging setup + panic hook (writes PANIC + backtrace to fjord.log), backend
//                        with the activity tap (activity.rs), app_id, periodic timers, apply saved
//                        config + the startup gate (profile picker or auto-login), then one wire_*
//                        call per area — the callbacks live in that area's module; helpers that
//                        moved out of main.rs are re-exported below the `mod` list
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

/// Bonfire's developer-api.md: `/plugins/profiles/switch` and `/verify-pin` allow 5 failed
/// attempts per 15 minutes (anti-brute-force on the PIN) — only the status tells it from
/// a wrong PIN (400 = wrong PIN/bad request, 429 = rate-limited). Lets switch_to_profile
/// show a readable message instead of the raw 429 text (also for the idle-lock unlock).
pub(crate) fn is_rate_limited(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .and_then(|e| e.status())
        .map(|s| s.as_u16() == 429)
        .unwrap_or(false)
}

/// True if `client` is still the session's live client (Arc::ptr_eq). Long background
/// sweeps write per-user data (MediaItems carry UserData) into shared FjordState caches;
/// after a sign-out or a different account signing in, the old sweep's results must not
/// land in the new session. Like ws.rs's guard. Used by spawn_screen_cache_refresh and
/// prewarm::spawn_metadata_prewarm.
pub(crate) fn session_current(state: &Mutex<FjordState>, client: &Arc<JellyfinClient>) -> bool {
    state
        .lock()
        .unwrap()
        .client
        .as_ref()
        .is_some_and(|c| Arc::ptr_eq(c, client))
}

/// The Seerr twin of `session_current`: Discover's long fetches hold an
/// `Arc<SeerrClient>`, compared against `FjordState.seerr_client`. reset_session_state
/// clears it on sign-out and profile switch (another Jellyfin user can have a different
/// Seerr connection, or none).
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

// Rate-limits the 7 screens' "revalidate on cache hit" (spawn_*_revalidate in collection/
// detail/series/season/artist/person/album.rs): without it, going back and forth between
// a few recently viewed items re-fired the full detail + list + poster fetch every time.
// Jellyfin ids are unique GUIDs, so one shared map is enough.
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

/// Strips HTML out of a metadata field, turning paragraphs/list items/line breaks into
/// plain-text equivalents. Some metadata providers (anime ones especially) return
/// Overview/bio text as real HTML that Jellyfin passes through. A plain linear scan —
/// the tags seen in the wild (p, br, strong, em, a, ul/li, div, span) are few; no
/// regex/html crate in the workspace.
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

/// `watchlist` = the LOCAL Jellyfin ids on the Seerr watchlist
/// (`FjordState.jellyfin_watchlist_ids`), read at construction time — a live patch alone
/// would be wiped by the next rebuild.
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

/// Apply `fresh` cards to `old`'s model: with the same ids in the same order, mutate the
/// EXISTING rows via set_row_data instead of returning a new ModelRc — a new model makes
/// Slint recreate every delegate (each poster Image), a visible flash. Only a real
/// membership/order change builds a new model. Used by every place that pushes a fresh
/// Vec<CardItem> (poster.rs, movies.rs, home.rs, context_menu.rs's WS upserts, …).
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

/// Wraps a future with `debug!` timing (`timing: <label> took …`): the session setup
/// joins are fully parallel, so what makes them slow is the single slowest request —
/// this names it in the log.
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

/// Drop the last Unicode GRAPHEME CLUSTER from `s`, not the last `char`: a char-based
/// trim leaves a dangling combining mark (NFD "café") or half a flag emoji.
/// unicode-segmentation's UAX #29 boundaries handle both. Used by the on-screen keyboard
/// and the drawn fields' backspace.
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

// ── Text cursor for the hand-drawn search fields ────────────────────────────
// `cursor` counts grapheme clusters before the caret (the unit trim_last_grapheme uses,
// so an accented letter or a flag is one step); out-of-range cursors clamp to the end.
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
// The one way to close LoginScreen (auth.rs's success paths, profile.rs's pickers and
// cancel-add-account): it also closes the on-screen keyboard — a stuck
// show-onscreen-keyboard would swallow input app-wide (keys.rs checks it first).
pub(crate) fn close_login_screen(g: &AppState) {
    g.set_show_login(false);
    g.set_show_onscreen_keyboard(false);
    g.set_onscreen_keyboard_target(ss(""));
    g.set_onscreen_keyboard_cursor(0);
}

/// How many previous sessions' logs to keep besides the current one — one generation
/// wasn't enough during multi-restart testing (the log needed got rotated away).
/// Bounded, because an unrotated log once reached 6.4 GB.
const LOG_GENERATIONS_KEPT: usize = 10;

/// Owner-only logs (they hold the server address and user/device ids): folder 0700,
/// `fjord.log` created 0600 before the appender opens it (appending keeps the mode),
/// every rotated generation 0600. Best-effort — the HTPC's log folder is a link to an
/// NFS share — so failures are returned and logged once tracing is up.
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

/// Rotates `fjord.log` → `fjord.log.1` → … → `fjord.log.N` (deleted past N), once at the
/// start of every launch, before anything is written. Best-effort (`let _ =`): a
/// rotation failure (a file open elsewhere) must never block startup.
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
    // Logs live in logs/ (fjord.log + rotated generations); remove the old flat
    // fjord.log / fjord.log.old once (nothing worth keeping).
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
    // Settings → General log level (default "info"), read straight from disk before the
    // subscriber exists (the full config load comes later). Applies on the next launch;
    // RUST_LOG overrides it.
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

    // Global mouse/keyboard activity tap for the idle lock (activity.rs) — must run before
    // MainWindow::new(): it selects the winit backend with our handler attached.
    // Self-contained and best-effort: on failure the backend selector never sets a platform
    // (checked in its source), so Slint's normal lazy default applies.
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

    // Set the Wayland/X11 app_id: Slint's winit backend only names the window when an xdg
    // app_id is set (no fallback to the binary name), and without it KDE's task manager
    // can't match the window to fjord.desktop — so clicking the launcher while Fjord runs
    // started a second process instead of raising the window. (A single-instance guard was
    // considered and rejected: a background process can't focus a window on Wayland, but
    // a taskbar click can.) Must happen before the window is shown (window.run() below);
    // the .desktop files declare StartupWMClass=fjord. A failure must never stop startup.
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
        // Also fills the Home/Movies/TV Watchlist rows, so it runs at startup, not only on the
        // first Discover visit (its own fetched-once guard makes the double call safe). Called
        // AFTER the `s` guard above is dropped: it locks `state` itself, and std::sync::Mutex
        // isn't reentrant — calling it under `s` hung startup before any window appeared.
        discover::ensure_discover_watchlist(
            Arc::clone(&state),
            window.as_weak(),
            rt.handle().clone(),
        );
        apply_settings_to_window(&window, &state.lock().unwrap());

        // The startup gate (profile::should_show_picker_at_startup): with 0–1 profiles always
        // AutoLogin (the plain auto-login below); otherwise the launch policies may ask via a
        // picker, open the PIN pad for a remembered/default profile, or require a password.
        let gate = {
            let mut s = state.lock().unwrap();
            profile::should_show_picker_at_startup(&mut s.config)
        };

        // Launch Fullscreen is device-scoped and applies before every branch below — the
        // pickers (and the profile picked there) need the window fullscreen already.
        if state.lock().unwrap().config.device.launch_fullscreen {
            window.window().set_fullscreen(true);
        }

        match gate {
            profile::StartupGate::ShowAccountPicker => {
                profile::open_account_picker(&state, &window, false);
                // Refresh every known account's Bonfire data in the background while the cold-start
                // picker shows from cache (changes made elsewhere, e.g. a kick via Jellyfin's web UI,
                // correct themselves a moment later) — see sync_all_known_accounts_in_background.
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
                // "Remember this login" starts as this account's stored value (off), so a plain re-login
                // doesn't turn it back on — like profile::require_login_for_account.
                g.set_login_remember(false);
                g.set_show_login(true);
                // Deferred: this runs before window.run() starts the event loop (see
                // profile::grab_focus_deferred).
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

    // On-screen keyboard string helpers Slint can't do itself (strings have no
    // length/substring) — screen-agnostic.
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
