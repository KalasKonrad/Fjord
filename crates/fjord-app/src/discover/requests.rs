// ── fjord-app · discover/requests.rs ────────────────────────────────────────
//   discover_toggle_blocklist  POST/DELETE blocklist; patches availability everywhere, removes the
//                              card from Discover models on add
//   read_current_request_preference / store_request_preference  Request Options' remembered choice
//   submit_request             POST /request (seasons, is4k, tags, profileId — 0/Default omitted); on
//                              success flips only the requested tier's status, patches the card, adds
//                              to the Watchlist (one combined toast), refreshes the Requested row
//   submit_edit_request        PUT the existing request (tier can't change)
//   discover_request_action    Cancel / Approve / Decline by request id; syncs known requests, reloads
//                              an open RequestDetailScreen for the same item
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// Add/remove Blocklist (Discover context menu row, RequestDetailScreen button):
/// POST/DELETE, then patch `availability` on every visible card and RequestDetail's
/// status fields if it shows the same item. No id set (unlike Watchlist): Blocklisted is
/// a `MediaStatus` value every card already carries (`availability_tag`). An available
/// library item is never blocklist-eligible, so there's no Jellyfin-star equivalent.
pub(crate) fn discover_toggle_blocklist(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    tmdb_id: i64,
    media_type: String,
    title: String,
    adding: bool,
) {
    debug!(
        "seerr: discover_toggle_blocklist tmdb={tmdb_id} media_type={media_type} adding={adding}"
    );
    let (client, user_id) = {
        let s = state.lock().unwrap();
        let Some(client) = s.seerr_client.clone() else {
            drop(s);
            show_toast(ww.clone(), "Not connected to Seerr".into());
            return;
        };
        (client, s.seerr_user_id)
    };
    let is_session_auth = client.is_session_auth();
    let item_type: &'static str = if media_type == "movie" {
        "DiscoverMovie"
    } else {
        "DiscoverTv"
    };

    rt.spawn(async move {
        let result = if adding {
            let Some(user_id) = user_id else {
                show_toast(ww.clone(), "Couldn't resolve your Seerr account".into());
                return;
            };
            client
                .add_blocklist(tmdb_id, &media_type, &title, user_id)
                .await
        } else {
            client.remove_blocklist(tmdb_id, &media_type).await
        };
        match result {
            Ok(()) => {
                debug!("seerr: discover_toggle_blocklist succeeded tmdb={tmdb_id} adding={adding}");
                let new_availability: &'static str = if adding { "blocklisted" } else { "" };
                // remove_blocklist deletes the whole Media row server-side (Seerr's route source), so
                // the item reverts to untouched: availability and both tier labels reset to empty.
                let new_status_label: &'static str = if adding { "Blocklisted" } else { "" };
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        // Blocklisted items disappear from Discover (search_result_to_meta): a fresh fetch
                        // already drops them, this removes the card from models already on screen.
                        // Un-blocklisting has nothing to add back — the item reappears on the next fetch.
                        if adding {
                            remove_card_from_all_models(&g, item_type, tmdb_id);
                        }
                        if g.get_show_request_detail()
                            && g.get_request_detail_media_type().as_str() == media_type
                            && g.get_request_detail_tmdb_id() == tmdb_id as i32
                        {
                            g.set_request_detail_availability(new_availability.into());
                            // Blocklisting sets BOTH tiers' status together
                            // server-side (confirmed from Seerr's real
                            // Blocklist entity — user confirmed this is the
                            // expected behavior, not a bug to guard
                            // against), so both labels update together too.
                            g.set_request_detail_status(new_status_label.into());
                            g.set_request_detail_status_4k(new_status_label.into());
                        }
                    }
                });
                show_toast(
                    ww.clone(),
                    if adding {
                        "Added to Blocklist"
                    } else {
                        "Removed from Blocklist"
                    }
                    .into(),
                );
            }
            Err(e) => handle_seerr_error(
                &state,
                &ww,
                is_session_auth,
                "Couldn't update blocklist",
                &e,
            ),
        }
    });
}

/// Snapshots the CURRENT Request Options modal state — both tiers' Quality/
/// Profile/Tags, not just whichever tier is actually being submitted, so
/// toggling to the other tier and back within a later session still finds
/// what was independently picked there (see `RequestPreference`'s own doc
/// comment in config.rs for the full design). Pure read, no `state`/config
/// access — called on the UI thread, before `g` is dropped, so the actual
/// persist (`store_request_preference`, below) can run later from wherever
/// a submit's own success branch happens to land (a Tokio task, off the UI
/// thread, after `g` is long gone).
fn read_current_request_preference(g: &AppState) -> RequestPreference {
    let want_4k = g.get_request_detail_want_4k();
    let read_selected_ids = |model: ModelRc<TagItem>| -> Vec<i64> {
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .filter(|t| t.selected)
            .map(|t| t.id as i64)
            .collect()
    };
    let tag_ids_active = read_selected_ids(g.get_request_detail_tags());
    let tag_ids_alt = read_selected_ids(g.get_request_detail_tags_alt());
    let profile_active = g.get_request_detail_selected_profile_id();
    let profile_alt = g.get_request_detail_selected_profile_id_alt();
    // Each Vec/id is consumed exactly once — swap via tuple destructuring
    // rather than a ternary per field, which would need each value read
    // twice (Vec<i64> isn't Copy).
    let (profile_id_2k, profile_id_4k) = if want_4k {
        (profile_alt, profile_active)
    } else {
        (profile_active, profile_alt)
    };
    let (tag_ids_2k, tag_ids_4k) = if want_4k {
        (tag_ids_alt, tag_ids_active)
    } else {
        (tag_ids_active, tag_ids_alt)
    };
    RequestPreference {
        want_4k,
        profile_id_2k,
        profile_id_4k,
        tag_ids_2k,
        tag_ids_4k,
    }
}

/// Writes an already-snapshotted preference (`read_current_request_preference`,
/// above) as the new remembered default for `media_type`. Called only from a
/// successful submit (`submit_request`/`submit_edit_request`), never on
/// Cancel or an intermediate toggle — those shouldn't overwrite what's
/// remembered.
fn store_request_preference(
    state: &Arc<Mutex<FjordState>>,
    media_type: &str,
    pref: RequestPreference,
) {
    let cfg = {
        let mut s = state.lock().unwrap();
        let target = if media_type == "movie" {
            &mut s.config.active_mut().request_pref_movie
        } else {
            &mut s.config.active_mut().request_pref_tv
        };
        *target = pref;
        s.config.clone()
    };
    save_config(&cfg);
}

pub(crate) fn submit_request(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    // Guard against re-submitting the SAME tier that's selected — 2K and 4K are
    // requestable independently (see tier_status_label).
    let is_4k = g.get_request_detail_want_4k();
    let tier_already_requested = if is_4k {
        g.get_request_detail_status_4k().as_str() != ""
    } else {
        g.get_request_detail_status().as_str() != ""
    };
    if g.get_request_detail_requesting() || tier_already_requested {
        return;
    }
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    let media_type = g.get_request_detail_media_type().to_string();
    let tmdb_id = g.get_request_detail_tmdb_id() as i64;

    let seasons_selector = if media_type == "tv" {
        let model = g.get_request_detail_seasons();
        let total = model.row_count();
        let selected: Vec<u32> = (0..total)
            .filter_map(|i| model.row_data(i))
            .filter(|s| s.selected)
            .map(|s| s.season_number as u32)
            .collect();
        if selected.is_empty() {
            show_toast(ww.clone(), "Select at least one season to request".into());
            return;
        }
        Some(if selected.len() == total {
            SeasonsSelector::all()
        } else {
            SeasonsSelector::Numbers(selected)
        })
    } else {
        None
    };
    let tag_ids: Vec<i64> = {
        let model = g.get_request_detail_tags();
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .filter(|t| t.selected)
            .map(|t| t.id as i64)
            .collect()
    };
    // 0 means the synthetic "Default" row — don't send profileId at all,
    // same as an unset choice.
    let profile_id = match g.get_request_detail_selected_profile_id() {
        0 => None,
        id => Some(id as i64),
    };
    // Snapshotted here, before g is dropped — persisted only on success,
    // below (see read_current_request_preference's own doc comment).
    let pref_snapshot = read_current_request_preference(&g);
    // A NEW request also adds the item to the Watchlist (requesting is a clear sign of
    // interest), unless it's already on it (avoids a redundant POST with undocumented
    // semantics). Editing a request doesn't.
    let already_on_watchlist = g.get_request_detail_on_watchlist();
    let title_snapshot = g.get_request_detail_title().to_string();

    g.set_request_detail_requesting(true);
    drop(g);

    let rt2 = rt.clone();
    rt.spawn(async move {
        let result = client
            .create_request(
                &media_type,
                tmdb_id,
                seasons_selector,
                is_4k,
                tag_ids,
                profile_id,
            )
            .await;
        match result {
            Ok(req) => {
                store_request_preference(&state, &media_type, pref_snapshot);
                let ww2 = ww.clone();
                let mt = media_type.clone();
                let request_id = req.id.to_string();
                let pending = req.is_pending();
                {
                    let mut s = state.lock().unwrap();
                    s.discover_known_requests.insert(
                        (
                            if media_type == "movie" {
                                "DiscoverMovie"
                            } else {
                                "DiscoverTv"
                            },
                            tmdb_id.to_string(),
                        ),
                        KnownRequest {
                            request_id: request_id.clone(),
                            pending,
                            mine: true,
                        },
                    );
                }
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        g.set_request_detail_requesting(false);
                        // Only the tier just requested — the other tier stays requestable.
                        if is_4k {
                            g.set_request_detail_status_4k("Requested".into());
                        } else {
                            g.set_request_detail_status("Requested".into());
                        }
                        patch_discover_card_availability(&g, &mt, tmdb_id, "requested");
                        // Real bug fixed 2026-07-18 — see
                        // patch_discover_card_request_state's own doc comment.
                        patch_discover_card_request_state(
                            &g,
                            &mt,
                            tmdb_id,
                            &request_id,
                            pending,
                            true,
                        );
                    }
                });
                if already_on_watchlist {
                    show_toast(ww.clone(), "Requested".into());
                } else {
                    // One toast once the auto-add lands; a failed add shows
                    // its own "Couldn't update watchlist" instead.
                    discover_toggle_watchlist(
                        Arc::clone(&state),
                        ww.clone(),
                        rt2.clone(),
                        tmdb_id,
                        media_type.clone(),
                        title_snapshot,
                        true,
                        Some("Requested — added to Watchlist"),
                    );
                }
                // A brand-new request was never in discover-requested, so
                // patch_discover_card_availability can't update that row — re-fetch it.
                refresh_requested_row(Arc::clone(&state), ww, rt2);
            }
            Err(e) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        AppState::get(&w).set_request_detail_requesting(false);
                    }
                });
                handle_seerr_error(&state, &ww, is_session_auth, "Request failed", &e);
            }
        }
    });
}

/// Confirm in the Request Options modal while `request-options-editing` (Edit Request):
/// like `submit_request` but PUTs the existing request, and never sends
/// `request-detail-want-4k` — editing can't change the tier
/// (`SeerrClient::update_request`). No `status != ""` guard — it obviously has one.
pub(crate) fn submit_edit_request(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    if g.get_request_detail_requesting() {
        return;
    }
    let Ok(request_id) = g.get_request_options_editing_request_id().parse::<i64>() else {
        return;
    };
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    let media_type = g.get_request_detail_media_type().to_string();

    let seasons_selector = if media_type == "tv" {
        let model = g.get_request_detail_seasons();
        let total = model.row_count();
        let selected: Vec<u32> = (0..total)
            .filter_map(|i| model.row_data(i))
            .filter(|s| s.selected)
            .map(|s| s.season_number as u32)
            .collect();
        if selected.is_empty() {
            show_toast(ww.clone(), "Select at least one season".into());
            return;
        }
        Some(if selected.len() == total {
            SeasonsSelector::all()
        } else {
            SeasonsSelector::Numbers(selected)
        })
    } else {
        None
    };
    let tag_ids: Vec<i64> = {
        let model = g.get_request_detail_tags();
        (0..model.row_count())
            .filter_map(|i| model.row_data(i))
            .filter(|t| t.selected)
            .map(|t| t.id as i64)
            .collect()
    };
    let profile_id = match g.get_request_detail_selected_profile_id() {
        0 => None,
        id => Some(id as i64),
    };
    // Snapshotted here, before g is dropped — persisted only on success,
    // below. want_4k is still read (unlike the API call itself, which
    // deliberately never sends it — see this function's own doc comment)
    // since it's a valid, correct value to remember even though editing
    // can't change it: it already reflects whichever tier this request
    // belongs to, set when the modal opened for editing.
    let pref_snapshot = read_current_request_preference(&g);

    g.set_request_detail_requesting(true);
    drop(g);

    rt.spawn(async move {
        let result = client
            .update_request(
                request_id,
                &media_type,
                seasons_selector,
                tag_ids,
                profile_id,
            )
            .await;
        match result {
            Ok(()) => {
                store_request_preference(&state, &media_type, pref_snapshot);
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        let g = AppState::get(&w);
                        g.set_request_detail_requesting(false);
                        g.set_show_request_options(false);
                        g.set_show_request_detail(false);
                    }
                });
                show_toast(ww, "Request updated".into());
            }
            Err(e) => {
                let ww2 = ww.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        AppState::get(&w).set_request_detail_requesting(false);
                    }
                });
                handle_seerr_error(&state, &ww, is_session_auth, "Edit request failed", &e);
            }
        }
    });
}

/// The Discover context menu's Cancel/Approve/Decline: one Seerr call by request id,
/// then Cancel removes the card from `discover-requested` while Approve/Decline leave it
/// (its badge refreshes with the next landing fetch). Reloads RequestDetailScreen (via
/// `open_discover_item`) when it shows this request's item — simpler than patching
/// request-detail-status/-4k/-request-id per tier.
pub(crate) fn discover_request_action(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    request_id: i64,
    action: &'static str, // "cancel" | "approve" | "decline"
    remove_on_success: bool,
) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        show_toast(ww.clone(), "Not connected to Seerr".into());
        return;
    };
    let is_session_auth = client.is_session_auth();
    let rt2 = rt.clone();
    rt.spawn(async move {
        let result = match action {
            "cancel" => client.delete_request(request_id).await,
            "approve" => client.approve_request(request_id).await,
            _ => client.decline_request(request_id).await,
        };
        // Keep request state in sync: Approve sets pending=false (so Cancel — DELETE needs
        // PENDING — isn't offered), Cancel/Decline remove the entry from
        // discover_known_requests.
        let req_key = request_id.to_string();
        {
            let mut s = state.lock().unwrap();
            if remove_on_success {
                // cancel/decline: the request no longer exists.
                s.discover_known_requests
                    .retain(|_, k| k.request_id != req_key);
            } else {
                // approve: still exists, just no longer Pending.
                if let Some(k) = s
                    .discover_known_requests
                    .values_mut()
                    .find(|k| k.request_id == req_key)
                {
                    k.pending = false;
                }
            }
        }
        match result {
            Ok(()) => {
                let ww2 = ww.clone();
                let state2 = Arc::clone(&state);
                let rt3 = rt2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww2.upgrade() else { return };
                    let g = AppState::get(&w);
                    if remove_on_success {
                        let model = g.get_discover_requested();
                        let kept: Vec<CardItem> = (0..model.row_count())
                            .filter_map(|i| model.row_data(i))
                            .filter(|c| c.request_id.as_str() != request_id.to_string())
                            .collect();
                        g.set_discover_requested(ModelRc::new(VecModel::from(kept)));
                        // The item itself may also be visible in the search
                        // grid (unaffected by the Requested-row removal
                        // above) — clear its now-stale request state there
                        // too rather than leaving it pointed at a
                        // cancelled/declined request id.
                        let results = g.get_discover_results();
                        for i in 0..results.row_count() {
                            if let Some(mut card) = results.row_data(i)
                                && card.request_id.as_str() == request_id.to_string()
                            {
                                card.request_id = "".into();
                                card.request_pending = false;
                                card.request_mine = false;
                                results.set_row_data(i, card);
                                break;
                            }
                        }
                    } else {
                        // Approve: patch request_pending=false in place on
                        // every model the card might currently be visible
                        // in, rather than removing it — an approved request
                        // stays in "Requested" until it's actually fulfilled.
                        for model in [g.get_discover_requested(), g.get_discover_results()] {
                            for i in 0..model.row_count() {
                                if let Some(mut card) = model.row_data(i)
                                    && card.request_id.as_str() == request_id.to_string()
                                {
                                    card.request_pending = false;
                                    model.set_row_data(i, card);
                                    break;
                                }
                            }
                        }
                    }
                    // The open detail page shows this request's item: reload it rather than hand-patch
                    // the per-tier fields (it would otherwise show a deleted request or a stale label).
                    if g.get_show_request_detail()
                        && g.get_request_detail_request_id().as_str() == request_id.to_string()
                    {
                        let media_type = g.get_request_detail_media_type().to_string();
                        let tmdb_id = g.get_request_detail_tmdb_id().to_string();
                        open_discover_item(media_type, tmdb_id, state2, ww2.clone(), rt3);
                    }
                });
                let verb = match action {
                    "cancel" => "cancelled",
                    "approve" => "approved",
                    _ => "declined",
                };
                show_toast(ww, format!("Request {verb}"));
            }
            Err(e) => {
                handle_seerr_error(&state, &ww, is_session_auth, "Couldn't update request", &e)
            }
        }
    });
}
