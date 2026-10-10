// ── fjord-app · profile_edit.rs ──────────────────────────────────────────────
//   Bonfire profile create/edit/delete (Manage Profiles + ProfileEditScreen), on top of bonfire.rs.
//   open_manage_profiles_screen  bonfire_list_profiles() for the active master
//                       (profile::is_true_master — also a session impersonating a foreign group
//                       account); drops the master's own entry and other masters (`is_master`),
//                       reads the per-master profile cap, builds the tile list; cursor reset on
//                       every open
//   on_manage_profiles_select/-add  resolve a tile from FjordState.manage_profiles_cache (no
//                       second round trip) → open_profile_edit_screen
//   open_my_profile_edit_screen  the master editing itself (from the sidebar; no Delete)
//   open_profile_edit_screen  fills ProfileEditScreen's AppState fields (edit) or blanks (create),
//                       resets zones/cursors, fetches libraries + devices in parallel for the two
//                       checklists. The parental rating is write-only in Bonfire: edit mode starts
//                       at UNKNOWN_RATING and sends nothing unless the user picks a value
//   on_profile_edit_pin_key/-master_pin_key  digits into FjordState.profile_edit_pin_buffer /
//                       -master_pin_buffer (the new PIN vs. the master's authorization PIN; never
//                       round-tripped through Slint)
//   on_profile_edit_avatar_color_selected/-toggle_library/-toggle_device
//   on_profile_edit_save  Create/UpdateProfileRequest from AppState + the PIN buffers → back to
//                       Manage Profiles with a fresh fetch; refreshes the local profiles
//                       (sync_bonfire_subprofiles); an is_self save also patches the local
//                       lockout_minutes; PIN buffers cleared on success and failure
//   on_profile_edit_delete  bonfire_delete_profile, same success/failure handling
//   on_profile_edit_cancel  close without saving, back to the already-fetched list
//   close_profile_edit_screen  the one close path; also closes the on-screen keyboard
//   handle_key_profile_edit  raw-key D-pad dispatch (from keys.rs's show_profile_edit tier):
//                       existing_profile_edit_zones (checklists skipped when empty — must match
//                       profile_edit.slint), dropdown popup (open_profile_edit_dropdown /
//                       apply_profile_edit_dropdown_selection), PIN pads, on-screen keyboard
//   wire_profile_edit   callbacks moved from main() (0.5.0 step 3): Manage Profiles +
//                       ProfileEditScreen
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::models::{BonfireProfile, CreateProfileRequest, UpdateProfileRequest};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tracing::{debug, warn};

use crate::config::{FjordState, save_config};
use crate::keys::key;
use crate::{AppState, MainWindow, ProfileTile, ToggleListItem};
use slint::Global;

fn ss(s: &str) -> SharedString {
    SharedString::from(s)
}

/// Closes ProfileEditScreen together with the on-screen keyboard — the one close path for
/// Cancel, Save and Delete. keys.rs's keyboard gate runs before every other tier, so a stray
/// show-onscreen-keyboard would swallow input on the next screen (same choke point as
/// close_login_screen).
fn close_profile_edit_screen(g: &AppState) {
    g.set_show_profile_edit(false);
    g.set_show_onscreen_keyboard(false);
    g.set_onscreen_keyboard_target(ss(""));
    g.set_onscreen_keyboard_cursor(0);
}

const DEFAULT_AVATAR_HEX: &str = "#4a90d9";

// Kept in lockstep by hand with profile_edit.slint's own
// avatar-palette-colors/-hex parallel arrays — same caveat that file's own
// header comment already documents for those two, just a third copy now
// (needed here so keyboard Enter on the avatar zone can resolve a cursor
// position to a hex string without round-tripping through Slint).
const AVATAR_PALETTE_HEX: [&str; 8] = [
    "#4a90d9", "#d94a6b", "#4ad98e", "#d9a04a", "#9a4ad9", "#4ac9d9", "#d9d94a", "#d96b4a",
];

// Same values as profile_edit.slint's two SettingsDropdown `model:` arrays — kept in sync by
// hand. "Any"/"Never" display the stored ""/"0", like each dropdown's `selected(v)`.
const PARENTAL_RATING_MODEL: [&str; 13] = [
    "Any",
    "G",
    "PG",
    "PG-13",
    "R",
    "NC-17",
    "TV-Y",
    "TV-Y7",
    "TV-G",
    "TV-PG",
    "TV-14",
    "TV-MA",
    "Not Rated",
];
// Bonfire never returns a profile's current maxParentalRating (write-only: accepted by
// create/update, absent from every GET — checked in its developer-api.md). Edit mode shows
// this "Unknown" sentinel instead of a misleading "Any"; it is a display state only, never
// a dropdown option, and is treated like "" (omitted from the save request) wherever
// parental_rating.is_empty() is checked.
const UNKNOWN_RATING: &str = "__unknown__";
const LOCKOUT_MODEL: [&str; 6] = ["Never", "5", "15", "30", "60", "120"];

// Same 12-key row-major layout as profile.rs's own PIN entry (the profile
// picker's PIN_VALS) — VirtualKeyboard's real key order, duplicated here
// rather than shared since it's a 12-element literal with no natural home
// in a third file both would import from.
const PIN_VALS: [&str; 12] = [
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "backspace",
    "0",
    "confirm",
];

fn default_avatar_color() -> slint::Color {
    slint::Color::from_rgb_u8(0x4a, 0x90, 0xd9)
}

fn bonfire_profile_to_tile(p: &BonfireProfile) -> ProfileTile {
    ProfileTile {
        user_id: ss(&p.profile_user_id),
        display_name: ss(&p.profile_name),
        avatar_color: crate::profile::parse_hex_color(&p.avatar_color)
            .unwrap_or_else(default_avatar_color),
        avatar_initial: ss(&p.avatar_initial),
        has_pin: p.has_pin,
        requires_pin: p.requires_pin,
        is_bonfire: p.is_bonfire,
        // ManageProfilesScreen's own list (this function's only caller)
        // already filters out the calling master's own entry and every
        // `is_master` entry before building tiles — so nothing shown here
        // is ever a group's own root tile in the first place.
        is_root: false,
    }
}

/// Fetches every sub-profile under the active master account and shows
/// ManageProfilesScreen. Gated on `!Config.active().is_bonfire` — a Bonfire
/// sub-profile isn't a master, so it shouldn't reach this at all (Settings'
/// own row is gated the same way; this is the defensive second check).
pub(crate) fn open_manage_profiles_screen(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    let g = AppState::get(window);
    let (client, is_master) = {
        let s = state.lock().unwrap();
        (
            s.client.clone(),
            crate::profile::is_true_master(s.config.active()),
        )
    };
    if !is_master {
        crate::show_toast(
            window.as_weak(),
            "Only a master account can manage profiles".to_string(),
        );
        return;
    }
    let Some(client) = client else { return };
    g.set_manage_profiles_error(ss(""));
    g.set_manage_profiles_cursor(0);
    g.set_manage_profiles_close_focused(false);
    g.set_show_manage_profiles(true);
    window.invoke_grab_keyboard_focus();

    let ww = window.as_weak();
    let state2 = Arc::clone(state);
    rt.spawn(async move {
        match client.bonfire_list_profiles().await {
            Ok(profiles) => {
                if !crate::session_current(&state2, &client) {
                    return;
                }
                // Bonfire's /list includes the calling master itself: filter it out (as
                // sync_bonfire_subprofiles does) so it can't be edited/deleted here or count toward
                // the profile cap.
                let master_id = client.user_id.clone();
                // The profile cap is per master (max_sub_profiles, admin-adjustable) and only the
                // master's own /list entry carries it — read it before that entry is filtered out.
                // 0/absent → 5, Bonfire's default.
                let max_sub_profiles = profiles
                    .iter()
                    .find(|p| p.profile_user_id == master_id)
                    .map(|p| p.max_sub_profiles)
                    .filter(|&n| n > 0)
                    .unwrap_or(5);
                // Also exclude other masters (`is_master`, reached through a cross-household
                // group): this screen lists only sub-profiles this session administers (the server
                // would refuse the rest).
                let profiles: Vec<_> = profiles
                    .into_iter()
                    .filter(|p| p.profile_user_id != master_id && !p.is_master)
                    .collect();
                let tiles: Vec<ProfileTile> =
                    profiles.iter().map(bonfire_profile_to_tile).collect();
                state2.lock().unwrap().manage_profiles_cache = profiles;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_manage_profiles_list(ModelRc::new(VecModel::from(tiles)));
                        g.set_manage_profiles_max_sub_profiles(max_sub_profiles as i32);
                    }
                });
            }
            Err(e) => {
                warn!("bonfire_list_profiles: {e:#}");
                let msg = format!("Couldn't load profiles: {e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        AppState::get(&w).set_manage_profiles_error(ss(&msg));
                    }
                });
            }
        }
    });
}

pub(crate) fn on_manage_profiles_select(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    user_id: SharedString,
) {
    let existing = {
        let s = state.lock().unwrap();
        s.manage_profiles_cache
            .iter()
            .find(|p| p.profile_user_id == user_id.as_str())
            .cloned()
    };
    let Some(existing) = existing else {
        AppState::get(window).set_manage_profiles_error(ss("That profile is no longer available"));
        return;
    };
    open_profile_edit_screen(state, window, rt, Some(existing), false);
}

pub(crate) fn on_manage_profiles_add(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    open_profile_edit_screen(state, window, rt, None, false);
}

/// The master editing itself (from the sidebar). Manage Profiles excludes the master's own
/// tile to prevent self-deletion, not self-editing; Bonfire's create/update/delete need the
/// master token (sub-profiles can never self-manage), and nothing rules out a master
/// targeting its own profileId. Whether the server accepts that is not verified live.
pub(crate) fn open_my_profile_edit_screen(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
) {
    let (client, is_master) = {
        let s = state.lock().unwrap();
        (
            s.client.clone(),
            crate::profile::is_true_master(s.config.active()),
        )
    };
    if !is_master {
        crate::show_toast(
            window.as_weak(),
            "Only a master account can edit its own profile here".to_string(),
        );
        return;
    }
    let Some(client) = client else { return };
    let ww = window.as_weak();
    let state2 = Arc::clone(state);
    let rt2 = rt.clone();
    rt.spawn(async move {
        match client.bonfire_list_profiles().await {
            Ok(profiles) => {
                if !crate::session_current(&state2, &client) {
                    return;
                }
                let mine = profiles
                    .into_iter()
                    .find(|p| p.profile_user_id == client.user_id);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    match mine {
                        Some(p) => open_profile_edit_screen(&state2, &w, &rt2, Some(p), true),
                        None => crate::show_toast(
                            w.as_weak(),
                            "Couldn't find your own profile — is Bonfire installed on this server?"
                                .to_string(),
                        ),
                    }
                });
            }
            Err(e) => {
                warn!("open_my_profile_edit_screen: bonfire_list_profiles: {e:#}");
                let msg = format!("{e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        crate::show_toast(w.as_weak(), msg);
                    }
                });
            }
        }
    });
}

/// `existing: None` = create mode, `Some(profile)` = edit mode. `is_self`: the master
/// editing its own profile (see open_my_profile_edit_screen) — hides Delete and changes where
/// Save/Cancel return to. Sets every AppState field synchronously (no flash of stale data)
/// before the async libraries/devices fetch.
pub(crate) fn open_profile_edit_screen(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    rt: &tokio::runtime::Handle,
    existing: Option<BonfireProfile>,
    is_self: bool,
) {
    let g = AppState::get(window);
    let is_create = existing.is_none();
    g.set_profile_edit_is_self(is_self);
    let color_hex = existing
        .as_ref()
        .map(|p| p.avatar_color.clone())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| DEFAULT_AVATAR_HEX.to_string());

    g.set_show_manage_profiles(false);
    g.set_profile_edit_is_create(is_create);
    g.set_profile_edit_target_id(ss(existing
        .as_ref()
        .map(|p| p.profile_user_id.as_str())
        .unwrap_or("")));
    g.set_profile_edit_name_initial(ss(existing
        .as_ref()
        .map(|p| p.profile_name.as_str())
        .unwrap_or("")));
    g.set_profile_edit_avatar_color(ss(&color_hex));
    g.set_profile_edit_avatar_preview(
        crate::profile::parse_hex_color(&color_hex).unwrap_or_else(default_avatar_color),
    );
    g.set_profile_edit_pin_len(0);
    g.set_profile_edit_has_pin(existing.as_ref().map(|p| p.has_pin).unwrap_or(false));
    g.set_profile_edit_master_pin_len(0);
    // Create mode: no restriction yet, "Any" ("") is accurate. Edit mode: the current value
    // is unknown (see UNKNOWN_RATING).
    g.set_profile_edit_parental_rating(ss(if is_create { "" } else { UNKNOWN_RATING }));
    g.set_profile_edit_blocked_tags_initial(ss(&existing
        .as_ref()
        .map(|p| p.blocked_tags.join(", "))
        .unwrap_or_default()));
    g.set_profile_edit_allowed_tags_initial(ss(&existing
        .as_ref()
        .map(|p| p.allowed_tags.join(", "))
        .unwrap_or_default()));
    g.set_profile_edit_lockout_minutes(ss(&existing
        .as_ref()
        .map(|p| p.lockout_minutes.to_string())
        .unwrap_or_else(|| "0".to_string())));
    g.set_profile_edit_lan_bypass(
        existing
            .as_ref()
            .map(|p| p.bypass_pin_on_local_network)
            .unwrap_or(false),
    );
    g.set_profile_edit_saving(false);
    g.set_profile_edit_error(ss(""));
    g.set_profile_edit_libraries(ModelRc::new(VecModel::<ToggleListItem>::default()));
    g.set_profile_edit_devices(ModelRc::new(VecModel::<ToggleListItem>::default()));

    // Reset every zone/cursor property on open (zones: see profile-edit-zone in
    // app_state.slint); zone 0 = Name.
    g.set_profile_edit_zone(0);
    g.set_profile_edit_text_editing(false);
    g.set_profile_edit_avatar_cursor(0);
    g.set_profile_edit_pin_cursor(0);
    g.set_profile_edit_master_pin_cursor(0);
    g.set_profile_edit_dropdown_open(false);
    g.set_profile_edit_dropdown_key(ss(""));
    g.set_profile_edit_dropdown_cursor(0);
    g.set_profile_edit_libraries_cursor(0);
    g.set_profile_edit_devices_cursor(0);
    g.set_profile_edit_button_focused(0);
    // A stray delete-confirm from a previous visit would reopen the dialog (and its key gate).
    g.set_show_profile_edit_delete_confirm(false);
    g.set_profile_edit_delete_confirm_focused(0);
    // On-screen keyboard (2026-08-23) — same reasoning as the delete-confirm
    // reset right above: a stray true left over from elsewhere would gate
    // input dispatch the instant this screen shows.
    g.set_show_onscreen_keyboard(false);
    g.set_onscreen_keyboard_target(ss(""));
    g.set_onscreen_keyboard_cursor(0);

    {
        let mut s = state.lock().unwrap();
        s.profile_edit_pin_buffer.clear();
        s.profile_edit_master_pin_buffer.clear();
    }

    g.set_show_profile_edit(true);
    window.invoke_grab_keyboard_focus();

    let (client, enabled_folders, allowed_device_ids) = {
        let s = state.lock().unwrap();
        (
            s.client.clone(),
            existing
                .as_ref()
                .map(|p| p.enabled_folders.clone())
                .unwrap_or_default(),
            existing
                .as_ref()
                .map(|p| p.allowed_device_ids.clone())
                .unwrap_or_default(),
        )
    };
    let Some(client) = client else { return };
    let ww = window.as_weak();
    let state2 = Arc::clone(state);
    rt.spawn(async move {
        let (libs_res, devices_res) = tokio::join!(
            client.bonfire_list_libraries(),
            client.bonfire_list_devices()
        );
        if !crate::session_current(&state2, &client) {
            return;
        }
        let libraries: Vec<ToggleListItem> = match libs_res {
            Ok(libs) => libs
                .into_iter()
                .map(|l| ToggleListItem {
                    id: ss(&l.id),
                    name: ss(&l.name),
                    subtitle: ss(&l.collection_type),
                    // Create mode: nothing owned yet — default every library
                    // enabled (opt-out), matching Jellyfin's own new-user
                    // default of full library access. Edit mode: pre-select
                    // whatever enabled_folders already lists.
                    selected: if is_create {
                        true
                    } else {
                        enabled_folders.contains(&l.id)
                    },
                })
                .collect(),
            Err(e) => {
                warn!("bonfire_list_libraries: {e:#}");
                vec![]
            }
        };
        let devices: Vec<ToggleListItem> = match devices_res {
            Ok(devs) => devs
                .into_iter()
                .map(|d| ToggleListItem {
                    id: ss(&d.device_id),
                    name: ss(&d.device_name),
                    subtitle: ss(&format!("{} · last seen {}", d.client, d.last_seen)),
                    selected: allowed_device_ids.contains(&d.device_id),
                })
                .collect(),
            Err(e) => {
                warn!("bonfire_list_devices: {e:#}");
                vec![]
            }
        };
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_profile_edit_libraries(ModelRc::new(VecModel::from(libraries)));
            g.set_profile_edit_devices(ModelRc::new(VecModel::from(devices)));
        });
    });
}

pub(crate) fn on_profile_edit_pin_key(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    key: SharedString,
) {
    let g = AppState::get(window);
    match key.as_str() {
        "backspace" => {
            let mut s = state.lock().unwrap();
            s.profile_edit_pin_buffer.pop();
            let len = s.profile_edit_pin_buffer.len();
            drop(s);
            g.set_profile_edit_pin_len(len as i32);
        }
        "confirm" => {} // this screen's real "confirm" is the Save button
        digit if digit.len() == 1 && digit.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            let mut s = state.lock().unwrap();
            s.profile_edit_pin_buffer.push_str(digit);
            let len = s.profile_edit_pin_buffer.len();
            drop(s);
            g.set_profile_edit_pin_len(len as i32);
        }
        _ => {}
    }
}

pub(crate) fn on_profile_edit_master_pin_key(
    state: &Arc<Mutex<FjordState>>,
    window: &MainWindow,
    key: SharedString,
) {
    let g = AppState::get(window);
    match key.as_str() {
        "backspace" => {
            let mut s = state.lock().unwrap();
            s.profile_edit_master_pin_buffer.pop();
            let len = s.profile_edit_master_pin_buffer.len();
            drop(s);
            g.set_profile_edit_master_pin_len(len as i32);
        }
        "confirm" => {}
        digit if digit.len() == 1 && digit.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
            let mut s = state.lock().unwrap();
            s.profile_edit_master_pin_buffer.push_str(digit);
            let len = s.profile_edit_master_pin_buffer.len();
            drop(s);
            g.set_profile_edit_master_pin_len(len as i32);
        }
        _ => {}
    }
}

pub(crate) fn on_profile_edit_avatar_color_selected(window: &MainWindow, hex: SharedString) {
    let g = AppState::get(window);
    g.set_profile_edit_avatar_preview(
        crate::profile::parse_hex_color(hex.as_str()).unwrap_or_else(default_avatar_color),
    );
    g.set_profile_edit_avatar_color(hex);
}

pub(crate) fn on_profile_edit_toggle_library(window: &MainWindow, idx: i32) {
    let model = AppState::get(window).get_profile_edit_libraries();
    let Some(mut item) = model.row_data(idx as usize) else {
        return;
    };
    item.selected = !item.selected;
    model.set_row_data(idx as usize, item);
}

pub(crate) fn on_profile_edit_toggle_device(window: &MainWindow, idx: i32) {
    let model = AppState::get(window).get_profile_edit_devices();
    let Some(mut item) = model.row_data(idx as usize) else {
        return;
    };
    item.selected = !item.selected;
    model.set_row_data(idx as usize, item);
}

pub(crate) fn on_profile_edit_cancel(state: &Arc<Mutex<FjordState>>, window: &MainWindow) {
    let g = AppState::get(window);
    close_profile_edit_screen(&g);
    {
        let mut s = state.lock().unwrap();
        s.profile_edit_pin_buffer.clear();
        s.profile_edit_master_pin_buffer.clear();
    }
    // Return to Manage Profiles unless this was "Edit My Profile" (is_self, opened from the
    // sidebar). Nothing was saved, so its list needs no re-fetch.
    if !g.get_profile_edit_is_self() {
        g.set_show_manage_profiles(true);
    }
    window.invoke_grab_keyboard_focus();
}

fn split_tags(csv: &str) -> Vec<String> {
    csv.split(',')
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .collect()
}

fn selected_ids(model: &ModelRc<ToggleListItem>) -> Vec<String> {
    (0..model.row_count())
        .filter_map(|i| model.row_data(i))
        .filter(|item| item.selected)
        .map(|item| item.id.to_string())
        .collect()
}

pub(crate) fn on_profile_edit_save(
    state: Arc<Mutex<FjordState>>,
    window: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    name: SharedString,
    blocked_tags_csv: SharedString,
    allowed_tags_csv: SharedString,
) {
    let Some(w) = window.upgrade() else { return };
    let g = AppState::get(&w);
    let name = name.trim().to_string();
    if name.is_empty() {
        g.set_profile_edit_error(ss("Name can't be empty"));
        return;
    }
    let is_create = g.get_profile_edit_is_create();
    let is_self = g.get_profile_edit_is_self();
    let target_id = g.get_profile_edit_target_id().to_string();
    let avatar_color = g.get_profile_edit_avatar_color().to_string();
    let parental_rating = g.get_profile_edit_parental_rating().to_string();
    let lockout_minutes: i64 = g.get_profile_edit_lockout_minutes().parse().unwrap_or(0);
    let lan_bypass = g.get_profile_edit_lan_bypass();
    let enabled_folders = selected_ids(&g.get_profile_edit_libraries());
    let allowed_device_ids = selected_ids(&g.get_profile_edit_devices());
    let blocked_tags = split_tags(&blocked_tags_csv);
    let allowed_tags = split_tags(&allowed_tags_csv);

    let (pin, master_pin, client) = {
        let s = state.lock().unwrap();
        (
            (!s.profile_edit_pin_buffer.is_empty()).then(|| s.profile_edit_pin_buffer.clone()),
            (!s.profile_edit_master_pin_buffer.is_empty())
                .then(|| s.profile_edit_master_pin_buffer.clone()),
            s.client.clone(),
        )
    };
    let Some(client) = client else { return };

    // Cloned before the request-building code below moves the originals —
    // needed afterward, in the success branch, only for is_self's own
    // local ProfileSettings update (see that branch's own comment for why
    // this doesn't apply to the ordinary Manage-Profiles-editing-a-sub-
    // profile case, which has no equivalent local record to keep in sync).
    let pin_was_set = pin.is_some();
    let name_for_local = name.clone();
    let avatar_color_for_local = avatar_color.clone();

    g.set_profile_edit_saving(true);
    g.set_profile_edit_error(ss(""));

    let ww = window.clone();
    let state2 = Arc::clone(&state);
    let rt_task = rt.clone();
    rt.spawn(async move {
        let result: Result<()> = if is_create {
            let req = CreateProfileRequest {
                profile_name: name,
                pin,
                avatar_color: (!avatar_color.is_empty()).then_some(avatar_color),
                // UNKNOWN_RATING (the untouched-Edit-mode sentinel — see its
                // own doc comment) must be treated identically to "" here:
                // if the user never actually opened this dropdown, nothing
                // should be sent, exactly as if the field were blank —
                // sending the literal sentinel string would corrupt the
                // profile's real rating server-side.
                max_parental_rating: (!parental_rating.is_empty()
                    && parental_rating != UNKNOWN_RATING)
                    .then_some(parental_rating),
                enabled_folders: Some(enabled_folders),
                blocked_tags: Some(blocked_tags),
                allowed_tags: Some(allowed_tags),
                lockout_minutes: Some(lockout_minutes),
                master_pin,
                bypass_pin_on_local_network: Some(lan_bypass),
                allowed_device_ids: Some(allowed_device_ids),
                profile_image: None,
            };
            client.bonfire_create_profile(&req).await.map(|_| ())
        } else {
            let req = UpdateProfileRequest {
                profile_id: target_id,
                profile_name: name,
                pin,
                avatar_color: (!avatar_color.is_empty()).then_some(avatar_color),
                // UNKNOWN_RATING (the untouched-Edit-mode sentinel — see its
                // own doc comment) must be treated identically to "" here:
                // if the user never actually opened this dropdown, nothing
                // should be sent, exactly as if the field were blank —
                // sending the literal sentinel string would corrupt the
                // profile's real rating server-side.
                max_parental_rating: (!parental_rating.is_empty()
                    && parental_rating != UNKNOWN_RATING)
                    .then_some(parental_rating),
                enabled_folders: Some(enabled_folders),
                blocked_tags: Some(blocked_tags),
                allowed_tags: Some(allowed_tags),
                lockout_minutes: Some(lockout_minutes),
                master_pin,
                bypass_pin_on_local_network: Some(lan_bypass),
                allowed_device_ids: Some(allowed_device_ids),
                profile_image: None,
            };
            client.bonfire_update_profile(&req).await
        };

        match result {
            Ok(()) => {
                {
                    let mut s = state2.lock().unwrap();
                    s.profile_edit_pin_buffer.clear();
                    s.profile_edit_master_pin_buffer.clear();
                }
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_profile_edit_saving(false);
                    close_profile_edit_screen(&g);
                    if is_self {
                        // No Manage Profiles list to refresh from here —
                        // instead, keep the LOCAL ProfileSettings entry
                        // (what the sidebar row, and the account/profile
                        // picker tiles, actually read — not a live Bonfire
                        // fetch) in sync with what was just saved.
                        // sync_bonfire_subprofiles deliberately never
                        // touches the calling session's own entry (see its
                        // own doc comment) so nothing else will ever do
                        // this automatically.
                        let cfg = {
                            let mut s = state2.lock().unwrap();
                            let p = s.config.active_mut();
                            p.display_name = name_for_local.clone();
                            if !avatar_color_for_local.is_empty() {
                                p.avatar_color = avatar_color_for_local.clone();
                            }
                            p.avatar_initial.clear(); // re-derive from the (possibly new) name — see ProfileTile's own fallback
                            if pin_was_set {
                                p.has_pin = true;
                            } // blank PIN field means "keep the current one," never a removal
                            p.lockout_minutes = lockout_minutes; // Bonfire Phase 4 — keep the idle-lock timer's own read in sync immediately, not just on the next sync_bonfire_subprofiles
                            s.config.clone()
                        };
                        save_config(&cfg);
                        crate::profile::push_current_profile_tile(&g, &cfg);
                        crate::profile::refresh_profile_settings_dropdown(&g, &cfg);
                        crate::profile::refresh_account_settings_dropdown(&g, &cfg);
                    } else {
                        // Refresh the local Config.profiles entries (has_pin, name, avatar; a newly
                        // created profile is added): the picker and switch_to_profile read only
                        // those, so a PIN set here made the next switch fail with a 400
                        // (passwordless switch) until the next login.
                        crate::profile::sync_bonfire_subprofiles(
                            Arc::clone(&client),
                            Arc::clone(&state2),
                            rt_task.clone(),
                            ww.clone(),
                        );
                        // Fresh fetch, not the stale pre-save list — the just-
                        // created/edited profile needs to show up/update.
                        open_manage_profiles_screen(&state2, &w, &rt_task);
                    }
                });
            }
            Err(e) => {
                warn!("profile save failed: {e:#}");
                // Clear both PIN pads on failure too, so retyping doesn't append to the wrong
                // value.
                {
                    let mut s = state2.lock().unwrap();
                    s.profile_edit_pin_buffer.clear();
                    s.profile_edit_master_pin_buffer.clear();
                }
                let msg = format!("{e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_profile_edit_saving(false);
                        g.set_profile_edit_pin_len(0);
                        g.set_profile_edit_master_pin_len(0);
                        g.set_profile_edit_error(ss(&msg));
                    }
                });
            }
        }
    });
}

// ── Full D-pad keyboard navigation ──────────────────────────────────────────
// Dispatched from keys.rs's show_profile_edit raw-key tier via handle_key_profile_edit (same
// factoring as discover's handle_key_request_options). Zone list: profile-edit-zone in
// app_state.slint.

/// The zone list with gaps (like discover's existing_option_zones): zones 4/9 (libraries/
/// devices checklists) are skipped when empty. Must match profile_edit.slint's
/// `if AppState.profile-edit-libraries/-devices.length > 0` gates.
fn existing_profile_edit_zones(g: &AppState) -> Vec<i32> {
    let mut zones = vec![0, 1, 2, 3];
    if g.get_profile_edit_libraries().row_count() > 0 {
        zones.push(4);
    }
    zones.extend([5, 6, 7, 8]);
    if g.get_profile_edit_devices().row_count() > 0 {
        zones.push(9);
    }
    zones.extend([10, 11]);
    zones
}

/// Resets the entered zone's own sub-cursor to 0 — called on every zone
/// transition so leftover cursor state from a previous visit to that zone
/// never survives (mirrors discover.rs::option_zone_focus_reset).
fn profile_edit_zone_focus_reset(g: &AppState, zone: i32) {
    match zone {
        1 => g.set_profile_edit_avatar_cursor(0),
        2 => g.set_profile_edit_pin_cursor(0),
        4 => g.set_profile_edit_libraries_cursor(0),
        9 => g.set_profile_edit_devices_cursor(0),
        10 => g.set_profile_edit_master_pin_cursor(0),
        11 => g.set_profile_edit_button_focused(0),
        _ => {}
    }
}

/// Screen-local mirror of settings.rs::open_dropdown_popup — same
/// interaction SHAPE (a second, hand-built keyboard-driven overlay, since
/// SettingsDropdown itself has no keyboard path of its own — confirmed by
/// reading its real current definition before assuming otherwise), a
/// smaller purpose-built instance rather than reusing Settings' own
/// row-keyed dispatch tables, which aren't a general-purpose primitive.
/// `dd_key` is `"rating"` or `"lockout"`.
fn open_profile_edit_dropdown(dd_key: &str, g: &AppState) {
    let (model, current): (&[&str], String) = match dd_key {
        "rating" => (&PARENTAL_RATING_MODEL, {
            let v = g.get_profile_edit_parental_rating().to_string();
            if v.is_empty() {
                "Any".to_string()
            } else if v == UNKNOWN_RATING {
                // Not a real option in PARENTAL_RATING_MODEL — position()
                // below correctly falls back to cursor 0 ("Any"), just a
                // reasonable starting point for the browse, not a claim
                // that's actually the current value.
                "Unknown".to_string()
            } else {
                v
            }
        }),
        "lockout" => (&LOCKOUT_MODEL, {
            let v = g.get_profile_edit_lockout_minutes().to_string();
            if v == "0" { "Never".to_string() } else { v }
        }),
        _ => return,
    };
    let cursor = model.iter().position(|v| *v == current).unwrap_or(0) as i32;
    g.set_profile_edit_dropdown_key(ss(dd_key));
    g.set_profile_edit_dropdown_model(ModelRc::new(VecModel::from(
        model.iter().map(|v| ss(v)).collect::<Vec<_>>(),
    )));
    g.set_profile_edit_dropdown_cursor(cursor);
    g.set_profile_edit_dropdown_display(ss(&current));
    g.set_profile_edit_dropdown_open(true);
}

/// Confirms the popup's row, translating "Any"/"Never" back to the stored ""/"0" (as the
/// SettingsDropdowns' `selected(v)` does for the mouse).
pub(crate) fn apply_profile_edit_dropdown_selection(g: &AppState, cursor: i32) {
    let dd_key = g.get_profile_edit_dropdown_key().to_string();
    let Some(v) = g
        .get_profile_edit_dropdown_model()
        .row_data(cursor as usize)
    else {
        return;
    };
    match dd_key.as_str() {
        "rating" => {
            g.set_profile_edit_parental_rating(if v.as_str() == "Any" { ss("") } else { v })
        }
        "lockout" => {
            g.set_profile_edit_lockout_minutes(if v.as_str() == "Never" { ss("0") } else { v })
        }
        _ => {}
    }
}

/// Raw-key dispatch for ProfileEditScreen, called from keys.rs's show_profile_edit tier for
/// every key except Ctrl+Q and Escape-while-not-editing (handled there: it must tell "close
/// the dropdown" from "cancel the screen"). Escape inside a focused LineEdit never gets here
/// — the field's own key-pressed handles it.
pub(crate) fn handle_key_profile_edit(raw_key: &str, g: &AppState) -> bool {
    // Top-priority sub-state: the dropdown popup (zones 3/7's own Enter).
    if g.get_profile_edit_dropdown_open() {
        let model_len = g.get_profile_edit_dropdown_model().row_count() as i32;
        let cursor = g.get_profile_edit_dropdown_cursor();
        match raw_key {
            key::UP => g.set_profile_edit_dropdown_cursor((cursor - 1).max(0)),
            key::DOWN => {
                g.set_profile_edit_dropdown_cursor((cursor + 1).min((model_len - 1).max(0)))
            }
            key::RETURN => {
                apply_profile_edit_dropdown_selection(g, cursor);
                g.set_profile_edit_dropdown_open(false);
            }
            key::ESCAPE | key::LEFT => g.set_profile_edit_dropdown_open(false),
            _ => {}
        }
        return true;
    }

    let zones = existing_profile_edit_zones(g);
    let zone = g.get_profile_edit_zone();
    let zone_pos = zones.iter().position(|&z| z == zone).unwrap_or(0);
    let prev_zone = || zone_pos.checked_sub(1).and_then(|i| zones.get(i)).copied();
    let next_zone = || zones.get(zone_pos + 1).copied();
    let goto = |g: &AppState, z: i32| {
        g.set_profile_edit_zone(z);
        profile_edit_zone_focus_reset(g, z);
    };

    match zone {
        // Zones 0/5/6 — Name / Blocked tags / Allowed tags. Enter opens the on-screen keyboard
        // without touching native focus: `fs` keeps it, so keys.rs's keyboard tier sees the next
        // key (grabbing and releasing LineEdit focus in one call was unreliable).
        // dispatch-onscreen-key edits the field's text directly. The field gets real focus when
        // Done closes the keyboard (_kb-close-mirror in profile_edit.slint).
        0 | 5 | 6 => match raw_key {
            key::RETURN => {
                let target = match zone {
                    0 => "profile-edit-name",
                    5 => "profile-edit-blocked-tags",
                    _ => "profile-edit-allowed-tags", // 6
                };
                debug!("profile-edit: zone={zone} opening onscreen keyboard target={target}");
                if !crate::keys::open_onscreen_keyboard(g, target) {
                    // Keyboard off in Settings: Enter starts typing in the field itself.
                    g.set_profile_edit_text_editing(true);
                }
            }
            key::UP => {
                if let Some(p) = prev_zone() {
                    goto(g, p);
                }
            }
            key::DOWN => {
                if let Some(n) = next_zone() {
                    goto(g, n);
                }
            }
            _ => {}
        },
        // Zone 1 — avatar color swatch strip.
        1 => {
            let cursor = g.get_profile_edit_avatar_cursor();
            match raw_key {
                key::LEFT => g.set_profile_edit_avatar_cursor((cursor - 1).max(0)),
                key::RIGHT => g.set_profile_edit_avatar_cursor((cursor + 1).min(7)),
                key::UP => {
                    if let Some(p) = prev_zone() {
                        goto(g, p);
                    }
                }
                key::DOWN => {
                    if let Some(n) = next_zone() {
                        goto(g, n);
                    }
                }
                key::RETURN => {
                    if let Some(hex) = AVATAR_PALETTE_HEX.get(cursor as usize) {
                        g.invoke_profile_edit_avatar_color_selected((*hex).into());
                    }
                }
                _ => {}
            }
        }
        // Zones 2/10 — the two PIN pads (own PIN / master confirmation PIN): the same 12-key grid
        // as the profile picker's PIN entry, including real keyboards — digit keys press the
        // matching key (and move the grid cursor), Backspace deletes the last digit.
        2 | 10 => {
            let is_master = zone == 10;
            let cursor = if is_master {
                g.get_profile_edit_master_pin_cursor()
            } else {
                g.get_profile_edit_pin_cursor()
            };
            let set_cursor = |g: &AppState, v: i32| {
                if is_master {
                    g.set_profile_edit_master_pin_cursor(v);
                } else {
                    g.set_profile_edit_pin_cursor(v);
                }
            };
            let send_key = |g: &AppState, v: &str| {
                if is_master {
                    g.invoke_profile_edit_master_pin_key(v.into());
                } else {
                    g.invoke_profile_edit_pin_key(v.into());
                }
            };
            match raw_key {
                key::LEFT => set_cursor(g, (cursor - 1).max(0)),
                key::RIGHT => set_cursor(g, (cursor + 1).min(11)),
                key::UP => {
                    if cursor < 3 {
                        if let Some(p) = prev_zone() {
                            goto(g, p);
                        }
                    } else {
                        set_cursor(g, cursor - 3);
                    }
                }
                key::DOWN => {
                    if cursor >= 9 {
                        if let Some(n) = next_zone() {
                            goto(g, n);
                        }
                    } else {
                        set_cursor(g, cursor + 3);
                    }
                }
                key::RETURN => {
                    if let Some(v) = PIN_VALS.get(cursor as usize) {
                        send_key(g, v);
                    }
                }
                key::BACKSPACE => {
                    set_cursor(g, 9);
                    send_key(g, "backspace");
                }
                digit
                    if digit.len() == 1
                        && digit.chars().next().is_some_and(|c| c.is_ascii_digit()) =>
                {
                    let d = digit.chars().next().unwrap();
                    let idx = if d == '0' {
                        10
                    } else {
                        d.to_digit(10).unwrap() as i32 - 1
                    };
                    set_cursor(g, idx);
                    send_key(g, digit);
                }
                _ => {}
            }
        }
        // Zones 3/7 — Max parental rating / Auto-lock dropdowns.
        3 | 7 => match raw_key {
            key::RETURN => {
                open_profile_edit_dropdown(if zone == 3 { "rating" } else { "lockout" }, g)
            }
            key::UP => {
                if let Some(p) = prev_zone() {
                    goto(g, p);
                }
            }
            key::DOWN => {
                if let Some(n) = next_zone() {
                    goto(g, n);
                }
            }
            _ => {}
        },
        // Zones 4/9 — Enabled libraries / Allowed devices checklists.
        4 | 9 => {
            let is_devices = zone == 9;
            let model = if is_devices {
                g.get_profile_edit_devices()
            } else {
                g.get_profile_edit_libraries()
            };
            let count = model.row_count() as i32;
            let cursor = if is_devices {
                g.get_profile_edit_devices_cursor()
            } else {
                g.get_profile_edit_libraries_cursor()
            };
            let set_cursor = |g: &AppState, v: i32| {
                if is_devices {
                    g.set_profile_edit_devices_cursor(v);
                } else {
                    g.set_profile_edit_libraries_cursor(v);
                }
            };
            match raw_key {
                key::UP => {
                    if cursor <= 0 {
                        if let Some(p) = prev_zone() {
                            goto(g, p);
                        }
                    } else {
                        set_cursor(g, cursor - 1);
                    }
                }
                key::DOWN => {
                    if cursor >= count - 1 {
                        if let Some(n) = next_zone() {
                            goto(g, n);
                        }
                    } else {
                        set_cursor(g, cursor + 1);
                    }
                }
                key::RETURN => {
                    if is_devices {
                        g.invoke_profile_edit_toggle_device(cursor);
                    } else {
                        g.invoke_profile_edit_toggle_library(cursor);
                    }
                }
                _ => {}
            }
        }
        // Zone 8 — "Skip PIN on this network" (LAN bypass toggle).
        8 => match raw_key {
            key::RETURN => g.set_profile_edit_lan_bypass(!g.get_profile_edit_lan_bypass()),
            key::UP => {
                if let Some(p) = prev_zone() {
                    goto(g, p);
                }
            }
            key::DOWN => {
                if let Some(n) = next_zone() {
                    goto(g, n);
                }
            }
            _ => {}
        },
        // Zone 11 — Delete(conditional)/Cancel/Save button row. Enter's
        // actual activation happens in Slint (a changed tracker on the
        // already-bumped kb-activate-pulse counter — Rust can't read live
        // LineEdit.text to build the Save call itself).
        11 => {
            let delete_shown = !g.get_profile_edit_is_create() && !g.get_profile_edit_is_self();
            let max_btn = if delete_shown { 2 } else { 1 };
            let focused = g.get_profile_edit_button_focused();
            match raw_key {
                key::LEFT => g.set_profile_edit_button_focused((focused - 1).max(0)),
                key::RIGHT => g.set_profile_edit_button_focused((focused + 1).min(max_btn)),
                key::UP => {
                    if let Some(p) = prev_zone() {
                        goto(g, p);
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
    true
}

pub(crate) fn on_profile_edit_delete(
    state: Arc<Mutex<FjordState>>,
    window: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let Some(w) = window.upgrade() else { return };
    let g = AppState::get(&w);
    // Defensive — the Delete button is already hidden in Slint whenever
    // profile-edit-is-self is true (self-delete makes no sense: it would
    // sign the master out of the very account it just used to delete
    // itself), but every other destructive action in this app pairs its
    // UI gate with a matching Rust-side check rather than trusting the
    // Slint condition alone.
    if g.get_profile_edit_is_self() {
        return;
    }
    let target_id = g.get_profile_edit_target_id().to_string();
    if target_id.is_empty() {
        return;
    }
    let (master_pin, client) = {
        let s = state.lock().unwrap();
        (
            (!s.profile_edit_master_pin_buffer.is_empty())
                .then(|| s.profile_edit_master_pin_buffer.clone()),
            s.client.clone(),
        )
    };
    let Some(client) = client else { return };

    g.set_profile_edit_saving(true);
    g.set_profile_edit_error(ss(""));

    let ww = window.clone();
    let state2 = Arc::clone(&state);
    let rt_task = rt.clone();
    rt.spawn(async move {
        match client
            .bonfire_delete_profile(&target_id, master_pin.as_deref())
            .await
        {
            Ok(()) => {
                {
                    let mut s = state2.lock().unwrap();
                    s.profile_edit_pin_buffer.clear();
                    s.profile_edit_master_pin_buffer.clear();
                }
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    let g = AppState::get(&w);
                    g.set_profile_edit_saving(false);
                    close_profile_edit_screen(&g);
                    open_manage_profiles_screen(&state2, &w, &rt_task);
                });
            }
            Err(e) => {
                warn!("profile delete failed: {e:#}");
                // Same fix as on_profile_edit_save's own Err branch above.
                {
                    let mut s = state2.lock().unwrap();
                    s.profile_edit_pin_buffer.clear();
                    s.profile_edit_master_pin_buffer.clear();
                }
                let msg = format!("{e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        g.set_profile_edit_saving(false);
                        g.set_profile_edit_pin_len(0);
                        g.set_profile_edit_master_pin_len(0);
                        g.set_profile_edit_error(ss(&msg));
                    }
                });
            }
        }
    });
}

// ── wire_profile_edit (moved from main(), 0.5.0 step 3) ──────────────────
/// Wires Manage Profiles + ProfileEditScreen: open_manage_profiles, manage_profiles_select,
/// manage_profiles_add, profile_edit_pin_key, profile_edit_master_pin_key,
/// profile_edit_avatar_color_selected, profile_edit_toggle_library, profile_edit_toggle_device,
/// profile_edit_cancel, profile_edit_save, profile_edit_delete.
pub(crate) fn wire_profile_edit(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── manage profiles / profile edit (Bonfire Phase 2, 2026-08-09) ────────────
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_manage_profiles(move || {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::open_manage_profiles_screen(&state, &w, &rt_handle);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_manage_profiles_select(move |user_id| {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::on_manage_profiles_select(&state, &w, &rt_handle, user_id);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_manage_profiles_add(move || {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::on_manage_profiles_add(&state, &w, &rt_handle);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_profile_edit_pin_key(move |key| {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::on_profile_edit_pin_key(&state, &w, key);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_profile_edit_master_pin_key(move |key| {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::on_profile_edit_master_pin_key(&state, &w, key);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_profile_edit_avatar_color_selected(move |hex| {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::on_profile_edit_avatar_color_selected(&w, hex);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_profile_edit_toggle_library(move |idx| {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::on_profile_edit_toggle_library(&w, idx);
            }
        });
    }
    {
        let window_weak = window.as_weak();
        AppState::get(&window).on_profile_edit_toggle_device(move |idx| {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::on_profile_edit_toggle_device(&w, idx);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        AppState::get(&window).on_profile_edit_cancel(move || {
            if let Some(w) = window_weak.upgrade() {
                profile_edit::on_profile_edit_cancel(&state, &w);
            }
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_profile_edit_save(move |name, blocked_tags, allowed_tags| {
            profile_edit::on_profile_edit_save(
                Arc::clone(&state),
                window_weak.clone(),
                rt_handle.clone(),
                name,
                blocked_tags,
                allowed_tags,
            );
        });
    }
    {
        let state = Arc::clone(&state);
        let window_weak = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_profile_edit_delete(move || {
            profile_edit::on_profile_edit_delete(
                Arc::clone(&state),
                window_weak.clone(),
                rt_handle.clone(),
            );
        });
    }
}
