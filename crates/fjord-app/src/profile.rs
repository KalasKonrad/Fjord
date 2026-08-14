// ── fjord-app · profile.rs ───────────────────────────────────────────────────
//   Bonfire Phase 1, step 6 (2026-08-09) — the profile picker + the actual switching logic.
//   avatar_color_for / parse_hex_color   ProfileSettings.avatar_color (a stored hex string,
//                       format not guaranteed by Bonfire's own docs) -> a real slint::Color,
//                       falling back to a deterministic per-user_id palette pick on any parse
//                       failure or empty string
//   build_tile          ProfileSettings -> ProfileTile (theme.slint); display_name/avatar_initial
//                       fall back to the Jellyfin user_id itself when never populated (a plain
//                       account added before any avatar metadata existed for it)
//   open_profile_picker  builds AppState.profile-picker-profiles from Config.profiles (local
//                       data only — no live bonfire_list_profiles() refresh attempted here; see
//                       sync_bonfire_subprofiles for where that actually happens), shows the screen
//   on_profile_picker_select / on_profile_picker_add_account / on_cancel_add_account / on_profile_pin_key
//                       registered as AppState callbacks from main.rs (need state/rt, which
//                       keys.rs's raw-key dispatch for this screen deliberately doesn't hold —
//                       same "Slint callback bridges to async work" shape as every other
//                       keyboard-triggered async action in this app)
//   switch_to_profile    the real switch: resolves a token (bonfire_switch_profile for an
//                       is_bonfire target, using the MASTER's own stored token — not
//                       necessarily whatever client is currently active, since the picker can
//                       show before any session exists; the target's own stored token,
//                       re-validated via check_auth(), for an independent plain account),
//                       THEN reset_session_state, THEN finish_session_setup (auth.rs) — same
//                       "resolve first, tear down only once we know we can actually proceed"
//                       ordering this avoids leaving the user stranded on a failed switch
//   sync_bonfire_subprofiles  fire-and-forget, called after every successful finish_session_setup
//                       (auth.rs) — GET .../list, upserts a local ProfileSettings entry per
//                       returned sub-profile (add-only in v1; a sub-profile deleted server-side
//                       is not pruned locally yet, deliberately deferred, see its own doc comment)
//   refresh_profile_settings_dropdown  (step 7, 2026-08-09) pushes Settings → Profiles →
//                       "Default Profile"'s option list + current label from Config.profiles —
//                       a dynamic dropdown (same shape as audio-device/font-family) since the
//                       option list isn't fixed at compile time. Called from
//                       apply_settings_to_window (main.rs) and finish_session_setup (auth.rs),
//                       the two points Config.profiles can meaningfully have just changed.
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, warn};

use slint::Global;
use crate::config::{save_config, FjordState, ProfileSettings};
use crate::playback::VideoState;
use crate::{AppState, MainWindow, ProfileTile};

fn ss(s: &str) -> SharedString { SharedString::from(s) }

// pub(crate): also used by profile_edit.rs (Bonfire Phase 2) to resolve the
// live avatar-preview swatch from whichever palette color the user picked.
pub(crate) fn parse_hex_color(s: &str) -> Option<slint::Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 { return None; }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some(slint::Color::from_rgb_u8(r, g, b))
}

/// Deterministic per-user_id fallback (a fixed 8-color palette) — used
/// whenever `avatar_color` is empty (a plain account, which has no Bonfire-
/// supplied color at all) or fails to parse (Bonfire's own docs don't
/// guarantee the string is a `#rrggbb` hex — it's just documented as
/// "string"). Deterministic, not random, so the same profile keeps the same
/// color across sessions without needing to persist a randomly-picked one.
fn avatar_color_for(hex: &str, seed: &str) -> slint::Color {
    if !hex.is_empty() {
        if let Some(c) = parse_hex_color(hex) { return c; }
    }
    const PALETTE: [(u8, u8, u8); 8] = [
        (0x4a, 0x90, 0xd9), (0xd9, 0x4a, 0x6b), (0x4a, 0xd9, 0x8e), (0xd9, 0xa0, 0x4a),
        (0x9a, 0x4a, 0xd9), (0x4a, 0xc9, 0xd9), (0xd9, 0xd9, 0x4a), (0xd9, 0x6b, 0x4a),
    ];
    let hash: u32 = seed.bytes().fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = PALETTE[(hash as usize) % PALETTE.len()];
    slint::Color::from_rgb_u8(r, g, b)
}

pub(crate) fn build_tile(p: &ProfileSettings) -> ProfileTile {
    let display_name = if p.display_name.is_empty() { p.user_id.clone() } else { p.display_name.clone() };
    let avatar_initial = if p.avatar_initial.is_empty() {
        display_name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default()
    } else {
        p.avatar_initial.clone()
    };
    ProfileTile {
        user_id:        ss(&p.user_id),
        display_name:   ss(&display_name),
        avatar_color:   avatar_color_for(&p.avatar_color, &p.user_id),
        avatar_initial: ss(&avatar_initial),
        has_pin:        p.has_pin,
        // requires-pin mirrors has-pin here — this app has no way to know
        // Bonfire's own bypassPinOnLocalNetwork verdict without asking the
        // server (requires-pin is what the real API response would set that
        // to; local data only ever has has-pin). Worth revisiting once a
        // live refresh path exists — see open_profile_picker's own doc
        // comment for why that isn't built yet.
        requires_pin:   p.has_pin,
        is_bonfire:      p.is_bonfire,
    }
}

/// Pushes the Settings → Profiles → "Default Profile" dropdown's option
/// list and current display value from `Config.profiles`/
/// `device.default_profile_id`. A dynamic dropdown, not a fixed
/// compile-time model — its options are
/// literally the set of known profiles, which changes at runtime (Add
/// Account, a Bonfire sync) — so it has to be pushed into AppState
/// explicitly rather than resolved lazily when the popup opens, same as
/// audio-device/font-family/streaming-region. Duplicate display labels
/// (two profiles both named "Guest" before either has real Bonfire
/// metadata) resolve to whichever matches first, the same known, accepted
/// limitation those other dynamic dropdowns already have.
pub(crate) fn refresh_profile_settings_dropdown(g: &AppState<'_>, cfg: &crate::config::Config) {
    fn label(p: &ProfileSettings) -> String {
        if p.display_name.is_empty() { p.user_id.clone() } else { p.display_name.clone() }
    }
    let labels: Vec<SharedString> = cfg.profiles.iter()
        .filter(|p| !p.user_id.is_empty())
        .map(|p| ss(&label(p)))
        .collect();
    let current = cfg.profiles.iter()
        .find(|p| p.user_id == cfg.device.default_profile_id)
        .map(label)
        .unwrap_or_default();
    g.set_settings_default_profile_display(ModelRc::new(VecModel::from(labels)));
    g.set_settings_default_profile_desc(ss(&current));
}

/// What the startup gate decided (2026-08-14, replacing a plain `bool` — see
/// `should_show_picker_at_startup`'s own doc comment for the real bug this
/// closes): silently resume, show the full picker grid untargeted, or show
/// the picker but jump straight into PIN entry for an already-known
/// profile.
pub(crate) enum StartupGate {
    AutoLogin,
    ShowPicker,
    ShowPickerPin(String),
}

/// Decides what should happen at startup instead of the previous plain
/// "show the picker or not" bool. With 0 or 1 known profile there's nothing
/// to pick between — always `AutoLogin`, so every existing single-profile
/// install behaves exactly as it always has. With 2+, `DeviceConfig.
/// launch_policy` decides which profile (if any) to resume silently:
/// "always_ask" always shows the picker; "remember_last" resumes
/// `Config.active_profile_id` if it still has a stored token; "default"
/// resumes `DeviceConfig.default_profile_id` instead, if IT has a stored
/// token — the one side effect this function has is setting
/// `cfg.active_profile_id` to match when that target is usable, so
/// `Config::active()`'s own existing resolution naturally picks it up for
/// the auto-login attempt that follows.
///
/// **Real bug fixed 2026-08-14, live-reported via a direct question** ("what
/// if the las user had a pin? will it just show the pin screen?"): neither
/// branch above ever checked `has_pin` before deciding to resume silently —
/// a PIN-protected profile set as either "Remember Last" or "Default
/// Profile" was resumed with ZERO PIN prompt on every single launch,
/// completely defeating the point of putting a PIN on it in the first
/// place (this is exactly the "requiresPin still has to gate the PIN
/// prompt even when the picker itself is skipped" requirement the original
/// Bonfire design doc called for — it just was never actually wired up).
/// Both branches now check the resolved target's `has_pin` before ever
/// returning `AutoLogin`; a PIN-protected target returns
/// `ShowPickerPin(user_id)` instead, so the caller shows the picker
/// pre-focused on that profile with its PIN modal already open, rather
/// than either skipping the PIN (the bug) or falling back to an
/// untargeted full picker (losing the policy's whole "which profile"
/// intent for no reason). `has_pin` is the last-known-from-`/list` flag,
/// not a live `requiresPin` check (Fjord has no client yet at this point
/// in startup to ask Bonfire directly) — a real, LAN-bypass-aware
/// "does this actually need a PIN right now" answer only exists once
/// `switch_to_profile` itself calls `bonfire_switch_profile`/
/// `verify_pin`; this is the best available signal without one.
pub(crate) fn should_show_picker_at_startup(cfg: &mut crate::config::Config) -> StartupGate {
    let known = cfg.profiles.iter().filter(|p| !p.user_id.is_empty()).count();
    if known < 2 {
        return StartupGate::AutoLogin;
    }
    match cfg.device.launch_policy.as_str() {
        "remember_last" => {
            if cfg.active_profile_id.is_empty() || cfg.active().token.is_empty() {
                StartupGate::ShowPicker
            } else if cfg.active().has_pin {
                StartupGate::ShowPickerPin(cfg.active_profile_id.clone())
            } else {
                StartupGate::AutoLogin
            }
        }
        "default" => {
            let target_id = cfg.device.default_profile_id.clone();
            let target = cfg.profiles.iter()
                .find(|p| p.user_id == target_id && !p.token.is_empty())
                .cloned();
            match target {
                Some(t) if t.has_pin => StartupGate::ShowPickerPin(t.user_id),
                Some(_) => {
                    cfg.active_profile_id = target_id;
                    StartupGate::AutoLogin
                }
                None => StartupGate::ShowPicker,
            }
        }
        // "always_ask" and any unrecognized value fail safe to asking.
        _ => StartupGate::ShowPicker,
    }
}

/// Builds the picker's tile list from `Config.profiles` (local data only —
/// every profile Fjord has ever successfully signed into or discovered via
/// a prior `sync_bonfire_subprofiles` run) and shows the screen. Deliberately
/// does NOT attempt a live `bonfire_list_profiles()` refresh here: at the
/// point this most commonly needs to show (app startup, before any session
/// is active), there is no client yet to call it with anyway; a genuinely
/// live refresh path is a clean follow-up once this ships, not a gap in the
/// core switching flow itself.
/// `cancelable` (2026-08-14, the sidebar "Switch Profile" action): the
/// original startup-only picker has nothing to cancel back to (it's the
/// unavoidable gate before any session exists), so it never had Escape/Back
/// handling — reopening it from a LIVE session needs one, so the user isn't
/// forced to pick a new profile just because they looked. `keys.rs`'s
/// picker dispatch tier reads `profile-picker-cancelable` to decide whether
/// Escape/Backspace does anything at the top-level tile grid.
pub(crate) fn open_profile_picker(state: &Arc<Mutex<FjordState>>, window: &MainWindow, cancelable: bool) {
    let profiles: Vec<ProfileTile> = {
        let s = state.lock().unwrap();
        s.config.profiles.iter()
            .filter(|p| !p.user_id.is_empty())
            .map(build_tile)
            .collect()
    };
    let g = AppState::get(window);
    g.set_profile_picker_profiles(ModelRc::new(VecModel::from(profiles)));
    g.set_profile_picker_cursor(0);
    g.set_profile_picker_error(ss(""));
    g.set_profile_picker_loading(false);
    g.set_profile_picker_cancelable(cancelable);
    g.set_show_profile_pin_entry(false);
    g.set_show_login(false);
    g.set_show_profile_picker(true);
    window.invoke_grab_keyboard_focus();
}

/// Startup-gate variant of `open_profile_picker` (2026-08-14, the PIN-bypass
/// fix — see `should_show_picker_at_startup`'s own doc comment for the real
/// bug this closes). Shows the exact same full tile grid, non-cancelable
/// (there's no live session yet to cancel back to — matches the plain
/// startup picker), but with the cursor pre-set to `user_id` and the PIN
/// modal already open, instead of making the user re-select a profile the
/// launch policy had already resolved. Mirrors `on_profile_picker_select`'s
/// own PIN-open field sequence exactly, since that's the only other place
/// this modal gets opened from.
pub(crate) fn open_profile_picker_with_pin(state: &Arc<Mutex<FjordState>>, window: &MainWindow, user_id: &str) {
    open_profile_picker(state, window, false);
    let target = {
        let s = state.lock().unwrap();
        s.config.profiles.iter().find(|p| p.user_id == user_id).cloned()
    };
    let Some(target) = target else { return };
    let g = AppState::get(window);
    let profiles = g.get_profile_picker_profiles();
    if let Some(idx) = (0..profiles.row_count())
        .find(|&i| profiles.row_data(i).is_some_and(|t| t.user_id == user_id))
    {
        g.set_profile_picker_cursor(idx as i32);
    }
    g.set_profile_pin_target_id(ss(user_id));
    g.set_profile_pin_target_name(ss(&target.display_name));
    g.set_profile_pin_cursor(0);
    g.set_profile_pin_len(0);
    g.set_profile_pin_error(ss(""));
    state.lock().unwrap().profile_pin_buffer.clear();
    g.set_show_profile_pin_entry(true);
}

/// Populates the sidebar's own profile row (2026-08-14) — called on every
/// session start/switch (`finish_session_setup`'s own UI-update closure),
/// same site `refresh_profile_settings_dropdown` is already called from.
pub(crate) fn push_current_profile_tile(g: &AppState, cfg: &crate::config::Config) {
    g.set_current_profile_tile(build_tile(cfg.active()));
}

/// Which rows the sidebar quick-menu shows, in order — "gaps are fine"
/// dynamic list, same idiom `existing_jellyfin_menu_rows`/
/// `existing_discover_menu_rows` already use elsewhere (context_menu.rs),
/// just returning display labels directly here instead of integer ids,
/// since Slint just renders whatever list Rust gives it (no separate
/// hand-duplicated `if` conditions to keep in lockstep, unlike those two).
pub(crate) fn sidebar_profile_menu_rows(cfg: &crate::config::Config) -> Vec<&'static str> {
    let mut rows = Vec::with_capacity(4);
    if cfg.profiles.iter().filter(|p| !p.user_id.is_empty()).count() >= 2 {
        rows.push("Switch Profile");
    }
    if !cfg.active().is_bonfire {
        rows.push("Manage Profiles");
    }
    rows.push("Profile Settings");
    rows.push("Sign Out");
    rows
}

pub(crate) fn on_open_sidebar_profile_menu(state: &Arc<Mutex<FjordState>>, window: &MainWindow) {
    let g = AppState::get(window);
    let rows = sidebar_profile_menu_rows(&state.lock().unwrap().config);
    g.set_sidebar_profile_menu_rows(ModelRc::new(VecModel::from(
        rows.into_iter().map(ss).collect::<Vec<_>>()
    )));
    g.set_sidebar_profile_menu_focused(0);
    g.set_show_sidebar_profile_menu(true);
}

/// No `video`/`VideoState` param needed here — "Switch Profile" only OPENS
/// the picker; the actual teardown-then-switch (which does need it) already
/// happens inside `switch_to_profile` once a target tile is picked.
pub(crate) fn on_sidebar_profile_menu_action(
    idx:    i32,
    state:  &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt:     &tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    let rows = sidebar_profile_menu_rows(&state.lock().unwrap().config);
    let Some(&label) = rows.get(idx as usize) else { return };
    g.set_show_sidebar_profile_menu(false);
    match label {
        "Switch Profile" => open_profile_picker(state, window, true),
        "Manage Profiles" => crate::profile_edit::open_manage_profiles_screen(state, window, rt),
        "Profile Settings" => {
            g.set_show_browse(false);
            g.set_show_library(false);
            g.set_active_nav(10);
            g.invoke_nav_selected(10);
            g.set_focused_section(-1);
            g.set_settings_section(ss("profiles"));
            g.set_settings_focused(ss(""));
        }
        "Sign Out" => g.invoke_sign_out(),
        _ => {}
    }
}

pub(crate) fn on_profile_picker_select(
    state: &Arc<Mutex<FjordState>>,
    video: &Arc<Mutex<VideoState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    user_id: SharedString,
) {
    let g = AppState::get(window);
    // Guard against a second concurrent switch attempt from an impatient
    // repeat press while one is already in flight — see profile-picker-loading's
    // own doc comment in app_state.slint for the live report this closes.
    if g.get_profile_picker_loading() { return; }
    let target = {
        let s = state.lock().unwrap();
        s.config.profiles.iter().find(|p| p.user_id == user_id.as_str()).cloned()
    };
    let Some(target) = target else {
        g.set_profile_picker_error(ss("That profile is no longer available"));
        return;
    };
    if target.has_pin {
        g.set_profile_pin_target_id(user_id.clone());
        g.set_profile_pin_target_name(ss(&target.display_name.clone()));
        g.set_profile_pin_cursor(0);
        g.set_profile_pin_len(0);
        g.set_profile_pin_error(ss(""));
        state.lock().unwrap().profile_pin_buffer.clear();
        g.set_show_profile_pin_entry(true);
    } else {
        g.set_profile_picker_loading(true);
        switch_to_profile(Arc::clone(state), Arc::clone(video), window.as_weak(), rt.clone(), user_id.to_string(), None);
    }
}

pub(crate) fn on_profile_picker_add_account(window: &MainWindow) {
    let g = AppState::get(window);
    g.set_login_append_mode(true);
    g.set_show_profile_picker(false);
    g.set_show_login(true);
    g.set_status(ss(""));
    window.invoke_grab_keyboard_focus();
}

pub(crate) fn on_cancel_add_account(state: &Arc<Mutex<FjordState>>, window: &MainWindow) {
    let g = AppState::get(window);
    g.set_login_append_mode(false);
    g.set_show_login(false);
    // profile-picker-cancelable is an in-out property that outlives the
    // picker being hidden — carries forward whatever the picker was opened
    // with (startup-gate false, sidebar Switch-Profile true) rather than
    // hardcoding either, so backing out of "+ Add Account" returns to a
    // picker with the same cancel behavior it had before.
    open_profile_picker(state, window, g.get_profile_picker_cancelable());
}

pub(crate) fn on_profile_pin_key(
    state: &Arc<Mutex<FjordState>>,
    video: &Arc<Mutex<VideoState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    key: SharedString,
) {
    let g = AppState::get(window);
    match key.as_str() {
        "backspace" => {
            let mut s = state.lock().unwrap();
            s.profile_pin_buffer.pop();
            let len = s.profile_pin_buffer.len();
            drop(s);
            g.set_profile_pin_len(len as i32);
        }
        "confirm" => {
            // Same re-entrancy guard as on_profile_picker_select — a
            // repeated Enter on the keypad's own confirm key while a switch
            // triggered by an earlier confirm is still in flight must not
            // fire a second one.
            if g.get_profile_picker_loading() { return; }
            let (target_id, pin) = {
                let s = state.lock().unwrap();
                (g.get_profile_pin_target_id().to_string(), s.profile_pin_buffer.clone())
            };
            if pin.is_empty() {
                g.set_profile_pin_error(ss("Enter your PIN first"));
                return;
            }
            g.set_profile_picker_loading(true);
            switch_to_profile(Arc::clone(state), Arc::clone(video), window.as_weak(), rt.clone(), target_id, Some(pin));
        }
        digit if digit.len() == 1 && digit.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            let mut s = state.lock().unwrap();
            s.profile_pin_buffer.push_str(digit);
            let len = s.profile_pin_buffer.len();
            drop(s);
            g.set_profile_pin_len(len as i32);
        }
        _ => {}
    }
}

/// The real switch. Resolves a valid token for the target BEFORE tearing
/// down the current session (`reset_session_state`) — so a failed switch
/// (wrong PIN, a stale stored token for an independent account, a removed
/// Bonfire sub-profile) never leaves the user stranded with no active
/// session at all.
pub(crate) fn switch_to_profile(
    state:  Arc<Mutex<FjordState>>,
    video:  Arc<Mutex<VideoState>>,
    ww:     slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
    target_user_id: String,
    pin: Option<String>,
) {
    let started = std::time::Instant::now();
    debug!("switch_to_profile({target_user_id}): starting");
    let rt2 = rt.clone();
    rt.spawn(async move {
        // Every early-return path below must clear profile-picker-loading —
        // live-reported 2026-08-14 ("no feedback whats going on") added the
        // flag specifically so a switch shows visible progress; a bail that
        // forgot to clear it would leave the picker permanently stuck
        // looking busy instead, a strictly worse regression than the
        // original silent-delay report. `clear_loading` is a small helper
        // so every one of the (previously easy to miss) 4 failure points in
        // this function shares one implementation instead of hand-repeating
        // the invoke_from_event_loop dance at each.
        fn clear_loading(ww: &slint::Weak<MainWindow>, msg: Option<String>) {
            let ww = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let g = AppState::get(&w);
                    g.set_profile_picker_loading(false);
                    if let Some(msg) = msg {
                        g.set_profile_pin_error(ss(&msg));
                        g.set_profile_picker_error(ss(&msg));
                    }
                }
            });
        }

        let (device_id, target) = {
            let s = state.lock().unwrap();
            (s.config.device.device_id.clone(),
             s.config.profiles.iter().find(|p| p.user_id == target_user_id).cloned())
        };
        let Some(mut target) = target else {
            clear_loading(&ww, Some("That profile is no longer available".to_string()));
            return;
        };

        let resolved: Result<(String, String)> = async {
            if target.is_bonfire {
                let master = {
                    let s = state.lock().unwrap();
                    s.config.profiles.iter().find(|p| p.user_id == target.master_user_id).cloned()
                };
                let Some(master) = master else {
                    bail!("the master account for this profile isn't signed in on this device");
                };
                let server_url = url::Url::parse(&master.server_url)?;
                let master_client = fjord_api::JellyfinClient::new(
                    server_url, master.user_id.clone(), master.token.clone(), device_id.clone(),
                )?;
                let sw = master_client.bonfire_switch_profile(&target_user_id, pin.as_deref()).await?;
                Ok((master.server_url.clone(), sw.active_profile_token))
            } else {
                if target.token.is_empty() || target.server_url.is_empty() {
                    bail!("no saved sign-in for this profile — use \u{201c}+ Add Account\u{201d} instead");
                }
                let server_url = url::Url::parse(&target.server_url)?;
                let probe = fjord_api::JellyfinClient::new(
                    server_url, target.user_id.clone(), target.token.clone(), device_id.clone(),
                )?;
                probe.check_auth().await
                    .map_err(|e| anyhow!("saved sign-in for this profile has expired: {e}"))?;
                Ok((target.server_url.clone(), target.token.clone()))
            }
        }.await;

        let (server_url_str, token) = match resolved {
            Ok(v) => v,
            Err(e) => {
                warn!("switch_to_profile({target_user_id}) failed after {:.2}s: {e:#}", started.elapsed().as_secs_f64());
                clear_loading(&ww, Some(format!("{e:#}")));
                return;
            }
        };
        debug!("switch_to_profile({target_user_id}): token resolved after {:.2}s", started.elapsed().as_secs_f64());

        let server_url = match url::Url::parse(&server_url_str) {
            Ok(u) => u,
            Err(e) => {
                warn!("switch_to_profile: bad server_url {server_url_str:?}: {e}");
                clear_loading(&ww, Some("Something went wrong signing in — try again".to_string()));
                return;
            }
        };
        let client = match fjord_api::JellyfinClient::new(
            server_url.clone(), target_user_id.clone(), token.clone(), device_id.clone(),
        ) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                warn!("switch_to_profile: client build failed: {e}");
                clear_loading(&ww, Some("Something went wrong signing in — try again".to_string()));
                return;
            }
        };

        // Session is genuinely changing now that a token is confirmed valid — tear the old one down.
        // profile-picker-loading is deliberately NOT cleared here — the picker
        // screen itself is about to be hidden by finish_session_setup once it
        // completes, so there's nothing left for a stuck-true flag to affect;
        // reset_session_state's own broader teardown doesn't touch picker-
        // specific state at all (it's scoped to content-screen/playback state).
        crate::reset_session_state(&video, &ww, &rt2, &state);

        target.server_url = server_url_str;
        target.token       = token;
        let cfg = {
            let mut s = state.lock().unwrap();
            if let Some(p) = s.config.profiles.iter_mut().find(|p| p.user_id == target_user_id) {
                *p = target;
            } else {
                s.config.profiles.push(target);
            }
            s.config.active_profile_id = target_user_id.clone();
            s.config.clone()
        };
        save_config(&cfg);

        let target_user_id_log = target_user_id.clone();
        crate::auth::finish_session_setup(client, cfg, target_user_id, server_url, state, ww, rt2).await;
        debug!("switch_to_profile({target_user_id_log}): finish_session_setup completed after {:.2}s total", started.elapsed().as_secs_f64());
    });
}

/// Fire-and-forget, called from `finish_session_setup` (auth.rs) after
/// every successful session start — `bonfire_list_profiles()` already
/// returns `Ok(vec![])` on a 404 (plugin absent), so this is always safe to
/// attempt regardless of whether the server actually has Bonfire installed.
/// Add-only in v1: upserts a local `ProfileSettings` entry per sub-profile
/// the plugin reports, but never prunes one that's disappeared server-side
/// (deleted via Bonfire's own web UI) — deliberately deferred rather than
/// risk deleting the wrong local entry; a stale local tile for an already-
/// removed sub-profile just fails cleanly at switch time instead (the
/// master's own `/switch` call 404s/400s, surfaced as a toast).
pub(crate) fn sync_bonfire_subprofiles(
    client: Arc<fjord_api::JellyfinClient>,
    state:  Arc<Mutex<FjordState>>,
    rt:     tokio::runtime::Handle,
) {
    rt.spawn(async move {
        // Logged unconditionally (2026-08-11) — this function was previously
        // silent on every path except a genuine non-404 error, which is
        // exactly what let a real gap (this call missing from
        // spawn_auto_login entirely — see that call site's own doc comment)
        // go unnoticed across two separate live HTPC sessions: nothing in
        // the log distinguished "never even tried" from "tried and found
        // nothing." The next report should show this directly instead of
        // needing another code-reading pass.
        tracing::debug!("sync_bonfire_subprofiles: checking bonfire_list_profiles");
        let profiles = match client.bonfire_list_profiles().await {
            Ok(p) => p,
            Err(e) => { tracing::debug!("bonfire_list_profiles: {e:#}"); return; }
        };
        if profiles.is_empty() {
            tracing::debug!("sync_bonfire_subprofiles: 0 sub-profiles reported (plugin absent, no sub-profiles configured, or not a master account)");
            return;
        }
        tracing::debug!("sync_bonfire_subprofiles: {} sub-profile(s) reported", profiles.len());
        let master_user_id = client.user_id.clone();
        let cfg = {
            let mut s = state.lock().unwrap();
            // Session guard — bail if the active client changed while this
            // background sync was in flight (Arc::ptr_eq, same idiom ws.rs
            // already uses for exactly this class of race).
            if !s.client.as_ref().is_some_and(|c| Arc::ptr_eq(c, &client)) {
                tracing::debug!("sync_bonfire_subprofiles: session changed mid-flight, discarding");
                return;
            }
            for bp in &profiles {
                // Real bug, found 2026-08-14 while investigating a live
                // "can't select a profile, 401 on every switch" report:
                // Bonfire's own `/list` response includes the calling
                // MASTER account's own profile alongside its real
                // sub-profiles (confirmed from this exact dev machine's
                // config.json — the master's own already-correct,
                // `is_bonfire: false` entry had been silently overwritten
                // to `is_bonfire: true, master_user_id: <itself>`). That
                // self-referencing corruption made `switch_to_profile`
                // treat clicking the master's own tile as a Bonfire
                // sub-profile switch INTO itself — a nonsensical request
                // Bonfire's server has no reason to accept. Skip the
                // master's own id here so its original, correct
                // (non-Bonfire) entry is never touched by this upsert.
                if bp.profile_user_id == master_user_id {
                    tracing::debug!("sync_bonfire_subprofiles: skipping self entry ({master_user_id}) in /list response");
                    continue;
                }
                if let Some(existing) = s.config.profiles.iter_mut().find(|p| p.user_id == bp.profile_user_id) {
                    existing.display_name   = bp.profile_name.clone();
                    existing.avatar_color   = bp.avatar_color.clone();
                    existing.avatar_initial = bp.avatar_initial.clone();
                    existing.is_bonfire     = true;
                    existing.master_user_id = master_user_id.clone();
                    existing.has_pin        = bp.has_pin;
                } else {
                    s.config.profiles.push(ProfileSettings {
                        user_id:        bp.profile_user_id.clone(),
                        display_name:   bp.profile_name.clone(),
                        avatar_color:   bp.avatar_color.clone(),
                        avatar_initial: bp.avatar_initial.clone(),
                        is_bonfire:     true,
                        master_user_id: master_user_id.clone(),
                        has_pin:        bp.has_pin,
                        ..Default::default()
                    });
                }
            }
            s.config.clone()
        };
        save_config(&cfg);
    });
}
