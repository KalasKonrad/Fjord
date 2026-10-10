// ── fjord-app · profile.rs ───────────────────────────────────────────────────
//   Profile/account pickers and switching (Bonfire). An ACCOUNT is a plain login or a whole
//   Bonfire household (master + sub-profiles), grouped by account_root_id; the account picker
//   only appears with 2+ accounts, and the profile picker is always scoped to one account.
//   avatar_color_for / parse_hex_color  ProfileSettings.avatar_color (hex, format not guaranteed)
//                       → slint::Color, else a deterministic per-user_id palette colour
//   AccountGroup / account_root_id / group_into_accounts  account grouping, root-first per group
//                       (own user_id for a plain or group account, else master_user_id)
//   is_true_master      !is_bonfire || is_group_account — master authority (incl. an impersonated
//                       foreign group account); use it for every "am I a sub-profile" check
//   build_account_tile / build_tile  AccountGroup → AccountTile, ProfileSettings → ProfileTile
//   StartupGate / should_show_picker_at_startup  the startup decision: account tier
//                       (account_launch_policy), then remember_login (false → RequireLogin before
//                       anything else — a plain account has no PIN), then the profile tier
//                       (launch_policy) within that account
//   open_profile_picker / open_profile_picker_with_pin  account-scoped picker (sections per
//                       household: build_profile_sections, linked_account_roots), local data only;
//                       the _with_pin variant opens straight into PIN entry
//   open_account_picker  account-tier picker from group_into_accounts()
//   account_requires_login / already_active_account / require_login_for_account  the
//                       remember_login gate for picker switches (not for the account already in use);
//                       require_login_for_account mirrors StartupGate::RequireLogin's dispatch
//   on_profile_picker_select / on_account_picker_select / on_profile_pin_key  picker callbacks
//                       (need state/rt, which keys.rs's raw-key tiers don't hold)
//   on_account_picker_add_account / on_settings_add_account / on_cancel_add_account  Add Account
//                       → LoginScreen in append mode; login-append-source decides where Back returns
//   switch_to_profile    resolve a token first (Bonfire /switch with the master's stored token for a
//                       sub-profile; the target's own token, re-validated by check_auth, for a plain
//                       account), only then reset_session_state + finish_session_setup — a failed
//                       switch leaves the current session intact; clear_loading clears both pickers'
//                       loading/error and the PIN buffer on every failure path
//   sync_bonfire_subprofiles  after every session start: GET /list, upsert reported profiles
//                       (sub-profile vs. group account by bp.is_master; skips the master's own entry
//                       and independently-known accounts), prune vanished ones, record linked
//                       households, refresh an open picker
//   sync_all_known_accounts_in_background  the same for every known true master, right before a
//                       cold-start picker shows (changes made elsewhere correct themselves)
//   refresh_profile_settings_dropdown / refresh_account_settings_dropdown  Settings → Profiles →
//                       "Default Profile"/"Default Account" (dynamic dropdowns; called from
//                       apply_settings_to_window and finish_session_setup)
//   sidebar_profile_menu_rows / on_open_sidebar_profile_menu / on_sidebar_profile_menu_action
//                       sidebar quick-menu: Switch Profile (current account has 2+ profiles or a
//                       linked account) / Switch Account (always) / Manage Profiles + Edit My
//                       Profile (true master) / Profile Settings / Sign Out
//   push_current_profile_tile  the sidebar's current-profile row (finish_session_setup, spawn_auto_login)
//   on_remember_login_toggle/-confirm/-confirm_cancel  Settings → "Remember this login": OFF is
//                       immediate; ON asks for the password in a small modal first
//   wire_idle_lock_timer  15 s timer: once activity::ActivityClock::idle_for() exceeds the active
//                       profile's lockout_minutes (Bonfire profile with has_pin), reset_session_state
//                       + PIN picker (or require_login_for_account); unpaused playback counts as activity
//   open_bonfire_group_screen / on_bonfire_group_generate/-join_submit/-kick/-leave/-delete/
//   -settings_changed / existing_bonfire_group_zones  Settings → Profiles → "Bonfire Group"
//                       (true master): host/join a cross-household group, Kick/Leave, visibility
//                       toggles + the LAN-bypass grant (confirmed OFF→ON); D-pad zones per state
//   wire_pickers           callbacks moved from main() (0.5.0 step 3): profile/account pickers, remember-login, PIN pad, sidebar profile menu
//   wire_bonfire_group     callbacks moved from main() (0.5.0 step 3): BonfireGroupScreen
// ─────────────────────────────────────────────────────────────────────────────
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use anyhow::{Result, anyhow, bail};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, warn};

use crate::config::{FjordState, ProfileSettings, save_config};
use crate::playback::VideoState;
use crate::{AppState, MainWindow, ProfileSection, ProfileTile};
use slint::Global;

fn ss(s: &str) -> SharedString {
    SharedString::from(s)
}

/// Grabs keyboard focus on the event loop's next tick. The pickers can open from
/// `main()`'s synchronous startup gate, before `window.run()` pumps the loop, where a
/// direct `invoke_grab_keyboard_focus()` doesn't take effect. From a running session
/// the deferral is one tick, so it's safe to use always.
pub(crate) fn grab_focus_deferred(window: &MainWindow) {
    let ww = window.as_weak();
    let _ = slint::invoke_from_event_loop(move || {
        if let Some(w) = ww.upgrade() {
            w.invoke_grab_keyboard_focus();
        }
    });
}

// pub(crate): also used by profile_edit.rs (Bonfire Phase 2) to resolve the
// live avatar-preview swatch from whichever palette color the user picked.
pub(crate) fn parse_hex_color(s: &str) -> Option<slint::Color> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
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
pub(crate) fn avatar_color_for(hex: &str, seed: &str) -> slint::Color {
    if !hex.is_empty()
        && let Some(c) = parse_hex_color(hex)
    {
        return c;
    }
    const PALETTE: [(u8, u8, u8); 8] = [
        (0x4a, 0x90, 0xd9),
        (0xd9, 0x4a, 0x6b),
        (0x4a, 0xd9, 0x8e),
        (0xd9, 0xa0, 0x4a),
        (0x9a, 0x4a, 0xd9),
        (0x4a, 0xc9, 0xd9),
        (0xd9, 0xd9, 0x4a),
        (0xd9, 0x6b, 0x4a),
    ];
    let hash: u32 = seed
        .bytes()
        .fold(0u32, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u32));
    let (r, g, b) = PALETTE[(hash as usize) % PALETTE.len()];
    slint::Color::from_rgb_u8(r, g, b)
}

/// One "account" in `Config.profiles`: the root (a plain login, or a Bonfire
/// household master) plus the Bonfire sub-profiles reached through it. A runtime
/// view only — `Config.profiles` stays one flat `Vec`; rebuilt where needed (cheap,
/// households are small).
pub(crate) struct AccountGroup {
    /// The grouping key — the root profile's own `user_id`. Also what
    /// `DeviceConfig.default_account_id` stores.
    pub root_id: String,
    pub server_url: String,
    /// Root first (guaranteed present — see `account_root_id`'s own doc
    /// comment for why an orphan sub-profile can't happen), sub-profiles
    /// after in encounter order.
    pub profiles: Vec<ProfileSettings>,
}

/// The account a profile belongs to: its own `user_id` when it is a root
/// (`!is_bonfire`, or `is_group_account` — a foreign master reached via a Bonfire
/// group roots itself), else its master's `user_id`. A sub-profile's master is
/// always in `Config.profiles` too (`sync_bonfire_subprofiles` only adds
/// sub-profiles after the master's login), so there are no orphans.
pub(crate) fn account_root_id(p: &ProfileSettings) -> &str {
    if p.is_bonfire && !p.is_group_account {
        &p.master_user_id
    } else {
        &p.user_id
    }
}

/// True when this session has master-level authority over its own account: never
/// Bonfire-discovered (`!is_bonfire`), or a foreign master's account reached through
/// a Bonfire group (`is_group_account` — that switch returns a fully privileged
/// session). False only for a real sub-profile. Use this, not bare `is_bonfire`, for
/// "am I a sub-profile" checks (`sidebar_profile_menu_rows`, `sync_bonfire_subprofiles`'
/// self-skip, `open_manage_profiles_screen`, `open_my_profile_edit_screen`,
/// `open_bonfire_group_screen`, the settings-is-master-profile push).
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
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(p.clone());
    }
    order
        .into_iter()
        .filter_map(|key| {
            let mut members = groups.remove(&key)?;
            // Root first. The key is `!is_true_master`, not `is_bonfire`: in a group account
            // every member, the root included, has `is_bonfire == true`. It matters beyond
            // looks — `should_show_picker_at_startup`'s single-account guard reads
            // `.profiles.first()` to keep a group account from auto-resuming without a PIN.
            members.sort_by_key(|p| !is_true_master(p));
            let server_url = members.first()?.server_url.clone();
            Some(AccountGroup {
                root_id: key,
                server_url,
                profiles: members,
            })
        })
        .collect()
}

/// `ProfileSettings` -> `AccountTile` (theme.slint) for the account-tier
/// picker. Uses the group's root for avatar/display-name (the "account" is
/// represented by whoever owns it), `profiles.len()` for the subtitle
/// ("N profiles" when > 1, hidden at exactly 1 by the Slint side).
pub(crate) fn build_account_tile(group: &AccountGroup) -> crate::AccountTile {
    let root = group.profiles.first();
    let display_name = root
        .map(|p| {
            if p.display_name.is_empty() {
                p.user_id.clone()
            } else {
                p.display_name.clone()
            }
        })
        .unwrap_or_default();
    let avatar_initial = root
        .and_then(|p| {
            if p.avatar_initial.is_empty() {
                display_name
                    .chars()
                    .next()
                    .map(|c| c.to_uppercase().to_string())
            } else {
                Some(p.avatar_initial.clone())
            }
        })
        .unwrap_or_default();
    let avatar_color_src = root.map(|p| p.avatar_color.as_str()).unwrap_or("");
    crate::AccountTile {
        root_id: ss(&group.root_id),
        display_name: ss(&display_name),
        avatar_color: avatar_color_for(avatar_color_src, &group.root_id),
        avatar_initial: ss(&avatar_initial),
        server_url: ss(&group.server_url),
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
    let display_name = if p.display_name.is_empty() {
        p.user_id.clone()
    } else {
        p.display_name.clone()
    };
    let avatar_initial = if p.avatar_initial.is_empty() {
        display_name
            .chars()
            .next()
            .map(|c| c.to_uppercase().to_string())
            .unwrap_or_default()
    } else {
        p.avatar_initial.clone()
    };
    ProfileTile {
        user_id: ss(&p.user_id),
        display_name: ss(&display_name),
        avatar_color: avatar_color_for(&p.avatar_color, &p.user_id),
        avatar_initial: ss(&avatar_initial),
        has_pin: p.has_pin,
        // requires-pin mirrors has-pin here — this app has no way to know
        // Bonfire's own bypassPinOnLocalNetwork verdict without asking the
        // server (requires-pin is what the real API response would set that
        // to; local data only ever has has-pin). Worth revisiting once a
        // live refresh path exists — see open_profile_picker's own doc
        // comment for why that isn't built yet.
        requires_pin: p.has_pin,
        is_bonfire: p.is_bonfire,
        // `is_true_master`, not `!is_bonfire`: a group account's own root also has
        // `is_bonfire == true` (same reason as `group_into_accounts`' sort key).
        is_root: is_true_master(p),
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
        if p.display_name.is_empty() {
            p.user_id.clone()
        } else {
            p.display_name.clone()
        }
    }
    // Options = the Default Account's own profiles: the "default" launch policy only
    // searches inside the account tier 1 resolved to (`should_show_picker_at_startup`),
    // so a default profile under another account could never apply. With no Default
    // Account configured (`default_account_id` stays "" until set) it falls back to the
    // active account, so the list is never empty.
    let account_id = if cfg.device.default_account_id.is_empty() {
        account_root_id(cfg.active()).to_string()
    } else {
        cfg.device.default_account_id.clone()
    };
    let labels: Vec<SharedString> = cfg
        .profiles
        .iter()
        .filter(|p| !p.user_id.is_empty() && account_root_id(p) == account_id)
        .map(|p| ss(&label(p)))
        .collect();
    let current = cfg
        .profiles
        .iter()
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
        root.map(|p| {
            if p.display_name.is_empty() {
                p.user_id.clone()
            } else {
                p.display_name.clone()
            }
        })
        .unwrap_or_default()
    }
    let accounts = group_into_accounts(&cfg.profiles);
    let labels: Vec<SharedString> = accounts.iter().map(|a| ss(&label(a))).collect();
    let current = accounts
        .iter()
        .find(|a| a.root_id == cfg.device.default_account_id)
        .map(label)
        .unwrap_or_default();
    g.set_settings_default_account_display(ModelRc::new(VecModel::from(labels)));
    g.set_settings_default_account_desc(ss(&current));
}

/// What the startup gate decided (two tiers — see `should_show_picker_at_startup`):
/// - `AutoLogin` — resume silently, no picker.
/// - `ShowAccountPicker` — 2+ accounts, and `account_launch_policy` asks or the
///   resolved account's root has no valid stored token.
/// - `ShowProfilePicker(account_root_id)` — one account resolved with 2+ profiles,
///   and `launch_policy` asks or the resolved profile isn't usable; scoped to that
///   account's profiles.
/// - `ShowProfilePickerPin(account_root_id, target_user_id)` — as above, straight
///   into PIN entry for a known profile.
/// - `RequireLogin(server_url, username)` — the resolved account's root has
///   `remember_login == false`: never resume silently; Login is pre-filled with the
///   server and username (the root's `display_name` — a Jellyfin login's `Name`), so
///   only the password is asked.
pub(crate) enum StartupGate {
    AutoLogin,
    ShowAccountPicker,
    ShowProfilePicker(String),
    ShowProfilePickerPin(String, String),
    RequireLogin(String, String),
}

/// Decides what happens at startup, in two tiers. With 0–1 accounts (a Bonfire
/// household is one account) there's nothing to pick at the account tier, so it
/// resolves straight to that account's profile tier.
///
/// **Tier 1 — account.** With 2+ accounts, `DeviceConfig.account_launch_policy`
/// picks one: "always_ask" → `ShowAccountPicker`; "remember_last" → the account
/// containing `Config.active_profile_id`; "default" → `DeviceConfig.default_account_id`.
/// The resolved account's root needs a valid stored token, else the account picker shows.
///
/// **`remember_login` gate — right after an account resolves.** A plain account has
/// no PIN, so resuming it silently next to a PIN-protected household would bypass
/// every PIN there. `ProfileSettings.remember_login` (default `true`) is read on the
/// account's ROOT; `false` returns `RequireLogin` (password re-entry) before tier 2.
///
/// **Tier 2 — profile, within the resolved account.** `launch_policy`/`has_pin`/
/// `default_profile_id` over `account.profiles`; a single-profile account only
/// checks its own `has_pin`.
pub(crate) fn should_show_picker_at_startup(cfg: &mut crate::config::Config) -> StartupGate {
    let accounts = group_into_accounts(&cfg.profiles);
    if accounts.is_empty() {
        return StartupGate::AutoLogin; // nothing saved at all — the ordinary "no session" path handles this, not this gate
    }

    // A foreign group account must never auto-resume, in any branch — including this
    // single-account shortcut, which has no other filter. Sign-out already removes
    // group accounts found via the signed-out master (it matches `synced_via`), so this
    // is defense in depth: "never auto-resume" holds structurally.
    let account = if accounts.len() < 2 {
        accounts
            .into_iter()
            .next()
            .filter(|a| !a.profiles.first().is_some_and(|r| r.is_group_account))
    } else {
        match cfg.device.account_launch_policy.as_str() {
            "remember_last" => accounts
                .into_iter()
                .find(|a| {
                    a.profiles
                        .iter()
                        .any(|p| p.user_id == cfg.active_profile_id)
                })
                .filter(|a| {
                    a.profiles
                        .iter()
                        .find(|p| p.user_id == a.root_id)
                        .is_some_and(|r| !r.token.is_empty() && !r.is_group_account)
                }),
            "default" => {
                let target = cfg.device.default_account_id.clone();
                accounts
                    .into_iter()
                    .find(|a| a.root_id == target)
                    .filter(|a| {
                        a.profiles
                            .iter()
                            .find(|p| p.user_id == a.root_id)
                            .is_some_and(|r| !r.token.is_empty() && !r.is_group_account)
                    })
            }
            // "always_ask" and any unrecognized value fail safe to asking.
            _ => None,
        }
    };
    let Some(account) = account else {
        return StartupGate::ShowAccountPicker;
    };

    let Some(root) = account
        .profiles
        .iter()
        .find(|p| p.user_id == account.root_id)
    else {
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
            return StartupGate::ShowProfilePickerPin(
                account.root_id.clone(),
                root.user_id.clone(),
            );
        }
        cfg.active_profile_id = root.user_id.clone();
        return StartupGate::AutoLogin;
    }
    match cfg.device.launch_policy.as_str() {
        "remember_last" => {
            let target = account
                .profiles
                .iter()
                .find(|p| p.user_id == cfg.active_profile_id)
                .cloned();
            match target {
                Some(t) if t.token.is_empty() => {
                    StartupGate::ShowProfilePicker(account.root_id.clone())
                }
                Some(t) if t.has_pin => {
                    StartupGate::ShowProfilePickerPin(account.root_id.clone(), t.user_id)
                }
                Some(t) => {
                    cfg.active_profile_id = t.user_id.clone();
                    StartupGate::AutoLogin
                }
                None => StartupGate::ShowProfilePicker(account.root_id.clone()),
            }
        }
        "default" => {
            let target_id = cfg.device.default_profile_id.clone();
            let target = account
                .profiles
                .iter()
                .find(|p| p.user_id == target_id && !p.token.is_empty())
                .cloned();
            match target {
                Some(t) if t.has_pin => {
                    StartupGate::ShowProfilePickerPin(account.root_id.clone(), t.user_id)
                }
                Some(t) => {
                    cfg.active_profile_id = t.user_id.clone();
                    StartupGate::AutoLogin
                }
                None => StartupGate::ShowProfilePicker(account.root_id.clone()),
            }
        }
        _ => StartupGate::ShowProfilePicker(account.root_id.clone()),
    }
}

/// The other account roots linked to `account_root_id` via a Bonfire group
/// (`ProfileSettings.bonfire_linked_roots`, written by `sync_bonfire_subprofiles`),
/// keeping only ids that still resolve to an `AccountGroup` (a pruned link yields
/// no broken section). Only the syncing session's own root carries the list, so a
/// foreign account's picker gets no extra sections.
pub(crate) fn linked_account_roots(
    cfg: &crate::config::Config,
    account_root_id: &str,
) -> Vec<String> {
    let Some(root) = cfg.profiles.iter().find(|p| p.user_id == account_root_id) else {
        return Vec::new();
    };
    let accounts = group_into_accounts(&cfg.profiles);
    root.bonfire_linked_roots
        .iter()
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
fn build_profile_sections(
    cfg: &crate::config::Config,
    account_root_id: &str,
) -> Vec<ProfileSection> {
    let accounts = group_into_accounts(&cfg.profiles);
    let Some(primary) = accounts.iter().find(|a| a.root_id == account_root_id) else {
        return Vec::new();
    };
    let mut groups: Vec<(String, &AccountGroup)> = vec![(String::new(), primary)];
    for id in linked_account_roots(cfg, account_root_id) {
        if let Some(a) = accounts.iter().find(|a| a.root_id == id) {
            let name = a
                .profiles
                .first()
                .map(|p| {
                    if p.display_name.is_empty() {
                        p.user_id.clone()
                    } else {
                        p.display_name.clone()
                    }
                })
                .unwrap_or_default();
            groups.push((format!("{name}'s Bonfire"), a));
        }
    }
    let multi = groups.len() > 1;
    groups
        .into_iter()
        .enumerate()
        .map(|(i, (mut header, group))| {
            if multi && i == 0 {
                header = "Your Bonfire".to_string();
            }
            ProfileSection {
                header: ss(&header),
                tiles: ModelRc::new(VecModel::from(
                    group.profiles.iter().map(build_tile).collect::<Vec<_>>(),
                )),
            }
        })
        .collect()
}

/// Shows the profile picker for one account (`account_root_id` = its root
/// `user_id`), built from local `Config.profiles` — no live `bonfire_list_profiles()`
/// here (at startup there's no client yet).
/// `cancelable`: opened from a live session (sidebar "Switch Profile"), so
/// Escape/Back may close it (keys.rs reads profile-picker-cancelable). Back goes to
/// the account tier (`back_mode = "accounts"`) when `via_account_picker` or
/// `!cancelable` (cold start); otherwise — the sidebar case — it's `"cancel"`:
/// close and keep the current profile.
pub(crate) fn open_profile_picker(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    cancelable: bool,
    via_account_picker: bool,
    account_root_id: &str,
) {
    let sections: Vec<ProfileSection> = {
        let s = state.lock().unwrap();
        build_profile_sections(&s.config, account_root_id)
    };
    // See open_account_picker's own doc comment for why this was added.
    tracing::debug!(
        "open_profile_picker(account_root_id={account_root_id}): {} section(s) — {}",
        sections.len(),
        sections
            .iter()
            .map(|sec| {
                let header = if sec.header.is_empty() {
                    "<unlabeled>"
                } else {
                    sec.header.as_str()
                };
                format!("{header}({} tile(s))", sec.tiles.row_count())
            })
            .collect::<Vec<_>>()
            .join(", "),
    );
    let g = AppState::get(window);
    g.set_profile_picker_sections(ModelRc::new(VecModel::from(sections)));
    g.set_profile_picker_section(0);
    g.set_profile_picker_cursor(0);
    g.set_profile_picker_error(ss(""));
    g.set_profile_picker_loading(false);
    g.set_profile_picker_cancelable(cancelable);
    g.set_profile_picker_back_mode(ss(if via_account_picker || !cancelable {
        "accounts"
    } else {
        "cancel"
    }));
    g.set_profile_picker_account_root_id(ss(account_root_id));
    g.set_profile_picker_back_focused(false);
    g.set_profile_picker_quit_focused(false);
    g.set_show_profile_pin_entry(false);
    g.set_show_account_picker(false);
    crate::close_login_screen(&g);
    g.set_show_profile_picker(true);
    grab_focus_deferred(window);
}

/// Where (`section`, `cursor`) a `user_id` sits in `sections` — `None` if it's in
/// none (pruned since the list was built).
fn find_profile_tile_position(
    sections: &ModelRc<ProfileSection>,
    user_id: &str,
) -> Option<(i32, i32)> {
    for s in 0..sections.row_count() {
        let Some(section) = sections.row_data(s) else {
            continue;
        };
        for i in 0..section.tiles.row_count() {
            if section
                .tiles
                .row_data(i)
                .is_some_and(|t| t.user_id == user_id)
            {
                return Some((s as i32, i as i32));
            }
        }
    }
    None
}

/// Startup-gate variant of `open_profile_picker` (`ShowProfilePickerPin`): the same
/// account-scoped, non-cancelable grid with the cursor on `user_id` and the PIN
/// modal already open, so the user doesn't re-pick a profile the launch policy
/// already chose. Mirrors `on_profile_picker_select`'s PIN-open sequence.
pub(crate) fn open_profile_picker_with_pin(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    account_root_id: &str,
    user_id: &str,
) {
    open_profile_picker(state, window, false, false, account_root_id);
    let target = {
        let s = state.lock().unwrap();
        s.config
            .profiles
            .iter()
            .find(|p| p.user_id == user_id)
            .cloned()
    };
    let Some(target) = target else { return };
    let g = AppState::get(window);
    // Real search over the nested sections, not hardcoded to section 0 —
    // in every current caller this always resolves to section 0 in
    // practice (nothing today can target a linked household's profile
    // through the PIN-entry path yet), but the search itself is written to
    // stay correct once it can.
    if let Some((section, cursor)) =
        find_profile_tile_position(&g.get_profile_picker_sections(), user_id)
    {
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

/// Inactivity auto-lock (see this file's TOC): a repeating `slint::Timer` like
/// `wire_nw_timer`, forgotten in `main()`. It runs on the UI thread, so calling
/// `reset_session_state`/`open_profile_picker_with_pin`/`require_login_for_account`
/// directly is safe. Every read sits in a scoped block that drops the `MutexGuard`
/// before calling anything that locks `state` again — `std::sync::Mutex` isn't
/// reentrant, and a nested lock hangs the UI thread silently.
pub(crate) fn wire_idle_lock_timer(
    window_weak: slint::Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    video: Arc<Mutex<VideoState>>,
    rt_handle: tokio::runtime::Handle,
    clock: crate::activity::ActivityClock,
) -> slint::Timer {
    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, std::time::Duration::from_secs(15), move || {
        let Some(w) = window_weak.upgrade() else { return };
        let g = AppState::get(&w);

        // Guard dropped at the end of this statement, before anything else runs.
        let (client_present, cfg, live_requires_pin) = {
            let s = state.lock().unwrap();
            (s.client.is_some(), s.config.clone(), s.live_requires_pin.clone())
        };
        if !client_present { return; }
        // A manual "Switch Profile"/"Switch Account" is already in
        // progress (per this app's own design, left fully intact with
        // s.client still Some until a switch actually completes), or a
        // previous firing of this same timer already opened the picker —
        // never redundantly re-fire reset_session_state on top of it.
        if g.get_show_profile_picker() || g.get_show_account_picker() { return; }

        let active = cfg.active();
        // Prefer the live, freshly-synced requires-pin over the persisted `has_pin` when
        // one was captured — a stale `has_pin` would demand a PIN for a profile that's
        // LAN-bypassed right now.
        let requires_pin = live_requires_pin.get(&active.user_id).copied().unwrap_or(active.has_pin);
        if !requires_pin || active.lockout_minutes <= 0 { return; }

        // Active, non-paused playback continuously counts as activity —
        // implemented as "keep resetting the clock to now while genuinely
        // playing," not a separate skip-the-check branch, matching how
        // music_idle_ticks already treats its own gating condition.
        // Paused playback does NOT suppress the clock: someone stepping
        // away with a movie paused is exactly the scenario a household
        // security lock exists to catch, matching how a phone still locks
        // with an app open and paused.
        if (g.get_is_playing() || g.get_is_audio_playing()) && !g.get_is_paused() {
            clock.touch();
            return;
        }

        let idle_for = clock.idle_for();
        if idle_for < std::time::Duration::from_secs(active.lockout_minutes as u64 * 60) { return; }

        let account_root = account_root_id(cfg.active()).to_string();
        let user_id = cfg.active().user_id.clone();
        debug!("wire_idle_lock_timer: locking profile {user_id} after {:.0}s idle (lockout_minutes={})", idle_for.as_secs_f64(), active.lockout_minutes);

        // Evaluate before `reset_session_state` below sets `FjordState.client = None` —
        // `already_active_account` reads `s.client.is_some()`. In practice it's always true
        // here (the locked profile is the active one); the `require_login_for_account`
        // branch keeps this correct if it's ever reused for another profile.
        let needs_login = if already_active_account(&state, &account_root) {
            None
        } else {
            let s = state.lock().unwrap();
            account_requires_login(&s.config, &account_root).cloned()
        };

        crate::reset_session_state(&video, &w.as_weak(), &rt_handle, &state);

        if let Some(root) = needs_login {
            require_login_for_account(&state, &w, &account_root, &root);
            clock.touch();
            return;
        }
        open_profile_picker_with_pin(&state, &w, &account_root, &user_id);
        clock.touch();
    });
    timer
}

/// Populates the sidebar's own profile row (2026-08-14) — called on every
/// session start/switch (`finish_session_setup`'s own UI-update closure),
/// same site `refresh_profile_settings_dropdown` is already called from.
pub(crate) fn push_current_profile_tile(g: &AppState, cfg: &crate::config::Config) {
    g.set_current_profile_tile(build_tile(cfg.active()));
}

/// Which rows the sidebar quick-menu shows, in order — a "gaps are fine" list like
/// context_menu.rs's `existing_*_menu_rows`, but returning labels (Slint renders
/// the list as given, nothing is duplicated in Slint).
///
/// "Switch Profile" shows when the CURRENT account has 2+ profiles (or a linked
/// account exists) and opens the picker scoped to the current account. "Switch
/// Account" is always shown — the account picker's "+ Add Account" tile is how a
/// second account gets added.
pub(crate) fn sidebar_profile_menu_rows(cfg: &crate::config::Config) -> Vec<&'static str> {
    let mut rows = Vec::with_capacity(6);
    let accounts = group_into_accounts(&cfg.profiles);
    let current_root = account_root_id(cfg.active());
    let current_profile_count = accounts
        .iter()
        .find(|a| a.root_id == current_root)
        .map(|a| a.profiles.len())
        .unwrap_or(1);
    // Also shown when a Bonfire-linked account exists, so a single-profile household
    // can reach the linked one without "Switch Account".
    if current_profile_count >= 2 || !linked_account_roots(cfg, current_root).is_empty() {
        rows.push("Switch Profile");
    }
    rows.push("Switch Account");
    // `is_true_master`, not `is_bonfire`: an impersonated foreign group account has
    // `is_bonfire == true` locally but is a fully privileged session.
    if is_true_master(cfg.active()) {
        rows.push("Manage Profiles");
        // A master edits its sub-profiles via Manage Profiles; this row edits its own
        // profile (Bonfire's `/update` takes a profileId). Gated like Manage Profiles: a
        // sub-profile can't self-manage (Bonfire answers 401 to a non-master token).
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
        rows.into_iter().map(ss).collect::<Vec<_>>(),
    )));
    g.set_sidebar_profile_menu_focused(0);
    g.set_show_sidebar_profile_menu(true);
}

/// No `video`/`VideoState` param needed here — "Switch Profile" only OPENS
/// the picker; the actual teardown-then-switch (which does need it) already
/// happens inside `switch_to_profile` once a target tile is picked.
pub(crate) fn on_sidebar_profile_menu_action(
    idx: i32,
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    let rows = sidebar_profile_menu_rows(&state.lock().unwrap().config);
    let Some(&label) = rows.get(idx as usize) else {
        return;
    };
    g.set_show_sidebar_profile_menu(false);
    match label {
        "Switch Profile" => {
            let (root_id, client) = {
                let s = state.lock().unwrap();
                (
                    account_root_id(s.config.active()).to_string(),
                    s.client.clone(),
                )
            };
            // via_account_picker=false — reached straight from a live
            // session, never through the account tier (see
            // open_profile_picker's own doc comment for the bug this
            // distinction fixes).
            open_profile_picker(state, window, true, false, &root_id);
            // Start a fresh Bonfire sync now that the picker is open, so a server-side
            // deletion since the last login/switch shows without a restart (see
            // `sync_bonfire_subprofiles`).
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
        // Opens the global sign-out confirmation (app_state.slint show-sign-out-confirm);
        // the menu was already hidden at the top of this function, so nothing overlaps it.
        "Sign Out" => {
            g.set_sign_out_confirm_focused(0);
            g.set_show_sign_out_confirm(true);
        }
        _ => {}
    }
}

/// Whether reaching this ACCOUNT (any of its profiles — the root, or a Bonfire
/// sub-profile switching via the root's token) must force a full password re-login,
/// per `remember_login` on the account's ROOT. The same rule
/// `should_show_picker_at_startup` applies at tier 1, enforced here for the picker
/// paths (`on_account_picker_select`/`on_profile_picker_select`) — the account picker
/// is the likeliest way such an account is opened. Returns the root's
/// `(server_url, display_name)` when a re-login is required, `None` otherwise.
fn account_requires_login<'a>(
    cfg: &'a crate::config::Config,
    account_root_id: &str,
) -> Option<&'a ProfileSettings> {
    let root = cfg.profiles.iter().find(|p| p.user_id == account_root_id)?;
    // Never for a group account: it has no password of its own (always switched into
    // via one of my accounts' tokens + Bonfire's PIN). `remember_login` is normally unset
    // for it, but toggling "Remember this login" while impersonating one would otherwise
    // demand a password Fjord can never check.
    if root.is_group_account {
        return None;
    }
    (!root.remember_login).then_some(root)
}

/// True when the live client's active profile belongs to `account_root` — the
/// session is already authenticated as this account. `remember_login` guards silently
/// resuming a stored credential; it must not re-demand the password for switching
/// between profiles of the account already in use.
fn already_active_account(state: &Arc<Mutex<FjordState>>, account_root: &str) -> bool {
    let s = state.lock().unwrap();
    s.client.is_some() && account_root_id(s.config.active()) == account_root
}

/// Shared tail for `remember_login == false` — like main()'s `StartupGate::RequireLogin`
/// dispatch: server/username prefill, append mode off, the open picker closed.
/// `login-remember` starts as this account's stored choice (off), so a plain re-login
/// doesn't silently turn it back on. `active_profile_id` points at the account's root
/// now, so `do_login`'s non-append branch updates the right entry.
fn require_login_for_account(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    account_root_id: &str,
    root: &ProfileSettings,
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
    if g.get_profile_picker_loading() {
        return;
    }
    let target = {
        let s = state.lock().unwrap();
        s.config
            .profiles
            .iter()
            .find(|p| p.user_id == user_id.as_str())
            .cloned()
    };
    let Some(target) = target else {
        g.set_profile_picker_error(ss("That profile is no longer available"));
        return;
    };
    let account_root = account_root_id(&target).to_string();
    if !already_active_account(state, &account_root)
        && let Some(root) = {
            let s = state.lock().unwrap();
            account_requires_login(&s.config, &account_root).cloned()
        }
    {
        require_login_for_account(state, window, &account_root, &root);
        return;
    }
    // LAN-bypass PIN staleness fix (2026-09-04) — prefer the live,
    // freshly-synced requires_pin over the persisted has_pin whenever a
    // live value has been captured for this exact profile.
    let requires_pin = state
        .lock()
        .unwrap()
        .live_requires_pin
        .get(target.user_id.as_str())
        .copied()
        .unwrap_or(target.has_pin);
    if requires_pin {
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
        switch_to_profile(
            Arc::clone(state),
            Arc::clone(video),
            window.as_weak(),
            rt.clone(),
            user_id.to_string(),
            None,
        );
    }
}

/// Account-tier picker (2026-08-14, the 2-tier account/profile redesign).
/// Shown only when 2+ distinct accounts exist at all — a single-account
/// install never reaches this screen via the startup gate (see
/// `should_show_picker_at_startup`); it's still reachable mid-session via
/// the sidebar's "Switch Profile" action even with just 1 known account,
/// so there's always a way back to it once a second one exists.
pub(crate) fn open_account_picker(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    cancelable: bool,
) {
    let accounts: Vec<crate::AccountTile> = {
        let s = state.lock().unwrap();
        group_into_accounts(&s.config.profiles)
            .iter()
            .map(build_account_tile)
            .collect()
    };
    // Logged at debug — without it, picker/switch problems can't be traced in fjord.log.
    tracing::debug!(
        "open_account_picker: {} account(s) — {}",
        accounts.len(),
        accounts
            .iter()
            .map(|a| format!(
                "{}(root={}, n={})",
                a.display_name, a.root_id, a.profile_count
            ))
            .collect::<Vec<_>>()
            .join(", "),
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

/// Picking an account tile: a direct switch (single-profile account, no PIN), the PIN
/// modal (single profile with a PIN), or the profile picker scoped to this account
/// (2+ profiles) — `should_show_picker_at_startup`'s tier 2, on a click.
/// `account_requires_login` is checked first: a forced-login account shows Login
/// immediately, never the PIN modal or the profile picker.
pub(crate) fn on_account_picker_select(
    state: &Arc<Mutex<FjordState>>,
    video: &Arc<Mutex<VideoState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    root_id: SharedString,
) {
    let g = AppState::get(window);
    if g.get_account_picker_loading() {
        return;
    }
    debug!("on_account_picker_select(root_id={root_id}): clicked");
    let group = {
        let s = state.lock().unwrap();
        group_into_accounts(&s.config.profiles)
            .into_iter()
            .find(|a| a.root_id == root_id.as_str())
    };
    let Some(group) = group else {
        debug!("on_account_picker_select({root_id}): no matching account group found");
        g.set_account_picker_error(ss("That account is no longer available"));
        return;
    };
    if !already_active_account(state, &group.root_id)
        && let Some(root) = {
            let s = state.lock().unwrap();
            account_requires_login(&s.config, &group.root_id).cloned()
        }
    {
        debug!("on_account_picker_select({root_id}): remember_login==false, requiring fresh login");
        require_login_for_account(state, window, &group.root_id, &root);
        return;
    }
    debug!(
        "on_account_picker_select({root_id}): {} profile(s) in group",
        group.profiles.len()
    );
    if group.profiles.len() < 2 {
        let Some(root) = group.profiles.into_iter().next() else {
            return;
        };
        // LAN-bypass PIN staleness fix (2026-09-04) — same prefer-live shape
        // as on_profile_picker_select above.
        let requires_pin = state
            .lock()
            .unwrap()
            .live_requires_pin
            .get(root.user_id.as_str())
            .copied()
            .unwrap_or(root.has_pin);
        if requires_pin {
            open_profile_picker_with_pin(state, window, &root.user_id, &root.user_id);
        } else {
            g.set_account_picker_loading(true);
            switch_to_profile(
                Arc::clone(state),
                Arc::clone(video),
                window.as_weak(),
                rt.clone(),
                root.user_id,
                None,
            );
        }
    } else {
        // via_account_picker=true — this IS the account tier, so Back
        // should genuinely return here, not skip past it.
        open_profile_picker(
            state,
            window,
            g.get_account_picker_cancelable(),
            true,
            &group.root_id,
        );
    }
}

pub(crate) fn on_account_picker_add_account(window: &MainWindow) {
    let g = AppState::get(window);
    g.set_login_append_mode(true);
    g.set_login_append_source(ss("account_picker"));
    // Clear any RequireLogin prefill — Add Account is for a new account.
    g.set_login_server_prefill(ss(""));
    g.set_login_username_prefill(ss(""));
    // Add Account always starts with "Remember this login" checked; an unchecked box
    // from an earlier RequireLogin prompt must not carry over.
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
            if g.get_profile_picker_loading() {
                return;
            }
            let (target_id, pin) = {
                let s = state.lock().unwrap();
                (
                    g.get_profile_pin_target_id().to_string(),
                    s.profile_pin_buffer.clone(),
                )
            };
            if pin.is_empty() {
                g.set_profile_pin_error(ss("Enter your PIN first"));
                return;
            }
            g.set_profile_picker_loading(true);
            switch_to_profile(
                Arc::clone(state),
                Arc::clone(video),
                window.as_weak(),
                rt.clone(),
                target_id,
                Some(pin),
            );
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
    state: Arc<Mutex<FjordState>>,
    video: Arc<Mutex<VideoState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    target_user_id: String,
    pin: Option<String>,
) {
    let started = std::time::Instant::now();
    debug!("switch_to_profile({target_user_id}): starting");
    let rt2 = rt.clone();
    rt.spawn(async move {
        // Every early return below must clear profile-picker-loading, or the picker stays
        // looking busy; `clear_loading` is the one helper they all use.
        fn clear_loading(state: &Arc<Mutex<FjordState>>, ww: &slint::Weak<MainWindow>, msg: Option<String>) {
            // A failed switch clears the typed PIN too (profile_pin_buffer / profile-pin-len),
            // so the next attempt starts clean instead of appending to the wrong digits.
            state.lock().unwrap().profile_pin_buffer.clear();
            let ww = ww.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let g = AppState::get(&w);
                    g.set_profile_picker_loading(false);
                    g.set_profile_pin_len(0);
                    // A switch can start from either picker (an account tile with one profile too),
                    // each with its own loading/error properties; clearing both is harmless for the
                    // hidden one.
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
                // A group account's `master_user_id` is empty (it roots itself): authenticate this
                // switch with the account of mine that discovered it (`synced_via`). A real
                // sub-profile's `master_user_id` names its master directly.
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
                // `/switch` reports which Jellyfin user the minted token authenticates as. The
                // client is still built with our own `target_user_id` (that part of the API isn't
                // live-verified), but a mismatch is logged.
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
                // Bonfire rate-limits /switch and /verify-pin (5 failures / 15 min): show a readable
                // message instead of a raw 429 — reachable from manual PIN entry and the idle-lock
                // unlock.
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

/// Before a cold-start picker shows (all 3 picker arms of the startup gate), runs
/// `sync_bonfire_subprofiles` for every known true-master account with its stored
/// token. A cold-start picker is built from `Config.profiles` as last saved, so a
/// change made elsewhere since (another device, Jellyfin's web UI — e.g. a group
/// kick) would show stale tiles until some account is used. The picker still opens
/// instantly from cache and corrects itself a moment later through the same refresh
/// closure. Cosmetic only: Bonfire's `/switch` re-checks membership server-side.
/// A stale token just fails and is logged inside `sync_bonfire_subprofiles`;
/// sub-profile entries are skipped (that function would bail for them anyway).
pub(crate) fn sync_all_known_accounts_in_background(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    let (device_id, accounts) = {
        let s = state.lock().unwrap();
        let accounts: Vec<(String, String, String)> = s
            .config
            .profiles
            .iter()
            .filter(|p| is_true_master(p) && !p.token.is_empty() && !p.server_url.is_empty())
            .map(|p| (p.user_id.clone(), p.server_url.clone(), p.token.clone()))
            .collect();
        (s.config.device.device_id.clone(), accounts)
    };
    for (user_id, server_url, token) in accounts {
        let Ok(url) = url::Url::parse(&server_url) else {
            continue;
        };
        let Ok(client) =
            fjord_api::JellyfinClient::new(url, user_id.clone(), token, device_id.clone())
        else {
            continue;
        };
        tracing::debug!("sync_all_known_accounts_in_background: syncing {user_id}");
        sync_bonfire_subprofiles(
            Arc::new(client),
            Arc::clone(state),
            rt.clone(),
            window.as_weak(),
        );
    }
}

/// Fire-and-forget after every session start (`finish_session_setup`); safe without
/// the plugin (`bonfire_list_profiles()` returns `Ok(vec![])` on 404). Upserts a local
/// `ProfileSettings` per reported sub-profile, and prunes this household's local
/// sub-profiles the server no longer reports (otherwise a deleted one stays an
/// unreachable tile in the picker). Pruning only touches entries with `is_bonfire &&
/// master_user_id == this master`, and only after a successful, non-empty `/list`.
/// With `window` it refreshes whichever picker is showing once the sync lands (the
/// picker itself opens from local state).
pub(crate) fn sync_bonfire_subprofiles(
    client: Arc<fjord_api::JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    rt: tokio::runtime::Handle,
    window: slint::Weak<MainWindow>,
) {
    rt.spawn(async move {
        // Only a true master may sync: the loop below writes the CALLING session's id as
        // `master_user_id` onto every reported profile, so running it as a sub-profile would
        // re-parent the whole household under that sub-profile (`load_config` repairs configs
        // already corrupted that way). `is_true_master`, not `is_bonfire`: an impersonated
        // group account is a full master too and must discover its own sub-profiles.
        {
            let s = state.lock().unwrap();
            if s.config.profiles.iter().any(|p| p.user_id == client.user_id && !is_true_master(p)) {
                tracing::debug!("sync_bonfire_subprofiles: skipping — this session ({}) is itself a Bonfire sub-profile, not a master", client.user_id);
                return;
            }
        }
        // Logged always, so the log tells "never tried" apart from "found nothing".
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
        // One line per reported entry: shows whether an unexpected profile comes from the
        // server or from stale local config.
        for bp in &profiles {
            tracing::debug!(
                "sync_bonfire_subprofiles: reported entry name={:?} id={} is_master={} master_user_id={:?}",
                bp.profile_name, bp.profile_user_id, bp.is_master, bp.master_user_id
            );
        }
        let master_user_id = client.user_id.clone();
        let cfg = {
            let mut s = state.lock().unwrap();
            // Bail only if a DIFFERENT session became active meanwhile (Arc::ptr_eq, as in
            // ws.rs). `None` doesn't bail: `sync_all_known_accounts_in_background` runs before
            // any session exists, and a sign-out racing this leaves only this account's own
            // entries to write.
            if s.client.as_ref().is_some_and(|c| !Arc::ptr_eq(c, &client)) {
                tracing::debug!("sync_bonfire_subprofiles: a different session became active mid-flight, discarding");
                return;
            }
            let mut linked_roots: Vec<String> = Vec::new();
            for bp in &profiles {
                // Record requires_pin for every entry (see FjordState.live_requires_pin), before
                // the self-entry skip below — the value comes with /list anyway.
                s.live_requires_pin.insert(bp.profile_user_id.clone(), bp.requires_pin);
                // Skip the master's own entry: /list includes the calling master, and upserting it
                // would rewrite it as a Bonfire sub-profile of itself (switching to it then fails
                // with 401).
                if bp.profile_user_id == master_user_id {
                    tracing::debug!("sync_bonfire_subprofiles: skipping self entry ({master_user_id}) in /list response");
                    continue;
                }
                // Record this entry's account root as linked to me (the picker's extra sections,
                // `linked_account_roots`). It must happen here, before the "already a known
                // independent account" skip below: linkage matters even for a household this
                // device also knows independently.
                let root = if bp.is_master {
                    bp.profile_user_id.clone()
                } else if !bp.master_user_id.is_empty() {
                    bp.master_user_id.clone()
                } else {
                    master_user_id.clone()
                };
                // Only OTHER households' roots: /list also returns my own household's sub-profiles,
                // whose root is my own master — recording that would duplicate section 0.
                if root != master_user_id && !linked_roots.contains(&root) {
                    linked_roots.push(root);
                }
                // A group member's /list reports every master in the group, including ones this
                // device already has its own login for (`is_bonfire: false`, own token). Leave those
                // alone: the upsert would downgrade them to Bonfire-group switching with a PIN and
                // overwrite fields that come from their own login. Only for `bp.is_master` (a
                // sub-profile can't be logged into independently).
                if bp.is_master
                    && let Some(existing) = s.config.profiles.iter().find(|p| p.user_id == bp.profile_user_id)
                    && !existing.is_bonfire && !existing.token.is_empty() {
                    tracing::debug!("sync_bonfire_subprofiles: skipping {} — already a known independent account on this device", bp.profile_user_id);
                    continue;
                }
                // `bp.is_master` tells a sub-profile of MY household apart from another master's
                // account reached through a cross-household group — they're classified differently
                // (see ProfileSettings.is_group_account / synced_via): a group account roots ITSELF
                // with an empty master_user_id (never self-referencing — see
                // repair_bonfire_profile_corruption). A sub-profile's master is `bp.master_user_id`
                // as reported, NOT the calling client's id: in a group, /list also returns other
                // masters' sub-profiles, which must stay with their own master. The calling id is
                // only a fallback if a sub-profile arrives without a master id. A later sync
                // re-files any entries an older version misfiled.
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
            // Prune (rules in this function's doc comment), scoped by `synced_via`, not
            // `master_user_id`: a group account's `master_user_id` is empty, so only `synced_via`
            // ("this client discovered it") also prunes a group account after leaving the group.
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

        // Refresh whichever picker is open. A no-op for the session-start callers
        // (finish_session_setup / spawn_auto_login, no picker showing then); only the
        // sidebar's mid-session "Switch Profile" / "Switch Account" can have one open.
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

// ── Bonfire Group (cross-household groups) ──────────────────────────────────
// Screen shape: app_state.slint's show-bonfire-group; how group accounts are
// classified: sync_bonfire_subprofiles above (is_group_account, synced_via,
// is_true_master).

fn push_bonfire_group_status(g: &AppState<'_>, status: &fjord_api::models::BonfireGroupStatus) {
    // Debug logging, 2026-08-29 — this whole function had none at all,
    // which left "can't tell which of the 3 screen states is even showing"
    // undiagnosable from a log alone the first time this was live-tested.
    debug!(
        "bonfire_group: status is_owner={} is_member={} owned_code={:?} owned_members={} joined_owner={:?}",
        status.is_owner,
        status.is_member,
        status.owned_code,
        status.owned_members.len(),
        status.joined_owner_name,
    );
    g.set_bonfire_group_is_owner(status.is_owner);
    g.set_bonfire_group_is_member(status.is_member);
    g.set_bonfire_group_owned_code(ss(status.owned_code.as_deref().unwrap_or("")));
    let members: Vec<crate::BonfireGroupMemberTile> = status
        .owned_members
        .iter()
        .map(|m| crate::BonfireGroupMemberTile {
            user_id: ss(&m.user_id),
            username: ss(&m.username),
        })
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
    client: Arc<fjord_api::JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    window: slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
) {
    rt.spawn(async move {
        match client.bonfire_status().await {
            Ok(status) => {
                if !crate::session_current(&state, &client) {
                    return;
                }
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
pub(crate) fn open_bonfire_group_screen(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    let (client, is_master) = {
        let s = state.lock().unwrap();
        (s.client.clone(), is_true_master(s.config.active()))
    };
    if !is_master {
        crate::show_toast(
            window.as_weak(),
            "Only a master account can manage a Bonfire group".to_string(),
        );
        return;
    }
    let Some(client) = client else { return };
    g.set_bonfire_group_error(ss(""));
    g.set_bonfire_group_join_code(ss(""));
    g.set_bonfire_group_zone(0);
    g.set_bonfire_group_loading(true);
    g.set_show_bonfire_group(true);
    window.invoke_grab_keyboard_focus();

    sync_bonfire_subprofiles(
        Arc::clone(&client),
        Arc::clone(state),
        rt.clone(),
        window.as_weak(),
    );
    refresh_bonfire_group_status(client, Arc::clone(state), window.as_weak(), rt);
}

pub(crate) fn on_bonfire_group_generate(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
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
pub(crate) fn on_bonfire_group_join_submit(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    let code = g.get_bonfire_group_join_code().to_string().to_uppercase();
    debug!(
        "bonfire_group: join submit, code={code:?} (len={})",
        code.len()
    );
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
                    if let Some(w) = ww2.upgrade() {
                        AppState::get(&w).set_bonfire_group_join_code(ss(""));
                    }
                });
                sync_bonfire_subprofiles(
                    Arc::clone(&client),
                    Arc::clone(&state2),
                    rt2.clone(),
                    ww.clone(),
                );
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

pub(crate) fn on_bonfire_group_kick(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    member_id: SharedString,
) {
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
                sync_bonfire_subprofiles(
                    Arc::clone(&client),
                    Arc::clone(&state2),
                    rt2.clone(),
                    ww.clone(),
                );
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

pub(crate) fn on_bonfire_group_leave(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
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
                sync_bonfire_subprofiles(
                    Arc::clone(&client),
                    Arc::clone(&state2),
                    rt2.clone(),
                    ww.clone(),
                );
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

pub(crate) fn on_bonfire_group_delete(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
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
                sync_bonfire_subprofiles(
                    Arc::clone(&client),
                    Arc::clone(&state2),
                    rt2.clone(),
                    ww.clone(),
                );
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
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    hide_my: bool,
    hide_others: bool,
    allow_lan_bypass: bool,
) {
    let g = AppState::get(window);
    let client = state.lock().unwrap().client.clone();
    let Some(client) = client else { return };
    let ww = window.as_weak();
    rt.spawn(async move {
        if let Err(e) = client
            .bonfire_settings(hide_my, hide_others, Some(allow_lan_bypass))
            .await
        {
            warn!("bonfire_settings: {e:#}");
            let msg = format!("Couldn't save group settings: {e:#}");
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    AppState::get(&w).set_bonfire_group_error(ss(&msg));
                }
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

/// D-pad zone list. Two independent, always-rendered sections — hosting and joining
/// (an account can own one group and be a member of another), then the toggles. All
/// zones form one contiguous range 0..total (no gaps, unlike
/// `existing_profile_edit_zones`); zone -1 (the floating "✕") is handled in keys.rs.
///
/// - **Hosting**, zones `0..host_count`: `!is_owner` → zone 0 = "Generate Join Code"
///   (`host_count = 1`). `is_owner` → zone 0 = code display (no-op), zones
///   `1..=n_members` = one per member (Kick), zone `n_members + 1` = "Delete Group"
///   (`host_count = n_members + 2`).
/// - **Join**, from `join_base = host_count`: `!is_member` → `join_base` = join-code
///   field, `join_base + 1` = "Join" (`join_count = 2`). `is_member` → `join_base` =
///   "Leave Group" (`join_count = 1`).
/// - **Toggles**, from `toggle_base = join_base + join_count`, always 3:
///   hide-my-sub-profiles, hide-others, allow-lan-bypass.
///
/// Kept in sync by hand with bonfire_group.slint's `host-count`/`join-base`/
/// `toggle-base` and keys.rs's RETURN dispatch — there's no shared source of truth.
pub(crate) fn existing_bonfire_group_zones(g: &AppState<'_>) -> Vec<i32> {
    let host_count = if g.get_bonfire_group_is_owner() {
        g.get_bonfire_group_owned_members().row_count() as i32 + 2
    } else {
        1
    };
    let join_count = if g.get_bonfire_group_is_member() {
        1
    } else {
        2
    };
    (0..host_count + join_count + 3).collect()
}

// ── "Remember this login" toggle ─────────────────────────────────────────────
// OFF is immediate and local; ON needs the password re-checked first, via a small
// confirm modal (remember_login_confirm.slint) rather than the full LoginScreen /
// do_login, which would rebuild the whole session.

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
            s.config
                .profiles
                .iter()
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
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
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
        (
            root_id,
            root.server_url.clone(),
            root.display_name.clone(),
            s.config.device.device_id.clone(),
        )
    };
    let ww = window.as_weak();
    let state2 = Arc::clone(state);
    rt.spawn(async move {
        // Matches do_login's own client construction exactly — see its doc
        // comment for why a bare default reqwest::Client (no timeout) is
        // avoided.
        let login_http = match reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
        {
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
        match crate::auth::authenticate_with_fallback(
            &login_http,
            &server,
            &username,
            &password,
            &device_id,
        )
        .await
        {
            Ok(_) => {
                let cfg = {
                    let mut s = state2.lock().unwrap();
                    if let Some(root) = s.config.profiles.iter_mut().find(|p| p.user_id == root_id)
                    {
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

// ── wire_pickers (moved from main(), 0.5.0 step 3) ───────────────────────
/// Wires profile/account pickers, remember-login, PIN pad, sidebar profile menu: profile_picker_select, cancel_add_account, account_picker_select, account_picker_add_account, settings_add_account, settings_remember_login_toggle, remember_login_confirm, remember_login_confirm_cancel, profile_picker_back_to_accounts, profile_picker_cancel, profile_pin_key, open_sidebar_profile_menu, sidebar_profile_menu_action.
pub(crate) fn wire_pickers(
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
    // ── profile picker (Bonfire Phase 1, step 6, 2026-08-09) ────────────────────
    {
        let state = Arc::clone(&state);
        let video = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_profile_picker_select(move |user_id| {
            if let Some(w) = window_weak.upgrade() {
                profile::on_profile_picker_select(&state, &video, &w, &rt_handle, user_id);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_cancel_add_account(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_cancel_add_account(&state, &w);
            }
        });
    }

    // ── account picker (2026-08-14, the 2-tier account/profile redesign) ───────
    {
        let state = Arc::clone(&state);
        let video = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_account_picker_select(move |root_id| {
            if let Some(w) = window_weak.upgrade() {
                profile::on_account_picker_select(&state, &video, &w, &rt_handle, root_id);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_account_picker_add_account(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_account_picker_add_account(&w);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_settings_add_account(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_settings_add_account(&w);
            }
        });
    }
    // "Remember this login" toggle + its confirm-password modal
    // (2026-08-17) — see app_state.slint's own settings-remember-login doc
    // comment for the full design.
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_settings_remember_login_toggle(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_remember_login_toggle(&state, &w);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_remember_login_confirm(move |password| {
            if let Some(w) = window_weak.upgrade() {
                profile::on_remember_login_confirm(&state, &w, &rt_handle, password);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_remember_login_confirm_cancel(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_remember_login_confirm_cancel(&w);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_profile_picker_back_to_accounts(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::open_account_picker(
                    &state,
                    &w,
                    AppState::get(&w).get_profile_picker_cancelable(),
                );
            }
        });
    }
    // Close the picker without switching, keeping the current session — the sidebar's
    // "Switch Profile" never went through the account tier (see app_state.slint's
    // profile-picker-back-mode).
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_profile_picker_cancel(move || {
            if let Some(w) = window_weak.upgrade() {
                AppState::get(&w).set_show_profile_picker(false);
                w.invoke_grab_keyboard_focus();
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let video = Arc::clone(&video);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_profile_pin_key(move |key| {
            if let Some(w) = window_weak.upgrade() {
                profile::on_profile_pin_key(&state, &video, &w, &rt_handle, key);
            }
        });
    }
    // ── sidebar profile row + quick-menu (2026-08-14) ───────────────────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_open_sidebar_profile_menu(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_open_sidebar_profile_menu(&state, &w);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_sidebar_profile_menu_action(move |idx| {
            if let Some(w) = window_weak.upgrade() {
                profile::on_sidebar_profile_menu_action(idx, &state, &w, &rt_handle);
            }
        });
    }
}

// ── wire_bonfire_group (moved from main(), 0.5.0 step 3) ─────────────────
/// Wires BonfireGroupScreen: open_bonfire_group, bonfire_group_generate, bonfire_group_join_code_submit, bonfire_group_join_code_append, bonfire_group_join_code_backspace, bonfire_group_kick, bonfire_group_leave, bonfire_group_delete, bonfire_group_settings_changed.
pub(crate) fn wire_bonfire_group(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── Bonfire Group (Phase 5, cross-household groups, 2026-08-29) ────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_bonfire_group(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::open_bonfire_group_screen(&state, &w, &rt_handle);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_group_generate(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_bonfire_group_generate(&state, &w, &rt_handle);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_group_join_code_submit(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_bonfire_group_join_submit(&state, &w, &rt_handle);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_bonfire_group_join_code_append(move |ch| {
            // Traced at debug, so the log shows whether typing reaches the join-code field
            // (on-screen keys, physical keyboard, or neither).
            debug!("bonfire_group: join-code append {ch:?}");
            if let Some(w) = window_weak.upgrade() {
                text_field::JOIN_CODE.insert(&AppState::get(&w), ch.as_str());
            }
        });
    }
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_bonfire_group_join_code_backspace(move || {
            debug!("bonfire_group: join-code backspace");
            if let Some(w) = window_weak.upgrade() {
                text_field::JOIN_CODE.backspace(&AppState::get(&w));
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_group_kick(move |member_id| {
            if let Some(w) = window_weak.upgrade() {
                profile::on_bonfire_group_kick(&state, &w, &rt_handle, member_id);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_group_leave(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_bonfire_group_leave(&state, &w, &rt_handle);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_group_delete(move || {
            if let Some(w) = window_weak.upgrade() {
                profile::on_bonfire_group_delete(&state, &w, &rt_handle);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_bonfire_group_settings_changed(
            move |hide_my, hide_others, allow_lan_bypass| {
                if let Some(w) = window_weak.upgrade() {
                    profile::on_bonfire_group_settings_changed(
                        &state,
                        &w,
                        &rt_handle,
                        hide_my,
                        hide_others,
                        allow_lan_bypass,
                    );
                }
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A "pure" group account (its root never logged into independently, so its
    /// `is_bonfire` is true as well) must still sort root-first, whatever order the sync
    /// inserted its members in. The sub-profile is inserted BEFORE the root here: a stable
    /// sort on `is_bonfire` (identical keys) would keep that order; `!is_true_master` must
    /// put the root first.
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
