// ── fjord-app · auth.rs ──────────────────────────────────────────────────────
//   candidate_server_urls / authenticate_with_fallback / is_connectivity_failure  a typed address
//             without a scheme tries https, then http (only on a real connectivity failure)
//   fell_back_to_http / note_if_http_fallback  a schemeless address that only answered over http →
//             one log line + toast (Jellyfin login, Seerr connects; unit-tested)
//   LoginOptions  { append, remember }
//   do_login  authenticate (30 s timeout), update or add the profile (append = Add Account; a direct
//             login resets Bonfire-discovery fields; display_name backfilled), persist, then
//             finish_session_setup
//   finish_session_setup  shared tail of login and profile switch: profile settings + Seerr client for
//             the new profile, sidebar tile, warm start from this user's cached home/series/screen
//             caches, then home data/series/system info/plugins in parallel; persists caches, starts
//             the WebSocket, posters, movie collections, Bonfire sync; closes the pickers
//   spawn_not_watched_rows  the slow "Not watched" rows, off the login path (session-guarded)
//   wire_login             callbacks moved from main() (0.5.0 step 3): the login screen
//   wire_sign_out          callbacks moved from main() (0.5.0 step 3): Sign Out — removes the account and
//             its Bonfire sub-profiles/group accounts, logs their server sessions out, keeps
//             device_id and settings, then the account picker (if any account is left) or Login
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::JellyfinClient;
use slint::SharedString;
use tracing::{error, info, warn};
use url::Url;

use crate::AppState;
use crate::MainWindow;
use crate::config::{FjordState, ensure_device_id, load_screen_caches, save_config};
use crate::home::{
    fetch_home_data, fetch_movie_collections, home_data_sections, load_home_cache,
    load_series_cache, push_home_data, push_home_data_preserving_posters,
    refresh_row_preserving_posters, save_home_cache, save_series_cache,
};
use crate::poster::{spawn_poster_loading, spawn_series_poster_loading};
use crate::seerr_auth;
use crate::{apply_settings_to_window, items_to_model, ws};
use slint::Global;

fn ss(s: &str) -> SharedString {
    SharedString::from(s)
}

/// True when `typed` had no scheme and the address that answered is plain
/// `http://` — https didn't answer and Fjord fell back (the only way a
/// schemeless address ends up on http, see candidate_server_urls).
pub(crate) fn fell_back_to_http(typed: &str, resolved: &Url) -> bool {
    let typed = typed.trim().to_ascii_lowercase();
    resolved.scheme() == "http" && !typed.starts_with("http://") && !typed.starts_with("https://")
}

/// No silent plain HTTP (2026-10-09 security review): one toast + a log
/// line when `what` ("Jellyfin"/"Seerr") was reached only over http://
/// after https didn't answer. Settings shows "not encrypted" permanently.
pub(crate) fn note_if_http_fallback(
    ww: &slint::Weak<crate::MainWindow>,
    what: &str,
    typed: &str,
    resolved: &Url,
) {
    if fell_back_to_http(typed, resolved) {
        warn!(
            "{what}: {typed} didn't answer over https — connected over unencrypted http ({resolved})"
        );
        crate::show_toast(
            ww.clone(),
            format!("{what}: connected without encryption — the server didn't answer over https"),
        );
    }
}

/// Candidate server URLs to try, in order, from user-typed input: an explicit scheme
/// (any case) is the ONLY candidate — never retried under another; otherwise https
/// first, then plain http (like browsers and other Jellyfin clients). Also used by
/// seerr_auth::resolve_seerr_url.
pub(crate) fn candidate_server_urls(input: &str) -> Vec<String> {
    let trimmed = input.trim();
    let lower = trimmed.to_ascii_lowercase();
    if lower.starts_with("http://") || lower.starts_with("https://") {
        vec![trimmed.to_string()]
    } else {
        vec![format!("https://{trimmed}"), format!("http://{trimmed}")]
    }
}

/// Whether `e` is a real connectivity failure (DNS, refused, TLS handshake, timeout)
/// — no response at all — rather than a response that failed to parse or had an error
/// status. Only `is_connect()`/`is_timeout()`: a 2xx with a non-JSON body (captive
/// portal, another service on the port, an SSO page) also has `status() == None`, and
/// treating it as "unreachable" would fall back to plaintext HTTP — password included —
/// instead of showing the real error. Shared with seerr_auth::resolve_seerr_url.
pub(crate) fn is_connectivity_failure(e: &anyhow::Error) -> bool {
    e.downcast_ref::<reqwest::Error>()
        .is_some_and(|re| re.is_connect() || re.is_timeout())
}

/// Tries each of `candidate_server_urls`'s candidates in order, moving on
/// to the next one ONLY when a candidate fails with a genuine connectivity
/// error (see `is_connectivity_failure`). A candidate that reaches the
/// server and gets a real error back (401 wrong password, 500, etc.) is
/// treated as the final answer — retrying that same request under a
/// different scheme would never fix a wrong password, and would just
/// double the wait before showing the real error.
pub(crate) async fn authenticate_with_fallback(
    http: &reqwest::Client,
    server: &str,
    user: &str,
    pass: &str,
    device_id: &str,
) -> Result<(Url, fjord_api::models::AuthResponse)> {
    let candidates = candidate_server_urls(server);
    let mut last_err: Option<anyhow::Error> = None;
    for (i, candidate) in candidates.iter().enumerate() {
        let server_url = Url::parse(candidate)?;
        match fjord_api::authenticate(http, &server_url, user, pass, device_id).await {
            Ok(auth) => return Ok((server_url, auth)),
            Err(e) => {
                let is_connectivity = is_connectivity_failure(&e);
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

/// The two "how to handle this login" flags, grouped instead of more loose parameters
/// (clippy's too-many-arguments; like context_menu::OpenMenuArgs).
pub(crate) struct LoginOptions {
    /// "+ Add Account": keep every existing profile and add this one, instead of
    /// replacing the active profile.
    pub append: bool,
    /// 2026-08-14, the account/profile redesign — persisted onto the
    /// resulting `ProfileSettings.remember_login`.
    pub remember: bool,
}

pub(crate) fn do_login(
    server: String,
    user: String,
    pass: String,
    opts: LoginOptions,
    state: Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    let LoginOptions { append, remember } = opts;
    if let Some(w) = window_weak.upgrade() {
        AppState::get(&w).set_status(ss("Connecting…"));
    }

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
            // A bare host (no scheme) works: https first, then http (authenticate_with_fallback).
            let (server_url, auth) = authenticate_with_fallback(
                &login_http,
                &server,
                &user,
                &pass,
                &cfg.device.device_id,
            )
            .await?;
            info!("authenticated as {}", auth.user.name);
            note_if_http_fallback(&window_weak, "Jellyfin", &server, &server_url);
            // `append`: keep every profile and add this one alongside (a normal sign-in replaces
            // the active slot — right after a sign-out, wrong for adding a second account). An
            // already-known user_id (re-authenticating a stale token) is updated in place, not
            // duplicated.
            if append {
                if let Some(p) = cfg.profiles.iter_mut().find(|p| p.user_id == auth.user.id) {
                    p.server_url = server_url.to_string();
                    p.token = auth.access_token.clone();
                    // Backfill an empty display_name from the real Jellyfin username (old migrated
                    // profiles had none, and the tile then showed the raw user_id).
                    if p.display_name.is_empty() {
                        p.display_name = auth.user.name.clone();
                    }
                    // "Remember this login" is a per-attempt choice: a re-login takes the checkbox as it
                    // is now.
                    p.remember_login = remember;
                    // A successful direct username/password login proves independent access: reset the
                    // Bonfire-discovery fields (is_bonfire/is_group_account/master_user_id) so this account
                    // switches directly again — re-adding an account is the recovery for a Bonfire
                    // misclassification and must actually restore direct access.
                    p.is_bonfire = false;
                    p.is_group_account = false;
                    p.master_user_id.clear();
                    p.synced_via.clear();
                    // `has_pin` is cached from Bonfire's /list; a plain independent account has no PIN, so
                    // a stale true must not keep demanding one.
                    p.has_pin = false;
                } else {
                    cfg.profiles.push(crate::config::ProfileSettings {
                        server_url: server_url.to_string(),
                        user_id: auth.user.id.clone(),
                        token: auth.access_token.clone(),
                        display_name: auth.user.name.clone(),
                        remember_login: remember,
                        ..Default::default()
                    });
                }
            } else {
                let p = cfg.active_mut();
                p.server_url = server_url.to_string();
                p.user_id = auth.user.id.clone();
                p.token = auth.access_token.clone();
                if p.display_name.is_empty() {
                    p.display_name = auth.user.name.clone();
                }
                p.remember_login = remember;
                // Same fix as the append-existing-entry branch above, for
                // the identical reason — a plain (non-append) sign-in can
                // also land on an already-known, Bonfire-discovery-tainted
                // entry (e.g. cfg.active_mut()'s own fallback-to-first-entry
                // path, or a RequireLogin re-prompt against one), and a real
                // successful direct login is equally proof of independent
                // access here.
                p.is_bonfire = false;
                p.is_group_account = false;
                p.master_user_id.clear();
                p.synced_via.clear();
                p.has_pin = false;
            }
            // active_profile_id doubles as the active profile's own user_id
            // (Config::active()'s lookup key) — keep it in sync with the
            // identity just written above so it still names a real entry.
            cfg.active_profile_id = auth.user.id.clone();
            let user_id = cfg.active_profile_id.clone();
            save_config(&cfg);

            let client = Arc::new(JellyfinClient::new(
                server_url.clone(),
                auth.user.id,
                auth.access_token.clone(),
                cfg.device.device_id.clone(),
            )?);

            finish_session_setup(
                client,
                cfg,
                user_id,
                server_url,
                state,
                window_weak.clone(),
                rt_handle,
            )
            .await;
            Ok(())
        }
        .await;

        if let Err(e) = result {
            error!("login failed: {:#}", e);
            let msg = format!("{:#}", e);
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = window_weak.upgrade() {
                    AppState::get(&w).set_status(ss(&msg));
                }
            });
        }
    });
}

// ── finish_session_setup ─────────────────────────────────────────────────────
// Shared tail of every "we have a valid client for this profile — make it the active
// session and show it" flow: do_login and profile::switch_to_profile (the latter with
// a Bonfire-minted or stored token). Fetches home data/series/system info/plugins in
// parallel, persists `cfg` (already mutated by the caller — only read here), updates
// FjordState + AppState, starts the WebSocket, spawns poster loading and the
// movie-collections fetch.
pub(crate) async fn finish_session_setup(
    client: Arc<JellyfinClient>,
    cfg: crate::config::Config,
    user_id: String,
    server_url: Url,
    state: Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    // Set `s.config`/`s.client` before the network join so both commit closures below
    // can call apply_settings_to_window for the NEW profile — it's the only function that
    // pushes profile-scoped settings (languages, skip modes, Seerr, library sort…) into
    // AppState, and a switch must not keep the previous profile's.
    // Rebuild the Seerr client too (seerr_auth::build_seerr_client), with the region/
    // language lists + permissions (spawn_seerr_settings_fetch) and the version — the same
    // sequence as startup — or Discover calls would keep using the previous profile's
    // Seerr session while Settings showed the new one as connected.
    let (seerr_client, seerr_url, cfg_early) = {
        let mut s = state.lock().unwrap();
        s.config = cfg;
        s.client = Some(Arc::clone(&client));
        s.seerr_client = seerr_auth::build_seerr_client(s.config.active());
        (
            s.seerr_client.clone(),
            s.config.active().seerr_url.clone(),
            s.config.clone(),
        )
    };
    // Push the sidebar's profile tile now — everything it needs comes from the Config
    // set above; waiting for the join (~2.4 s) left the row blank after a switch
    // (reset_session_state had already cleared it).
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
        crate::spawn_seerr_settings_fetch(
            sc,
            Arc::clone(&state),
            window_weak.clone(),
            rt_handle.clone(),
        );
    }
    // Bonfire Phase 6 (2026-09-04) — Jellyfin-specific, not Seerr-specific,
    // so unconditional regardless of whether Seerr is even connected.
    crate::spawn_jellyfin_admin_check(
        Arc::clone(&client),
        Arc::clone(&state),
        window_weak.clone(),
        rt_handle.clone(),
    );

    // Warm start: a profile used before on this device has its own home/series cache
    // (per user_id) — paint from it now, like spawn_auto_login's push_cached_data, while
    // the network refresh runs (home and series are what blocks this function).
    // `warm_started` picks the final commit: a plain first-time build (with the correct
    // watchlist-star lookup) or the poster-preserving update over the earlier paint.
    // Also reload this profile's screen_caches.json (spawn_blocking — it can be ~1.3 MB
    // after a prewarm): reset_session_state just emptied the in-memory caches, and the
    // 60 s save timer would otherwise overwrite the file with almost nothing.
    let user_id_sc = user_id.clone();
    if let Some(file) = tokio::task::spawn_blocking(move || load_screen_caches(&user_id_sc))
        .await
        .ok()
        .flatten()
    {
        let mut s = state.lock().unwrap();
        s.item_detail_cache = file.item_detail;
        s.similar_items_cache = file.similar_items;
        s.boxset_items_cache = file.boxset_items;
        s.artist_albums_cache = file.artist_albums;
        s.person_filmography_cache = file.person_filmography;
        s.container_tracks_cache = file.container_tracks;
        s.person_tmdb_id_cache = file.person_tmdb_id;
    }

    let watchlist_warm = state.lock().unwrap().jellyfin_watchlist_ids.clone();
    let cached_home = load_home_cache(&user_id);
    let cached_series = load_series_cache(&user_id);
    let warm_started = cached_home.is_some() || cached_series.is_some();
    if warm_started {
        if let Some(hd) = &cached_home {
            let sections = home_data_sections(hd);
            spawn_poster_loading(
                Arc::clone(&client),
                sections,
                window_weak.clone(),
                rt_handle.clone(),
                Arc::clone(&state),
            );
        }
        if let Some(series) = &cached_series {
            spawn_series_poster_loading(
                Arc::clone(&client),
                series.clone(),
                window_weak.clone(),
                rt_handle.clone(),
                Arc::clone(&state),
            );
            state.lock().unwrap().all_series = series.clone();
        }
        // HomeData has no Clone derive — move the originals into the closure
        // directly instead (warm_started, captured above, is all the outer
        // scope needs afterward; neither is read again out here).
        let server_str_warm = server_url.to_string();
        let ww_warm = window_weak.clone();
        let state_warm = Arc::clone(&state);
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_warm.upgrade() else { return };
            let g = AppState::get(&w);
            crate::set_server_url_ui(&g, &server_str_warm);
            if let Some(hd) = &cached_home {
                push_home_data(&w, hd, &watchlist_warm);
            }
            if let Some(series) = &cached_series {
                g.set_all_series(items_to_model(series, &watchlist_warm));
            }
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

    // Timed (the `timing:` lines). include_not_watched=false: the "Not watched" rows are
    // slow (Jellyfin's SortBy=Random + recursive unplayed check, seconds) and not shown on
    // Home, so login doesn't wait for them — spawn_not_watched_rows patches them in later.
    let (home_data, series_res, sysinfo_res, plugins_res) = tokio::join!(
        crate::timed(
            "fetch_home_data (all rows)",
            fetch_home_data(&client, false)
        ),
        crate::timed("get_all_series", client.get_all_series()),
        crate::timed("get_system_info", client.get_system_info()),
        crate::timed("get_plugins", client.get_plugins()),
    );

    let series = series_res.unwrap_or_else(|e| {
        warn!("get_all_series: {:#}", e);
        vec![]
    });
    info!("loaded {} series", series.len());
    let (srv_name, srv_ver) = sysinfo_res
        .map(|i| (i.server_name, i.version))
        .unwrap_or_else(|e| {
            warn!("get_system_info: {:#}", e);
            (String::new(), String::new())
        });
    let plugins: std::collections::HashSet<String> = plugins_res
        .unwrap_or_else(|e| {
            warn!("get_plugins: {:#}", e);
            vec![]
        })
        .into_iter()
        .map(|p| p.name)
        .collect();
    {
        let mut s = state.lock().unwrap();
        // config/client already set at the top of this function (see the
        // real-bug comment there) — only what the join itself produced is
        // left to write here.
        s.available_plugins = plugins;
        s.all_series = series.clone();
    }

    // Bonfire sync: always attempted; get_plugins()/bonfire_list_profiles() degrade
    // gracefully without the plugin.
    crate::profile::sync_bonfire_subprofiles(
        Arc::clone(&client),
        Arc::clone(&state),
        rt_handle.clone(),
        window_weak.clone(),
    );

    // Save the fresh home data too (save_home_cache), or the next launch's warm start would
    // show whatever home.json the last cold auto-login wrote.
    save_home_cache(&user_id, &home_data);
    save_series_cache(&user_id, &series);
    let sections = home_data_sections(&home_data);
    let series2 = series.clone();
    let server_str = server_url.to_string();
    let ww = window_weak.clone();
    let ww_poster = window_weak.clone();
    let ww_series = window_weak.clone();
    let rt_handle_inner = rt_handle.clone();
    // Fresh session: no earlier rows to carry on_watchlist from, so read the persisted set
    // (FjordState.jellyfin_watchlist_ids). cfg_snapshot is for push_current_profile_tile.
    let (watchlist, cfg_snapshot) = {
        let s = state.lock().unwrap();
        (s.jellyfin_watchlist_ids.clone(), s.config.clone())
    };
    let state_late = Arc::clone(&state);
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            let g = AppState::get(&w);
            crate::set_server_url_ui(&g, &server_str);
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
                g.set_all_series(refresh_row_preserving_posters(
                    &g.get_all_series(),
                    &series2,
                ));
            } else {
                push_home_data(&w, &home_data, &watchlist);
                g.set_all_series(items_to_model(&series2, &watchlist));
            }
            crate::close_login_screen(&g);
            g.set_show_profile_picker(false);
            g.set_show_account_picker(false);
            g.set_status(ss(""));
            // Also refreshes the default-profile dropdown and settings-is-master-profile; it reads
            // the live FjordState for the audio-device/font display strings.
            apply_settings_to_window(&w, &state_late.lock().unwrap());
            // Sidebar profile row (2026-08-14) — same trigger point: every
            // session start or switch needs this repushed, not just the
            // very first login.
            crate::profile::push_current_profile_tile(&g, &cfg_snapshot);
            w.invoke_grab_keyboard_focus();
        }
    });
    let client2 = Arc::clone(&client);
    let client3 = Arc::clone(&client);
    let client4 = Arc::clone(&client);
    let client5 = Arc::clone(&client);
    let state_coll = state.clone();
    let state_ws = state.clone();
    let ws_abort = ws::start_websocket(
        client4,
        Arc::clone(&state_ws),
        window_weak.clone(),
        rt_handle_inner.clone(),
    );
    state_ws.lock().unwrap().ws_abort = Some(ws_abort);
    spawn_poster_loading(
        client,
        sections,
        ww_poster,
        rt_handle_inner.clone(),
        Arc::clone(&state),
    );
    spawn_series_poster_loading(
        client2,
        series,
        ww_series,
        rt_handle_inner.clone(),
        Arc::clone(&state),
    );
    rt_handle_inner.spawn(async move {
        let map = fetch_movie_collections(&client3).await;
        state_coll.lock().unwrap().movie_collections = map;
    });
    spawn_not_watched_rows(client5, Arc::clone(&state), window_weak, &rt_handle_inner);
}

/// The two rows `fetch_home_data(…, include_not_watched: false)` skipped above, fetched
/// off the login path and patched in when ready (like the poster loaders). Checks
/// `crate::session_current` first — a result must not land after a sign-out/switch.
fn spawn_not_watched_rows(
    client: Arc<JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
) {
    rt_handle.spawn(async move {
        let (nwm, nwt) = tokio::join!(
            crate::timed(
                "not_watched_movies (deferred)",
                client.get_unwatched(Some("Movie"))
            ),
            crate::timed(
                "not_watched_tv (deferred)",
                client.get_unwatched(Some("Series"))
            ),
        );
        let not_watched_movies = nwm.unwrap_or_else(|e| {
            warn!("not_watched_movies: {:#}", e);
            vec![]
        });
        let not_watched_tv = nwt.unwrap_or_else(|e| {
            warn!("not_watched_tv: {:#}", e);
            vec![]
        });
        let watchlist = state.lock().unwrap().jellyfin_watchlist_ids.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = window_weak.upgrade() else {
                return;
            };
            if !crate::session_current(&state, &client) {
                return;
            }
            let g = AppState::get(&w);
            g.set_not_watched_movies(items_to_model(&not_watched_movies, &watchlist));
            g.set_not_watched_tv(items_to_model(&not_watched_tv, &watchlist));
        });
    });
}

// ── wire_login (moved from main(), 0.5.0 step 3) ─────────────────────────
/// Wires the login screen: do_login.
pub(crate) fn wire_login(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── login ─────────────────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_do_login(move |server, user, pass, append, remember| {
            auth::do_login(
                server.to_string(),
                user.to_string(),
                pass.to_string(),
                auth::LoginOptions { append, remember },
                Arc::clone(&state),
                window_weak.clone(),
                rt_handle.clone(),
            );
        });
    }
}

// ── wire_sign_out (moved from main(), 0.5.0 step 3) ──────────────────────
/// Wires Sign Out: sign_out.
pub(crate) fn wire_sign_out(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    video: &std::sync::Arc<std::sync::Mutex<crate::playback::VideoState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    let video = std::sync::Arc::clone(video);
    // ── sign-out ──────────────────────────────────────────────────────────────
    {
        let state = Arc::clone(&state);
        let video_so = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rth_so = rt.handle().clone();
        AppState::get(&window).on_sign_out(move || {
            reset_session_state(&video_so, &window_weak, &rth_so, &state);

            let mut s = state.lock().unwrap();
            // Clear only the session — not config.json: deleting it also wiped device_id, so the
            // next login got a new DeviceId and Jellyfin invalidated the other machine's token.
            // Settings survive sign-out.
            // The signed-out account's Config.profiles entry is REMOVED, with every Bonfire
            // sub-profile it owns (blanking it in place broke grouping and every sub-profile
            // switch); sync_bonfire_subprofiles re-adds them on the next login as this master.
            let signed_out_user_id = s.config.active().user_id.clone();
            let forgotten = |p: &crate::config::ProfileSettings| {
                // Also match `synced_via`: a group account (discovered through this account's group)
                // has an empty `master_user_id` and would otherwise stay orphaned, with no local
                // account left to authenticate a switch into it.
                p.user_id == signed_out_user_id
                    || p.master_user_id == signed_out_user_id
                    || p.synced_via == signed_out_user_id
            };
            // Every login forgotten here also ends on the server, in the
            // background after the save (2026-10-09 security review: their
            // tokens used to stay valid indefinitely).
            let device_id = s.config.device.device_id.clone();
            let to_log_out: Vec<(String, String, String)> = s
                .config
                .profiles
                .iter()
                .filter(|p| {
                    !signed_out_user_id.is_empty()
                        && forgotten(p)
                        && !p.token.is_empty()
                        && !p.server_url.is_empty()
                })
                .map(|p| (p.server_url.clone(), p.user_id.clone(), p.token.clone()))
                .collect();
            s.config.profiles.retain(|p| !forgotten(p));
            if s.config.profiles.is_empty() {
                // Config.profiles is never empty — a genuine, enforced invariant
                // (see Config::active()/active_mut()'s own doc comments) — signing
                // out of the only known profile needs a fresh blank entry to keep
                // that invariant true, not leave the Vec empty.
                s.config
                    .profiles
                    .push(crate::config::ProfileSettings::default());
            }
            s.config.active_profile_id.clear();
            let cfg_to_save = s.config.clone();
            // With at least one account left, land on the account picker (pick it — no password
            // if its token is valid — or "+ Add Account"), not a bare Login. Counted after removing
            // the signed-out account, so signing out of the only one falls through to Login.
            // Unlike the cold-start gate, one remaining account is enough: it's a DIFFERENT
            // account from the one just removed.
            let any_accounts_remain =
                !profile::group_into_accounts(&cfg_to_save.profiles).is_empty();
            drop(s);
            save_config(&cfg_to_save);
            for (server_url, user_id, token) in to_log_out {
                let device_id = device_id.clone();
                rth_so.spawn(async move {
                    let client = url::Url::parse(&server_url)
                        .map_err(anyhow::Error::from)
                        .and_then(|url| {
                            JellyfinClient::new(url, user_id.clone(), token, device_id)
                        });
                    match client {
                        Ok(c) => match c.logout().await {
                            Ok(()) => info!("sign-out: ended the server session of {user_id}"),
                            Err(e) => warn!(
                                "sign-out: couldn't end the server session of {user_id}: {e:#}"
                            ),
                        },
                        Err(e) => warn!("sign-out: no client for {user_id}: {e:#}"),
                    }
                });
            }
            if let Some(w) = window_weak.upgrade() {
                let g = AppState::get(&w);
                g.set_show_connecting(false);
                g.set_show_offline(false);
                g.set_active_nav(0);
                set_server_url_ui(&g, "");
                g.set_server_name(ss(""));
                g.set_server_version(ss(""));
                g.set_settings_section(ss(""));
                g.set_settings_focused(ss(""));
                if any_accounts_remain {
                    profile::open_account_picker(&state, &w, false);
                } else {
                    // Fresh Login, not a RequireLogin re-prompt for an
                    // already-known account — always defaults checked,
                    // same reasoning as the Add-Account entry points.
                    g.set_login_remember(true);
                    g.set_show_login(true);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::{candidate_server_urls, fell_back_to_http};
    use url::Url;

    #[test]
    fn http_fallback_is_noticed() {
        let http = Url::parse("http://jellyfin.example.com").unwrap();
        let https = Url::parse("https://jellyfin.example.com").unwrap();
        assert!(fell_back_to_http("jellyfin.example.com", &http));
        assert!(fell_back_to_http(" Jellyfin.Example.com:8096 ", &http));
        assert!(!fell_back_to_http("http://jellyfin.example.com", &http)); // typed http on purpose
        assert!(!fell_back_to_http("HTTP://jellyfin.example.com", &http));
        assert!(!fell_back_to_http("jellyfin.example.com", &https));
    }

    #[test]
    fn bare_host_tries_https_then_http() {
        assert_eq!(
            candidate_server_urls("jellyfin.example.com"),
            vec![
                "https://jellyfin.example.com",
                "http://jellyfin.example.com"
            ],
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
        assert_eq!(
            candidate_server_urls("https://jellyfin.example.com"),
            vec!["https://jellyfin.example.com"]
        );
    }

    #[test]
    fn explicit_http_is_not_second_guessed() {
        assert_eq!(
            candidate_server_urls("http://jellyfin.example.com"),
            vec!["http://jellyfin.example.com"]
        );
    }

    #[test]
    fn explicit_scheme_is_case_insensitive() {
        assert_eq!(
            candidate_server_urls("HTTPS://jellyfin.example.com"),
            vec!["HTTPS://jellyfin.example.com"]
        );
        assert_eq!(
            candidate_server_urls("HTTP://jellyfin.example.com"),
            vec!["HTTP://jellyfin.example.com"]
        );
    }

    #[test]
    fn whitespace_is_trimmed() {
        assert_eq!(
            candidate_server_urls("  jellyfin.example.com  "),
            vec![
                "https://jellyfin.example.com",
                "http://jellyfin.example.com"
            ],
        );
    }
}
