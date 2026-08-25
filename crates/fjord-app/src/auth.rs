// ── fjord-app · auth.rs ──────────────────────────────────────────────────────
//   LoginOptions  { append, remember } — grouped to keep do_login's own arg count under
//             clippy's too_many_arguments threshold (2026-08-14); `remember` is written into
//             the just-authenticated ProfileSettings.remember_login in all 3 branches below
//   do_login  authenticate, persist config, then finish_session_setup; the authenticate()
//             HTTP client carries an explicit 30s timeout (previously a bare
//             reqwest::Client::new() with no timeout — the one call in the app that could
//             hang indefinitely against an unreachable server); backfills a blank
//             ProfileSettings.display_name from the login response's own auth.user.name
//             (2026-08-14 — the append-new/append-existing/normal branches all had this gap;
//             spawn_auto_login gets the equivalent backfill via a real GET /Users/{id} call,
//             since the token-resume path never sees a login response at all)
//   finish_session_setup  shared tail of every "we have a valid client, now make it the
//             active session" flow (Bonfire Phase 1, step 6, 2026-08-09) — fetch home
//             data/series/system info/plugins, persist cfg, update FjordState/AppState,
//             start WebSocket, spawn poster loading + movie-collections fetch, refresh
//             Settings → Profiles → Default Profile AND Default Account's dropdowns (step 7 +
//             2026-08-14). Both commit closures also close show-account-picker now (2026-08-14
//             fix — a switch initiated directly from the account tier, a single-profile
//             account's own tile, left it visibly stuck open otherwise). Reused verbatim
//             by profile.rs's switch_to_profile so the two flows can't drift. Warm-starts
//             from this user_id's own on-disk home/series cache (2026-08-14, "still takes
//             some time to login") before the blocking network join even starts, mirroring
//             spawn_auto_login's own push_cached_data — a repeat switch/login for a
//             previously-used profile can show real content almost immediately instead of
//             waiting the ~2.5s a real log showed get_all_series/next_up costing. Also now
//             warm-starts screen_caches.json the same way (2026-08-16, code review — this
//             file was previously never reloaded on a switch at all, only on plain
//             auto-login, so the periodic save timer could silently overwrite a real
//             persisted file with the empty set reset_session_state had just cleared).
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::JellyfinClient;
use slint::SharedString;
use tracing::{error, info, warn};
use url::Url;

use slint::Global;
use crate::AppState;
use crate::seerr_auth;
use crate::config::{FjordState, save_config, ensure_device_id, load_screen_caches};
use crate::home::{
    fetch_home_data, fetch_movie_collections, home_data_sections, load_home_cache, load_series_cache,
    push_home_data, push_home_data_preserving_posters, refresh_row_preserving_posters,
    save_home_cache, save_series_cache,
};
use crate::{apply_settings_to_window, items_to_model, ws};
use crate::poster::{spawn_poster_loading, spawn_series_poster_loading};
use crate::MainWindow;

fn ss(s: &str) -> SharedString { SharedString::from(s) }

/// Candidate server URLs to try, in order, from raw user-typed input.
/// Live-reported 2026-08-14: the LoginScreen's address field required an
/// explicit `http://`/`https://` prefix or authentication failed outright
/// with a raw URL-parse error — user asked for "jellyfin.example.com" to
/// just work, an explicit scheme (any case — "HTTPS://" included) to still
/// be respected as-is, and HTTPS to be preferred, falling back to HTTP.
/// If the trimmed input already starts with a scheme (checked
/// case-insensitively), that's the ONLY candidate — an explicit scheme is
/// never second-guessed or retried under a different one. Otherwise HTTPS
/// is tried first (matching how browsers and every other Jellyfin client
/// default an ambiguous address today), with a plain HTTP candidate as the
/// fallback right behind it.
///
/// `pub(crate)` since 2026-08-23 — `seerr_auth.rs::resolve_seerr_url` reuses
/// this verbatim (it has zero Jellyfin-specific typing) rather than
/// duplicating the same candidate-ordering logic for Seerr's own
/// server-URL field, which had the identical bare-host-fails-outright gap.
pub(crate) fn candidate_server_urls(input: &str) -> Vec<String> {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        vec![trimmed.to_string()]
    } else {
        vec![format!("https://{trimmed}"), format!("http://{trimmed}")]
    }
}

/// Tries each of `candidate_server_urls`'s candidates in order, moving on
/// to the next one ONLY when a candidate fails with a genuine connectivity
/// error (DNS failure, connection refused, TLS handshake failure, timeout —
/// anything that never got as far as a real HTTP response, tested via
/// `reqwest::Error::status().is_none()`). A candidate that reaches the
/// server and gets a real error back (401 wrong password, 500, etc.) is
/// treated as the final answer — retrying that same request under a
/// different scheme would never fix a wrong password, and would just
/// double the wait before showing the real error.
pub(crate) async fn authenticate_with_fallback(
    http:      &reqwest::Client,
    server:    &str,
    user:      &str,
    pass:      &str,
    device_id: &str,
) -> Result<(Url, fjord_api::models::AuthResponse)> {
    let candidates = candidate_server_urls(server);
    let mut last_err: Option<anyhow::Error> = None;
    for (i, candidate) in candidates.iter().enumerate() {
        let server_url = Url::parse(candidate)?;
        match fjord_api::authenticate(http, &server_url, user, pass, device_id).await {
            Ok(auth) => return Ok((server_url, auth)),
            Err(e) => {
                let is_connectivity = e.downcast_ref::<reqwest::Error>().is_some_and(|re| re.status().is_none());
                let is_last = i + 1 == candidates.len();
                if is_connectivity && !is_last {
                    info!("authenticate: {candidate} unreachable ({e:#}), trying next candidate");
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no server address given")))
}

/// The two "how to handle this login" flags, grouped (clippy's
/// too-many-arguments threshold, 2026-08-14 — `remember` was the 8th
/// parameter added to `do_login`) rather than left as loose scalars, same
/// "group loose scalars into a struct" precedent this codebase already
/// established for `context_menu.rs::OpenMenuArgs`.
pub(crate) struct LoginOptions {
    /// Bonfire Phase 1, step 6 (2026-08-09 — the picker's own "+ Add
    /// Account" tile) — keep every existing profile intact and add this
    /// one alongside them, rather than overwriting whichever profile is
    /// currently active.
    pub append:   bool,
    /// 2026-08-14, the account/profile redesign — persisted onto the
    /// resulting `ProfileSettings.remember_login`.
    pub remember: bool,
}

pub(crate) fn do_login(
    server:      String,
    user:        String,
    pass:        String,
    opts:        LoginOptions,
    state:       Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    let LoginOptions { append, remember } = opts;
    if let Some(w) = window_weak.upgrade() { AppState::get(&w).set_status(ss("Connecting…")); }

    let rt_handle_sp = rt_handle.clone();
    rt_handle.spawn(async move {
        let rt_handle = rt_handle_sp;
        let result: Result<()> = async {
            // Clone existing config so player/app settings survive sign-out + re-login.
            // Only auth fields are overwritten below.
            let mut cfg = state.lock().unwrap().config.clone();
            ensure_device_id(&mut cfg);
            // Matches JellyfinClient's own timeout — this call previously used a
            // bare default reqwest::Client (no timeout at all), the one place in
            // the app a black-holed connection could hang indefinitely.
            let login_http = reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()?;
            // Live-reported 2026-08-14: typing a bare host ("jellyfin.example.com",
            // no scheme) failed outright with a raw URL-parse error — every other
            // Jellyfin client and every browser treats a schemeless address as
            // "try to figure it out," not "reject it." See
            // authenticate_with_fallback's own doc comment for the exact rule.
            let (server_url, auth) = authenticate_with_fallback(
                &login_http, &server, &user, &pass, &cfg.device.device_id,
            ).await?;
            info!("authenticated as {}", auth.user.name);
            // `append` (Bonfire Phase 1, step 6, 2026-08-09 — the picker's own
            // "+ Add Account" tile) means "keep every existing profile intact,
            // add this one alongside them" rather than the normal sign-in
            // behavior of overwriting whichever profile is currently active
            // (correct for "sign back into the same slot after sign-out", wrong
            // for genuinely adding a second account). If this exact user_id is
            // already known locally (re-authenticating a profile whose stored
            // token had gone stale), update that entry in place instead of
            // creating a duplicate.
            if append {
                if let Some(p) = cfg.profiles.iter_mut().find(|p| p.user_id == auth.user.id) {
                    p.server_url = server_url.to_string();
                    p.token      = auth.access_token.clone();
                    // Real bug fix, 2026-08-14, live-reported ("on an old login
                    // the profilename is just random letters and numbers instead
                    // of the profile name"): a blank display_name (every
                    // pre-Bonfire migrated profile, and any profile that's only
                    // ever gone through auto-login) fell back to the raw
                    // user_id GUID in build_tile — backfill it here from the
                    // real Jellyfin username while we have it, same as the
                    // brand-new-entry branch below already does via
                    // ..Default::default() + this explicit set.
                    if p.display_name.is_empty() { p.display_name = auth.user.name.clone(); }
                    // 2026-08-14, the account/profile redesign — "remember
                    // this login" is a per-attempt choice, so a re-login
                    // (this branch: the profile was already known, e.g. its
                    // token had gone stale) always takes whatever the
                    // checkbox says on THIS attempt, not whatever it was
                    // set to originally.
                    p.remember_login = remember;
                } else {
                    cfg.profiles.push(crate::config::ProfileSettings {
                        server_url: server_url.to_string(),
                        user_id:    auth.user.id.clone(),
                        token:      auth.access_token.clone(),
                        display_name: auth.user.name.clone(),
                        remember_login: remember,
                        ..Default::default()
                    });
                }
            } else {
                let p = cfg.active_mut();
                p.server_url = server_url.to_string();
                p.user_id    = auth.user.id.clone();
                p.token      = auth.access_token.clone();
                if p.display_name.is_empty() { p.display_name = auth.user.name.clone(); }
                p.remember_login = remember;
            }
            // active_profile_id doubles as the active profile's own user_id
            // (Config::active()'s lookup key) — keep it in sync with the
            // identity just written above so it still names a real entry.
            cfg.active_profile_id = auth.user.id.clone();
            let user_id = cfg.active_profile_id.clone();
            save_config(&cfg);

            let client = Arc::new(JellyfinClient::new(
                server_url.clone(), auth.user.id, auth.access_token.clone(), cfg.device.device_id.clone(),
            )?);

            finish_session_setup(client, cfg, user_id, server_url, state, window_weak.clone(), rt_handle).await;
            Ok(())
        }.await;

        if let Err(e) = result {
            error!("login failed: {:#}", e);
            let msg = format!("{:#}", e);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = window_weak.upgrade() { AppState::get(&w).set_status(ss(&msg)); }
            });
        }
    });
}

// ── finish_session_setup ─────────────────────────────────────────────────────
// Shared tail of every "we already have a valid client for this profile, now
// make it the active session and show its content" flow — extracted from
// do_login (Bonfire Phase 1, step 6, 2026-08-09) so profile.rs's own
// switch_to_profile can reuse it verbatim instead of re-deriving the same
// ~10-field setup and risking drift, the same reasoning reset_session_state
// was extracted for. Fetches home data/series/system info/plugins in
// parallel, persists cfg (already fully mutated by the caller — this fn
// only reads it), updates FjordState + AppState, starts the WebSocket,
// spawns poster loading + the movie-collections fetch.
//
// Callers differ only in how `client` was obtained (a fresh password
// sign-in here; a Bonfire-minted or already-stored token in profile.rs) and
// in what `cfg` looks like going in (do_login mutates cfg.active_mut()
// in place; a profile switch finds-or-creates a different profiles[] entry
// instead) — everything from here on is identical either way.
pub(crate) async fn finish_session_setup(
    client:      Arc<JellyfinClient>,
    cfg:         crate::config::Config,
    user_id:     String,
    server_url:  Url,
    state:       Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    // Real bug, live-reported 2026-08-14 with a screenshot ("the settings
    // etc do not seams to be diffferent for different profiles" — Seerr
    // connection/Streaming Region/Discover Region all showing the SAME
    // values after switching to a different profile): `apply_settings_to_
    // window` — the ONLY function that pushes any profile-scoped setting
    // (subtitle/audio language, skip modes, Seerr enabled/connection status/
    // trailer quality, library sort, ...) into AppState at all — was called
    // exactly once in the whole app, at startup, using whichever profile
    // happened to be active on disk at that moment. Neither this function
    // nor switch_to_profile ever called it again, so every one of those
    // settings silently kept reflecting the ORIGINAL profile for the rest
    // of the running session after ANY switch (picker or sidebar) — not
    // just a Settings-screen display bug: subtitle language, skip-mode
    // behavior, and which Seerr account's credentials get used for Discover
    // API calls were all genuinely wrong post-switch, not just displayed
    // wrong. `s.config`/`s.client` are hoisted up here (both already fully
    // known — `cfg`/`client` are plain parameters, not something the join
    // below produces) specifically so BOTH commit closures below (the warm-
    // start one and the post-join one) can call apply_settings_to_window
    // and have it read the correct, already-current profile immediately —
    // the original code only wrote these AFTER the network join completed.
    //
    // `s.seerr_client` had the IDENTICAL gap, one level more severe: it was
    // ONLY ever built once, at app startup (main.rs's own config-load
    // block) — do_login/finish_session_setup/switch_to_profile never
    // rebuilt it at all. push_seerr_status's "connected" check is purely
    // Config-flag-derived (never checks seerr_client itself), so the
    // Settings screen could show a fully plausible "Connected" state for
    // the NEW profile while every actual Discover/Watchlist/etc. API call
    // kept silently using the OLD profile's live Seerr session underneath
    // — a real cross-profile data-isolation gap, not just a display one.
    // Rebuilt the same way startup does (seerr_auth::build_seerr_client),
    // plus the region/language dropdown lists + admin-permission flags
    // (spawn_seerr_settings_fetch) and the version string
    // (spawn_refresh_seerr_version) — both of those were ALSO startup-only
    // before this, mirroring main.rs's own config-load block exactly so
    // there's one canonical "bring Seerr up for whichever profile is now
    // active" sequence instead of two independently-drifting copies of it.
    let (seerr_client, seerr_url, cfg_early) = {
        let mut s = state.lock().unwrap();
        s.config = cfg;
        s.client = Some(Arc::clone(&client));
        s.seerr_client = seerr_auth::build_seerr_client(s.config.active());
        (s.seerr_client.clone(), s.config.active().seerr_url.clone(), s.config.clone())
    };
    // Real gap, live-reported with a video (2026-08-21) — "the profile that
    // is highlighted is blank for several seconds after you have switched
    // profile." The sidebar's own Profile row (avatar/name) is driven by
    // push_current_profile_tile, but it was only ever called from this
    // function's FINAL commit closure, after the whole outer tokio::join!
    // (fetch_home_data/get_all_series/get_system_info/get_plugins, ~2.4s+
    // in a real log) completes. reset_session_state (run just before this
    // function, tearing down the OUTGOING session) already resets
    // current-profile-tile back to a blank Default — so the sidebar showed
    // nothing for the entire fetch window, even though everything this
    // needs (avatar color/initial, display name) comes straight from the
    // Config just set two lines above and needs no network round trip at
    // all. Pushed here instead, immediately — mirroring the same "hoist
    // early so the very first paint already has it" reasoning this
    // function's own doc comment already gives for s.config/s.client
    // themselves.
    {
        let ww_early = window_weak.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_early.upgrade() {
                crate::profile::push_current_profile_tile(&AppState::get(&w), &cfg_early);
            }
        });
    }
    if let Some(sc) = seerr_client {
        if let Ok(base_url) = Url::parse(&seerr_url) {
            seerr_auth::spawn_refresh_seerr_version(base_url, window_weak.clone(), &rt_handle);
        }
        crate::spawn_seerr_settings_fetch(sc, Arc::clone(&state), window_weak.clone(), rt_handle.clone());
    }

    // Warm start (2026-08-14, direct follow-up during a live HTPC test:
    // "still takes some time to 'login'" — from a real Bonfire switch, not
    // the general auto-login case the earlier "Login speed" fix already
    // covered). A real log showed the timing instrumentation's own numbers:
    // finish_session_setup's ~2.5s total was almost entirely get_all_series
    // (2.3s) and fetch_home_data's next_up branch (2.0s) — both genuinely
    // needed for a correct refresh, but a profile switch (or a repeat
    // Add-Account login) very plausibly already has its own on-disk home/
    // series cache from a prior session under this exact user_id
    // (namespaced per-profile since Phase 1 step 2) — this function simply
    // never checked. spawn_auto_login already warm-starts from exactly this
    // cache (push_cached_data) before its own background network refresh;
    // mirrored here for the two fields actually on THIS function's blocking
    // path (Movies/Collections/Artists/Albums/Playlists aren't, so they
    // aren't warm-started here either). `warm_started` gates the final
    // commit closure below between a plain first-time build (no earlier
    // paint to preserve, and — critically — items_to_model's own correct
    // watchlist-star lookup, which the "preserving" variant deliberately
    // skips in favor of carrying an OLD row's value forward) and the
    // preserving-posters variant once there IS an earlier paint worth not
    // flashing over.
    // Real bug, code-review 2026-08-16 ("60s cache-save timer overwrites
    // the wrong profile's cache"): a switch/re-login into a profile that's
    // been used on this device before previously never reloaded ITS OWN
    // screen_caches.json at all — only spawn_auto_login did, for the
    // ordinary "resume the same session" path. reset_session_state (run
    // just before this function, as part of tearing down the OUTGOING
    // session) had already cleared all six in-memory caches to empty, so
    // the very next tick of the periodic 60s save timer silently
    // overwrote this profile's real, possibly prewarm-sized persisted
    // file with an almost-empty one. Mirrors spawn_auto_login's own warm-
    // start of this exact file — spawn_blocking, since it can reach
    // ~1.3MB after a library prewarm and a plain synchronous read here
    // would block whatever thread is running this async fn for however
    // long that takes.
    let user_id_sc = user_id.clone();
    if let Some(file) = tokio::task::spawn_blocking(move || load_screen_caches(&user_id_sc)).await.ok().flatten() {
        let mut s = state.lock().unwrap();
        s.item_detail_cache        = file.item_detail;
        s.similar_items_cache      = file.similar_items;
        s.boxset_items_cache       = file.boxset_items;
        s.artist_albums_cache      = file.artist_albums;
        s.person_filmography_cache = file.person_filmography;
        s.container_tracks_cache   = file.container_tracks;
        s.person_tmdb_id_cache     = file.person_tmdb_id;
    }

    let watchlist_warm = state.lock().unwrap().jellyfin_watchlist_ids.clone();
    let cached_home   = load_home_cache(&user_id);
    let cached_series = load_series_cache(&user_id);
    let warm_started = cached_home.is_some() || cached_series.is_some();
    if warm_started {
        if let Some(hd) = &cached_home {
            let sections = home_data_sections(hd);
            spawn_poster_loading(Arc::clone(&client), sections, window_weak.clone(), rt_handle.clone(), Arc::clone(&state));
        }
        if let Some(series) = &cached_series {
            spawn_series_poster_loading(Arc::clone(&client), series.clone(), window_weak.clone(), rt_handle.clone(), Arc::clone(&state));
            state.lock().unwrap().all_series = series.clone();
        }
        // HomeData has no Clone derive — move the originals into the closure
        // directly instead (warm_started, captured above, is all the outer
        // scope needs afterward; neither is read again out here).
        let server_str_warm  = server_url.to_string();
        let ww_warm    = window_weak.clone();
        let state_warm = Arc::clone(&state);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_warm.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_server_url(ss(&server_str_warm));
            if let Some(hd) = &cached_home { push_home_data(&w, hd, &watchlist_warm); }
            if let Some(series) = &cached_series { g.set_all_series(items_to_model(series, &watchlist_warm)); }
            // Real bug fix, 2026-08-14 — see this function's own top-of-body
            // comment. state.config was already hoisted to the new profile
            // above, so this correctly reflects it from the very first paint.
            apply_settings_to_window(&w, &state_warm.lock().unwrap());
            crate::close_login_screen(&g);
            g.set_show_profile_picker(false);
            g.set_show_account_picker(false);
            w.invoke_grab_keyboard_focus();
        });
    }

    // Timed (2026-08-14, "what makes login slow") — fetch_home_data is
    // itself a join, timed the same way internally; this outer join is the
    // true top-level breakdown of finish_session_setup's own cost. That
    // instrumentation found a real, fixable cost: a real login logged
    // `not_watched_tv took 4.103s` against every other branch finishing in
    // 1-2.7s (see fetch_home_data's own doc comment for why — Jellyfin's
    // own SortBy=Random + recursive per-episode unplayed check, not a
    // client bug). include_not_watched=false here — those two rows aren't
    // even shown on the Home dashboard this screen is about to display, so
    // there's no reason login should wait on them; spawn_not_watched_rows
    // below fetches and patches them in separately, without blocking this
    // join at all.
    let (home_data, series_res, sysinfo_res, plugins_res) = tokio::join!(
        crate::timed("fetch_home_data (all rows)", fetch_home_data(&client, false)),
        crate::timed("get_all_series",              client.get_all_series()),
        crate::timed("get_system_info",             client.get_system_info()),
        crate::timed("get_plugins",                 client.get_plugins()),
    );

    let series = series_res.unwrap_or_else(|e| { warn!("get_all_series: {:#}", e); vec![] });
    info!("loaded {} series", series.len());
    let (srv_name, srv_ver) = sysinfo_res
        .map(|i| (i.server_name, i.version))
        .unwrap_or_else(|e| { warn!("get_system_info: {:#}", e); (String::new(), String::new()) });
    let plugins: std::collections::HashSet<String> = plugins_res
        .unwrap_or_else(|e| { warn!("get_plugins: {:#}", e); vec![] })
        .into_iter().map(|p| p.name).collect();
    {
        let mut s = state.lock().unwrap();
        // config/client already set at the top of this function (see the
        // real-bug comment there) — only what the join itself produced is
        // left to write here.
        s.available_plugins = plugins;
        s.all_series = series.clone();
    }

    // Bonfire Phase 1, step 6 (2026-08-09): best-effort, always attempted —
    // get_plugins()/bonfire_list_profiles() both already degrade gracefully
    // when the plugin isn't installed, so this costs nothing extra for the
    // overwhelming majority of servers that don't have it.
    crate::profile::sync_bonfire_subprofiles(Arc::clone(&client), Arc::clone(&state), rt_handle.clone(), window_weak.clone());

    // Real bug, live-reported 2026-08-21 ("if i close fjord it still shows
    // the old cache before it reloads from the server") — this function
    // saved the fresh series list right below but never the fresh HOME
    // data (Continue Watching/Next Up/Recently Added/Favorites — the bulk
    // of what a session actually shows) at all; save_home_cache wasn't
    // even in this file's own imports. Only spawn_auto_login's own tail
    // (main.rs) ever wrote home.json — the ordinary cold "resume an
    // already-saved session" path. Every session that goes through THIS
    // function instead — a fresh login, an Add-Account login, or (per the
    // live-reported HTPC log that surfaced this) any Bonfire profile
    // switch — fetched and displayed fresh home data correctly for the
    // live session, then silently never wrote it back to disk, so the
    // NEXT launch's warm-start kept reading whatever home.json last held
    // from a genuine cold auto-login, however old that was.
    save_home_cache(&user_id, &home_data);
    save_series_cache(&user_id, &series);
    let sections        = home_data_sections(&home_data);
    let series2         = series.clone();
    let server_str      = server_url.to_string();
    let ww              = window_weak.clone();
    let ww_poster       = window_weak.clone();
    let ww_series       = window_weak.clone();
    let rt_handle_inner = rt_handle.clone();
    // Fresh session — no prior CardItem rows for these to carry an existing
    // on_watchlist forward from, so the persisted set has to be read
    // explicitly here (2026-07-20, see FjordState.jellyfin_watchlist_ids'
    // own doc comment). cfg_snapshot is read in the same lock, purely for
    // push_current_profile_tile below (apply_settings_to_window, called a
    // few lines down, reads straight from `state_late` instead and no
    // longer needs a snapshot passed in — see this function's own top-of-
    // body comment for why it's called from here at all now).
    let (watchlist, cfg_snapshot) = {
        let s = state.lock().unwrap();
        (s.jellyfin_watchlist_ids.clone(), s.config.clone())
    };
    let state_late = Arc::clone(&state);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            let g = AppState::get(&w);
            g.set_server_url(ss(&server_str));
            g.set_server_name(ss(&srv_name));
            g.set_server_version(ss(&srv_ver));
            // warm_started: an earlier paint from cache already happened
            // above (before this join even started) — preserve it (posters
            // + whatever on_watchlist it already carried) rather than
            // flashing every row blank again. No earlier paint (a genuine
            // first-time login/switch for this user_id) → plain fresh
            // build, which is also the one that does a real watchlist-star
            // lookup (the preserving variant deliberately doesn't — see
            // this function's own warm-start comment above).
            if warm_started {
                push_home_data_preserving_posters(&w, &home_data);
                g.set_all_series(refresh_row_preserving_posters(&g.get_all_series(), &series2));
            } else {
                push_home_data(&w, &home_data, &watchlist);
                g.set_all_series(items_to_model(&series2, &watchlist));
            }
            crate::close_login_screen(&g);
            g.set_show_profile_picker(false);
            g.set_show_account_picker(false);
            g.set_status(ss(""));
            // Real bug fix, 2026-08-14 — see this function's own top-of-body
            // comment. apply_settings_to_window already calls
            // refresh_profile_settings_dropdown + sets settings-is-master-
            // profile internally, so those two former standalone calls are
            // folded into it rather than left as duplicate logic; it needs
            // the live FjordState (not just the Config snapshot) for
            // audio_devices/system_fonts display-string lookups.
            apply_settings_to_window(&w, &state_late.lock().unwrap());
            // Sidebar profile row (2026-08-14) — same trigger point: every
            // session start or switch needs this repushed, not just the
            // very first login.
            crate::profile::push_current_profile_tile(&g, &cfg_snapshot);
            w.invoke_grab_keyboard_focus();
        }
    });
    let client2      = Arc::clone(&client);
    let client3      = Arc::clone(&client);
    let client4      = Arc::clone(&client);
    let client5      = Arc::clone(&client);
    let state_coll   = state.clone();
    let state_ws     = state.clone();
    let ws_abort = ws::start_websocket(client4, Arc::clone(&state_ws), window_weak.clone(), rt_handle_inner.clone());
    state_ws.lock().unwrap().ws_abort = Some(ws_abort);
    spawn_poster_loading(client, sections, ww_poster, rt_handle_inner.clone(), Arc::clone(&state));
    spawn_series_poster_loading(client2, series, ww_series, rt_handle_inner.clone(), Arc::clone(&state));
    rt_handle_inner.spawn(async move {
        let map = fetch_movie_collections(&client3).await;
        state_coll.lock().unwrap().movie_collections = map;
    });
    spawn_not_watched_rows(client5, Arc::clone(&state), window_weak, &rt_handle_inner);
}

/// The two rows `fetch_home_data(..., include_not_watched: false)` skipped
/// above (2026-08-14, "what makes login slow" — see that call site's own
/// doc comment for the real timing evidence). Fetched here instead, fully
/// independent of the login-blocking join, and patched in whenever they're
/// ready — same "fire in the background, patch in later" shape as
/// `spawn_poster_loading`/`spawn_series_poster_loading`/the
/// movie-collections fetch right above. A session-guard on the resolved
/// `AppState.person-id`-style pattern isn't needed here (this isn't a
/// per-screen open), but the same class of risk — a stale result landing
/// after a sign-out/switch — is still real, so `crate::session_current` is
/// checked before writing anything, matching every other background patch
/// in this codebase.
fn spawn_not_watched_rows(
    client:      Arc<JellyfinClient>,
    state:       Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   &tokio::runtime::Handle,
) {
    rt_handle.spawn(async move {
        let (nwm, nwt) = tokio::join!(
            crate::timed("not_watched_movies (deferred)", client.get_unwatched(Some("Movie"))),
            crate::timed("not_watched_tv (deferred)",      client.get_unwatched(Some("Series"))),
        );
        let not_watched_movies = nwm.unwrap_or_else(|e| { warn!("not_watched_movies: {:#}", e); vec![] });
        let not_watched_tv     = nwt.unwrap_or_else(|e| { warn!("not_watched_tv: {:#}", e);     vec![] });
        let watchlist = state.lock().unwrap().jellyfin_watchlist_ids.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = window_weak.upgrade() else { return };
            if !crate::session_current(&state, &client) { return; }
            let g = AppState::get(&w);
            g.set_not_watched_movies(items_to_model(&not_watched_movies, &watchlist));
            g.set_not_watched_tv(items_to_model(&not_watched_tv, &watchlist));
        });
    });
}

#[cfg(test)]
mod tests {
    use super::candidate_server_urls;

    #[test]
    fn bare_host_tries_https_then_http() {
        assert_eq!(
            candidate_server_urls("jellyfin.example.com"),
            vec!["https://jellyfin.example.com", "http://jellyfin.example.com"],
        );
    }

    #[test]
    fn bare_host_with_port_tries_https_then_http() {
        assert_eq!(
            candidate_server_urls("192.168.1.10:8096"),
            vec!["https://192.168.1.10:8096", "http://192.168.1.10:8096"],
        );
    }

    #[test]
    fn explicit_https_is_not_second_guessed() {
        assert_eq!(candidate_server_urls("https://jellyfin.example.com"), vec!["https://jellyfin.example.com"]);
    }

    #[test]
    fn explicit_http_is_not_second_guessed() {
        assert_eq!(candidate_server_urls("http://jellyfin.example.com"), vec!["http://jellyfin.example.com"]);
    }

    #[test]
    fn explicit_scheme_is_case_insensitive() {
        assert_eq!(candidate_server_urls("HTTPS://jellyfin.example.com"), vec!["HTTPS://jellyfin.example.com"]);
        assert_eq!(candidate_server_urls("HTTP://jellyfin.example.com"), vec!["HTTP://jellyfin.example.com"]);
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(
            candidate_server_urls("  jellyfin.example.com  "),
            vec!["https://jellyfin.example.com", "http://jellyfin.example.com"],
        );
    }
}
