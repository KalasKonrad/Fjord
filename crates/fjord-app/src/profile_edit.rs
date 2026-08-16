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
//                       left at "" (omitted from the save request via
//                       skip_serializing_if) this can't clobber whatever's
//                       already set server-side, which is the same "omit,
//                       don't send null" discipline every other field here
//                       already follows.
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
use crate::{AppState, MainWindow, ProfileTile, ToggleListItem};

fn ss(s: &str) -> SharedString { SharedString::from(s) }

const DEFAULT_AVATAR_HEX: &str = "#4a90d9";

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
                let profiles: Vec<_> = profiles.into_iter()
                    .filter(|p| p.profile_user_id != master_id)
                    .collect();
                let tiles: Vec<ProfileTile> = profiles.iter().map(bonfire_profile_to_tile).collect();
                state2.lock().unwrap().manage_profiles_cache = profiles;
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        AppState::get(&w).set_manage_profiles_list(ModelRc::new(VecModel::from(tiles)));
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
    g.set_profile_edit_parental_rating(ss(""));
    g.set_profile_edit_blocked_tags_initial(ss(&existing.as_ref().map(|p| p.blocked_tags.join(", ")).unwrap_or_default()));
    g.set_profile_edit_allowed_tags_initial(ss(&existing.as_ref().map(|p| p.allowed_tags.join(", ")).unwrap_or_default()));
    g.set_profile_edit_lockout_minutes(ss(&existing.as_ref().map(|p| p.lockout_minutes.to_string()).unwrap_or_else(|| "0".to_string())));
    g.set_profile_edit_lan_bypass(existing.as_ref().map(|p| p.bypass_pin_on_local_network).unwrap_or(false));
    g.set_profile_edit_saving(false);
    g.set_profile_edit_error(ss(""));
    g.set_profile_edit_libraries(ModelRc::new(VecModel::<ToggleListItem>::default()));
    g.set_profile_edit_devices(ModelRc::new(VecModel::<ToggleListItem>::default()));

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
                max_parental_rating: (!parental_rating.is_empty()).then_some(parental_rating),
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
                max_parental_rating: (!parental_rating.is_empty()).then_some(parental_rating),
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
