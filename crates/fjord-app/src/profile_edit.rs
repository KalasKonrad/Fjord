// ── fjord-app · profile_edit.rs ──────────────────────────────────────────────
//   Bonfire Phase 2 (2026-08-09) — native profile create/edit/delete, on top
//   of the bonfire.rs client module Phase 1 step 5 already built.
//   open_manage_profiles_screen  fetches bonfire_list_profiles() for the active
//                       master (gated on !Config.active().is_bonfire — a
//                       Bonfire sub-profile can't manage siblings, per
//                       bonfire_list_profiles' own "all profiles under THIS
//                       master account" doc comment), builds
//                       ManageProfilesScreen's tile list from the result;
//                       filters the calling master's own profile_user_id out
//                       of the response first (2026-08-16, code review — /list
//                       includes it alongside real sub-profiles, same self-
//                       exclusion sync_bonfire_subprofiles already does);
//                       resets manage-profiles-cursor to 0 on every open
//   on_manage_profiles_select/-add  resolve a tile (via FjordState.manage_profiles_cache,
//                       the last fetch — avoids a second round trip just to
//                       open the edit form) -> open_profile_edit_screen
//   open_profile_edit_screen  populates ProfileEditScreen's AppState fields from
//                       an existing BonfireProfile (edit mode) or blanks
//                       (create mode); fetches bonfire_list_libraries()/
//                       bonfire_list_devices() in parallel to build the two
//                       checklists. max_parental_rating is NEVER pre-filled —
//                       BonfireProfile (the /list response) simply doesn't
//                       carry it, even though create/update both accept it;
//                       re-confirmed 2026-08-17 via Bonfire's real
//                       developer-api.md that NO endpoint anywhere ever
//                       returns it (write-only, a genuine upstream gap, not
//                       something Fjord can read around). Edit mode starts
//                       the field at UNKNOWN_RATING, an honest "we don't
//                       know" sentinel distinct from a real "Any" pick
//                       (live-questioned: silently defaulting to "Any" was
//                       misleading, since it looked like a confirmed
//                       value) — never sent to the server unless the user
//                       actually opens the dropdown and picks something,
//                       same "omit, don't send null" discipline every
//                       other field here already follows.
//   on_profile_edit_pin_key/-master_pin_key  digit accumulation into
//                       FjordState.profile_edit_pin_buffer/-master_pin_buffer
//                       — two separate buffers (the profile's own new PIN vs.
//                       the calling master's authorization PIN), same
//                       never-round-tripped-through-Slint discipline as
//                       profile.rs's own profile_pin_buffer
//   on_profile_edit_avatar_color_selected/-toggle_library/-toggle_device
//   on_profile_edit_save  builds Create/UpdateProfileRequest from AppState +
//                       the two PIN buffers, calls bonfire_create_profile/
//                       bonfire_update_profile, then re-opens
//                       ManageProfilesScreen with a fresh fetch on success;
//                       both PIN buffers + their -len display counterparts
//                       are cleared on FAILURE too now (2026-08-16, code
//                       review — previously only on success, leaving a wrong
//                       PIN in place for the next attempt to silently append onto)
//   on_profile_edit_delete  bonfire_delete_profile, same success path, same
//                       clear-on-failure fix as on_profile_edit_save
//   on_profile_edit_cancel  closes without saving, returns to the
//                       already-fetched ManageProfilesScreen list
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use anyhow::Result;
use fjord_api::models::{BonfireProfile, CreateProfileRequest, UpdateProfileRequest};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};
use tracing::warn;

use slint::Global;
use crate::config::{save_config, FjordState};
use crate::keys::key;
use crate::{AppState, MainWindow, ProfileTile, ToggleListItem};

fn ss(s: &str) -> SharedString { SharedString::from(s) }

const DEFAULT_AVATAR_HEX: &str = "#4a90d9";

// Kept in lockstep by hand with profile_edit.slint's own
// avatar-palette-colors/-hex parallel arrays — same caveat that file's own
// header comment already documents for those two, just a third copy now
// (needed here so keyboard Enter on the avatar zone can resolve a cursor
// position to a hex string without round-tripping through Slint).
const AVATAR_PALETTE_HEX: [&str; 8] = [
    "#4a90d9", "#d94a6b", "#4ad98e", "#d9a04a",
    "#9a4ad9", "#4ac9d9", "#d9d94a", "#d96b4a",
];

// Same values as profile_edit.slint's two SettingsDropdown `model:` arrays
// — kept in lockstep by hand, same caveat as AVATAR_PALETTE_HEX above.
// "Any"/"Never" are the display sentinels for the stored ""/"0" values,
// matching each dropdown's own `selected(v) => ...` translation in Slint.
const PARENTAL_RATING_MODEL: [&str; 13] = [
    "Any", "G", "PG", "PG-13", "R", "NC-17",
    "TV-Y", "TV-Y7", "TV-G", "TV-PG", "TV-14", "TV-MA", "Not Rated",
];
// Live-questioned 2026-08-17 ("thats a bit bad to not know what parental
// ration a profile is on") — re-verified directly against Bonfire's real
// developer-api.md (WebFetch) rather than assumed a second time: confirmed
// genuinely no endpoint anywhere ever returns a profile's current
// maxParentalRating — it's write-only (accepted by create/update, absent
// from every GET response including /list, /admin/mappings, everything).
// A real upstream limitation, not something Fjord can read around. Silently
// defaulting the Edit dropdown to "Any" was actively misleading, though —
// it looked like a confirmed value when it was really just "we have no
// idea." This sentinel is the honest middle ground: shown ONLY as the
// pre-interaction display state (never a real, pickable dropdown option —
// PARENTAL_RATING_MODEL above is unchanged), so it's visually and
// functionally distinct from actually choosing "Any" (a deliberate "no
// restriction" pick, once the user has genuinely opened the dropdown and
// selected it). Must be treated as equivalent to "" (omit from the save
// request) everywhere parental_rating.is_empty() is checked — see
// on_profile_edit_save's own updated check.
const UNKNOWN_RATING: &str = "__unknown__";
const LOCKOUT_MODEL: [&str; 6] = ["Never", "5", "15", "30", "60", "120"];

// Same 12-key row-major layout as profile.rs's own PIN entry (the profile
// picker's PIN_VALS) — VirtualKeyboard's real key order, duplicated here
// rather than shared since it's a 12-element literal with no natural home
// in a third file both would import from.
const PIN_VALS: [&str; 12] = ["1", "2", "3", "4", "5", "6", "7", "8", "9", "backspace", "0", "confirm"];

fn default_avatar_color() -> slint::Color { slint::Color::from_rgb_u8(0x4a, 0x90, 0xd9) }

fn bonfire_profile_to_tile(p: &BonfireProfile) -> ProfileTile {
    ProfileTile {
        user_id:        ss(&p.profile_user_id),
        display_name:   ss(&p.profile_name),
        avatar_color:   crate::profile::parse_hex_color(&p.avatar_color).unwrap_or_else(default_avatar_color),
        avatar_initial: ss(&p.avatar_initial),
        has_pin:        p.has_pin,
        requires_pin:   p.requires_pin,
        is_bonfire:     p.is_bonfire,
    }
}

/// Fetches every sub-profile under the active master account and shows
/// ManageProfilesScreen. Gated on `!Config.active().is_bonfire` — a Bonfire
/// sub-profile isn't a master, so it shouldn't reach this at all (Settings'
/// own row is gated the same way; this is the defensive second check).
pub(crate) fn open_manage_profiles_screen(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    let g = AppState::get(window);
    let (client, is_bonfire) = {
        let s = state.lock().unwrap();
        (s.client.clone(), s.config.active().is_bonfire)
    };
    if is_bonfire {
        crate::show_toast(window.as_weak(), "Only a master account can manage profiles".to_string());
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
                if !crate::session_current(&state2, &client) { return; }
                // Real bug, code-review 2026-08-16: Bonfire's own /list
                // response includes the calling master's own profile
                // alongside its real sub-profiles — sync_bonfire_subprofiles
                // (profile.rs) already filters this exact case out (see its
                // own doc comment, and the real corruption bug that fix
                // closed), but this screen hit the same endpoint with no
                // equivalent filter, showing the master's own tile mixed
                // into the sub-profile grid — selectable, editable, and
                // (via Delete) attemptable against its own account. Also
                // inflated the "< 5 profiles" Add-Profile cap check by one.
                let master_id = client.user_id.clone();
                // Real bug, live-reported 2026-08-17: "the add profile did
                // not dissapear when max profiles for master accaunt was
                // reached" — Bonfire's real cap is per-master
                // (max_sub_profiles, admin-adjustable server-side), not the
                // hardcoded "5" this screen and its keyboard clamp both
                // used before. Only the MASTER's own /list entry — the one
                // about to be filtered out — carries this field, so it has
                // to be read here, before the filter runs. 0/absent (a
                // server that never populated it) falls back to Bonfire's
                // own documented default of 5, matching the old behavior.
                let max_sub_profiles = profiles.iter()
                    .find(|p| p.profile_user_id == master_id)
                    .map(|p| p.max_sub_profiles)
                    .filter(|&n| n > 0)
                    .unwrap_or(5);
                let profiles: Vec<_> = profiles.into_iter()
                    .filter(|p| p.profile_user_id != master_id)
                    .collect();
                let tiles: Vec<ProfileTile> = profiles.iter().map(bonfire_profile_to_tile).collect();
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
                    if let Some(w) = ww.upgrade() { AppState::get(&w).set_manage_profiles_error(ss(&msg)); }
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
        s.manage_profiles_cache.iter().find(|p| p.profile_user_id == user_id.as_str()).cloned()
    };
    let Some(existing) = existing else {
        AppState::get(window).set_manage_profiles_error(ss("That profile is no longer available"));
        return;
    };
    open_profile_edit_screen(state, window, rt, Some(existing), false);
}

pub(crate) fn on_manage_profiles_add(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    open_profile_edit_screen(state, window, rt, None, false);
}

/// The master editing ITSELF — 2026-08-17, live-questioned ("shuld they
/// not be able to changepin etc on there own profile?"). Distinct entry
/// point from Manage Profiles (which deliberately excludes the master's
/// own tile — Finding 2, code review 2026-08-16 — since Bonfire's own
/// `/list` response mixes the caller's own entry in with real
/// sub-profiles): that exclusion was about not letting the master
/// accidentally *delete itself* via a screen meant for managing
/// subordinates, not about self-editing being unsupported. Confirmed via
/// the real Bonfire API docs (fetched directly, not assumed) that
/// create/update/delete all require master-token auth — sub-profiles can
/// NEVER self-manage (a hard server-side limitation, not a Fjord gap) —
/// but nothing in the docs rules out the master targeting its own
/// `profileId`, so this is offered, gated the same defensive way Manage
/// Profiles already is. Whether the server actually accepts a
/// self-targeted `update` call is unverified either way — real "needs a
/// live test" territory, same as the rest of this crate's Bonfire module.
pub(crate) fn open_my_profile_edit_screen(state: &Arc<Mutex<FjordState>>, window: &MainWindow, rt: &tokio::runtime::Handle) {
    let (client, is_bonfire) = {
        let s = state.lock().unwrap();
        (s.client.clone(), s.config.active().is_bonfire)
    };
    if is_bonfire {
        crate::show_toast(window.as_weak(), "Only a master account can edit its own profile here".to_string());
        return;
    }
    let Some(client) = client else { return };
    let ww = window.as_weak();
    let state2 = Arc::clone(state);
    let rt2 = rt.clone();
    rt.spawn(async move {
        match client.bonfire_list_profiles().await {
            Ok(profiles) => {
                if !crate::session_current(&state2, &client) { return; }
                let mine = profiles.into_iter().find(|p| p.profile_user_id == client.user_id);
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    match mine {
                        Some(p) => open_profile_edit_screen(&state2, &w, &rt2, Some(p), true),
                        None => crate::show_toast(w.as_weak(), "Couldn't find your own profile — is Bonfire installed on this server?".to_string()),
                    }
                });
            }
            Err(e) => {
                warn!("open_my_profile_edit_screen: bonfire_list_profiles: {e:#}");
                let msg = format!("{e:#}");
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() { crate::show_toast(w.as_weak(), msg); }
                });
            }
        }
    });
}

/// `existing: None` = create mode, `Some(profile)` = edit mode.
/// `is_self`: the master editing its own profile (see
/// `open_my_profile_edit_screen`'s own doc comment) rather than a
/// sub-profile via Manage Profiles — hides Delete and changes where
/// Save/Cancel return to. Sets every AppState field synchronously (so the
/// screen shows correctly-populated content the instant it appears,
/// matching every other screen-open function's own "no flash of stale
/// data" precedent) before the async libraries/devices fetch runs.
pub(crate) fn open_profile_edit_screen(
    state:    &Arc<Mutex<FjordState>>,
    window:   &MainWindow,
    rt:       &tokio::runtime::Handle,
    existing: Option<BonfireProfile>,
    is_self:  bool,
) {
    let g = AppState::get(window);
    let is_create = existing.is_none();
    g.set_profile_edit_is_self(is_self);
    let color_hex = existing.as_ref()
        .map(|p| p.avatar_color.clone())
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| DEFAULT_AVATAR_HEX.to_string());

    g.set_show_manage_profiles(false);
    g.set_profile_edit_is_create(is_create);
    g.set_profile_edit_target_id(ss(existing.as_ref().map(|p| p.profile_user_id.as_str()).unwrap_or("")));
    g.set_profile_edit_name_initial(ss(existing.as_ref().map(|p| p.profile_name.as_str()).unwrap_or("")));
    g.set_profile_edit_avatar_color(ss(&color_hex));
    g.set_profile_edit_avatar_preview(crate::profile::parse_hex_color(&color_hex).unwrap_or_else(default_avatar_color));
    g.set_profile_edit_pin_len(0);
    g.set_profile_edit_has_pin(existing.as_ref().map(|p| p.has_pin).unwrap_or(false));
    g.set_profile_edit_master_pin_len(0);
    // Create mode: a brand-new profile genuinely has no rating restriction
    // yet, so "Any" (empty string) is accurate. Edit mode: Bonfire never
    // reports the CURRENT value (see UNKNOWN_RATING's own doc comment) —
    // start at the honest "Unknown" sentinel instead of a misleading "Any".
    g.set_profile_edit_parental_rating(ss(if is_create { "" } else { UNKNOWN_RATING }));
    g.set_profile_edit_blocked_tags_initial(ss(&existing.as_ref().map(|p| p.blocked_tags.join(", ")).unwrap_or_default()));
    g.set_profile_edit_allowed_tags_initial(ss(&existing.as_ref().map(|p| p.allowed_tags.join(", ")).unwrap_or_default()));
    g.set_profile_edit_lockout_minutes(ss(&existing.as_ref().map(|p| p.lockout_minutes.to_string()).unwrap_or_else(|| "0".to_string())));
    g.set_profile_edit_lan_bypass(existing.as_ref().map(|p| p.bypass_pin_on_local_network).unwrap_or(false));
    g.set_profile_edit_saving(false);
    g.set_profile_edit_error(ss(""));
    g.set_profile_edit_libraries(ModelRc::new(VecModel::<ToggleListItem>::default()));
    g.set_profile_edit_devices(ModelRc::new(VecModel::<ToggleListItem>::default()));

    // Full D-pad retrofit, 2026-08-17 — every new zone/cursor property (see
    // app_state.slint's own profile-edit-zone doc comment for the full zone
    // list) reset to its starting position on every open, same "no stale
    // state left over from a previous visit" discipline the ~18 fields
    // above already follow. zone=0 is the first real zone (Name).
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
            existing.as_ref().map(|p| p.enabled_folders.clone()).unwrap_or_default(),
            existing.as_ref().map(|p| p.allowed_device_ids.clone()).unwrap_or_default(),
        )
    };
    let Some(client) = client else { return };
    let ww = window.as_weak();
    let state2 = Arc::clone(state);
    rt.spawn(async move {
        let (libs_res, devices_res) = tokio::join!(client.bonfire_list_libraries(), client.bonfire_list_devices());
        if !crate::session_current(&state2, &client) { return; }
        let libraries: Vec<ToggleListItem> = match libs_res {
            Ok(libs) => libs.into_iter().map(|l| ToggleListItem {
                id: ss(&l.id), name: ss(&l.name), subtitle: ss(&l.collection_type),
                // Create mode: nothing owned yet — default every library
                // enabled (opt-out), matching Jellyfin's own new-user
                // default of full library access. Edit mode: pre-select
                // whatever enabled_folders already lists.
                selected: if is_create { true } else { enabled_folders.contains(&l.id) },
            }).collect(),
            Err(e) => { warn!("bonfire_list_libraries: {e:#}"); vec![] }
        };
        let devices: Vec<ToggleListItem> = match devices_res {
            Ok(devs) => devs.into_iter().map(|d| ToggleListItem {
                id: ss(&d.device_id), name: ss(&d.device_name),
                subtitle: ss(&format!("{} · last seen {}", d.client, d.last_seen)),
                selected: allowed_device_ids.contains(&d.device_id),
            }).collect(),
            Err(e) => { warn!("bonfire_list_devices: {e:#}"); vec![] }
        };
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            g.set_profile_edit_libraries(ModelRc::new(VecModel::from(libraries)));
            g.set_profile_edit_devices(ModelRc::new(VecModel::from(devices)));
        });
    });
}

pub(crate) fn on_profile_edit_pin_key(state: &Arc<Mutex<FjordState>>, window: &MainWindow, key: SharedString) {
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

pub(crate) fn on_profile_edit_master_pin_key(state: &Arc<Mutex<FjordState>>, window: &MainWindow, key: SharedString) {
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
    g.set_profile_edit_avatar_preview(crate::profile::parse_hex_color(hex.as_str()).unwrap_or_else(default_avatar_color));
    g.set_profile_edit_avatar_color(hex);
}

pub(crate) fn on_profile_edit_toggle_library(window: &MainWindow, idx: i32) {
    let model = AppState::get(window).get_profile_edit_libraries();
    let Some(mut item) = model.row_data(idx as usize) else { return };
    item.selected = !item.selected;
    model.set_row_data(idx as usize, item);
}

pub(crate) fn on_profile_edit_toggle_device(window: &MainWindow, idx: i32) {
    let model = AppState::get(window).get_profile_edit_devices();
    let Some(mut item) = model.row_data(idx as usize) else { return };
    item.selected = !item.selected;
    model.set_row_data(idx as usize, item);
}

pub(crate) fn on_profile_edit_cancel(state: &Arc<Mutex<FjordState>>, window: &MainWindow) {
    let g = AppState::get(window);
    g.set_show_profile_edit(false);
    {
        let mut s = state.lock().unwrap();
        s.profile_edit_pin_buffer.clear();
        s.profile_edit_master_pin_buffer.clear();
    }
    // Manage Profiles is the entry point that opened this screen, UNLESS
    // it was "Edit My Profile" (is_self) — that one has no Manage
    // Profiles list to return to, since it's opened directly from the
    // sidebar. Either way its list, if it exists, is still whatever the
    // last fetch left it at — no re-fetch needed since nothing was saved.
    if !g.get_profile_edit_is_self() {
        g.set_show_manage_profiles(true);
    }
    window.invoke_grab_keyboard_focus();
}

fn split_tags(csv: &str) -> Vec<String> {
    csv.split(',').map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect()
}

fn selected_ids(model: &ModelRc<ToggleListItem>) -> Vec<String> {
    (0..model.row_count())
        .filter_map(|i| model.row_data(i))
        .filter(|item| item.selected)
        .map(|item| item.id.to_string())
        .collect()
}

pub(crate) fn on_profile_edit_save(
    state:  Arc<Mutex<FjordState>>,
    window: slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
    name:   SharedString,
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
    let is_create      = g.get_profile_edit_is_create();
    let is_self        = g.get_profile_edit_is_self();
    let target_id       = g.get_profile_edit_target_id().to_string();
    let avatar_color    = g.get_profile_edit_avatar_color().to_string();
    let parental_rating = g.get_profile_edit_parental_rating().to_string();
    let lockout_minutes: i64 = g.get_profile_edit_lockout_minutes().parse().unwrap_or(0);
    let lan_bypass       = g.get_profile_edit_lan_bypass();
    let enabled_folders  = selected_ids(&g.get_profile_edit_libraries());
    let allowed_device_ids = selected_ids(&g.get_profile_edit_devices());
    let blocked_tags = split_tags(&blocked_tags_csv);
    let allowed_tags = split_tags(&allowed_tags_csv);

    let (pin, master_pin, client) = {
        let s = state.lock().unwrap();
        (
            (!s.profile_edit_pin_buffer.is_empty()).then(|| s.profile_edit_pin_buffer.clone()),
            (!s.profile_edit_master_pin_buffer.is_empty()).then(|| s.profile_edit_master_pin_buffer.clone()),
            s.client.clone(),
        )
    };
    let Some(client) = client else { return };

    // Cloned before the request-building code below moves the originals —
    // needed afterward, in the success branch, only for is_self's own
    // local ProfileSettings update (see that branch's own comment for why
    // this doesn't apply to the ordinary Manage-Profiles-editing-a-sub-
    // profile case, which has no equivalent local record to keep in sync).
    let pin_was_set          = pin.is_some();
    let name_for_local       = name.clone();
    let avatar_color_for_local = avatar_color.clone();

    g.set_profile_edit_saving(true);
    g.set_profile_edit_error(ss(""));

    let ww       = window.clone();
    let state2   = Arc::clone(&state);
    let rt_task  = rt.clone();
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
                max_parental_rating: (!parental_rating.is_empty() && parental_rating != UNKNOWN_RATING)
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
                max_parental_rating: (!parental_rating.is_empty() && parental_rating != UNKNOWN_RATING)
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
                    g.set_show_profile_edit(false);
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
                            if !avatar_color_for_local.is_empty() { p.avatar_color = avatar_color_for_local.clone(); }
                            p.avatar_initial.clear(); // re-derive from the (possibly new) name — see ProfileTile's own fallback
                            if pin_was_set { p.has_pin = true; } // blank PIN field means "keep the current one," never a removal
                            s.config.clone()
                        };
                        save_config(&cfg);
                        crate::profile::push_current_profile_tile(&g, &cfg);
                        crate::profile::refresh_profile_settings_dropdown(&g, &cfg);
                        crate::profile::refresh_account_settings_dropdown(&g, &cfg);
                    } else {
                        // Real bug, live-reported 2026-08-17: setting a PIN
                        // on a sub-profile via Manage Profiles, then
                        // immediately trying to switch to it, failed with a
                        // raw 400 from Bonfire's own /switch — traced to
                        // Fjord attempting a PASSWORDLESS switch (no `pin`
                        // param at all), because the LOCAL Config.profiles
                        // entry for that sub-profile (what
                        // should_show_picker_at_startup/switch_to_profile/
                        // the picker's own tiles all actually read — never
                        // a live Bonfire fetch) still had `has_pin: false`.
                        // The only thing that ever updates that local entry
                        // is sync_bonfire_subprofiles, called at
                        // login/switch/auto-login — never after an ordinary
                        // profile EDIT, so a PIN (or name/avatar) change
                        // made right here just sat unreflected locally
                        // until the next full session-setup happened to run
                        // for the master. Fixed by calling the same
                        // sync_bonfire_subprofiles primitive here too — it
                        // already does exactly the right thing (patches
                        // display_name/avatar_color/avatar_initial/has_pin
                        // on a matching existing entry, or adds one for a
                        // brand-new profile just created via this same
                        // screen — a second, related gap this closes as a
                        // side effect, since a fresh Add-Profile previously
                        // wouldn't appear in Config.profiles until the next
                        // login/switch either).
                        crate::profile::sync_bonfire_subprofiles(Arc::clone(&client), Arc::clone(&state2), rt_task.clone(), ww.clone());
                        // Fresh fetch, not the stale pre-save list — the just-
                        // created/edited profile needs to show up/update.
                        open_manage_profiles_screen(&state2, &w, &rt_task);
                    }
                });
            }
            Err(e) => {
                warn!("profile save failed: {e:#}");
                // Real bug, code-review 2026-08-16: only the Ok branch
                // cleared these, contradicting profile_edit_pin_buffer's
                // own "same discipline [as profile_pin_buffer]" doc
                // comment — a failed save left both PIN pads holding their
                // typed digits, so retyping without noticing appended onto
                // the already-wrong value instead of starting clean.
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

// ── Full D-pad keyboard navigation, 2026-08-17 ───────────────────────────────
// Live-reported twice ("no keybord navigation in manage profiles" / "still
// no keybord nav in edit profile"); AskUserQuestion confirmed the user
// wanted the whole screen, not just the two PIN pads. Dispatched from
// keys.rs's show_profile_edit raw-key tier via handle_key_profile_edit,
// mirroring discover.rs::handle_key_request_options's own factoring for a
// comparably-sized multi-zone screen rather than growing keys.rs's own
// match arms indefinitely. See app_state.slint's profile-edit-zone doc
// comment for the full 12-zone list.

/// The "gaps are fine" filtered zone list (same idiom as
/// discover.rs::existing_option_zones) — zones 4/9 (the libraries/devices
/// checklists) are skipped when their fetched list is empty. Must stay
/// hand-in-lockstep with profile_edit.slint's own two
/// `if AppState.profile-edit-libraries/-devices.length > 0` gates — the
/// exact class of drift this codebase already documents once for
/// context_menu.rs/.slint (Phase 163).
fn existing_profile_edit_zones(g: &AppState) -> Vec<i32> {
    let mut zones = vec![0, 1, 2, 3];
    if g.get_profile_edit_libraries().row_count() > 0 { zones.push(4); }
    zones.extend([5, 6, 7, 8]);
    if g.get_profile_edit_devices().row_count() > 0 { zones.push(9); }
    zones.extend([10, 11]);
    zones
}

/// Resets the entered zone's own sub-cursor to 0 — called on every zone
/// transition so leftover cursor state from a previous visit to that zone
/// never survives (mirrors discover.rs::option_zone_focus_reset).
fn profile_edit_zone_focus_reset(g: &AppState, zone: i32) {
    match zone {
        1  => g.set_profile_edit_avatar_cursor(0),
        2  => g.set_profile_edit_pin_cursor(0),
        4  => g.set_profile_edit_libraries_cursor(0),
        9  => g.set_profile_edit_devices_cursor(0),
        10 => g.set_profile_edit_master_pin_cursor(0),
        11 => g.set_profile_edit_button_focused(0),
        _  => {}
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
            if v.is_empty() { "Any".to_string() }
            else if v == UNKNOWN_RATING {
                // Not a real option in PARENTAL_RATING_MODEL — position()
                // below correctly falls back to cursor 0 ("Any"), just a
                // reasonable starting point for the browse, not a claim
                // that's actually the current value.
                "Unknown".to_string()
            } else { v }
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
        model.iter().map(|v| ss(v)).collect::<Vec<_>>()
    )));
    g.set_profile_edit_dropdown_cursor(cursor);
    g.set_profile_edit_dropdown_display(ss(&current));
    g.set_profile_edit_dropdown_open(true);
}

/// Confirms whichever row the dropdown popup's cursor is on, translating
/// the "Any"/"Never" display sentinels back to the stored ""/"0" values —
/// same translation each SettingsDropdown's own `selected(v) => ...`
/// handler already does in Slint for the mouse path.
pub(crate) fn apply_profile_edit_dropdown_selection(g: &AppState, cursor: i32) {
    let dd_key = g.get_profile_edit_dropdown_key().to_string();
    let Some(v) = g.get_profile_edit_dropdown_model().row_data(cursor as usize) else { return };
    match dd_key.as_str() {
        "rating"  => g.set_profile_edit_parental_rating(if v.as_str() == "Any" { ss("") } else { v }),
        "lockout" => g.set_profile_edit_lockout_minutes(if v.as_str() == "Never" { ss("0") } else { v }),
        _ => {}
    }
}

/// Main raw-key dispatch for ProfileEditScreen, called from keys.rs's
/// show_profile_edit tier for every key except Ctrl+Q (handled there) and
/// Escape-while-not-text-editing (also handled there, since it needs to
/// distinguish "close the dropdown popup" from "cancel the whole screen").
/// Escape while a LineEdit holds real Slint focus never reaches this
/// function at all — it's consumed entirely by that field's own
/// key-pressed(event) hook in profile_edit.slint (Slint's key routing
/// walks only the focused item's own ancestor chain, confirmed against
/// i-slint-core's own source before relying on it).
pub(crate) fn handle_key_profile_edit(raw_key: &str, g: &AppState) -> bool {
    // Top-priority sub-state: the dropdown popup (zones 3/7's own Enter).
    if g.get_profile_edit_dropdown_open() {
        let model_len = g.get_profile_edit_dropdown_model().row_count() as i32;
        let cursor = g.get_profile_edit_dropdown_cursor();
        match raw_key {
            key::UP => g.set_profile_edit_dropdown_cursor((cursor - 1).max(0)),
            key::DOWN => g.set_profile_edit_dropdown_cursor((cursor + 1).min((model_len - 1).max(0))),
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
    let goto = |g: &AppState, z: i32| { g.set_profile_edit_zone(z); profile_edit_zone_focus_reset(g, z); };

    match zone {
        // Zones 0/5/6 — Name / Blocked tags / Allowed tags. Enter hands off
        // to native LineEdit focus (a Slint-side changed-tracker on
        // profile-edit-text-editing performs the actual .focus() call —
        // Rust has no way to call a named element's method directly).
        0 | 5 | 6 => match raw_key {
            key::RETURN => g.set_profile_edit_text_editing(true),
            key::UP     => if let Some(p) = prev_zone() { goto(g, p); },
            key::DOWN   => if let Some(n) = next_zone() { goto(g, n); },
            _ => {}
        },
        // Zone 1 — avatar color swatch strip.
        1 => {
            let cursor = g.get_profile_edit_avatar_cursor();
            match raw_key {
                key::LEFT  => g.set_profile_edit_avatar_cursor((cursor - 1).max(0)),
                key::RIGHT => g.set_profile_edit_avatar_cursor((cursor + 1).min(7)),
                key::UP    => if let Some(p) = prev_zone() { goto(g, p); },
                key::DOWN  => if let Some(n) = next_zone() { goto(g, n); },
                key::RETURN => if let Some(hex) = AVATAR_PALETTE_HEX.get(cursor as usize) {
                    g.invoke_profile_edit_avatar_color_selected((*hex).into());
                },
                _ => {}
            }
        }
        // Zones 2/10 — the two PIN pads (own PIN / master confirmation PIN).
        // Same 12-key row-major grid math as the profile picker's own PIN
        // entry (keys.rs::show_profile_picker) — including the identical
        // real-keyboard fix (2026-08-17, live-reported: "you cant use numpad
        // or numbers if you have a real keybord and backspace dont work"):
        // raw digit keys act as if the matching on-screen key was pressed
        // (syncing the grid cursor to match, same mouse-sync discipline as
        // every click handler on this screen), and Backspace deletes the
        // last digit rather than doing nothing — this zone never gave
        // Backspace any meaning before, so repurposing it is a pure addition,
        // not a behavior change.
        2 | 10 => {
            let is_master = zone == 10;
            let cursor = if is_master { g.get_profile_edit_master_pin_cursor() } else { g.get_profile_edit_pin_cursor() };
            let set_cursor = |g: &AppState, v: i32| {
                if is_master { g.set_profile_edit_master_pin_cursor(v); } else { g.set_profile_edit_pin_cursor(v); }
            };
            let send_key = |g: &AppState, v: &str| {
                if is_master { g.invoke_profile_edit_master_pin_key(v.into()); }
                else { g.invoke_profile_edit_pin_key(v.into()); }
            };
            match raw_key {
                key::LEFT  => set_cursor(g, (cursor - 1).max(0)),
                key::RIGHT => set_cursor(g, (cursor + 1).min(11)),
                key::UP    => if cursor < 3 { if let Some(p) = prev_zone() { goto(g, p); } } else { set_cursor(g, cursor - 3); },
                key::DOWN  => if cursor >= 9 { if let Some(n) = next_zone() { goto(g, n); } } else { set_cursor(g, cursor + 3); },
                key::RETURN => if let Some(v) = PIN_VALS.get(cursor as usize) {
                    send_key(g, v);
                },
                key::BACKSPACE => {
                    set_cursor(g, 9);
                    send_key(g, "backspace");
                }
                digit if digit.len() == 1 && digit.chars().next().is_some_and(|c| c.is_ascii_digit()) => {
                    let d = digit.chars().next().unwrap();
                    let idx = if d == '0' { 10 } else { d.to_digit(10).unwrap() as i32 - 1 };
                    set_cursor(g, idx);
                    send_key(g, digit);
                }
                _ => {}
            }
        }
        // Zones 3/7 — Max parental rating / Auto-lock dropdowns.
        3 | 7 => match raw_key {
            key::RETURN => open_profile_edit_dropdown(if zone == 3 { "rating" } else { "lockout" }, g),
            key::UP     => if let Some(p) = prev_zone() { goto(g, p); },
            key::DOWN   => if let Some(n) = next_zone() { goto(g, n); },
            _ => {}
        },
        // Zones 4/9 — Enabled libraries / Allowed devices checklists.
        4 | 9 => {
            let is_devices = zone == 9;
            let model = if is_devices { g.get_profile_edit_devices() } else { g.get_profile_edit_libraries() };
            let count = model.row_count() as i32;
            let cursor = if is_devices { g.get_profile_edit_devices_cursor() } else { g.get_profile_edit_libraries_cursor() };
            let set_cursor = |g: &AppState, v: i32| {
                if is_devices { g.set_profile_edit_devices_cursor(v); } else { g.set_profile_edit_libraries_cursor(v); }
            };
            match raw_key {
                key::UP => if cursor <= 0 {
                    if let Some(p) = prev_zone() { goto(g, p); }
                } else {
                    set_cursor(g, cursor - 1);
                },
                key::DOWN => if cursor >= count - 1 {
                    if let Some(n) = next_zone() { goto(g, n); }
                } else {
                    set_cursor(g, cursor + 1);
                },
                key::RETURN => if is_devices { g.invoke_profile_edit_toggle_device(cursor); }
                    else { g.invoke_profile_edit_toggle_library(cursor); },
                _ => {}
            }
        }
        // Zone 8 — "Skip PIN on this network" (LAN bypass toggle).
        8 => match raw_key {
            key::RETURN => g.set_profile_edit_lan_bypass(!g.get_profile_edit_lan_bypass()),
            key::UP     => if let Some(p) = prev_zone() { goto(g, p); },
            key::DOWN   => if let Some(n) = next_zone() { goto(g, n); },
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
                key::LEFT  => g.set_profile_edit_button_focused((focused - 1).max(0)),
                key::RIGHT => g.set_profile_edit_button_focused((focused + 1).min(max_btn)),
                key::UP    => if let Some(p) = prev_zone() { goto(g, p); },
                _ => {}
            }
        }
        _ => {}
    }
    true
}

pub(crate) fn on_profile_edit_delete(state: Arc<Mutex<FjordState>>, window: slint::Weak<MainWindow>, rt: tokio::runtime::Handle) {
    let Some(w) = window.upgrade() else { return };
    let g = AppState::get(&w);
    // Defensive — the Delete button is already hidden in Slint whenever
    // profile-edit-is-self is true (self-delete makes no sense: it would
    // sign the master out of the very account it just used to delete
    // itself), but every other destructive action in this app pairs its
    // UI gate with a matching Rust-side check rather than trusting the
    // Slint condition alone.
    if g.get_profile_edit_is_self() { return; }
    let target_id = g.get_profile_edit_target_id().to_string();
    if target_id.is_empty() { return; }
    let (master_pin, client) = {
        let s = state.lock().unwrap();
        ((!s.profile_edit_master_pin_buffer.is_empty()).then(|| s.profile_edit_master_pin_buffer.clone()), s.client.clone())
    };
    let Some(client) = client else { return };

    g.set_profile_edit_saving(true);
    g.set_profile_edit_error(ss(""));

    let ww      = window.clone();
    let state2  = Arc::clone(&state);
    let rt_task = rt.clone();
    rt.spawn(async move {
        match client.bonfire_delete_profile(&target_id, master_pin.as_deref()).await {
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
                    g.set_show_profile_edit(false);
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
