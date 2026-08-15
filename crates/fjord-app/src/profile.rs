// ── fjord-app · profile.rs ───────────────────────────────────────────────────
//   Bonfire Phase 1, step 6 (2026-08-09) — the profile picker + switching logic.
//   2-tier account/profile redesign (2026-08-14, live-reported design feedback —
//   "it shuld be 2 layers? like accaunt then profiles?") — see StartupGate's own
//   doc comment for the full resolution order; the short version: an ACCOUNT is
//   either a plain login or a whole Bonfire household (master + its sub-profiles),
//   grouped by account_root_id (master_user_id for a sub-profile, own user_id
//   otherwise); the account picker only ever appears with 2+ known accounts; the
//   profile picker underneath it is always scoped to one account's own profiles.
//   avatar_color_for / parse_hex_color   ProfileSettings.avatar_color (a stored hex string,
//                       format not guaranteed by Bonfire's own docs) -> a real slint::Color,
//                       falling back to a deterministic per-user_id palette pick on any parse
//                       failure or empty string
//   AccountGroup / account_root_id / group_into_accounts   the account-tier grouping
//                       primitive — sorts each group's own profiles root-first
//   build_account_tile  AccountGroup -> AccountTile (theme.slint); server_url + a
//                       "N profiles" count for the account picker's own tile
//   build_tile          ProfileSettings -> ProfileTile (theme.slint); display_name/avatar_initial
//                       fall back to the Jellyfin user_id itself when never populated (a plain
//                       account added before any avatar metadata existed for it)
//   StartupGate          AutoLogin | ShowAccountPicker | ShowProfilePicker(account_root_id) |
//                       ShowProfilePickerPin(account_root_id, user_id) | RequireLogin(server_url)
//                       — see its own doc comment for the full 2-tier + remember_login resolution
//   should_show_picker_at_startup  the actual 2-tier resolution: account_launch_policy picks
//                       (or asks for) an account first, remember_login gates whether that
//                       account's root can EVER be silently resumed at all (false ->
//                       RequireLogin unconditionally, before profile-tier logic ever runs —
//                       the real PIN-bypass concern this whole redesign was built to close: a
//                       plain account has no PIN, so silently resuming it next to a
//                       PIN-protected household would be a built-in bypass), then launch_policy
//                       resolves within that one account's own profiles exactly as before
//   open_profile_picker  now always account-scoped (account_root_id param); builds
//                       AppState.profile-picker-profiles from just that account's own
//                       Config.profiles entries (local data only — no live
//                       bonfire_list_profiles() refresh attempted here; see
//                       sync_bonfire_subprofiles for where that happens), sets
//                       profile-picker-show-back-to-accounts (true iff 2+ accounts exist)
//   open_profile_picker_with_pin  same, jumping straight into PIN entry for one known profile
//   open_account_picker  builds AppState.account-picker-accounts from group_into_accounts(),
//                       shows the account-tier screen
//   open_switch_entry_point  the sidebar quick-menu's "Switch Profile" entry point — routes
//                       straight to the profile picker when only one account is known, else
//                       opens the account picker first
//   on_profile_picker_select / on_account_picker_select / on_profile_pin_key
//                       registered as AppState callbacks from main.rs (need state/rt, which
//                       keys.rs's raw-key dispatch for these screens deliberately doesn't hold —
//                       same "Slint callback bridges to async work" shape as every other
//                       keyboard-triggered async action in this app)
//   on_account_picker_add_account / on_settings_add_account  both just open LoginScreen with
//                       login-append-mode=true, differing only in login-append-source (drives
//                       on_cancel_add_account's own return destination — back to the account
//                       picker only when it was the one that opened Login)
//   on_cancel_add_account  Back from an append-mode Login: returns to the account picker only
//                       when login-append-source=="account_picker", else a plain close (the
//                       Settings → Profiles → "Add Account" row's own path — nothing to return to)
//   switch_to_profile    the real switch: resolves a token (bonfire_switch_profile for an
//                       is_bonfire target, using the MASTER's own stored token — not
//                       necessarily whatever client is currently active, since the picker can
//                       show before any session exists; the target's own stored token,
//                       re-validated via check_auth(), for an independent plain account),
//                       THEN reset_session_state, THEN finish_session_setup (auth.rs) — same
//                       "resolve first, tear down only once we know we can actually proceed"
//                       ordering this avoids leaving the user stranded on a failed switch;
//                       clear_loading clears BOTH the profile-picker and account-picker loading/
//                       error state unconditionally (harmless for whichever screen isn't up),
//                       since a single-profile account's own tile can switch directly from
//                       the account tier with no profile-picker step in between at all
//   sync_bonfire_subprofiles  fire-and-forget, called after every successful finish_session_setup
//                       (auth.rs) AND spawn_auto_login (main.rs, 2026-08-11 fix — see its own
//                       doc comment) — GET .../list, upserts a local ProfileSettings entry per
//                       returned sub-profile (add-only in v1; a sub-profile deleted server-side
//                       is not pruned locally yet, deliberately deferred; also skips any entry
//                       whose id matches the calling client's own user_id — a real
//                       self-referencing-master-profile corruption bug fixed 2026-08-14)
//   refresh_profile_settings_dropdown  (step 7, 2026-08-09) pushes Settings → Profiles →
//                       "Default Profile"'s option list + current label from Config.profiles —
//                       a dynamic dropdown (same shape as audio-device/font-family) since the
//                       option list isn't fixed at compile time.
//   refresh_account_settings_dropdown  (2026-08-14) the account-tier twin, over
//                       group_into_accounts() instead of the flat profile list — backs Settings
//                       → Profiles → "Default Account". Both dropdowns are called from
//                       apply_settings_to_window (main.rs) and finish_session_setup (auth.rs),
//                       the two points Config.profiles can meaningfully have just changed.
//   sidebar_profile_menu_rows / on_open_sidebar_profile_menu / on_sidebar_profile_menu_action
//                       the sidebar quick-menu (2026-08-14): Switch Profile (2+ profiles in the
//                       CURRENT account only — routes through open_switch_entry_point, not
//                       open_profile_picker directly, so a multi-account install always gets
//                       the account tier first) / Manage Profiles (master only) / Profile
//                       Settings / Sign Out
//   push_current_profile_tile  populates the sidebar's own current-profile avatar+name row;
//                       called from finish_session_setup AND spawn_auto_login (2026-08-14 fix —
//                       previously only the former, so a plain relaunch never showed it)
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, warn};

use slint::Global;
use crate::config::{save_config, FjordState, ProfileSettings};
use crate::playback::VideoState;
use crate::{AppState, MainWindow, ProfileTile};

fn ss(s: &str) -> SharedString { SharedString::from(s) }

/// Real bug, live-reported 2026-08-15 ("no keybord navigation worked" on
/// the account/profile picker): both picker screens can be opened directly
/// from `main()`'s own synchronous startup-gate dispatch (`match gate {...}`
/// in main.rs), which runs entirely BEFORE `window.run()` ever starts
/// pumping the Slint event loop. A plain `window.invoke_grab_keyboard_focus()`
/// call issued at that point doesn't reliably take effect — the exact same
/// class of "a Slint operation issued before the event loop is live doesn't
/// stick" bug this file's own FadeGate kick-timer race already hit once
/// (see CLAUDE.md's Bonfire section, the LoginScreen-flash writeup) — while
/// the ordinary auto-login path never showed this symptom purely because its
/// own equivalent grab call happens inside an async-spawned task, which
/// naturally lands after `window.run()` has already started by the time it
/// runs. Deferring via `invoke_from_event_loop` queues the grab for the
/// event loop's own next tick instead of executing it synchronously right
/// now — a genuine fix when called pre-`window.run()`, and an imperceptible
/// one-tick delay when called from an already-running session (the sidebar
/// quick-menu's "Switch Profile" path), so this is safe to use
/// unconditionally rather than needing two different call shapes depending
/// on the caller.
pub(crate) fn grab_focus_deferred(window: &MainWindow) {
    let ww = window.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() { w.invoke_grab_keyboard_focus(); }
    });
}

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

/// A group of `Config.profiles` entries sharing one "account" identity —
/// the root (a plain login, or a Bonfire household master) plus any
/// Bonfire sub-profiles switched into via that master's own token
/// (2026-08-14, the account-tier picker — live-reported design feedback:
/// "if there is no bonfire on the other server everyone can use that
/// accaunt as the session is saved... so it shuld be 2 layers? like
/// accaunt then profiles?"). NOT a persisted concept — `Config.profiles`
/// stays one flat `Vec`; this is purely a runtime view over it, rebuilt
/// wherever it's needed rather than cached, since it's cheap (profile
/// counts are realistically small — Bonfire caps a household at 5).
pub(crate) struct AccountGroup {
    /// The grouping key — the root profile's own `user_id`. Also what
    /// `DeviceConfig.default_account_id` stores.
    pub root_id:    String,
    pub server_url: String,
    /// Root first (guaranteed present — see `account_root_id`'s own doc
    /// comment for why an orphan sub-profile can't happen), sub-profiles
    /// after in encounter order.
    pub profiles:   Vec<ProfileSettings>,
}

/// The account a profile belongs to — its own `user_id` if it IS an
/// account root (`is_bonfire == false`), else its master's `user_id`.
/// Every Bonfire sub-profile in `Config.profiles` is guaranteed to have
/// its master's own entry present too — `sync_bonfire_subprofiles` only
/// ever discovers/adds sub-profiles after a successful MASTER login, so
/// there's no path to an orphan sub-profile with no root in the list.
pub(crate) fn account_root_id(p: &ProfileSettings) -> &str {
    if p.is_bonfire { &p.master_user_id } else { &p.user_id }
}

/// Groups `Config.profiles` into `AccountGroup`s — root-first within each
/// group, groups in first-seen order (stable, not sorted — matches this
/// app's existing "insertion order is fine" precedent for the profile
/// picker's own tile row).
pub(crate) fn group_into_accounts(profiles: &[ProfileSettings]) -> Vec<AccountGroup> {
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, Vec<ProfileSettings>> = HashMap::new();
    for p in profiles.iter().filter(|p| !p.user_id.is_empty()) {
        let key = account_root_id(p).to_string();
        if !groups.contains_key(&key) { order.push(key.clone()); }
        groups.entry(key).or_default().push(p.clone());
    }
    order.into_iter().filter_map(|key| {
        let mut members = groups.remove(&key)?;
        members.sort_by_key(|p| p.is_bonfire); // root (false) first, stable within that
        let server_url = members.first()?.server_url.clone();
        Some(AccountGroup { root_id: key, server_url, profiles: members })
    }).collect()
}

/// `ProfileSettings` -> `AccountTile` (theme.slint) for the account-tier
/// picker. Uses the group's root for avatar/display-name (the "account" is
/// represented by whoever owns it), `profiles.len()` for the subtitle
/// ("N profiles" when > 1, hidden at exactly 1 by the Slint side).
pub(crate) fn build_account_tile(group: &AccountGroup) -> crate::AccountTile {
    let root = group.profiles.first();
    let display_name = root.map(|p| {
        if p.display_name.is_empty() { p.user_id.clone() } else { p.display_name.clone() }
    }).unwrap_or_default();
    let avatar_initial = root.and_then(|p| {
        if p.avatar_initial.is_empty() {
            display_name.chars().next().map(|c| c.to_uppercase().to_string())
        } else {
            Some(p.avatar_initial.clone())
        }
    }).unwrap_or_default();
    let avatar_color_src = root.map(|p| p.avatar_color.as_str()).unwrap_or("");
    crate::AccountTile {
        root_id:       ss(&group.root_id),
        display_name:  ss(&display_name),
        avatar_color:  avatar_color_for(avatar_color_src, &group.root_id),
        avatar_initial: ss(&avatar_initial),
        server_url:    ss(&group.server_url),
        profile_count: group.profiles.len() as i32,
    }
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

/// Account-tier mirror of `refresh_profile_settings_dropdown` (2026-08-14)
/// — pushes Settings → Profiles → "Default Account"'s option list and
/// current display value from the grouped `AccountGroup`s, same dynamic-
/// dropdown reasoning (the option list is literally the set of known
/// accounts, which changes at runtime).
pub(crate) fn refresh_account_settings_dropdown(g: &AppState<'_>, cfg: &crate::config::Config) {
    fn label(group: &AccountGroup) -> String {
        let root = group.profiles.first();
        root.map(|p| if p.display_name.is_empty() { p.user_id.clone() } else { p.display_name.clone() })
            .unwrap_or_default()
    }
    let accounts = group_into_accounts(&cfg.profiles);
    let labels: Vec<SharedString> = accounts.iter().map(|a| ss(&label(a))).collect();
    let current = accounts.iter()
        .find(|a| a.root_id == cfg.device.default_account_id)
        .map(label)
        .unwrap_or_default();
    g.set_settings_default_account_display(ModelRc::new(VecModel::from(labels)));
    g.set_settings_default_account_desc(ss(&current));
}

/// What the startup gate decided. Two tiers now (2026-08-14, replacing the
/// earlier 3-way single-tier version — see this function's own doc comment
/// for the full account/profile resolution and the real bugs both tiers
/// close):
/// - `AutoLogin` — silently resume, no picker at all.
/// - `ShowAccountPicker` — 2+ distinct ACCOUNTS known, and either
///   `account_launch_policy` says to ask, or the resolved target account
///   isn't usable (no valid stored token on its root).
/// - `ShowProfilePicker(account_root_id)` — exactly one account resolved
///   (or was already the only one known), it has 2+ profiles, and either
///   `launch_policy` says to ask within it, or the resolved profile target
///   isn't usable — scoped to just that account's own profiles, not the
///   flat `Config.profiles` list.
/// - `ShowProfilePickerPin(account_root_id, target_user_id)` — same as
///   above, but jump straight into PIN entry for an already-known profile
///   within it.
/// - `RequireLogin(server_url, username)` — the resolved account's root has
///   `remember_login == false`: never silently resume it, always show a
///   fresh Login screen instead (pre-filled with its server address AND
///   username — 2026-08-15, live-reported: only the server was ever
///   pre-filled, but the username isn't secret either, only the password
///   genuinely needs re-entry; `username` is the root's own `display_name`,
///   which for a real Jellyfin login IS the login name, not a separate
///   display-only field — `UserDto` only ever has one `Name`), regardless
///   of what either launch policy would otherwise decide.
pub(crate) enum StartupGate {
    AutoLogin,
    ShowAccountPicker,
    ShowProfilePicker(String),
    ShowProfilePickerPin(String, String),
    RequireLogin(String, String),
}

/// Decides what should happen at startup — now a genuine two-tier
/// resolution (2026-08-14, live-reported design feedback: "if there is no
/// bonfire on the other server everyone can use that accaunt as the
/// session is saved... so it shuld be 2 layers? like accaunt then
/// profiles?"). With 0 or 1 known ACCOUNT (not raw profile — a Bonfire
/// household's master + all its sub-profiles together are still exactly
/// one account) there's nothing to pick between at the account tier, so it
/// resolves straight through to whichever profile-tier outcome that one
/// account's own contents produce — every existing single-profile,
/// single-account install behaves exactly as it always has.
///
/// **Tier 1 — account.** With 2+ accounts, `DeviceConfig.
/// account_launch_policy` decides which one (if any) to resolve silently,
/// the identical 3-way shape `launch_policy` already has one tier down:
/// "always_ask" always shows `ShowAccountPicker`; "remember_last" resolves
/// to whichever account currently contains `Config.active_profile_id`;
/// "default" resolves `DeviceConfig.default_account_id` instead. Either
/// way the resolved account's root must have a valid stored token, or the
/// account picker shows regardless.
///
/// **`remember_login` gate — checked the instant an account resolves,
/// before ever touching tier 2.** Live-reported directly, the core
/// motivating concern for this whole feature: a plain (non-Bonfire)
/// account has no PIN concept at all, so if the picker ever let it be
/// silently resumed alongside a PIN-protected household, it's a built-in
/// bypass around every PIN in that household — anyone can just pick the
/// unprotected account instead. `ProfileSettings.remember_login` (default
/// `true`, so no existing install's behavior changes unless explicitly
/// turned off) is checked on the resolved account's ROOT profile; `false`
/// returns `RequireLogin(server_url)` unconditionally, before any
/// profile-tier logic runs at all — a full password re-entry, not a PIN,
/// since a plain account has no PIN to fall back on.
///
/// **Tier 2 — profile, scoped to the resolved account only.** Exactly the
/// same `launch_policy`/`has_pin` logic this function already had before
/// this change (see the real PIN-bypass bug this closed, same day, earlier
/// in this file's history) — just now operating over `account.profiles`
/// instead of the flat `cfg.profiles`. A single-profile account (the
/// common case — a plain login, or a Bonfire master with no sub-profiles
/// configured yet) skips straight through with only its own `has_pin`
/// checked; a multi-profile account applies `launch_policy`/
/// `default_profile_id` exactly as before.
pub(crate) fn should_show_picker_at_startup(cfg: &mut crate::config::Config) -> StartupGate {
    let accounts = group_into_accounts(&cfg.profiles);
    if accounts.is_empty() {
        return StartupGate::AutoLogin; // nothing saved at all — the ordinary "no session" path handles this, not this gate
    }

    let account = if accounts.len() < 2 {
        accounts.into_iter().next()
    } else {
        match cfg.device.account_launch_policy.as_str() {
            "remember_last" => accounts.into_iter()
                .find(|a| a.profiles.iter().any(|p| p.user_id == cfg.active_profile_id))
                .filter(|a| a.profiles.iter().find(|p| p.user_id == a.root_id).is_some_and(|r| !r.token.is_empty())),
            "default" => {
                let target = cfg.device.default_account_id.clone();
                accounts.into_iter()
                    .find(|a| a.root_id == target)
                    .filter(|a| a.profiles.iter().find(|p| p.user_id == a.root_id).is_some_and(|r| !r.token.is_empty()))
            }
            // "always_ask" and any unrecognized value fail safe to asking.
            _ => None,
        }
    };
    let Some(account) = account else {
        return StartupGate::ShowAccountPicker;
    };

    let Some(root) = account.profiles.iter().find(|p| p.user_id == account.root_id) else {
        // Structurally shouldn't happen (see AccountGroup's own doc comment)
        // but fail safe to the account picker rather than a panic/unwrap.
        return StartupGate::ShowAccountPicker;
    };
    if !root.remember_login {
        // Set active_profile_id to this account's root NOW, even though no
        // session starts yet — do_login's non-append branch (what the
        // resulting LoginScreen uses, see main.rs's own RequireLogin arm)
        // writes into cfg.active_mut(), and that has to already resolve to
        // THIS account's entry so a successful re-login updates it in
        // place instead of some unrelated previously-active profile.
        cfg.active_profile_id = root.user_id.clone();
        return StartupGate::RequireLogin(root.server_url.clone(), root.display_name.clone());
    }

    // Account resolved and remembered — resolve the PROFILE within it now,
    // via the identical per-profile launch_policy logic as before, scoped
    // to just this account's own members.
    if account.profiles.len() < 2 {
        if root.has_pin {
            return StartupGate::ShowProfilePickerPin(account.root_id.clone(), root.user_id.clone());
        }
        cfg.active_profile_id = root.user_id.clone();
        return StartupGate::AutoLogin;
    }
    match cfg.device.launch_policy.as_str() {
        "remember_last" => {
            let target = account.profiles.iter().find(|p| p.user_id == cfg.active_profile_id).cloned();
            match target {
                Some(t) if t.token.is_empty() => StartupGate::ShowProfilePicker(account.root_id.clone()),
                Some(t) if t.has_pin => StartupGate::ShowProfilePickerPin(account.root_id.clone(), t.user_id),
                Some(t) => { cfg.active_profile_id = t.user_id.clone(); StartupGate::AutoLogin }
                None => StartupGate::ShowProfilePicker(account.root_id.clone()),
            }
        }
        "default" => {
            let target_id = cfg.device.default_profile_id.clone();
            let target = account.profiles.iter().find(|p| p.user_id == target_id && !p.token.is_empty()).cloned();
            match target {
                Some(t) if t.has_pin => StartupGate::ShowProfilePickerPin(account.root_id.clone(), t.user_id),
                Some(t) => { cfg.active_profile_id = t.user_id.clone(); StartupGate::AutoLogin }
                None => StartupGate::ShowProfilePicker(account.root_id.clone()),
            }
        }
        _ => StartupGate::ShowProfilePicker(account.root_id.clone()),
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
/// Always scoped to one ACCOUNT's own profiles now (2026-08-14, the
/// 2-tier account/profile redesign — see `should_show_picker_at_startup`'s
/// own doc comment for the full design). `account_root_id` is
/// `AccountGroup.root_id` — the account's own root `user_id` — never the
/// unscoped flat `Config.profiles` list anymore, even for the common
/// single-account case (which is simply the one account this function was
/// called with). `profile-picker-show-back-to-accounts` is set whenever
/// 2+ distinct accounts exist at all, independent of whether the account
/// tier was actually shown on the way here — a policy-resolved direct
/// skip to this account still leaves other accounts reachable via the
/// back button, not just ones reached by explicitly picking through the
/// account tier first.
pub(crate) fn open_profile_picker(
    state: &Arc<Mutex<FjordState>>, window: &MainWindow, cancelable: bool, account_root_id: &str,
) {
    let (profiles, show_back): (Vec<ProfileTile>, bool) = {
        let s = state.lock().unwrap();
        let accounts = group_into_accounts(&s.config.profiles);
        let profiles = accounts.iter()
            .find(|a| a.root_id == account_root_id)
            .map(|a| a.profiles.iter().map(build_tile).collect())
            .unwrap_or_default();
        (profiles, accounts.len() >= 2)
    };
    let g = AppState::get(window);
    g.set_profile_picker_profiles(ModelRc::new(VecModel::from(profiles)));
    g.set_profile_picker_cursor(0);
    g.set_profile_picker_error(ss(""));
    g.set_profile_picker_loading(false);
    g.set_profile_picker_cancelable(cancelable);
    g.set_profile_picker_show_back_to_accounts(show_back);
    g.set_profile_picker_account_root_id(ss(account_root_id));
    g.set_show_profile_pin_entry(false);
    g.set_show_account_picker(false);
    g.set_show_login(false);
    g.set_show_profile_picker(true);
    grab_focus_deferred(window);
}

/// Startup-gate variant of `open_profile_picker` (2026-08-14, the PIN-bypass
/// fix — see `should_show_picker_at_startup`'s own doc comment for the real
/// bug this closes). Shows the exact same tile grid (scoped to
/// `account_root_id`, same as `open_profile_picker`), non-cancelable
/// (there's no live session yet to cancel back to — matches the plain
/// startup picker), but with the cursor pre-set to `user_id` and the PIN
/// modal already open, instead of making the user re-select a profile the
/// launch policy had already resolved. Mirrors `on_profile_picker_select`'s
/// own PIN-open field sequence exactly, since that's the only other place
/// this modal gets opened from.
pub(crate) fn open_profile_picker_with_pin(
    state: &Arc<Mutex<FjordState>>, window: &MainWindow, account_root_id: &str, user_id: &str,
) {
    open_profile_picker(state, window, false, account_root_id);
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
        "Switch Profile" => open_switch_entry_point(state, window),
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

/// Account-tier picker (2026-08-14, the 2-tier account/profile redesign).
/// Shown only when 2+ distinct accounts exist at all — a single-account
/// install never reaches this screen via the startup gate (see
/// `should_show_picker_at_startup`); it's still reachable mid-session via
/// the sidebar's "Switch Profile" action even with just 1 known account,
/// so there's always a way back to it once a second one exists.
pub(crate) fn open_account_picker(state: &Arc<Mutex<FjordState>>, window: &MainWindow, cancelable: bool) {
    let accounts: Vec<crate::AccountTile> = {
        let s = state.lock().unwrap();
        group_into_accounts(&s.config.profiles).iter().map(build_account_tile).collect()
    };
    let g = AppState::get(window);
    g.set_account_picker_accounts(ModelRc::new(VecModel::from(accounts)));
    g.set_account_picker_cursor(0);
    g.set_account_picker_error(ss(""));
    g.set_account_picker_loading(false);
    g.set_account_picker_cancelable(cancelable);
    g.set_show_profile_picker(false);
    g.set_show_login(false);
    g.set_show_account_picker(true);
    grab_focus_deferred(window);
}

/// "Switch Profile" from the sidebar quick-menu (2026-08-14) — smart entry
/// point mirroring the startup gate's own tier-skip logic: only 1 known
/// account (whatever's actually being used right now) → go straight to
/// ITS profile-tier picker (nothing to choose at the account tier, same
/// reasoning `should_show_picker_at_startup` already applies); 2+ accounts
/// → the account-tier picker, so a live-session switch can also jump to a
/// completely different account, not just another profile within the
/// current one.
pub(crate) fn open_switch_entry_point(state: &Arc<Mutex<FjordState>>, window: &MainWindow) {
    let accounts = { let s = state.lock().unwrap(); group_into_accounts(&s.config.profiles) };
    if accounts.len() < 2 {
        if let Some(only) = accounts.into_iter().next() {
            open_profile_picker(state, window, true, &only.root_id);
        }
    } else {
        open_account_picker(state, window, true);
    }
}

/// Picking an account tile — resolves straight through to a switch (single-
/// profile account, no PIN), the PIN modal (single-profile account, has a
/// PIN), or the profile-tier picker scoped to this account (2+ profiles),
/// mirroring `should_show_picker_at_startup`'s own tier-2 resolution
/// exactly, just triggered by a click instead of the startup gate. The
/// `remember_login` gate is intentionally NOT re-checked here — reaching
/// the account picker at all only ever happens via an explicit user
/// action (this tile, or "Switch Profile"), which is itself already the
/// "I want to actively pick something" moment `remember_login` exists to
/// skip past when caller is silent/automatic; a user who deliberately
/// clicks an account tile is not the silent-bypass scenario that setting
/// guards against.
pub(crate) fn on_account_picker_select(
    state: &Arc<Mutex<FjordState>>, video: &Arc<Mutex<VideoState>>, window: &MainWindow,
    rt: &tokio::runtime::Handle, root_id: SharedString,
) {
    let g = AppState::get(window);
    if g.get_account_picker_loading() { return; }
    let group = {
        let s = state.lock().unwrap();
        group_into_accounts(&s.config.profiles).into_iter().find(|a| a.root_id == root_id.as_str())
    };
    let Some(group) = group else {
        g.set_account_picker_error(ss("That account is no longer available"));
        return;
    };
    if group.profiles.len() < 2 {
        let Some(root) = group.profiles.into_iter().next() else { return };
        if root.has_pin {
            open_profile_picker_with_pin(state, window, &root.user_id, &root.user_id);
        } else {
            g.set_account_picker_loading(true);
            switch_to_profile(Arc::clone(state), Arc::clone(video), window.as_weak(), rt.clone(), root.user_id, None);
        }
    } else {
        open_profile_picker(state, window, g.get_account_picker_cancelable(), &group.root_id);
    }
}

pub(crate) fn on_account_picker_add_account(window: &MainWindow) {
    let g = AppState::get(window);
    g.set_login_append_mode(true);
    g.set_login_append_source(ss("account_picker"));
    // Clear any stale RequireLogin prefill (2026-08-15) — Add Account is for
    // a genuinely NEW/different account; a leftover server/username from an
    // earlier remember_login==false re-prompt this session must not silently
    // apply here too.
    g.set_login_server_prefill(ss(""));
    g.set_login_username_prefill(ss(""));
    g.set_show_account_picker(false);
    g.set_show_login(true);
    g.set_status(ss(""));
    window.invoke_grab_keyboard_focus();
}

/// Reachable from Settings → Profiles too (2026-08-14) — always available
/// regardless of how many accounts already exist, unlike the picker tiles
/// (which only show once there's already a 2nd account to switch between).
/// This is the actual way to go from 1 known account to 2 in the first
/// place. `login_append_source` stays empty here (not "account_picker"),
/// so cancelling just returns to Settings, not a picker screen.
pub(crate) fn on_settings_add_account(window: &MainWindow) {
    let g = AppState::get(window);
    g.set_login_append_mode(true);
    g.set_login_append_source(ss(""));
    // Same stale-prefill guard as on_account_picker_add_account above.
    g.set_login_server_prefill(ss(""));
    g.set_login_username_prefill(ss(""));
    g.set_show_login(true);
    g.set_status(ss(""));
    window.invoke_grab_keyboard_focus();
}

pub(crate) fn on_cancel_add_account(state: &Arc<Mutex<FjordState>>, window: &MainWindow) {
    let g = AppState::get(window);
    g.set_login_append_mode(false);
    g.set_show_login(false);
    // Opened from the account-tier picker's own "+ Add Account" tile —
    // account-picker-cancelable is an in-out property that outlives the
    // picker being hidden, carrying forward whatever it was opened with,
    // same idiom the profile picker's own cancelable flag uses. Anything
    // else (Settings → Profiles' own "Add Account" row) has nothing to
    // reopen — the live session/Settings screen underneath was never
    // touched.
    if g.get_login_append_source().as_str() == "account_picker" {
        open_account_picker(state, window, g.get_account_picker_cancelable());
    }
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
                    // 2026-08-14, the 2-tier redesign — a switch can now also
                    // be initiated straight from the account-tier picker (a
                    // single-profile account's own tile), which has its own,
                    // separate loading/error properties. Clearing both
                    // unconditionally is harmless for whichever screen isn't
                    // actually showing (setting a property on a hidden
                    // screen has no visible effect) and correct for
                    // whichever one is, regardless of which one triggered
                    // this call.
                    g.set_account_picker_loading(false);
                    if let Some(msg) = msg {
                        g.set_profile_pin_error(ss(&msg));
                        g.set_profile_picker_error(ss(&msg));
                        g.set_account_picker_error(ss(&msg));
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
        // Real bug, live-reported 2026-08-15 ("switched to a sub-profile,
        // closed and reopened Fjord — the sub-profile now showed up as its
        // own separate account, with its former siblings grouped under it
        // instead of the real master"): this function is called
        // unconditionally after EVERY successful session start (see the
        // doc comment above the type), with no check that the calling
        // session is actually the master — but the loop below (still a few
        // lines down) unconditionally used client.user_id, whichever
        // profile is CURRENTLY active, as the master_user_id to write onto
        // every profile Bonfire's /list reports. Called as a sub-profile
        // rather than the true master, that silently reparents the whole
        // household under whichever sub-profile most recently logged in —
        // config.rs's load_config gained a matching repair pass for a
        // config.json already corrupted by this before the fix.
        //
        // Guard: only proceed if the LOCAL profile entry for this exact
        // session is known and is NOT itself a Bonfire sub-profile — a
        // genuine master session (a brand-new first-ever login included;
        // by the time this runs, finish_session_setup has already upserted
        // a local entry defaulting is_bonfire=false) passes; a session
        // that's itself a sub-profile is skipped before ever calling the
        // API at all, both preventing the corruption and skipping a
        // network round trip that would only get thrown away anyway.
        {
            let s = state.lock().unwrap();
            if s.config.profiles.iter().any(|p| p.user_id == client.user_id && p.is_bonfire) {
                tracing::debug!("sync_bonfire_subprofiles: skipping — this session ({}) is itself a Bonfire sub-profile, not a master", client.user_id);
                return;
            }
        }
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
