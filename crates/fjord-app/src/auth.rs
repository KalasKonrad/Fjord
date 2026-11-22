// ── fjord-app · auth.rs ──────────────────────────────────────────────────────
//   candidate_server_urls / first_answering_candidate / is_connectivity_failure  a typed address
//             without a scheme tries https, then http (only on a real connectivity failure);
//             authenticate_with_fallback = the password login through it
//   fell_back_to_http / note_if_http_fallback  a schemeless address that only answered over http →
//             one log line + toast (Jellyfin login, Seerr connects; unit-tested)
//   LoginOptions  { append, remember }
//   do_login  authenticate (30 s timeout), then complete_login
//   complete_login  shared by password and Quick Connect sign-ins: update or add the profile
//             (append = Add Account; a direct login resets Bonfire-discovery fields; display_name
//             backfilled), persist, then finish_session_setup
//   finish_session_setup  shared tail of login and profile switch: profile settings + Seerr client for
//             the new profile, sidebar tile, warm start from this user's cached home/series/screen
//             caches, then home data/series/system info/plugins in parallel; persists caches, starts
//             the WebSocket, posters, movie collections, Bonfire sync; closes the pickers
//   spawn_not_watched_rows / push_not_watched_row  the "Not watched" rows off the login path: TV
//             at once (home::not_watched_tv), Movies after its server query; each row then its
//             posters (session-guarded)
//   wire_login             callbacks moved from main() (0.5.0 step 3): the login screen, + wire_quick_connect
//   wire_quick_connect     Login's Quick Connect: start (resolve the address, /QuickConnect/Enabled, a
//             code → login-qc-code), poll (login.slint's 2 s Timer; one check at a time; approval →
//             finish_quick_connect → complete_login; expiry, Quick Connect turned off or
//             QC_MAX_FAILURES checks without an answer end the wait), cancel; QuickConnectState's
//             attempt counter drops answers of an older attempt; quick_connect_next (unit-tested)
//   wire_sign_out          callbacks moved from main() (0.5.0 step 3): Sign Out — removes the account and
//             its Bonfire sub-profiles/group accounts, logs their server sessions out, keeps
//             device_id and settings, then the account picker (if any account is left) or Login
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::JellyfinClient;
use slint::{ComponentHandle, SharedString};
use tracing::{debug, error, info, warn};
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

/// Runs `attempt` against each of `candidate_server_urls(server)` in order, moving on to the next
/// candidate ONLY when one fails with a genuine connectivity error (see `is_connectivity_failure`).
/// A candidate that reaches the server and gets a real error back (401 wrong password, 500, etc.)
/// is the final answer — retrying the same request under a different scheme would never fix a
/// wrong password, and would just double the wait before showing the real error. `what` names the
/// request in the log.
pub(crate) async fn first_answering_candidate<T, F, Fut>(
    server: &str,
    what: &str,
    mut attempt: F,
) -> Result<(Url, T)>
where
    F: FnMut(Url) -> Fut,
    Fut: std::future::Future<Output = Result<T>>,
{
    let candidates = candidate_server_urls(server);
    let mut last_err: Option<anyhow::Error> = None;
    for (i, candidate) in candidates.iter().enumerate() {
        let server_url = Url::parse(candidate)?;
        match attempt(server_url.clone()).await {
            Ok(answer) => return Ok((server_url, answer)),
            Err(e) => {
                let is_last = i + 1 == candidates.len();
                if is_connectivity_failure(&e) && !is_last {
                    info!("{what}: {candidate} unreachable ({e:#}), trying next candidate");
                    last_err = Some(e);
                    continue;
                }
                return Err(e);
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("no server address given")))
}

/// The password login through `first_answering_candidate`.
pub(crate) async fn authenticate_with_fallback(
    http: &reqwest::Client,
    server: &str,
    user: &str,
    pass: &str,
    device_id: &str,
) -> Result<(Url, fjord_api::models::AuthResponse)> {
    first_answering_candidate(server, "authenticate", move |url| async move {
        fjord_api::authenticate(http, &url, user, pass, device_id).await
    })
    .await
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
            complete_login(
                server_url,
                auth,
                LoginOptions { append, remember },
                cfg,
                state,
                window_weak.clone(),
                rt_handle,
            )
            .await
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

/// The shared tail of a password and a Quick Connect sign-in, once the server has handed out a
/// token: update or add the profile (`opts.append` = Add Account), persist, then
/// finish_session_setup. `cfg` is a copy of the current config with a device id.
async fn complete_login(
    server_url: Url,
    auth: fjord_api::models::AuthResponse,
    opts: LoginOptions,
    mut cfg: crate::config::Config,
    state: Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) -> Result<()> {
    let LoginOptions { append, remember } = opts;
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
            // A successful direct login (password or Quick Connect) proves independent access: reset the
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

    // Timed (the `timing:` lines). No "Not watched" rows here (None): they aren't on Home, the
    // TV pick needs the all_series this join fetches, and the Movies row is a slow server query
    // — spawn_not_watched_rows fills them afterwards.
    let (home_data, series_res, sysinfo_res, plugins_res) = tokio::join!(
        crate::timed("fetch_home_data (all rows)", fetch_home_data(&client, None)),
        crate::timed("get_all_series", client.get_all_series()),
        crate::timed("get_system_info", client.get_system_info()),
        crate::timed("get_plugins", client.get_plugins()),
    );

    let series = series_res.unwrap_or_else(|e| {
        warn!("get_all_series: {:#}", e);
        vec![]
    });
    info!("loaded {} series", series.len());
    let started = crate::home::started_series_ids(&home_data.continue_watching, &home_data.next_up);
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
    spawn_not_watched_rows(
        client5,
        Arc::clone(&state),
        started,
        window_weak,
        &rt_handle_inner,
    );
}

/// The two rows `fetch_home_data(…, None)` skipped above, filled off the login path: TV at
/// once (a local pick from the just-stored all_series), Movies when its server query returns —
/// each row, then its posters. Checks `crate::session_current` first — a result must not land
/// after a sign-out/switch.
fn spawn_not_watched_rows(
    client: Arc<JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    started: std::collections::HashSet<String>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
) {
    let rt = rt_handle.clone();
    rt_handle.spawn(async move {
        let tv = crate::home::not_watched_tv(&client, &state, &started)
            .await
            .unwrap_or_else(|e| {
                warn!("not_watched_tv: {:#}", e);
                vec![]
            });
        push_not_watched_row(
            crate::home::HomeSection::NotWatchedTv,
            tv,
            &client,
            &state,
            &window_weak,
            &rt,
        );
        let movies = crate::timed(
            "not_watched_movies (deferred)",
            client.get_unwatched(Some("Movie")),
        )
        .await
        .unwrap_or_else(|e| {
            warn!("not_watched_movies: {:#}", e);
            vec![]
        });
        push_not_watched_row(
            crate::home::HomeSection::NotWatchedMovies,
            movies,
            &client,
            &state,
            &window_weak,
            &rt,
        );
    });
}

/// One "Not watched" row from spawn_not_watched_rows: the cards now, posters after.
fn push_not_watched_row(
    sec: crate::home::HomeSection,
    items: Vec<fjord_api::models::MediaItem>,
    client: &Arc<JellyfinClient>,
    state: &Arc<Mutex<FjordState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
) {
    if !crate::session_current(state, client) {
        return;
    }
    let watchlist = state.lock().unwrap().jellyfin_watchlist_ids.clone();
    let model_items = items.clone();
    let ww = window_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            crate::push_section_model(&w, sec, items_to_model(&model_items, &watchlist));
        }
    });
    let mut sections = crate::home::HomeSection::empty_array();
    sections[sec as usize].1 = items;
    crate::spawn_poster_loading(
        Arc::clone(client),
        sections,
        window_weak.clone(),
        rt.clone(),
        Arc::clone(state),
    );
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
    wire_quick_connect(&window, &state, rt);
}

// ── Quick Connect ────────────────────────────────────────────────────────────
// Sign in by approving a 6-digit code in another Jellyfin app — no password typed with the
// remote. Login's Quick Connect button → start (resolve the typed address like a password login,
// check the server allows it, request a code); login.slint shows the code and calls poll every
// 2 s while it does; approval → authenticate with the secret → complete_login, like a password.

/// How long a Quick Connect call may take (the poll runs every 2 s).
const QC_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
/// Checks in a row that got no answer before the wait ends (~20 s of a vanished server).
const QC_MAX_FAILURES: u32 = 10;

/// The request the login screen is waiting on.
struct QuickConnectWait {
    server_url: Url,
    device_id: String,
    secret: String,
}

/// Shared by the start/poll/cancel callbacks.
#[derive(Default)]
struct QuickConnectState {
    /// Bumped by every start and cancel: an answer for an older attempt is dropped.
    attempt: u64,
    wait: Option<QuickConnectWait>,
    /// One status check at a time — a check may take longer than the 2 s poll interval.
    poll_in_flight: bool,
    /// Checks in a row without an answer.
    failures: u32,
}

/// What a status check means for the wait.
#[derive(Debug, PartialEq)]
enum QcNext {
    KeepWaiting,
    Approved,
    /// Stop waiting and show this.
    End(&'static str),
}

/// The wait's next step after a status check (`failures` = checks in a row without an answer,
/// this one included).
fn quick_connect_next(status: &Result<fjord_api::QuickConnectStatus>, failures: u32) -> QcNext {
    use fjord_api::QuickConnectStatus as S;
    match status {
        Ok(S::Waiting) => QcNext::KeepWaiting,
        Ok(S::Approved) => QcNext::Approved,
        Ok(S::Expired) => QcNext::End("The code expired — press Quick Connect for a new one"),
        Ok(S::Disabled) => QcNext::End("Quick Connect was turned off on the server"),
        Err(_) if failures >= QC_MAX_FAILURES => {
            QcNext::End("Lost the connection to the Jellyfin server — try Quick Connect again")
        }
        Err(_) => QcNext::KeepWaiting,
    }
}

/// The login screen's line for a failed step: a request that got no answer says the server
/// isn't there; anything else names the step and the server's reason.
fn quick_connect_error(what: &str, e: &anyhow::Error) -> String {
    if is_connectivity_failure(e) {
        return "Couldn't reach a Jellyfin server at that address".into();
    }
    format!("{what} — {}", crate::short_error(e))
}

fn wire_quick_connect(
    window: &MainWindow,
    state: &Arc<Mutex<FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    let qc = Arc::new(Mutex::new(QuickConnectState::default()));
    let http = reqwest::Client::builder()
        .timeout(QC_TIMEOUT)
        .build()
        .unwrap_or_default();
    let g = AppState::get(window);

    g.on_login_quickconnect_start({
        let qc = Arc::clone(&qc);
        let state = Arc::clone(state);
        let ww = window.as_weak();
        let rt = rt.handle().clone();
        let http = http.clone();
        move |server| {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let server = server.trim().to_string();
            if server.is_empty() {
                g.set_status(ss("Enter the server address first"));
                return;
            }
            let attempt = {
                let mut q = qc.lock().unwrap();
                q.attempt += 1;
                q.wait = None;
                q.failures = 0;
                q.attempt
            };
            g.set_login_qc_busy(true);
            g.set_status(ss(""));
            let device_id = state.lock().unwrap().config.device.device_id.clone();
            let (qc, ww, http) = (Arc::clone(&qc), ww.clone(), http.clone());
            rt.spawn(async move {
                let found = first_answering_candidate(&server, "quick connect", |url| {
                    let http = http.clone();
                    async move { fjord_api::quick_connect_enabled(&http, &url).await }
                })
                .await;
                let outcome = match found {
                    Ok((url, true)) => fjord_api::quick_connect_initiate(&http, &url, &device_id)
                        .await
                        .map(|r| Some((url, r)))
                        .map_err(|e| {
                            warn!("quick connect: couldn't get a code: {e:#}");
                            quick_connect_error("Couldn't start Quick Connect", &e)
                        }),
                    Ok((url, false)) => {
                        info!("quick connect: turned off on {url}");
                        Ok(None)
                    }
                    Err(e) => {
                        warn!("quick connect: no server at {server}: {e:#}");
                        Err(quick_connect_error("Couldn't start Quick Connect", &e))
                    }
                };
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    // Cancelled, restarted or Login closed meanwhile (close_login_screen clears busy).
                    if qc.lock().unwrap().attempt != attempt || !g.get_login_qc_busy() {
                        debug!("quick connect: dropped the answer of an older attempt");
                        return;
                    }
                    g.set_login_qc_busy(false);
                    match outcome {
                        Ok(Some((url, r))) => {
                            info!("quick connect: waiting for approval of a code from {url}");
                            note_if_http_fallback(&ww, "Jellyfin", &server, &url);
                            qc.lock().unwrap().wait = Some(QuickConnectWait {
                                server_url: url,
                                device_id,
                                secret: r.secret,
                            });
                            // The password card hides behind the code: the keys go to `fs` and
                            // its Cancel button.
                            g.set_show_onscreen_keyboard(false);
                            g.set_login_qc_code(ss(&r.code));
                            g.set_login_zone(7);
                            g.invoke_refocus();
                        }
                        Ok(None) => g.set_status(ss(
                            "Quick Connect is turned off on this server — an administrator can \
                             turn it on in the Dashboard (General)",
                        )),
                        Err(msg) => g.set_status(ss(&msg)),
                    }
                });
            });
        }
    });

    g.on_login_quickconnect_poll({
        let qc = Arc::clone(&qc);
        let state = Arc::clone(state);
        let ww = window.as_weak();
        let rt = rt.handle().clone();
        move || {
            let (attempt, server_url, device_id, secret) = {
                let mut q = qc.lock().unwrap();
                let Some(wait) = &q.wait else { return };
                if q.poll_in_flight {
                    return;
                }
                let found = (
                    q.attempt,
                    wait.server_url.clone(),
                    wait.device_id.clone(),
                    wait.secret.clone(),
                );
                q.poll_in_flight = true;
                found
            };
            let (qc, state, ww, rt2, http) = (
                Arc::clone(&qc),
                Arc::clone(&state),
                ww.clone(),
                rt.clone(),
                http.clone(),
            );
            rt.spawn(async move {
                let status =
                    fjord_api::quick_connect_status(&http, &server_url, &device_id, &secret).await;
                let _ = slint::invoke_from_event_loop(move || {
                    // Decided under the lock, acted on after it (the UI calls below).
                    let next = {
                        let mut q = qc.lock().unwrap();
                        q.poll_in_flight = false;
                        if q.attempt != attempt || q.wait.is_none() {
                            return;
                        }
                        q.failures = if status.is_err() { q.failures + 1 } else { 0 };
                        let next = quick_connect_next(&status, q.failures);
                        match next {
                            QcNext::KeepWaiting => None,
                            QcNext::Approved => Some(Ok(q.wait.take())),
                            QcNext::End(msg) => {
                                q.wait = None;
                                Some(Err(msg))
                            }
                        }
                    };
                    if let Err(e) = &status {
                        debug!("quick connect: status check failed: {e:#}");
                    }
                    let Some(next) = next else { return };
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_login_qc_code(ss(""));
                    match next {
                        Ok(Some(wait)) => {
                            info!("quick connect: approved — signing in");
                            g.set_status(ss("Signing in…"));
                            let opts = LoginOptions {
                                append: g.get_login_append_mode(),
                                remember: g.get_login_remember(),
                            };
                            rt2.spawn(finish_quick_connect(
                                wait,
                                opts,
                                state,
                                ww.clone(),
                                rt2.clone(),
                            ));
                        }
                        Ok(None) => {}
                        Err(msg) => {
                            info!("quick connect: stopped waiting — {msg}");
                            g.set_status(ss(msg));
                        }
                    }
                });
            });
        }
    });

    g.on_login_quickconnect_cancel({
        let qc = Arc::clone(&qc);
        let ww = window.as_weak();
        move || {
            {
                let mut q = qc.lock().unwrap();
                q.attempt += 1;
                q.wait = None;
                q.failures = 0;
            }
            info!("quick connect: cancelled");
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_login_qc_code(ss(""));
                g.set_login_qc_busy(false);
                g.set_status(ss(""));
            }
        }
    });
}

/// After approval: the session for the secret, then the same sign-in as a password login.
async fn finish_quick_connect(
    wait: QuickConnectWait,
    opts: LoginOptions,
    state: Arc<Mutex<FjordState>>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    let result: Result<()> = async {
        let http = reqwest::Client::builder().timeout(QC_TIMEOUT).build()?;
        let auth = fjord_api::authenticate_with_quick_connect(
            &http,
            &wait.server_url,
            &wait.device_id,
            &wait.secret,
        )
        .await?;
        info!("quick connect: authenticated as {}", auth.user.name);
        let mut cfg = state.lock().unwrap().config.clone();
        ensure_device_id(&mut cfg);
        complete_login(
            wait.server_url,
            auth,
            opts,
            cfg,
            state,
            window_weak.clone(),
            rt_handle,
        )
        .await
    }
    .await;
    if let Err(e) = result {
        error!("quick connect: sign-in failed: {e:#}");
        let msg = quick_connect_error("Couldn't finish Quick Connect", &e);
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = window_weak.upgrade() {
                AppState::get(&w).set_status(ss(&msg));
            }
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
    use super::{
        QC_MAX_FAILURES, QcNext, candidate_server_urls, fell_back_to_http, quick_connect_next,
    };
    use fjord_api::QuickConnectStatus as S;
    use url::Url;

    /// Approval signs in; expiry and a server that turned Quick Connect off end the wait at once;
    /// a check without an answer ends it only after QC_MAX_FAILURES in a row.
    #[test]
    fn the_quick_connect_wait_follows_the_server() {
        assert_eq!(quick_connect_next(&Ok(S::Waiting), 0), QcNext::KeepWaiting);
        assert_eq!(quick_connect_next(&Ok(S::Approved), 0), QcNext::Approved);
        assert!(
            matches!(quick_connect_next(&Ok(S::Expired), 0), QcNext::End(m) if m.contains("expired"))
        );
        assert!(matches!(
            quick_connect_next(&Ok(S::Disabled), 0),
            QcNext::End(_)
        ));
        let lost = || Err(anyhow::anyhow!("connection refused"));
        assert_eq!(quick_connect_next(&lost(), 1), QcNext::KeepWaiting);
        assert_eq!(
            quick_connect_next(&lost(), QC_MAX_FAILURES - 1),
            QcNext::KeepWaiting
        );
        assert!(matches!(
            quick_connect_next(&lost(), QC_MAX_FAILURES),
            QcNext::End(_)
        ));
    }

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
