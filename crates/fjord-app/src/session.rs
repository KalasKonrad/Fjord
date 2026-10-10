// ── fjord-app · session.rs ───────────────────────────────────────────────────
//   reset_session_state  the one teardown for sign-out and profile switch (moved from main.rs,
//                        0.5.0 step 3) — every new transient flag/cache/overlay is reset here
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use crate::MainWindow;
use crate::config::FjordState;
use crate::playback::VideoState;

// ── reset_session_state ──────────────────────────────────────────────────────
// The shared teardown for sign-out and profile switch (both mean a new, or no, user + token
// becomes active), so the two can't drift apart.
//
// Deliberately does NOT touch:
// - Config's auth/Seerr fields — sign-out blanks them, a switch sets the incoming profile's.
// - what shows next — the caller's business. Every content-bearing screen is force-closed
//   below, so a switch never flashes the outgoing profile's pages over the incoming one.
pub(crate) fn reset_session_state(
    video: &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
    state: &Arc<Mutex<FjordState>>,
) {
    // Moved from main.rs: names resolve as they did there.
    use crate::*;
    // The queue belongs to the session: clear it BEFORE the stop so
    // do_stop_playback's own push_queue_display publishes the empty state.
    {
        let mut vs = video.lock().unwrap();
        vs.playlist.clear();
        vs.playlist_index = 0;
        vs.queue.clear();
        vs.shuffle_order.clear();
    }
    // Stop any active playback before clearing state — a switch mid-playback
    // must not leave audio/video running behind the incoming profile's UI.
    do_stop_playback(video, window_weak, rt_handle, state);

    let mut s = state.lock().unwrap();
    if let Some(abort) = s.ws_abort.take() {
        abort.abort();
    }
    s.client = None;
    s.seerr_client = None;
    s.discover_landing_fetched = false;
    s.discover_filter_options_fetched = false;
    s.discover_known_requests.clear();
    s.discover_watchlist_ids.clear();
    s.jellyfin_watchlist_ids.clear();
    s.discover_watchlist_fetched = false;
    s.discover_calendar_entries.clear();
    s.seerr_discover_region = None;
    s.seerr_genres_movie.clear();
    s.seerr_genres_tv.clear();
    s.seerr_providers_movie.clear();
    s.seerr_providers_tv.clear();
    s.seerr_streaming_region = None;
    s.seerr_regions.clear();
    s.seerr_user_id = None;
    s.seerr_is_admin = false;
    s.seerr_can_manage_blocklist = false;
    s.seerr_admin_last_refresh = None;
    s.all_movies.clear();
    s.all_series.clear();
    s.all_collections.clear();
    s.all_artists.clear();
    s.all_albums.clear();
    s.all_playlists.clear();
    s.filtered_items.clear();
    s.series_open_id.clear();
    s.series_season_ids.clear();
    s.series_episode_items.clear();
    s.series_episode_cache.clear();
    s.movie_collections.clear();
    s.remembered_tracks.clear();
    s.movies_fetched = false;
    s.movie_posters_loaded = false;
    s.collections_fetched = false;
    s.artists_fetched = false;
    s.albums_fetched = false;
    s.playlists_fetched = false;
    s.browse_populated = false;
    s.last_nw_mov_refresh = None;
    s.last_nw_tv_refresh = None;
    // Screen-open caches hold per-user UserData (played/favorite) keyed only by item id: a
    // second account would see the first one's watched state on a cache hit.
    s.item_detail_cache.clear();
    s.similar_items_cache.clear();
    s.boxset_items_cache.clear();
    s.artist_albums_cache.clear();
    s.person_filmography_cache.clear();
    s.container_tracks_cache.clear();
    s.person_tmdb_id_cache.clear();
    s.person_other_work_cache.clear();
    s.local_person_by_tmdb_cache.clear(); // 2026-08-13 — a different Jellyfin user means a different library/Person set entirely
    // A resolve in flight at sign-out would otherwise block that tmdb_id for the whole next
    // session.
    s.person_discover_resolving = None;
    s.screen_revalidate_last_run.clear();
    // A rebind-collision dialog dismissed by clicking elsewhere would otherwise strand this
    // across sign-out.
    s.pending_keybind_rebind = None;
    s.available_plugins.clear();
    s.trailer_playable.clear();
    s.request_detail_trailers.clear();
    // ManageProfilesScreen/ProfileEditScreen (Bonfire Phase 2, 2026-08-09) —
    // same "clear it here, don't wait to discover the gap live" precedent
    // as everything else in this function.
    s.profile_edit_pin_buffer.clear();
    s.profile_edit_master_pin_buffer.clear();
    s.manage_profiles_cache.clear();
    // Admin capability and per-profile PIN requirements must not leak into the next session,
    // even for the moment before spawn_jellyfin_admin_check re-fetches the admin flag.
    s.live_requires_pin.clear();
    s.jellyfin_is_server_admin = false;
    drop(s);

    // On the UI thread via invoke_from_event_loop: switch_to_profile calls this from a Tokio
    // worker, where Weak::upgrade() silently returns None (every g.set_* below used to be a
    // no-op on a switch). One event-loop tick of delay on the UI thread is harmless.
    let video2 = Arc::clone(video);
    let window_weak2 = window_weak.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = window_weak2.upgrade() else {
            return;
        };
        let g = AppState::get(&w);
        g.set_show_browse(false);
        g.set_show_library(false);
        g.set_show_detail(false);
        g.set_show_series(false);
        g.set_show_season(false);
        g.set_show_person(false);
        g.set_show_collection(false);
        g.set_show_album(false);
        g.set_show_artist(false);
        // Clear every screen's id too: in-flight fetch commits only check "is this id still
        // open", so a stale one landing after the switch would re-open its screen with the old
        // profile's data. (The worst sites also check session_current.)
        g.set_detail_id(ss(""));
        g.set_series_id(ss(""));
        g.set_season_id(ss(""));
        g.set_person_id(ss(""));
        g.set_collection_id(ss(""));
        g.set_album_id(ss(""));
        g.set_artist_id(ss(""));
        g.set_show_context_menu(false);
        g.set_show_now_playing(false);
        // The sidebar profile menu and the outgoing profile's tile must not survive a switch
        // (finish_session_setup repushes the tile right after).
        g.set_show_sidebar_profile_menu(false);
        g.set_current_profile_tile(Default::default());
        // A switch always lands on Home (otherwise nav 7, the Profile row the switch started
        // from, stays selected and shows no content).
        g.set_active_nav(0);
        // show-account-picker/show-profile-picker are deliberately NOT cleared: every switch is
        // picker-driven, and the picker's "Signing in…" spinner must stay up until
        // finish_session_setup's commit closures (auth.rs) close it — closing it here revealed
        // the emptied dashboard for 1–3 s. A leftover picker can't reach Sign Out anyway (the
        // picker key tiers swallow all input). Stale Add-Account prefills are cleared.
        g.set_profile_picker_back_focused(false);
        g.set_profile_picker_quit_focused(false);
        g.set_account_picker_quit_focused(false);
        g.set_account_picker_back_focused(false);
        g.set_login_append_source(ss(""));
        g.set_login_server_prefill(ss(""));
        g.set_login_username_prefill(ss(""));
        // login-zone gates input dispatch: a stale value would misroute the first key on the next
        // Login open.
        g.set_login_zone(0);
        // show-onscreen-keyboard is keys.rs's top-level gate — a stray true would swallow every key
        // on the next screen.
        g.set_show_onscreen_keyboard(false);
        g.set_onscreen_keyboard_target(ss(""));
        g.set_onscreen_keyboard_cursor(0);
        // Discover's overlay screens: the outgoing profile's content must not stay open.
        g.set_show_request_detail(false);
        g.set_show_request_options(false);
        g.set_show_calendar(false);
        g.set_show_calendar_day_popup(false);
        // Blocklist + PlaylistPicker: same (the idle lock can catch a user on either). Their
        // main.slint FadeGates also exclude the pickers.
        g.set_show_blocklist(false);
        g.set_show_playlist_picker(false);
        g.set_playlist_picker_naming(false);
        g.set_playlist_picker_name(ss(""));
        // ManageProfilesScreen/ProfileEditScreen (Bonfire Phase 2, 2026-08-09)
        // — same reasoning: a switch mid-edit must not leave either open,
        // showing the outgoing profile's own household data.
        g.set_show_manage_profiles(false);
        g.set_manage_profiles_close_focused(false);
        g.set_show_profile_edit(false);
        // Only the profile-edit flags that gate input dispatch (keys.rs checks text-editing/
        // dropdown-open before routing a key); the cursor ints are display state that
        // open_profile_edit_screen re-initializes.
        g.set_profile_edit_zone(0);
        g.set_profile_edit_text_editing(false);
        g.set_profile_edit_dropdown_open(false);
        // Checked first in keys.rs's show_profile_edit tier — a stray true would swallow keys.
        g.set_show_profile_edit_delete_confirm(false);
        // BonfireGroupScreen: same, including its ConfirmDialog flags (checked first in keys.rs's
        // show_bonfire_group tier).
        g.set_show_bonfire_group(false);
        g.set_bonfire_group_zone(0);
        g.set_bonfire_group_join_code(ss(""));
        g.set_show_bonfire_kick_confirm(false);
        g.set_show_bonfire_leave_confirm(false);
        g.set_show_bonfire_delete_group_confirm(false);
        g.set_show_bonfire_lan_bypass_confirm(false);
        // BonfireAdminScreen: same as BonfireGroupScreen.
        g.set_show_bonfire_admin(false);
        g.set_bonfire_admin_back_focused(false);
        g.set_bonfire_admin_tab(0);
        g.set_bonfire_admin_cursor(-1);
        g.set_bonfire_admin_col(0);
        g.set_show_bonfire_admin_reset_confirm(false);
        g.set_bonfire_admin_reset_confirm_target(ss(""));
        g.set_bonfire_admin_error(ss(""));
        g.set_bonfire_admin_rows(slint::ModelRc::new(slint::VecModel::from(Vec::<
            BonfireAdminRow,
        >::new())));
        g.set_bonfire_admin_audit_rows(slint::ModelRc::new(slint::VecModel::from(Vec::<
            BonfireAuditRow,
        >::new(
        ))));
        g.set_jellyfin_is_server_admin(false);
        // Remember-login confirm modal (2026-08-17) — same reasoning: a
        // switch/sign-out mid-confirm shouldn't leave it open against a
        // session that's no longer active.
        g.set_show_remember_login_confirm(false);
        g.set_remember_login_confirm_error(ss(""));
        g.set_remember_login_confirm_loading(false);
        g.set_all_collections(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_all_artists(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_all_albums(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_all_playlists(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_library_music_view(0);
        g.set_recently_added_collections(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_unwatched_collections(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_recently_added_albums(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_recently_played_albums(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_favorite_movies(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_favorite_series(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_favorite_albums(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_music_playlists(items_to_model(&[], &std::collections::HashSet::new()));
        // The Home dashboard rows and the library grids' backing models: otherwise the outgoing
        // profile's history and library stay visible until the new data arrives.
        g.set_library_display(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_all_movies(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_all_series(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_continue_watching(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_next_up(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_recently_added(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_continue_watching_movies(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_recently_added_movies(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_not_watched_movies(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_continue_watching_tv(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_recently_added_tv(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_not_watched_tv(items_to_model(&[], &std::collections::HashSet::new()));
        // Dashboard Watchlist rows (2026-07-20) — same reset-completeness
        // gap this doc already documents having been bitten by once for
        // discover_watchlist_ids/discover_calendar_entries/seerr_discover_region.
        g.set_discover_watchlist_mixed(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_watchlist_movies(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_watchlist_tv(items_to_model(&[], &std::collections::HashSet::new()));
        // Dashboard Coming Up rows (2026-08-02) — same reasoning.
        g.set_discover_coming_up_mixed(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_coming_up_movies(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_discover_coming_up_tv(items_to_model(&[], &std::collections::HashSet::new()));
        g.set_seerr_is_admin(false);
        g.set_seerr_can_manage_blocklist(false);
        g.set_show_next_ep_banner(false);
        g.set_has_background_player(false);
        {
            let mut vs = video2.lock().unwrap();
            vs.playlist.clear();
            vs.playlist_index = 0;
            vs.queue.clear();
            vs.shuffle = false;
            vs.shuffle_order.clear();
            vs.repeat_mode = crate::playback::RepeatMode::Off;
        }
        push_queue_display(&video2.lock().unwrap(), &g);
        g.set_queue_shuffle(false);
        g.set_queue_repeat_mode(0);
        g.set_show_queue_panel(false);
        g.set_float_card_focused(-1);
        g.set_keybinding_focused(-1);
        g.set_keybinding_rebinding(false);
        g.set_show_keybinding_reset_confirm(false);
        g.set_show_keybinding_collision_confirm(false);
        // Destructive-action confirm dialogs (Sign Out/Disconnect Seerr/Clear Queue/Cancel
        // Request): a stray true could reopen one over the next screen (Sign Out and Cancel
        // Request are main.slint-level) or leave a key tier swallowing input.
        g.set_show_sign_out_confirm(false);
        g.set_show_seerr_disconnect_confirm(false);
        g.set_show_queue_clear_confirm(false);
        g.set_show_cancel_request_confirm(false);
    });
}
