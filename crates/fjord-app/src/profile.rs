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
//                       primitive — sorts each group's own profiles root-first; account_root_id
//                       returns own user_id for a plain account OR a group account
//                       (is_group_account, Bonfire Phase 5), else master_user_id for a genuine
//                       sub-profile
//   is_true_master      (Bonfire Phase 5) !is_bonfire || is_group_account — true when this
//                       session has independent master authority, whether never
//                       Bonfire-discovered at all or actively impersonating a foreign group
//                       account (a "fully privileged session," per Bonfire's own docs); replaces
//                       every prior bare is_bonfire/!is_bonfire authority check in this app
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
//                       AppState.profile-picker-sections ([ProfileSection], sectioned by
//                       household, 2026-08-31 — see build_profile_sections/linked_account_roots)
//                       from just that account's own Config.profiles entries plus any
//                       Bonfire-linked households (local data only — no live
//                       bonfire_list_profiles() refresh attempted here; see
//                       sync_bonfire_subprofiles for where that happens), sets
//                       profile-picker-show-back-to-accounts (true iff 2+ accounts exist)
//   open_profile_picker_with_pin  same, jumping straight into PIN entry for one known profile
//   open_account_picker  builds AppState.account-picker-accounts from group_into_accounts(),
//                       shows the account-tier screen
//   account_requires_login / require_login_for_account  (2026-08-16, code review) the
//                       remember_login==false gate, checked at the TOP of both
//                       on_account_picker_select and on_profile_picker_select — previously
//                       existed ONLY in should_show_picker_at_startup, silently bypassable by
//                       reaching the same account through a live-session picker instead;
//                       require_login_for_account mirrors StartupGate::RequireLogin's own
//                       dispatch exactly (server/username prefill, active_profile_id re-pointed
//                       at the account root, login-remember set to false reflecting the
//                       account's own already-known value)
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
//                       the account tier with no profile-picker step in between at all; also
//                       clears profile_pin_buffer/-len on every failure path (2026-08-16, code
//                       review — previously only cleared on success, contradicting the field's
//                       own doc comment), which is why it now takes `state` as a parameter
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
//                       the sidebar quick-menu: Switch Profile (2026-08-16, only shown when the
//                       CURRENT account has 2+ profiles — opens open_profile_picker scoped to
//                       that account directly, never the account tier) / Switch Account
//                       (2026-08-16, always shown, opens open_account_picker — the doorway to
//                       "+ Add Account" even with just 1 known account) / Manage Profiles
//                       (master only) / Profile Settings / Sign Out
//   push_current_profile_tile  populates the sidebar's own current-profile avatar+name row;
//                       called from finish_session_setup AND spawn_auto_login (2026-08-14 fix —
//                       previously only the former, so a plain relaunch never showed it)
//   on_remember_login_toggle/-confirm/-confirm_cancel  (2026-08-17) Settings → Profiles →
//                       "Remember this login" — OFF is immediate/local; ON opens a small
//                       standalone confirm-password modal (authenticate_with_fallback directly,
//                       never the full do_login/finish_session_setup pipeline) rather than
//                       flipping the field straight away, per the user's explicit choice
//   wire_idle_lock_timer  (Bonfire Phase 4, inactivity auto-lock, 2026-08-29) 15s repeating
//                       slint::Timer — fires reset_session_state + open_profile_picker_with_pin
//                       (or require_login_for_account, mirroring already_active_account's own
//                       established short-circuit — the auto-locked profile is always the one
//                       currently active, so remember_login's full re-login is never actually
//                       reachable in practice, same as the other two callers of that check)
//                       once FjordState.last_activity_at exceeds the active profile's own
//                       lockout_minutes; only for a Bonfire profile with has_pin && lockout_minutes>0;
//                       treats active non-paused playback as continuous activity rather than a
//                       separate suppression branch
//   sync_bonfire_subprofiles  (Bonfire Phase 5 update) now classifies each /list entry by
//                       bp.is_master — a genuine sub-profile keeps the original shape
//                       (master_user_id = calling client); a foreign master's own account
//                       (is_group_account) gets an EMPTY master_user_id (never
//                       self-referencing — see config.rs::repair_bonfire_profile_corruption's
//                       own doc comment) and synced_via = calling client; self-skip guard and
//                       prune condition both use is_true_master/synced_via accordingly, not
//                       bare is_bonfire/master_user_id
//   open_bonfire_group_screen / on_bonfire_group_generate/-join_submit/-kick/-leave/-delete/
//   -settings_changed / existing_bonfire_group_zones  (Bonfire Phase 5, 2026-08-29) the
//                       "Bonfire Group" screen (Settings → Profiles, master-only via
//                       is_true_master) — generate/join a cross-household group, per-member Kick
//                       (owner) or Leave (member), the 2 sub-profile-visibility toggles + the
//                       allowHouseholdLanBypass grant (gated behind a ConfirmDialog on the
//                       OFF->ON transition, Slint-side). existing_bonfire_group_zones resolves
//                       the D-pad zone list live (differs by owner/member/neither state, "gaps
//                       are fine" idiom, not a fixed enum) — see its own doc comment for the
//                       exact per-state numbering keys.rs's dispatch mirrors.
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{anyhow, bail, Result};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, warn};

use slint::Global;
use crate::config::{save_config, FjordState, ProfileSettings};
use crate::playback::VideoState;
use crate::{AppState, MainWindow, ProfileSection, ProfileTile};

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
/// account root (`is_bonfire == false`, OR `is_group_account == true` —
/// Bonfire Phase 5: a foreign master's own account, reached via a
/// cross-household group, roots ITSELF, never the calling master that
/// discovered it), else its master's `user_id`. Every Bonfire sub-profile
/// in `Config.profiles` is guaranteed to have its master's own entry
/// present too — `sync_bonfire_subprofiles` only ever discovers/adds
/// sub-profiles after a successful MASTER login, so there's no path to an
/// orphan sub-profile with no root in the list.
pub(crate) fn account_root_id(p: &ProfileSettings) -> &str {
    if p.is_bonfire && !p.is_group_account { &p.master_user_id } else { &p.user_id }
}

/// True when this session genuinely has independent master-level authority
/// over its own account — either it was never Bonfire-discovered at all
/// (`!is_bonfire`, the ordinary case), or it's a foreign master's account
/// currently being impersonated via a Bonfire group (`is_group_account`,
/// Phase 5 — Bonfire's own docs: "switching into another master account
/// linked via a Bonfire group returns a fully privileged session for that
/// account"). False only for a genuine sub-profile, which has none of its
/// own. Every site in this codebase that used to read bare `is_bonfire`/
/// `!is_bonfire` to mean "am I a powerless sub-profile" needs this instead
/// now that a session can be `is_bonfire == true` while still having full
/// authority — see this function's own call sites (`sidebar_profile_menu_rows`,
/// `sync_bonfire_subprofiles`'s self-skip guard, `open_manage_profiles_screen`,
/// `open_my_profile_edit_screen`, `open_bonfire_group_screen`,
/// `main.rs`'s `settings_is_master_profile` push).
pub(crate) fn is_true_master(p: &ProfileSettings) -> bool {
    !p.is_bonfire || p.is_group_account
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
        // Real bug, live-reported 2026-08-31 ("but what i shuld still be
        // able to switch to a bonfire master profile with out needing to
        // switch 'accaunt'..."), caught by an independent review pass
        // while designing the fix — this used to be
        // `sort_by_key(|p| p.is_bonfire)` ("root first"), which is WRONG
        // for a household whose root was never independently logged into:
        // in that case the root's own `is_bonfire` is `true` too (a group
        // account), so every member of the household shares the identical
        // sort key and a stable sort can't guarantee the root lands first
        // — whichever entry the sync loop happened to insert first wins.
        // Not merely cosmetic: `should_show_picker_at_startup`'s own
        // single-known-account guard reads `.profiles.first()` to decide
        // whether a resolved account is a group account that must never
        // silently auto-resume — if `.first()` returns a sub-profile
        // instead of the real root, that guard is defeated and a foreign
        // Bonfire-linked master could auto-login at startup with no PIN.
        // `is_true_master(p)` — "is this entry its own account's root" —
        // is the correct key; `!is_true_master(p)` sorts root first.
        members.sort_by_key(|p| !is_true_master(p));
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
        is_group_account: root.is_some_and(|p| p.is_group_account),
        // Scoped to profile_count == 1 — see AccountTile.has_pin's own doc
        // comment in theme.slint for why that's the one case this is
        // unambiguous (a multi-profile account never triggers a PIN prompt
        // directly from this tile; ProfilePickerScreen's own tiles already
        // show this per-profile regardless).
        has_pin: group.profiles.len() == 1 && root.is_some_and(|p| p.has_pin),
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
        // Same underlying bug as group_into_accounts' own sort-key fix
        // above, on the Slint side: the master-ring condition used to
        // read `!tile.is-bonfire`, which never renders on a "pure" group
        // account's own root (its is_bonfire is also true) — is_true_master
        // is the correct "is this tile its section's own root" check.
        is_root:        is_true_master(p),
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
    // Real bug, live-questioned 2026-08-17 ("shuld ev[e]ry accaunt be able
    // [to set a Default Profile] ... shuld it not just be for the default
    // accaunt?"): confirmed via should_show_picker_at_startup's own tier-2
    // resolution — the "default" launch_policy only ever searches WITHIN
    // whichever account tier-1 already resolved to
    // (`account.profiles.iter().find(|p| p.user_id == target_id ...)`), so
    // a default_profile_id pointing at a profile under a DIFFERENT account
    // could never actually resolve — a silently dead setting that just
    // fell through to showing the picker every time, reading as "feels
    // broken." Scoped the option list to ONLY the current Default
    // Account's own profiles, so this cross-account mismatch can no longer
    // even be picked via the UI — Default Account + Default Profile
    // together always describe one consistent (account, profile) pair,
    // regardless of which launch policy happens to be active right now.
    let account_id = cfg.device.default_account_id.clone();
    let labels: Vec<SharedString> = cfg.profiles.iter()
        .filter(|p| !p.user_id.is_empty() && account_root_id(p) == account_id)
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

    // Bonfire Phase 5: a foreign group account must NEVER be silently
    // auto-resumed, in ANY of the three branches below — including the
    // single-known-account short-circuit, which has no other filter at all.
    // Real, if rare, path to a lone group account here: sign-out already
    // removes a group account discovered via the signed-out master
    // (main.rs's own retain now checks `synced_via`, not just
    // `master_user_id`), but this is defense in depth, not a duplicate of
    // that fix — "never auto-resume" should hold structurally, not just
    // because every other removal path happens to be correct today.
    let account = if accounts.len() < 2 {
        accounts.into_iter()
            .next()
            .filter(|a| !a.profiles.first().is_some_and(|r| r.is_group_account))
    } else {
        match cfg.device.account_launch_policy.as_str() {
            "remember_last" => accounts.into_iter()
                .find(|a| a.profiles.iter().any(|p| p.user_id == cfg.active_profile_id))
                .filter(|a| a.profiles.iter().find(|p| p.user_id == a.root_id)
                    .is_some_and(|r| !r.token.is_empty() && !r.is_group_account)),
            "default" => {
                let target = cfg.device.default_account_id.clone();
                accounts.into_iter()
                    .find(|a| a.root_id == target)
                    .filter(|a| a.profiles.iter().find(|p| p.user_id == a.root_id)
                        .is_some_and(|r| !r.token.is_empty() && !r.is_group_account))
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
/// called with).
///
/// `via_account_picker`: real bug, live-reported 2026-08-19 ("if you was in
/// fjord and pressed switch profile you shuld go back to fjord as the same
/// profile you was") — the 2026-08-17 fix (see the retired doc comment this
/// replaced, still in git history) made the "Back" button unconditionally
/// go to the account tier, reasoning that a cold-start picker has no live
/// session to cancel back to (true for THAT case) — but it applied the
/// same behavior to the sidebar's own "Switch Profile" action, which opens
/// this screen DIRECTLY from an already-live session, never through the
/// account tier at all. Both signals are needed together, neither alone is
/// sufficient: `back_mode` is `"accounts"` when `via_account_picker` (you
/// genuinely came from there — go back one level, matching every other
/// screen's own "Back returns to where you came from" convention) OR
/// `!cancelable` (no live session exists at all — cold start, "Accounts"
/// is the only sensible destination, matching the 2026-08-17 fix's own
/// still-correct reasoning for that specific case); otherwise (`cancelable
/// && !via_account_picker` — the sidebar "Switch Profile" case exactly)
/// it's `"cancel"`, closing the picker and keeping whatever profile was
/// already active, the same live session throughout.
/// The set of OTHER account-root `user_id`s linked to `account_root_id` via
/// a Bonfire group (`ProfileSettings.bonfire_linked_roots`, populated by
/// `sync_bonfire_subprofiles`), filtered to only ids that still resolve to
/// a real `AccountGroup` in `cfg.profiles` — defends against staleness (a
/// linked account pruned since the last sync shouldn't produce a broken
/// section).
///
/// Naturally self-limiting, not by an extra guard here: `bonfire_linked_roots`
/// is only ever written onto the SYNCING session's own root entry (see
/// `sync_bonfire_subprofiles`'s own doc comment), so looking this up for a
/// FOREIGN account (e.g. while drilling into someone else's tile from
/// Account Picker) finds an empty/default list — nobody else's session
/// ever populates it locally — so no extra sections are ever incorrectly
/// attached to an unrelated account's own picker.
pub(crate) fn linked_account_roots(cfg: &crate::config::Config, account_root_id: &str) -> Vec<String> {
    let Some(root) = cfg.profiles.iter().find(|p| p.user_id == account_root_id) else {
        return Vec::new();
    };
    let accounts = group_into_accounts(&cfg.profiles);
    root.bonfire_linked_roots.iter()
        .filter(|id| accounts.iter().any(|a| &a.root_id == *id))
        .cloned()
        .collect()
}

/// Builds the `Vec<ProfileSection>` for `open_profile_picker` and for the
/// `sync_bonfire_subprofiles` refresh closure (the ONE place this section
/// list is assembled, shared by both so they can't drift apart). Section 0
/// is `account_root_id`'s own group, full member list including its own
/// root/master tile; then one more section per id from
/// `linked_account_roots`, each ALSO built from that other group's full
/// member list, root/master tile included — a linked household's own
/// master is just as directly clickable as its sub-profiles, which is the
/// entire point of this feature. Header is `""` when there's only one
/// section total (preserves the original unlabeled look); once there's a
/// second section, section 0's own header becomes `"Your Bonfire"`,
/// matching Bonfire's own reference "Who's Watching?" screen wording.
fn build_profile_sections(cfg: &crate::config::Config, account_root_id: &str) -> Vec<ProfileSection> {
    let accounts = group_into_accounts(&cfg.profiles);
    let Some(primary) = accounts.iter().find(|a| a.root_id == account_root_id) else {
        return Vec::new();
    };
    let mut groups: Vec<(String, &AccountGroup)> = vec![(String::new(), primary)];
    for id in linked_account_roots(cfg, account_root_id) {
        if let Some(a) = accounts.iter().find(|a| a.root_id == id) {
            let name = a.profiles.first().map(|p| {
                if p.display_name.is_empty() { p.user_id.clone() } else { p.display_name.clone() }
            }).unwrap_or_default();
            groups.push((format!("{name}'s Bonfire"), a));
        }
    }
    let multi = groups.len() > 1;
    groups.into_iter().enumerate().map(|(i, (mut header, group))| {
        if multi && i == 0 { header = "Your Bonfire".to_string(); }
        ProfileSection {
            header: ss(&header),
            tiles: ModelRc::new(VecModel::from(group.profiles.iter().map(build_tile).collect::<Vec<_>>())),
        }
    }).collect()
}

pub(crate) fn open_profile_picker(
    state: &Arc<Mutex<FjordState>>, window: &MainWindow, cancelable: bool, via_account_picker: bool, account_root_id: &str,
) {
    let sections: Vec<ProfileSection> = {
        let s = state.lock().unwrap();
        build_profile_sections(&s.config, account_root_id)
    };
    // See open_account_picker's own doc comment for why this was added.
    tracing::debug!(
        "open_profile_picker(account_root_id={account_root_id}): {} section(s) — {}",
        sections.len(),
        sections.iter().map(|sec| {
            let header = if sec.header.is_empty() { "<unlabeled>" } else { sec.header.as_str() };
            format!("{header}({} tile(s))", sec.tiles.row_count())
        }).collect::<Vec<_>>().join(", "),
    );
    let g = AppState::get(window);
    g.set_profile_picker_sections(ModelRc::new(VecModel::from(sections)));
    g.set_profile_picker_section(0);
    g.set_profile_picker_cursor(0);
    g.set_profile_picker_error(ss(""));
    g.set_profile_picker_loading(false);
    g.set_profile_picker_cancelable(cancelable);
    g.set_profile_picker_back_mode(ss(if via_account_picker || !cancelable { "accounts" } else { "cancel" }));
    g.set_profile_picker_account_root_id(ss(account_root_id));
    g.set_profile_picker_back_focused(false);
    g.set_profile_picker_quit_focused(false);
    g.set_show_profile_pin_entry(false);
    g.set_show_account_picker(false);
    crate::close_login_screen(&g);
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
/// Resolves which `(section, cursor)` a given `user_id` currently sits at
/// within `sections` — the nested-model counterpart to a flat
/// `(0..count).find(...)`, shared by every caller that needs to focus a
/// specific tile by id rather than just an index. Returns `None` if the id
/// isn't present in any section (e.g. it was pruned since the section list
/// was built).
fn find_profile_tile_position(sections: &ModelRc<ProfileSection>, user_id: &str) -> Option<(i32, i32)> {
    for s in 0..sections.row_count() {
        let Some(section) = sections.row_data(s) else { continue };
        for i in 0..section.tiles.row_count() {
            if section.tiles.row_data(i).is_some_and(|t| t.user_id == user_id) {
                return Some((s as i32, i as i32));
            }
        }
    }
    None
}

pub(crate) fn open_profile_picker_with_pin(
    state: &Arc<Mutex<FjordState>>, window: &MainWindow, account_root_id: &str, user_id: &str,
) {
    open_profile_picker(state, window, false, false, account_root_id);
    let target = {
        let s = state.lock().unwrap();
        s.config.profiles.iter().find(|p| p.user_id == user_id).cloned()
    };
    let Some(target) = target else { return };
    let g = AppState::get(window);
    // Real search over the nested sections, not hardcoded to section 0 —
    // in every current caller this always resolves to section 0 in
    // practice (nothing today can target a linked household's profile
    // through the PIN-entry path yet), but the search itself is written to
    // stay correct once it can.
    if let Some((section, cursor)) = find_profile_tile_position(&g.get_profile_picker_sections(), user_id) {
        g.set_profile_picker_section(section);
        g.set_profile_picker_cursor(cursor);
    }
    g.set_profile_pin_target_id(ss(user_id));
    g.set_profile_pin_target_name(ss(&target.display_name));
    g.set_profile_pin_cursor(0);
    g.set_profile_pin_len(0);
    g.set_profile_pin_error(ss(""));
    g.set_profile_pin_cancel_focused(false);
    state.lock().unwrap().profile_pin_buffer.clear();
    g.set_show_profile_pin_entry(true);
}

/// Bonfire Phase 4 (inactivity auto-lock, 2026-08-29) — see this file's own
/// TOC entry for the mechanism summary. Mirrors `wire_nw_timer`'s exact
/// registration shape (home.rs) — a plain repeating `slint::Timer`, wired
/// and then `std::mem::forget`'d in `main()` alongside the other 4
/// periodic timers. Runs on the Slint/UI thread like every `slint::Timer`
/// in this app, so calling `reset_session_state`/
/// `open_profile_picker_with_pin`/`require_login_for_account` directly
/// from this closure is safe — no cross-thread hazard of the kind this
/// project has been bitten by before (`push_coming_up_row`,
/// `switch_to_profile`'s own threading bug).
///
/// Locking discipline is a hard requirement, not incidental: every read is
/// done in a tightly-scoped block that lets the `MutexGuard` drop before
/// calling anything that locks again, mirroring `wire_nw_timer`'s own
/// already-correct idiom (`home.rs`'s `let (due_movies, due_tv) = { let s
/// = state.lock().unwrap(); (...) };`) — a function that itself locks
/// `state`, called from inside a block that already holds that same lock,
/// hangs the whole UI thread with no error and no diagnosable log output
/// (the exact `ensure_discover_watchlist` deadlock this codebase already
/// documents once, elsewhere).
pub(crate) fn wire_idle_lock_timer(
    window_weak: slint::Weak<MainWindow>,
    state:       Arc<Mutex<FjordState>>,
    video:       Arc<Mutex<VideoState>>,
    rt_handle:   tokio::runtime::Handle,
) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(15), move || {
        let Some(w) = window_weak.upgrade() else { return };
        let g = AppState::get(&w);

        // Guard dropped at the end of this statement, before anything else runs.
        let (client_present, cfg) = {
            let s = state.lock().unwrap();
            (s.client.is_some(), s.config.clone())
        };
        if !client_present { return; }
        // A manual "Switch Profile"/"Switch Account" is already in
        // progress (per this app's own design, left fully intact with
        // s.client still Some until a switch actually completes), or a
        // previous firing of this same timer already opened the picker —
        // never redundantly re-fire reset_session_state on top of it.
        if g.get_show_profile_picker() || g.get_show_account_picker() { return; }

        let active = cfg.active();
        if !active.has_pin || active.lockout_minutes <= 0 { return; }

        // Active, non-paused playback continuously counts as activity —
        // implemented as "keep resetting the clock to now while genuinely
        // playing," not a separate skip-the-check branch, matching how
        // music_idle_ticks already treats its own gating condition.
        // Paused playback does NOT suppress the clock: someone stepping
        // away with a movie paused is exactly the scenario a household
        // security lock exists to catch, matching how a phone still locks
        // with an app open and paused.
        if (g.get_is_playing() || g.get_is_audio_playing()) && !g.get_is_paused() {
            state.lock().unwrap().last_activity_at = std::time::Instant::now();
            return;
        }

        let idle_for = state.lock().unwrap().last_activity_at.elapsed();
        if idle_for < std::time::Duration::from_secs(active.lockout_minutes as u64 * 60) { return; }

        let account_root = account_root_id(cfg.active()).to_string();
        let user_id = cfg.active().user_id.clone();
        debug!("wire_idle_lock_timer: locking profile {user_id} after {:.0}s idle (lockout_minutes={})", idle_for.as_secs_f64(), active.lockout_minutes);

        crate::reset_session_state(&video, &w.as_weak(), &rt_handle, &state);

        // Mirrors already_active_account/account_requires_login's own
        // established pairing at on_profile_picker_select/
        // on_account_picker_select — in practice already_active_account is
        // always true here (the auto-locked profile IS the one that was
        // just active), so the require_login_for_account branch is a
        // defensive no-op today, not dead code: it stays correct-by-
        // construction if this function is ever reused for a different
        // profile than "the one currently running."
        if !already_active_account(&state, &account_root) {
            if let Some(root) = {
                let s = state.lock().unwrap();
                account_requires_login(&s.config, &account_root).cloned()
            } {
                require_login_for_account(&state, &w, &account_root, &root);
                state.lock().unwrap().last_activity_at = std::time::Instant::now();
                return;
            }
        }
        open_profile_picker_with_pin(&state, &w, &account_root, &user_id);
        state.lock().unwrap().last_activity_at = std::time::Instant::now();
    });
    timer
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
///
/// "Switch Profile" and "Switch Account" split into two predictable,
/// separately-named rows (2026-08-16, live-reported design feedback:
/// "mabey if there are more than one accaunt you shuld get a switch
/// accaunt too, and the profile switch shuld take you to the profile
/// switcher for the accaunt you are on" + "if you are on an accaunt that
/// dont have any profiles you shuld not see switch profile only switch
/// accaunt") — replaces the old single "Switch Profile" row's "smart"
/// routing (`open_switch_entry_point`, now removed), which conflated two
/// different actions under one name: with 2+ accounts known, clicking
/// "Switch Profile" landed on the ACCOUNT picker, not a profile picker,
/// despite its own name. Now: "Switch Profile" is shown only when the
/// CURRENT account itself has 2+ profiles (not "2+ profiles across every
/// known account," the old — also real — bug: a current account with only
/// itself could still show this row if some OTHER account happened to
/// push the total over 2, and selecting it would then confusingly jump to
/// the account tier instead of doing anything within the current
/// account), and always opens the profile picker scoped to the CURRENT
/// account specifically, never the account tier. "Switch Account" is
/// unconditionally shown (not gated on 2+ accounts already existing) —
/// per the user's own explicit ask, it's the direct doorway to the
/// account picker screen even with just one known account, since that
/// screen's own "+ Add Account" tile is the way to add a second one in
/// the first place.
pub(crate) fn sidebar_profile_menu_rows(cfg: &crate::config::Config) -> Vec<&'static str> {
    let mut rows = Vec::with_capacity(6);
    let accounts = group_into_accounts(&cfg.profiles);
    let current_root = account_root_id(cfg.active());
    let current_profile_count = accounts.iter()
        .find(|a| a.root_id == current_root)
        .map(|a| a.profiles.len())
        .unwrap_or(1);
    // Bonfire Phase 5 follow-up (2026-08-31) — a single-profile household
    // that's linked to someone else via a Bonfire group still needs a way
    // to reach them without a separate "Switch Account" step, so this row
    // now also shows whenever a linked account exists, not just when the
    // current household itself has 2+ profiles.
    if current_profile_count >= 2 || !linked_account_roots(cfg, current_root).is_empty() {
        rows.push("Switch Profile");
    }
    rows.push("Switch Account");
    // Bonfire Phase 5: `is_true_master`, not bare `is_bonfire` — a session
    // actively impersonating a foreign group account also has `is_bonfire
    // == true` on its own local entry, but it's "a fully privileged
    // session for that account" per Bonfire's own docs, genuinely a master
    // in its own right, and should see these rows too.
    if is_true_master(cfg.active()) {
        rows.push("Manage Profiles");
        // A master can already edit any of ITS sub-profiles via Manage
        // Profiles above, but never its own — Bonfire's `/update` targets
        // a profileId, and nothing in this codebase (until this row) ever
        // resolved the master's own id and opened the edit screen for it.
        // A sub-profile genuinely can't self-manage at all (real 401 from
        // Bonfire's own API for a non-master token, confirmed live via
        // the plugin's own developer-api.md) — this is why this row is
        // gated identically to Manage Profiles rather than always shown.
        // 2026-08-17, live-questioned ("shuld they not be able to
        // changepin etc on there own profile?"), confirmed via
        // AskUserQuestion: "Add a separate 'Edit My Profile' entry point."
        rows.push("Edit My Profile");
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
        "Switch Profile" => {
            let (root_id, client) = {
                let s = state.lock().unwrap();
                (account_root_id(s.config.active()).to_string(), s.client.clone())
            };
            // via_account_picker=false — reached straight from a live
            // session, never through the account tier (see
            // open_profile_picker's own doc comment for the bug this
            // distinction fixes).
            open_profile_picker(state, window, true, false, &root_id);
            // Real gap, live-reported 2026-08-21 ("It did get removed from
            // the manage profiles but not the switch profiles") — see
            // sync_bonfire_subprofiles' own doc comment for the full bug.
            // Kicks off a fresh sync now that the picker is open, so a
            // server-side deletion made since the last login/switch is
            // reflected without needing a full session restart.
            if let Some(client) = client {
                sync_bonfire_subprofiles(client, Arc::clone(state), rt.clone(), window.as_weak());
            }
        }
        "Switch Account" => {
            open_account_picker(state, window, true);
            let client = state.lock().unwrap().client.clone();
            if let Some(client) = client {
                sync_bonfire_subprofiles(client, Arc::clone(state), rt.clone(), window.as_weak());
            }
        }
        "Manage Profiles" => crate::profile_edit::open_manage_profiles_screen(state, window, rt),
        "Edit My Profile" => crate::profile_edit::open_my_profile_edit_screen(state, window, rt),
        "Profile Settings" => {
            g.set_show_browse(false);
            g.set_show_library(false);
            g.set_active_nav(10);
            g.invoke_nav_selected(10);
            g.set_focused_section(-1);
            g.set_settings_section(ss("profiles"));
            g.set_settings_focused(ss(""));
        }
        // Confirmation dialog, 2026-08-22 — see show-sign-out-confirm's own
        // doc comment in app_state.slint (this is one of its 3 trigger
        // sites, alongside Settings' own row and OfflineScreen's Change
        // Server button). set_show_sidebar_profile_menu(false) already ran
        // at the top of this function, so there's no z-order conflict with
        // the (later-declared, higher z-order) global dialog.
        "Sign Out" => {
            g.set_sign_out_confirm_focused(0);
            g.set_show_sign_out_confirm(true);
        }
        _ => {}
    }
}

/// Resolves whether reaching this ACCOUNT (any profile within it — the
/// root, or a Bonfire sub-profile switching via the root's own stored
/// token) must force a full password re-login instead of ever silently
/// switching, per `remember_login` checked on the account's ROOT profile
/// — the exact rule `should_show_picker_at_startup` already enforces at
/// tier 1, before any profile-tier logic runs.
///
/// Real bug, code-review 2026-08-16: this gate previously existed ONLY in
/// `should_show_picker_at_startup` — `on_account_picker_select`/
/// `on_profile_picker_select` (reachable any time via the sidebar's
/// "Switch Profile"/"Switch Account", not just at cold launch) never
/// checked it at all, silently resuming a `remember_login == false`
/// account from its stored token the instant its tile was clicked.
/// config.rs's own doc comment for the field states it applies
/// "regardless of what account/profile launch policy would otherwise
/// decide" — that word "regardless" was never actually true until this
/// fix, since the account picker is realistically the MOST likely way
/// such an account is ever opened (it can't auto-resolve at startup by
/// definition). Returns the root's own `(server_url, display_name)` when
/// a forced re-login is required, `None` when the switch may proceed.
fn account_requires_login<'a>(cfg: &'a crate::config::Config, account_root_id: &str) -> Option<&'a ProfileSettings> {
    let root = cfg.profiles.iter().find(|p| p.user_id == account_root_id)?;
    // Bonfire Phase 5: a group account can never trigger this — there is no
    // independent password for it to re-check in the first place (Fjord
    // never authenticates one directly; it's always switched into via one
    // of my own accounts' token + Bonfire's own PIN, if the target has
    // one). `remember_login` is never explicitly set for a group account
    // (see that field's own doc comment) so this is normally already a
    // no-op — but a user who happens to toggle "Remember this login" while
    // actively impersonating a group account (its own local entry IS the
    // active `cfg.active_mut()` in that state) would otherwise force a
    // real-password `RequireLogin` next time, which Fjord has no way to
    // satisfy for it at all.
    if root.is_group_account { return None; }
    (!root.remember_login).then_some(root)
}

/// Real bug, live-reported 2026-08-18: "when remember this login [is off]
/// and try to change a profile you get asked for the password, witch
/// shuld not happend as the profile is part of the current loged in
/// accaunt, but if you provied the password you dont go to the profile
/// you chose, you go to the profile you was on." Confirmed from the code:
/// `account_requires_login` was checked unconditionally by both
/// `on_profile_picker_select` and `on_account_picker_select`, with no
/// check for "is this account the one I'm already actively signed into
/// right now" — so switching between two sub-profiles of a household
/// you're ALREADY using, with that household's remember_login off,
/// forced a full re-login every single time, landing you on the
/// account's own ROOT afterward (require_login_for_account only ever
/// remembers the account, not which specific profile was originally
/// clicked) rather than the profile you actually picked — exactly
/// matching both halves of the report. `remember_login` is meant to gate
/// *silently resuming a stored, possibly-stale credential from disk* at
/// startup or from a cold picker — it was never meant to re-demand proof
/// for a session that is, right now, already live and authenticated as
/// this exact account. A live client whose own active profile's account
/// root matches the target is exactly that "already trusted" case.
fn already_active_account(state: &Arc<Mutex<FjordState>>, account_root: &str) -> bool {
    let s = state.lock().unwrap();
    s.client.is_some() && account_root_id(s.config.active()) == account_root
}

/// Shared tail for the `remember_login == false` case above — mirrors
/// `StartupGate::RequireLogin`'s own dispatch in main.rs exactly (server/
/// username prefill, append-mode off, closing whichever picker screen is
/// open), plus `login-remember` set to `false` up front so the checkbox
/// reflects this account's own already-known choice rather than silently
/// resetting to the default `true` and letting a plain re-login flip it
/// back on unless the user notices and re-unchecks it (resolved via
/// AskUserQuestion, 2026-08-16 — "reflect the account's own stored
/// value"). `active_profile_id` is re-pointed at this account's root NOW,
/// same as the startup gate's own RequireLogin arm, so `do_login`'s
/// non-append branch updates the right entry on a successful re-login.
fn require_login_for_account(
    state: &Arc<Mutex<FjordState>>, window: &MainWindow, account_root_id: &str, root: &ProfileSettings,
) {
    let (server_url, username) = (root.server_url.clone(), root.display_name.clone());
    state.lock().unwrap().config.active_profile_id = account_root_id.to_string();
    let g = AppState::get(window);
    g.set_login_server_prefill(ss(&server_url));
    g.set_login_username_prefill(ss(&username));
    g.set_login_append_mode(false);
    g.set_login_append_source(ss(""));
    g.set_login_remember(false);
    g.set_show_profile_picker(false);
    g.set_show_account_picker(false);
    g.set_show_login(true);
    window.invoke_grab_keyboard_focus();
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
    let account_root = account_root_id(&target).to_string();
    if !already_active_account(state, &account_root) {
        if let Some(root) = {
            let s = state.lock().unwrap();
            account_requires_login(&s.config, &account_root).cloned()
        } {
            require_login_for_account(state, window, &account_root, &root);
            return;
        }
    }
    if target.has_pin {
        g.set_profile_pin_target_id(user_id.clone());
        g.set_profile_pin_target_name(ss(&target.display_name.clone()));
        g.set_profile_pin_cursor(0);
        g.set_profile_pin_len(0);
        g.set_profile_pin_error(ss(""));
        g.set_profile_pin_cancel_focused(false);
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
    // Debug logging added 2026-08-30, live-reported ("i cant switch to
    // profiles from the bonfire groupe" / "tests profiles from antons
    // session" / "the tile/option isn't there at all") — this function and
    // on_account_picker_select/open_profile_picker had zero logging at
    // all, so a report like this couldn't be diagnosed from fjord.log the
    // way every other Bonfire report in this codebase has been.
    tracing::debug!(
        "open_account_picker: {} account(s) — {}",
        accounts.len(),
        accounts.iter()
            .map(|a| format!("{}(root={}, n={})", a.display_name, a.root_id, a.profile_count))
            .collect::<Vec<_>>().join(", "),
    );
    let g = AppState::get(window);
    g.set_account_picker_accounts(ModelRc::new(VecModel::from(accounts)));
    g.set_account_picker_cursor(0);
    g.set_account_picker_error(ss(""));
    g.set_account_picker_loading(false);
    g.set_account_picker_cancelable(cancelable);
    g.set_account_picker_quit_focused(false);
    g.set_account_picker_back_focused(false);
    g.set_show_profile_picker(false);
    crate::close_login_screen(&g);
    g.set_show_account_picker(true);
    grab_focus_deferred(window);
}

/// Picking an account tile — resolves straight through to a switch (single-
/// profile account, no PIN), the PIN modal (single-profile account, has a
/// PIN), or the profile-tier picker scoped to this account (2+ profiles),
/// mirroring `should_show_picker_at_startup`'s own tier-2 resolution
/// exactly, just triggered by a click instead of the startup gate.
///
/// **Real bug, fixed 2026-08-16 (code review)**: this function's own doc
/// comment used to argue the `remember_login` gate was "intentionally NOT
/// re-checked here," reasoning that a deliberate click isn't the silent-
/// bypass scenario the setting guards against. That reasoning didn't
/// survive contact with `config.rs`'s own unconditional contract for the
/// field ("regardless of what account/profile launch policy would
/// otherwise decide") — and the account picker is realistically the
/// single most likely way a `remember_login == false` account is ever
/// reached at all, precisely because it can never auto-resolve at
/// startup. Now checked via `account_requires_login` BEFORE either
/// branch below (single-profile shortcut or profile-tier picker) — a
/// forced-login account shows Login immediately, never the PIN modal and
/// never the profile-tier picker.
pub(crate) fn on_account_picker_select(
    state: &Arc<Mutex<FjordState>>, video: &Arc<Mutex<VideoState>>, window: &MainWindow,
    rt: &tokio::runtime::Handle, root_id: SharedString,
) {
    let g = AppState::get(window);
    if g.get_account_picker_loading() { return; }
    debug!("on_account_picker_select(root_id={root_id}): clicked");
    let group = {
        let s = state.lock().unwrap();
        group_into_accounts(&s.config.profiles).into_iter().find(|a| a.root_id == root_id.as_str())
    };
    let Some(group) = group else {
        debug!("on_account_picker_select({root_id}): no matching account group found");
        g.set_account_picker_error(ss("That account is no longer available"));
        return;
    };
    if !already_active_account(state, &group.root_id) {
        if let Some(root) = {
            let s = state.lock().unwrap();
            account_requires_login(&s.config, &group.root_id).cloned()
        } {
            debug!("on_account_picker_select({root_id}): remember_login==false, requiring fresh login");
            require_login_for_account(state, window, &group.root_id, &root);
            return;
        }
    }
    debug!("on_account_picker_select({root_id}): {} profile(s) in group", group.profiles.len());
    if group.profiles.len() < 2 {
        let Some(root) = group.profiles.into_iter().next() else { return };
        if root.has_pin {
            open_profile_picker_with_pin(state, window, &root.user_id, &root.user_id);
        } else {
            g.set_account_picker_loading(true);
            switch_to_profile(Arc::clone(state), Arc::clone(video), window.as_weak(), rt.clone(), root.user_id, None);
        }
    } else {
        // via_account_picker=true — this IS the account tier, so Back
        // should genuinely return here, not skip past it.
        open_profile_picker(state, window, g.get_account_picker_cancelable(), true, &group.root_id);
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
    // Real bug, code-review 2026-08-16: login-remember was never reset
    // anywhere, so an earlier unchecked "Remember this login" (from a
    // previous RequireLogin re-prompt or Add Account attempt this same
    // session) silently carried over here too. Add Account is always a
    // genuinely new/different account, so it always defaults checked —
    // resolved via AskUserQuestion alongside the RequireLogin-reflects-
    // stored-value fix (see require_login_for_account's own doc comment).
    g.set_login_remember(true);
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
    // Same login-remember reset as on_account_picker_add_account above.
    g.set_login_remember(true);
    g.set_show_login(true);
    g.set_status(ss(""));
    window.invoke_grab_keyboard_focus();
}

pub(crate) fn on_cancel_add_account(state: &Arc<Mutex<FjordState>>, window: &MainWindow) {
    let g = AppState::get(window);
    g.set_login_append_mode(false);
    crate::close_login_screen(&g);
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
        fn clear_loading(state: &Arc<Mutex<FjordState>>, ww: &slint::Weak<MainWindow>, msg: Option<String>) {
            // Real bug, code-review 2026-08-16: this previously never
            // touched profile_pin_buffer/profile-pin-len, contradicting
            // that field's own doc comment ("cleared on every fresh
            // PIN-entry open and on a successful/failed switch attempt
            // alike") — a wrong-PIN attempt left the typed digits in
            // place, so the next confirm appended more digits onto the
            // already-wrong PIN instead of starting clean.
            state.lock().unwrap().profile_pin_buffer.clear();
            let ww = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let g = AppState::get(&w);
                    g.set_profile_picker_loading(false);
                    g.set_profile_pin_len(0);
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
            clear_loading(&state, &ww, Some("That profile is no longer available".to_string()));
            return;
        };

        let resolved: Result<(String, String)> = async {
            if target.is_bonfire {
                // Bonfire Phase 5: a group account's own `master_user_id` is
                // deliberately empty (it roots itself, see
                // ProfileSettings.is_group_account's own doc comment) — the
                // local account to actually authenticate this switch call
                // with is instead whichever of MY OWN accounts discovered
                // it, recorded in `synced_via`. A genuine sub-profile is
                // unaffected — its `master_user_id` still names its real
                // master directly, exactly as before.
                let master_lookup_id: &str =
                    if target.is_group_account { &target.synced_via } else { &target.master_user_id };
                let master = {
                    let s = state.lock().unwrap();
                    s.config.profiles.iter().find(|p| p.user_id == master_lookup_id).cloned()
                };
                let Some(master) = master else {
                    bail!("the master account for this profile isn't signed in on this device");
                };
                let server_url = url::Url::parse(&master.server_url)?;
                let master_client = fjord_api::JellyfinClient::new(
                    server_url, master.user_id.clone(), master.token.clone(), device_id.clone(),
                )?;
                let sw = master_client.bonfire_switch_profile(&target_user_id, pin.as_deref()).await?;
                // Real gap, code-review 2026-08-16: sw.jellyfin_user_id (the
                // server's own statement of which Jellyfin user the minted
                // token actually authenticates as) was deserialized and then
                // silently discarded — the client below is always built
                // with Fjord's own locally-known target_user_id instead.
                // Deliberately NOT switched to trusting sw.jellyfin_user_id
                // instead (this corner of the Bonfire API is flagged
                // elsewhere in this codebase as not yet live-verified
                // against a real server, so a behavior change here would be
                // exactly the kind of untested assumption this project's
                // own standing discipline avoids) — just surfaced, so a
                // genuine mismatch is visible in fjord.log instead of
                // silently unnoticed.
                if !sw.jellyfin_user_id.is_empty() && sw.jellyfin_user_id != target_user_id {
                    warn!(
                        "bonfire_switch_profile({target_user_id}): server returned jellyfin_user_id={:?}, expected {target_user_id:?} — using the requested id anyway",
                        sw.jellyfin_user_id
                    );
                }
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
                // Bonfire's own real rate limit on /switch and /verify-pin
                // (5 failed attempts / 15 min, confirmed live against its
                // developer-api.md) — a raw "429 Too Many Requests" HTTP
                // error string is not something a user can act on; this is
                // reachable both from a manual PIN entry and from
                // wire_idle_lock_timer's own unlock attempt.
                let msg = if crate::is_rate_limited(&e) {
                    "Too many attempts — please wait a few minutes and try again".to_string()
                } else {
                    format!("{e:#}")
                };
                clear_loading(&state, &ww, Some(msg));
                return;
            }
        };
        debug!("switch_to_profile({target_user_id}): token resolved after {:.2}s", started.elapsed().as_secs_f64());

        let server_url = match url::Url::parse(&server_url_str) {
            Ok(u) => u,
            Err(e) => {
                warn!("switch_to_profile: bad server_url {server_url_str:?}: {e}");
                clear_loading(&state, &ww, Some("Something went wrong signing in — try again".to_string()));
                return;
            }
        };
        let client = match fjord_api::JellyfinClient::new(
            server_url.clone(), target_user_id.clone(), token.clone(), device_id.clone(),
        ) {
            Ok(c) => Arc::new(c),
            Err(e) => {
                warn!("switch_to_profile: client build failed: {e}");
                clear_loading(&state, &ww, Some("Something went wrong signing in — try again".to_string()));
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
/// Upserts a local `ProfileSettings` entry per sub-profile the plugin
/// reports, AND prunes any local sub-profile of THIS household that the
/// server no longer reports.
///
/// **Pruning added 2026-08-19, live-reported ("hmm can not manage my test 2
/// profile") — this was originally add-only, deliberately, reasoning that a
/// stale local tile for an already-deleted sub-profile "just fails cleanly
/// at switch time instead." That reasoning didn't hold up: confirmed via a
/// direct, live `GET /plugins/profiles/list` call (same one-off diagnostic
/// technique already established in this codebase) that a real deleted
/// sub-profile ("test 2") was genuinely absent from the server's own
/// response — but its local tile stayed fully visible in the "Who's
/// watching" picker (which reads purely from local `Config.profiles`, no
/// live check) with NO way to ever reach or remove it: Manage Profiles
/// fetches fresh from the server too, so it correctly never showed a tile
/// for it either — a permanent ghost profile, not a clean failure.**
/// Pruning is scoped tightly: only removes a local entry where
/// `is_bonfire && master_user_id == this household's master` AND its
/// `user_id` isn't in the just-reported set (the master's own self-entry
/// is never `is_bonfire`, so it can never match this and is never at
/// risk; a DIFFERENT household's sub-profiles have a different
/// `master_user_id` and are equally untouched). Only runs after a
/// genuinely successful, non-empty `/list` response — the existing
/// early-return guards above (fetch error, empty response) already stop
/// execution before this point, so a transient server hiccup can't cause
/// a false prune.
///
/// **Picker refresh added 2026-08-21, live-reported ("It did get removed
/// from the manage profiles but not the switch profiles").** Pruning
/// itself was always correct — the gap was that `ProfilePickerScreen`'s
/// own tile list is built once, synchronously, from whatever
/// `Config.profiles` holds at the moment it's opened (`open_profile_picker`),
/// and this function is only ever called at session-start time
/// (login/switch) — reopening the picker mid-session (the sidebar's own
/// "Switch Profile"/"Switch Account") never re-ran it, so a deletion made
/// server-side after the last login/switch stayed invisible to the picker
/// indefinitely, unlike Manage Profiles, which always re-fetches directly
/// from the server on every open and so was never affected. `window` (new
/// param, threaded through all 4 call sites) lets this function refresh
/// whichever picker is CURRENTLY showing once the sync completes, mirroring
/// `open_manage_profiles_screen`'s own "show now with what's cached, patch
/// in the fresh list once the fetch lands" shape — the picker itself still
/// opens instantly from local state as before (no added latency), it just
/// no longer needs a full session restart to catch up with the server.
pub(crate) fn sync_bonfire_subprofiles(
    client: Arc<fjord_api::JellyfinClient>,
    state:  Arc<Mutex<FjordState>>,
    rt:     tokio::runtime::Handle,
    window: slint::Weak<MainWindow>,
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
        //
        // Bonfire Phase 5 (cross-household groups): `is_true_master`, not
        // bare `is_bonfire` — a session actively impersonating a foreign
        // group account also has `is_bonfire == true` on its own local
        // entry, but per Bonfire's own docs it's "a fully privileged
        // session for that account," genuinely a master in its own right.
        // The old bare `is_bonfire` check would have wrongly skipped
        // syncing for exactly that case, meaning impersonating someone
        // else's account could never discover THEIR own sub-profiles or
        // group memberships.
        {
            let s = state.lock().unwrap();
            if s.config.profiles.iter().any(|p| p.user_id == client.user_id && !is_true_master(p)) {
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
            let mut linked_roots: Vec<String> = Vec::new();
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
                // Bonfire Phase 5 follow-up (2026-08-31, live-reported
                // "but what i shuld still be able to switch to a bonfire
                // master profile with out needing to switch 'accaunt'
                // thats whats bonfire grouping is for?") — record this
                // entry's own account-root id as "linked to me," used to
                // build extra sections directly into ProfilePickerScreen
                // (see linked_account_roots/open_profile_picker below).
                //
                // Deliberately placed here — UNCONDITIONALLY, right after
                // only the self-entry skip above, and BEFORE the "already
                // a known independent account" skip guard right below —
                // caught as a real bug by an independent review pass
                // before this ever shipped: the whole point of this field
                // is tracking linkage EVEN FOR a household that's ALSO
                // independently known on this device (that's the specific
                // case ProfileSettings.bonfire_linked_roots' own doc
                // comment calls out) — computing this any later, e.g.
                // alongside is_group_account/entry_master_user_id below,
                // would mean the independent-account skip's own `continue`
                // fires first for exactly that case, and the household
                // would silently never appear as a "Switch Profile"
                // section at all.
                let root = if bp.is_master {
                    bp.profile_user_id.clone()
                } else if !bp.master_user_id.is_empty() {
                    bp.master_user_id.clone()
                } else {
                    master_user_id.clone()
                };
                // Real bug, live-reported 2026-08-31 via a screenshot the
                // very first time this shipped: "Your Bonfire" showed up a
                // second time, duplicated, under a bogus "Anton's Bonfire"
                // header — because Bonfire's own /list also returns MY OWN
                // household's sub-profiles (Anso/Akira/Raphael), per
                // GetLinkedMasterUserIds' own {self} ∪ ... union (see this
                // function's own header doc comment) — and every one of
                // those entries resolves `root` to MY OWN master_user_id,
                // which then got recorded as "linked to me" indistinguishably
                // from a genuinely different household. `linked_roots` must
                // only ever name OTHER households' roots — build_profile_sections
                // treats every entry in it as "push one more section," so a
                // root equal to my own duplicates section 0 outright.
                if root != master_user_id && !linked_roots.contains(&root) {
                    linked_roots.push(root);
                }
                // Real bug, live-reported 2026-08-29 ("but anton is also
                // singed in to [this device already]"): a fellow group
                // member's own `/list` reports EVERY master in the group,
                // including one whose account this exact device already
                // has its OWN independent, direct login for (added earlier
                // via a real username/password "+ Add Account", genuinely
                // `is_bonfire: false`, with its own valid token) — before
                // this check, the upsert below unconditionally overwrote
                // such an entry's `is_bonfire`/`is_group_account`/
                // `master_user_id` the instant it was ALSO discovered this
                // way, silently downgrading it from "switch directly with
                // my own real token, no PIN" to "route through the
                // Bonfire-group mechanism, PIN required" — a strict
                // downgrade for an account this device could already reach
                // on its own. Skip the whole upsert for exactly this case;
                // the independent entry is already strictly better than
                // anything this discovery could tell us, and none of its
                // other fields (display_name/avatar/etc.) should be
                // overwritten from Bonfire's sub-profile-listing data
                // either — those are authoritative from the account's own
                // real login/edit path, not this one. Scoped to
                // `bp.is_master` only (i.e. this branch never applies to a
                // genuine sub-profile entry, which can't be independently
                // logged into in the first place).
                if bp.is_master {
                    if let Some(existing) = s.config.profiles.iter().find(|p| p.user_id == bp.profile_user_id) {
                        if !existing.is_bonfire && !existing.token.is_empty() {
                            tracing::debug!("sync_bonfire_subprofiles: skipping {} — already a known independent account on this device", bp.profile_user_id);
                            continue;
                        }
                    }
                }
                // Bonfire Phase 5: `bp.is_master` distinguishes a genuine
                // sub-profile of MY OWN household from another master's own
                // account, reached via a cross-household group — the two
                // need different local classification (see
                // ProfileSettings.is_group_account/synced_via's own doc
                // comments for why: a group account roots ITSELF, with an
                // empty master_user_id, never self-referencing — see
                // repair_bonfire_profile_corruption's own doc comment for
                // the collision that would cause).
                //
                // Real bug, live-reported 2026-08-29 ("i cant chose the
                // profile anton from the profile switcher when i am on test
                // accaunt" — screenshot showing Anso/Akira/Raphael, Anton's
                // OWN sub-profiles, merged into "test"'s account instead):
                // for a genuine sub-profile (`is_master: false`), this used
                // to unconditionally write `master_user_id: master_user_id`
                // — the CALLING client's own id — completely ignoring
                // `bp.master_user_id`, the real field Bonfire's own /list
                // response already provides on every entry stating who its
                // ACTUAL master is. Silently correct before cross-household
                // groups existed (/list could only ever return your own
                // household); once you're in a group with another master,
                // /list also includes THAT master's own sub-profiles
                // (visible to you via the shared group) — and this blindly
                // claimed them as your own. Fixed by trusting bp's own
                // field for a real sub-profile; the calling client's id is
                // only ever the right value for is_group_account (an
                // account roots itself) or as a defensive fallback if a
                // sub-profile entry somehow arrives with no master id at
                // all (shouldn't happen — Bonfire always populates this for
                // a real sub-profile). Self-heals any already-corrupted
                // local data automatically on the next sync that reports
                // the affected profiles, via this same upsert.
                let is_group_account = bp.is_master;
                let entry_master_user_id = if is_group_account {
                    String::new()
                } else if !bp.master_user_id.is_empty() {
                    bp.master_user_id.clone()
                } else {
                    master_user_id.clone()
                };
                if let Some(existing) = s.config.profiles.iter_mut().find(|p| p.user_id == bp.profile_user_id) {
                    existing.display_name   = bp.profile_name.clone();
                    existing.avatar_color   = bp.avatar_color.clone();
                    existing.avatar_initial = bp.avatar_initial.clone();
                    existing.is_bonfire     = true;
                    existing.is_group_account = is_group_account;
                    existing.master_user_id = entry_master_user_id;
                    existing.synced_via     = master_user_id.clone();
                    existing.has_pin        = bp.has_pin;
                    existing.lockout_minutes = bp.lockout_minutes;
                } else {
                    s.config.profiles.push(ProfileSettings {
                        user_id:        bp.profile_user_id.clone(),
                        display_name:   bp.profile_name.clone(),
                        avatar_color:   bp.avatar_color.clone(),
                        avatar_initial: bp.avatar_initial.clone(),
                        is_bonfire:     true,
                        is_group_account,
                        master_user_id: entry_master_user_id,
                        synced_via:     master_user_id.clone(),
                        has_pin:        bp.has_pin,
                        lockout_minutes: bp.lockout_minutes,
                        server_url:     client.server_url.to_string(),
                        ..Default::default()
                    });
                }
            }
            // Replace wholesale, matching the "eventually consistent,
            // replace on each sync" precedent the prune step right below
            // already uses. A direct lookup by `master_user_id`, not
            // `active_mut()` — the latter matches by `active_profile_id`
            // and silently falls back to `.profiles.first()` if that id
            // isn't found; a direct id lookup avoids that silent-wrong-
            // entry risk. `master_user_id == client.user_id` is already
            // guaranteed by this function's own self-entry-skip guard.
            if let Some(me) = s.config.profiles.iter_mut().find(|p| p.user_id == master_user_id) {
                me.bonfire_linked_roots = linked_roots;
            }
            // Prune — see this function's own doc comment for the real bug
            // this closes and exactly what it is/isn't allowed to remove.
            // Bonfire Phase 5: scoped via `synced_via`, not `master_user_id`
            // — a group account's own `master_user_id` is deliberately
            // empty, so the old `master_user_id == master_user_id`
            // condition would never match (and thus never prune) a group
            // account even after leaving its group; `synced_via` still
            // correctly records "this calling client is who discovered it"
            // for both categories.
            let reported: std::collections::HashSet<&str> =
                profiles.iter().map(|bp| bp.profile_user_id.as_str()).collect();
            let before = s.config.profiles.len();
            s.config.profiles.retain(|p| {
                !(p.is_bonfire && p.synced_via == master_user_id && !reported.contains(p.user_id.as_str()))
            });
            let pruned = before - s.config.profiles.len();
            if pruned > 0 {
                tracing::info!("sync_bonfire_subprofiles: pruned {pruned} sub-profile(s) no longer reported by the server");
            }
            s.config.clone()
        };
        save_config(&cfg);

        // Refresh whichever picker is currently open — see this function's
        // own doc comment above for the bug this closes. A no-op for the
        // ordinary session-start call sites (finish_session_setup/
        // spawn_auto_login), since neither picker screen is ever showing at
        // that point — only the sidebar's mid-session "Switch Profile"/
        // "Switch Account" call site can actually have one open while this
        // runs in the background.
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = window.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_show_profile_picker() {
                let root_id = g.get_profile_picker_account_root_id().to_string();
                let sections = build_profile_sections(&cfg, &root_id);
                // Defensive clamp, not a user-id search — this closure never
                // tracked a target id before, and still doesn't. What it
                // newly needs, because moving from a flat list to nested
                // sections introduces a real new failure mode, is this: an
                // out-of-range `profile-picker-section` means the nested
                // `for` loop's `AppState.profile-picker-section == s` check
                // matches nothing at all — the focus ring disappears
                // completely, invisibly, until some key happens to reset
                // it. A live Bonfire resync shrinking the linked-sections
                // count while the picker is open, with focus sitting in
                // the now-gone section, is exactly the scenario this
                // guards against.
                let section_count = sections.len() as i32;
                let clamped_section = g.get_profile_picker_section().clamp(0, (section_count - 1).max(0));
                let tile_count = sections.get(clamped_section as usize).map(|s| s.tiles.row_count() as i32).unwrap_or(0);
                let clamped_cursor = g.get_profile_picker_cursor().clamp(0, (tile_count - 1).max(0));
                g.set_profile_picker_sections(ModelRc::new(VecModel::from(sections)));
                g.set_profile_picker_section(clamped_section);
                g.set_profile_picker_cursor(clamped_cursor);
            }
            if g.get_show_account_picker() {
                let accounts = group_into_accounts(&cfg.profiles);
                let tiles: Vec<crate::AccountTile> = accounts.iter().map(build_account_tile).collect();
                g.set_account_picker_accounts(ModelRc::new(VecModel::from(tiles)));
            }
        });
    });
}

// ── Bonfire Group (Phase 5, cross-household groups, 2026-08-29) ────────────
// See app_state.slint's own doc comment on show-bonfire-group for the
// overall shape, and this module's own doc comment above sync_bonfire_
// subprofiles for the real correctness bug found and fixed while
// designing this feature (is_group_account/synced_via, is_true_master).

fn push_bonfire_group_status(g: &AppState<'_>, status: &fjord_api::models::BonfireGroupStatus) {
    // Debug logging, 2026-08-29 — this whole function had none at all,
    // which left "can't tell which of the 3 screen states is even showing"
    // undiagnosable from a log alone the first time this was live-tested.
    debug!(
        "bonfire_group: status is_owner={} is_member={} owned_code={:?} owned_members={} joined_owner={:?}",
        status.is_owner, status.is_member, status.owned_code,
        status.owned_members.len(), status.joined_owner_name,
    );
    g.set_bonfire_group_is_owner(status.is_owner);
    g.set_bonfire_group_is_member(status.is_member);
    g.set_bonfire_group_owned_code(ss(status.owned_code.as_deref().unwrap_or("")));
    let members: Vec<crate::BonfireGroupMemberTile> = status.owned_members.iter()
        .map(|m| crate::BonfireGroupMemberTile { user_id: ss(&m.user_id), username: ss(&m.username) })
        .collect();
    g.set_bonfire_group_owned_members(ModelRc::new(VecModel::from(members)));
    g.set_bonfire_group_joined_owner_name(ss(status.joined_owner_name.as_deref().unwrap_or("")));
    g.set_bonfire_group_hide_my_sub_profiles(status.hide_my_sub_profiles_from_others);
    g.set_bonfire_group_hide_others_sub_profiles(status.hide_others_sub_profiles_from_me);
    g.set_bonfire_group_allow_lan_bypass(status.allow_household_lan_bypass);
    g.set_bonfire_group_is_administrator(status.is_administrator);
    g.set_bonfire_group_has_pin(status.has_pin);
}

/// `rt.spawn`, not bare `tokio::spawn` — every other async dispatch in this
/// file (`sync_bonfire_subprofiles`, `switch_to_profile`, ...) takes an
/// explicit `tokio::runtime::Handle` for exactly this reason: these
/// functions are invoked directly from Slint callbacks, not from inside an
/// already-running Tokio task, so there's no ambient "current runtime" to
/// spawn onto without one.
fn refresh_bonfire_group_status(
    client: Arc<fjord_api::JellyfinClient>, state: Arc<Mutex<FjordState>>,
    window: slint::Weak<MainWindow>, rt: &tokio::runtime::Handle,
) {
    rt.spawn(async move {
        match client.bonfire_status().await {
            Ok(status) => {
                if !crate::session_current(&state, &client) { return; }
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = window.upgrade() else { return };
                    let g = AppState::get(&w);
                    push_bonfire_group_status(&g, &status);
                    g.set_bonfire_group_loading(false);
                });
            }
            Err(e) => {
                warn!("bonfire_status: {e:#}");
                let msg = format!("Couldn't load Bonfire group status: {e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = window.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_bonfire_group_error(ss(&msg));
                    g.set_bonfire_group_loading(false);
                });
            }
        }
    });
}

/// Master-only gate (mirrors `open_manage_profiles_screen`'s exact shape;
/// `is_true_master`, not bare `is_bonfire` — a session actively
/// impersonating a foreign group account can manage ITS OWN group too, see
/// `is_true_master`'s own doc comment). Fetches `bonfire_status()` to
/// populate the screen, and separately fires `sync_bonfire_subprofiles` in
/// the background (fire-and-forget, matching the sidebar's own "Switch
/// Profile"/"Switch Account" precedent) so a newly-joined member's account
/// is discoverable without needing a full session restart.
pub(crate) fn open_bonfire_group_screen(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    let g = AppState::get(window);
    let (client, is_master) = {
        let s = state.lock().unwrap();
        (s.client.clone(), is_true_master(s.config.active()))
    };
    if !is_master {
        crate::show_toast(window.as_weak(), "Only a master account can manage a Bonfire group".to_string());
        return;
    }
    let Some(client) = client else { return };
    g.set_bonfire_group_error(ss(""));
    g.set_bonfire_group_join_code(ss(""));
    g.set_bonfire_group_zone(0);
    g.set_bonfire_group_loading(true);
    g.set_show_bonfire_group(true);
    window.invoke_grab_keyboard_focus();

    sync_bonfire_subprofiles(Arc::clone(&client), Arc::clone(state), rt.clone(), window.as_weak());
    refresh_bonfire_group_status(client, Arc::clone(state), window.as_weak(), rt);
}

pub(crate) fn on_bonfire_group_generate(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    let g = AppState::get(window);
    let client = state.lock().unwrap().client.clone();
    let Some(client) = client else { return };
    g.set_bonfire_group_loading(true);
    let state2 = Arc::clone(state);
    let ww = window.as_weak();
    let rt2 = rt.clone();
    rt.spawn(async move {
        match client.bonfire_generate().await {
            Ok(_info) => refresh_bonfire_group_status(client, state2, ww, &rt2),
            Err(e) => {
                warn!("bonfire_generate: {e:#}");
                let msg = format!("Couldn't generate a join code: {e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_bonfire_group_error(ss(&msg));
                    g.set_bonfire_group_loading(false);
                });
            }
        }
    });
}

/// Error path reuses the existing `crate::is_rate_limited` helper for
/// Bonfire's own SEPARATE join rate limit (docs: "3 failed attempts in 15
/// minutes," distinct from the 5-in-15-min switch/PIN limit that helper
/// already exists for) — it's generic, just checks for a 429 status, so
/// it's directly reusable with no changes.
pub(crate) fn on_bonfire_group_join_submit(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    let g = AppState::get(window);
    let code = g.get_bonfire_group_join_code().to_string().to_uppercase();
    debug!("bonfire_group: join submit, code={code:?} (len={})", code.len());
    if code.is_empty() {
        debug!("bonfire_group: join submit — empty code, no-op");
        return;
    }
    let client = state.lock().unwrap().client.clone();
    let Some(client) = client else { return };
    g.set_bonfire_group_loading(true);
    let state2 = Arc::clone(state);
    let ww = window.as_weak();
    let rt2 = rt.clone();
    rt.spawn(async move {
        match client.bonfire_join(&code).await {
            Ok(_result) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() { AppState::get(&w).set_bonfire_group_join_code(ss("")); }
                });
                sync_bonfire_subprofiles(Arc::clone(&client), Arc::clone(&state2), rt2.clone(), ww.clone());
                refresh_bonfire_group_status(client, state2, ww, &rt2);
            }
            Err(e) => {
                warn!("bonfire_join: {e:#}");
                let msg = if crate::is_rate_limited(&e) {
                    "Too many attempts — please wait a few minutes and try again".to_string()
                } else {
                    format!("{e:#}")
                };
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_bonfire_group_error(ss(&msg));
                    g.set_bonfire_group_loading(false);
                });
            }
        }
    });
}

pub(crate) fn on_bonfire_group_kick(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle, member_id: SharedString) {
    let g = AppState::get(window);
    let client = state.lock().unwrap().client.clone();
    let Some(client) = client else { return };
    g.set_bonfire_group_loading(true);
    let state2 = Arc::clone(state);
    let ww = window.as_weak();
    let rt2 = rt.clone();
    let member_id = member_id.to_string();
    rt.spawn(async move {
        match client.bonfire_kick(&member_id).await {
            Ok(()) => {
                // The kicked member's account should disappear from the
                // caller's own next /list view too, per the docs' "each
                // other's" bidirectional framing.
                sync_bonfire_subprofiles(Arc::clone(&client), Arc::clone(&state2), rt2.clone(), ww.clone());
                refresh_bonfire_group_status(client, state2, ww, &rt2);
            }
            Err(e) => {
                warn!("bonfire_kick: {e:#}");
                let msg = format!("Couldn't remove that member: {e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_bonfire_group_error(ss(&msg));
                    g.set_bonfire_group_loading(false);
                });
            }
        }
    });
}

pub(crate) fn on_bonfire_group_leave(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    let g = AppState::get(window);
    let client = state.lock().unwrap().client.clone();
    let Some(client) = client else { return };
    g.set_bonfire_group_loading(true);
    let state2 = Arc::clone(state);
    let ww = window.as_weak();
    let rt2 = rt.clone();
    rt.spawn(async move {
        match client.bonfire_leave().await {
            // The owner's account should now prune out of Config.profiles
            // — sync_bonfire_subprofiles's own prune step (scoped via
            // synced_via) handles this once /list no longer reports it.
            Ok(()) => {
                sync_bonfire_subprofiles(Arc::clone(&client), Arc::clone(&state2), rt2.clone(), ww.clone());
                refresh_bonfire_group_status(client, state2, ww, &rt2);
            }
            Err(e) => {
                warn!("bonfire_leave: {e:#}");
                let msg = format!("Couldn't leave the group: {e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_bonfire_group_error(ss(&msg));
                    g.set_bonfire_group_loading(false);
                });
            }
        }
    });
}

pub(crate) fn on_bonfire_group_delete(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    let g = AppState::get(window);
    let client = state.lock().unwrap().client.clone();
    let Some(client) = client else { return };
    g.set_bonfire_group_loading(true);
    let state2 = Arc::clone(state);
    let ww = window.as_weak();
    let rt2 = rt.clone();
    rt.spawn(async move {
        match client.bonfire_delete_group().await {
            Ok(()) => {
                sync_bonfire_subprofiles(Arc::clone(&client), Arc::clone(&state2), rt2.clone(), ww.clone());
                refresh_bonfire_group_status(client, state2, ww, &rt2);
            }
            Err(e) => {
                warn!("bonfire_delete_group: {e:#}");
                let msg = format!("Couldn't delete the group: {e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_bonfire_group_error(ss(&msg));
                    g.set_bonfire_group_loading(false);
                });
            }
        }
    });
}

/// The two hide-toggles apply immediately (no confirm); `allow_lan_bypass`'s
/// OFF->ON transition is intercepted entirely on the Slint side (shows
/// `show-bonfire-lan-bypass-confirm` first) — by the time this Rust
/// callback is ever invoked with `allow_lan_bypass: true`, the user has
/// already confirmed the real risk. Always sends the full trio, matching
/// `bonfire_settings`'s own request shape.
pub(crate) fn on_bonfire_group_settings_changed(
    state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle,
    hide_my: bool, hide_others: bool, allow_lan_bypass: bool,
) {
    let g = AppState::get(window);
    let client = state.lock().unwrap().client.clone();
    let Some(client) = client else { return };
    let ww = window.as_weak();
    rt.spawn(async move {
        if let Err(e) = client.bonfire_settings(hide_my, hide_others, Some(allow_lan_bypass)).await {
            warn!("bonfire_settings: {e:#}");
            let msg = format!("Couldn't save group settings: {e:#}");
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() { AppState::get(&w).set_bonfire_group_error(ss(&msg)); }
            });
        }
    });
    // Optimistic local update — the request above is fire-and-forget from
    // this function's own perspective (errors surface via toast/error text
    // only); reflects the change immediately rather than waiting on a full
    // status round trip for what's just a settings toggle.
    g.set_bonfire_group_hide_my_sub_profiles(hide_my);
    g.set_bonfire_group_hide_others_sub_profiles(hide_others);
    g.set_bonfire_group_allow_lan_bypass(allow_lan_bypass);
}

/// D-pad zone list — restructured 2026-08-29 from 3 mutually-exclusive
/// states to 2 INDEPENDENT, always-rendered sections (hosting + join),
/// once a real screenshot of Bonfire's own official "Who's Watching?" UI
/// confirmed an account can genuinely be an owner AND a member of a
/// different group at the same time (a "Your Hosted Bonfire" section and
/// a "Join a Bonfire" section shown together, unconditionally, on the same
/// page) — the old model made "owner types a join code" structurally
/// impossible, which is exactly the gap a live report ("shuld you not be
/// able to join more than one group... if you have generated a groupe you
/// shuld still be able to join a groupe right?") named directly. Zone -1
/// (the floating "✕" button) is handled separately in keys.rs, matching
/// every other master-only screen's own Back-button convention, and isn't
/// part of this list.
///
/// Every zone across every combination is part of ONE contiguous range
/// (0..total, no gaps — unlike `existing_profile_edit_zones`, which has
/// genuine interior gaps for its own conditional zones) — verified by hand
/// for all 4 `(is_owner, is_member)` combinations × N∈{0,1,2} members
/// before shipping, since this exact class of D-pad zone arithmetic has
/// already caused several real live bugs on this screen today.
///
/// - **Hosting section**, zones `0..host_count`: `!is_owner` → zone 0 is
///   the "Generate Join Code" button (`host_count = 1`). `is_owner` →
///   zone 0 = code display (informational, no-op on Enter), zones
///   `1..=n_members` = one per owned member (Kick), zone
///   `host_count - 1` (== `n_members + 1`) = "Delete Group"
///   (`host_count = n_members + 2`).
/// - **Join section**, zones `join_base..join_base+join_count` where
///   `join_base = host_count`: `!is_member` → zone `join_base` = the
///   join-code field, zone `join_base + 1` = "Join" button
///   (`join_count = 2`). `is_member` → zone `join_base` = "Leave Group"
///   (`join_count = 1`; the "Joined X's group" text above it is
///   informational, not a zone).
/// - **Toggles section**, zones `toggle_base..toggle_base+3` where
///   `toggle_base = join_base + join_count` (always exactly 3 zones,
///   shown regardless of hosting/membership state — a single shared copy,
///   not duplicated per section like the old 3-state model had):
///   `toggle_base + 0` = hide-my-sub-profiles, `+1` = hide-others,
///   `+2` = allow-lan-bypass.
///
/// Kept in sync by hand with `bonfire_group.slint`'s own matching
/// `host-count`/`join-base`/`toggle-base` properties and `keys.rs`'s
/// `RETURN` dispatch — no shared source of truth between the three, the
/// same caveat this codebase already carries for a few other Rust/Slint
/// dual-side D-pad zone schemes.
pub(crate) fn existing_bonfire_group_zones(g: &AppState<'_>) -> Vec<i32> {
    let host_count = if g.get_bonfire_group_is_owner() {
        g.get_bonfire_group_owned_members().row_count() as i32 + 2
    } else {
        1
    };
    let join_count = if g.get_bonfire_group_is_member() { 1 } else { 2 };
    (0..host_count + join_count + 3).collect()
}

// ── "Remember this login" toggle (2026-08-17) ───────────────────────────────
// Live-questioned: "no why to change this on the accaunt without sinign out
// and in again." Per the user's explicit choice (of 3 offered via
// AskUserQuestion): OFF is immediate and local, no proof required; ON needs
// a real password re-check first, via a small standalone confirm modal
// (remember_login_confirm.slint) rather than the full LoginScreen/do_login
// pipeline — that would tear down and rebuild the whole active session for
// something that's really just "prove you still know the password," a much
// heavier and more disruptive operation than this needs to be.

/// Settings → Profiles → "Remember this login" row's dispatch — the single
/// handler both the mouse `ToggleSwitch.toggled` and the keyboard
/// `settings_row_action`'s `PROF_REMEMBER_LOGIN` arm call, so the two input
/// paths can't diverge on what toggling this row actually does.
pub(crate) fn on_remember_login_toggle(state: &Arc<Mutex<FjordState>>, window: &MainWindow) {
    let g = AppState::get(window);
    if g.get_settings_remember_login() {
        // Turning OFF — more restrictive, no confirmation needed.
        let cfg = {
            let mut s = state.lock().unwrap();
            let root_id = account_root_id(s.config.active()).to_string();
            if let Some(root) = s.config.profiles.iter_mut().find(|p| p.user_id == root_id) {
                root.remember_login = false;
            }
            s.config.clone()
        };
        save_config(&cfg);
        g.set_settings_remember_login(false);
    } else {
        // Turning ON — open the confirm-password modal instead of flipping
        // the field directly.
        let username = {
            let s = state.lock().unwrap();
            let root_id = account_root_id(s.config.active()).to_string();
            s.config.profiles.iter()
                .find(|p| p.user_id == root_id)
                .map(|p| p.display_name.clone())
                .unwrap_or_default()
        };
        g.set_remember_login_confirm_username(ss(&username));
        g.set_remember_login_confirm_error(ss(""));
        g.set_remember_login_confirm_loading(false);
        g.set_show_remember_login_confirm(true);
        window.invoke_grab_keyboard_focus();
    }
}

/// The confirm modal's own submit — a lightweight, standalone
/// `authenticate_with_fallback` call (the same one `do_login` itself uses)
/// against the account root's already-known server_url + username, never
/// touching the active session/websocket/home-data pipeline at all. On
/// success, flips remember_login back on for that root entry; on failure
/// (wrong password, unreachable server), shows an error and leaves it off.
pub(crate) fn on_remember_login_confirm(
    state:    &Arc<Mutex<FjordState>>,
    window:   &MainWindow,
    rt:       &tokio::runtime::Handle,
    password: SharedString,
) {
    let g = AppState::get(window);
    g.set_remember_login_confirm_loading(true);
    g.set_remember_login_confirm_error(ss(""));
    let (root_id, server, username, device_id) = {
        let s = state.lock().unwrap();
        let root_id = account_root_id(s.config.active()).to_string();
        let Some(root) = s.config.profiles.iter().find(|p| p.user_id == root_id) else {
            return;
        };
        (root_id, root.server_url.clone(), root.display_name.clone(), s.config.device.device_id.clone())
    };
    let ww     = window.as_weak();
    let state2 = Arc::clone(state);
    rt.spawn(async move {
        // Matches do_login's own client construction exactly — see its doc
        // comment for why a bare default reqwest::Client (no timeout) is
        // avoided.
        let login_http = match reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build() {
            Ok(c) => c,
            Err(e) => {
                warn!("remember_login confirm: building http client failed: {e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_remember_login_confirm_error(ss("Couldn't reach the server"));
                    g.set_remember_login_confirm_loading(false);
                });
                return;
            }
        };
        match crate::auth::authenticate_with_fallback(&login_http, &server, &username, &password, &device_id).await {
            Ok(_) => {
                let cfg = {
                    let mut s = state2.lock().unwrap();
                    if let Some(root) = s.config.profiles.iter_mut().find(|p| p.user_id == root_id) {
                        root.remember_login = true;
                    }
                    s.config.clone()
                };
                save_config(&cfg);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_settings_remember_login(true);
                    g.set_show_remember_login_confirm(false);
                    g.set_remember_login_confirm_loading(false);
                });
            }
            Err(e) => {
                warn!("remember_login confirm failed: {e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_remember_login_confirm_error(ss("Incorrect password"));
                    g.set_remember_login_confirm_loading(false);
                });
            }
        }
    });
}

/// Closes the confirm modal without changing anything — remember_login
/// stays off, exactly as it was before the toggle was pressed.
pub(crate) fn on_remember_login_confirm_cancel(window: &MainWindow) {
    let g = AppState::get(window);
    g.set_show_remember_login_confirm(false);
    g.set_remember_login_confirm_error(ss(""));
    g.set_remember_login_confirm_loading(false);
    window.invoke_grab_keyboard_focus();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real bug, live-reported 2026-08-31 ("but what i shuld still be able
    /// to switch to a bonfire master profile with out needing to switch
    /// 'accaunt'..."), caught by an independent review pass while designing
    /// the fix — a "pure" Bonfire group account (its own root never
    /// independently logged into, so its own `is_bonfire` is ALSO true)
    /// must still sort root-first within its own `AccountGroup`, regardless
    /// of which order the sync loop happened to insert its members in.
    /// Constructed here with the sub-profile inserted BEFORE the root
    /// specifically to defeat the old `sort_by_key(|p| p.is_bonfire)` — a
    /// stable sort over an identical key (both `true` here) preserves
    /// insertion order — the fixed `!is_true_master(p)` key must still put
    /// the root first regardless.
    #[test]
    fn group_into_accounts_sorts_pure_group_account_root_first() {
        let sub = ProfileSettings {
            user_id: "sub1".to_string(),
            is_bonfire: true,
            is_group_account: false,
            master_user_id: "root".to_string(),
            ..Default::default()
        };
        let root = ProfileSettings {
            user_id: "root".to_string(),
            is_bonfire: true,
            is_group_account: true,
            master_user_id: String::new(),
            ..Default::default()
        };
        let profiles = vec![sub, root]; // sub-profile inserted first, on purpose
        let groups = group_into_accounts(&profiles);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].root_id, "root");
        assert_eq!(
            groups[0].profiles[0].user_id, "root",
            "root must sort first, not whichever entry happened to be inserted first",
        );
        assert_eq!(groups[0].profiles[1].user_id, "sub1");
    }
}
