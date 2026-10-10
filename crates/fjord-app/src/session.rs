// ── fjord-app · session.rs ───────────────────────────────────────────────────
//   reset_session_state  the one teardown for sign-out and profile switch (moved from main.rs,
//                        0.5.0 step 3) — every new transient flag/cache/overlay is reset here
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use crate::MainWindow;
use crate::config::FjordState;
use crate::playback::VideoState;

// ── reset_session_state ──────────────────────────────────────────────────────
// Bonfire Phase 1, step 3 (2026-08-09): the shared teardown between signing
// out and (a later commit) switching to a different Bonfire sub-profile —
// the two are the same underlying event, a new (or no) Jellyfin user_id+
// token becoming active. Extracted from what was previously on_sign_out's
// own inline body, verbatim (zero behavior change for sign-out itself),
// so a future switch_to_profile can't drift out of sync with whatever
// sign-out's own cleanup grows to cover next.
//
// Deliberately does NOT touch:
// - Config's own auth/Seerr fields — sign-out blanks them; a switch needs to
//   SET them to the incoming profile's already-known values instead, so
//   there's no shared behavior to extract here.
// - what shows once this returns — sign-out shows the login screen; a
//   switch would show the new profile's dashboard. Every CONTENT-bearing
//   screen is force-closed below (so a switch can never flash the outgoing
//   profile's Detail/Series/etc. page over the incoming one), but the
//   final destination is each caller's own business.
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
    // Screen-open caches (Phase 102/103) hold per-user UserData
    // (played/favorite) keyed only by item id, with no user/server
    // scoping — a second account signing in on the same install would
    // otherwise silently see the first account's watched-state on any
    // item cached before sign-out, since a cache hit skips the network
    // fetch that would have corrected it. Cleared here rather than left
    // to the 60s save timer to persist the clear to screen_caches.json.
    s.item_detail_cache.clear();
    s.similar_items_cache.clear();
    s.boxset_items_cache.clear();
    s.artist_albums_cache.clear();
    s.person_filmography_cache.clear();
    s.container_tracks_cache.clear();
    s.person_tmdb_id_cache.clear();
    s.person_other_work_cache.clear();
    s.local_person_by_tmdb_cache.clear(); // 2026-08-13 — a different Jellyfin user means a different library/Person set entirely
    // A resolve genuinely in flight at the moment of sign-out/switch would
    // otherwise leave this Some(id) forever, permanently blocking any
    // future press for that exact tmdb_id in a brand-new session — low
    // probability, but this function's own history is "get these resets
    // right the first time," not "find it live a second time."
    s.person_discover_resolving = None;
    s.screen_revalidate_last_run.clear();
    // Code review, 2026-08-08: a rebind-collision dialog left open (or
    // dismissed via a mouse click elsewhere rather than its own
    // Cancel/Confirm) stranded this across sign-out, same class of gap as
    // the caches just above.
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
    // Bonfire Phase 6 (2026-09-04) — same "get resets right the first
    // time" discipline as every other session-scoped cache in this
    // function. jellyfin_is_server_admin is re-fetched unconditionally on
    // every session-establishment path (spawn_jellyfin_admin_check)
    // regardless — but leaving it stale-true here, even briefly, would
    // mean the incoming session's own Settings row could show admin
    // capability that doesn't apply to it for the split second before
    // that fetch resolves, the same class of cross-profile content leak
    // this function has already had to fix more than once elsewhere
    // (the Home dashboard rows never being cleared during a switch).
    // live_requires_pin is a different, per-profile map that could
    // equally mislead a new session if left populated from the old one.
    s.live_requires_pin.clear();
    s.jellyfin_is_server_admin = false;
    drop(s);

    // Real bug, found 2026-08-17 while chasing a live report ("the ui still
    // shows the old profile for 1-3s after you switched profile") even
    // after every model this function is supposed to clear was confirmed
    // present in the code — this whole block (originally gated on a
    // direct `window_weak.upgrade()`) was a SILENT NO-OP for every profile
    // switch that has ever happened, and always has been since this
    // function was first extracted. Confirmed directly from i-slint-core's
    // vendored source (`Weak::upgrade()`, api.rs): it checks
    // `std::thread::current().id()` against the thread that created the
    // window and returns `None` — no panic, nothing logged — on any other
    // thread. `on_sign_out` calls this function synchronously from a Slint
    // callback (the UI thread, upgrade() succeeds), but `switch_to_profile`
    // calls it from inside its own `rt.spawn(async move {...})`, a Tokio
    // WORKER thread — every `g.set_X(...)` below (screen closes, id
    // clears, all the dashboard-row clears including the 12 added for the
    // earlier "old profile lingers" fix) never actually ran on that path.
    // `do_stop_playback`'s own equivalent block (playback.rs) had the
    // identical bug for the same reason and is fixed the same way.
    // Dispatched via `invoke_from_event_loop` instead, which marshals onto
    // the UI thread regardless of which thread called this function — a
    // one-event-loop-tick deferral when already on the UI thread (harmless
    // here), and the actual fix for the Tokio-worker-thread case.
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
        // Bonfire Phase 1, step 8 (async-guard audit, 2026-08-09): clearing
        // just the show-X flags above isn't enough on its own — several of
        // these screens' in-flight-fetch commit closures gate solely on "is
        // this id still what's focused" (detail-id/series-id/season-id/
        // person-id/collection-id), which a profile switch never
        // invalidated before this fix, since nothing else clears them. A
        // stale closure landing after the switch (still holding the OLD
        // profile's data) would see its captured id still matches and call
        // set_show_X(true) again, silently re-opening a screen this
        // function JUST force-closed with content the new session may not
        // even be authorized to see. Clearing every id here closes that off
        // structurally, for every screen at once, rather than needing a
        // session_current() check threaded through each one's own fetch
        // chain individually (added anyway at the highest-severity sites —
        // see collection.rs/album.rs/artist.rs/person.rs/discover.rs/
        // poster.rs/prewarm.rs — since an id can coincidentally collide
        // across two sessions, e.g. the same item reopened under the new
        // profile before the old profile's stale fetch resolves).
        g.set_detail_id(ss(""));
        g.set_series_id(ss(""));
        g.set_season_id(ss(""));
        g.set_person_id(ss(""));
        g.set_collection_id(ss(""));
        g.set_album_id(ss(""));
        g.set_artist_id(ss(""));
        g.set_show_context_menu(false);
        g.set_show_now_playing(false);
        // Sidebar profile row/quick-menu (2026-08-14) — same reasoning as
        // every other content-bearing flag in this function: a switch mid-
        // menu must not leave it open, and the outgoing profile's own
        // name/avatar must not stay visible in the sidebar even briefly —
        // finish_session_setup always repushes current-profile-tile right
        // after this runs, but that's a reason this is safe to clear here,
        // not a reason to skip clearing it.
        g.set_show_sidebar_profile_menu(false);
        g.set_current_profile_tile(Default::default());
        // Real gap, live-reported 2026-08-21 alongside the profile-tile
        // fix above ("mabey it shuld show the home dashbord after a
        // switch"). Nothing ever reset active-nav on a switch — the sidebar
        // tab that happened to be active before (most commonly 7, the
        // Profile row itself, since that's how a switch is normally
        // triggered) just stayed selected, and nav==7 has no corresponding
        // dashboard content at all, so the content area rendered nothing
        // post-switch regardless of how fast the real data arrived. A
        // switch always lands on Home now, matching what every other
        // session-start path (a fresh login, an Add-Account login) already
        // does implicitly by virtue of active-nav defaulting to 0.
        g.set_active_nav(0);
        // show-account-picker/show-profile-picker are DELIBERATELY NOT
        // cleared here — real regression, live-reported 2026-08-17 ("the ui
        // still shows the old profile for 1-3s after you switched profile"
        // — a follow-up report AFTER the threading fix elsewhere in this
        // function had already landed). Every real call site of
        // switch_to_profile is picker-driven — the picker screen is what's
        // actively showing its own "Signing in…" spinner (profile-picker-
        // loading) for the whole switch duration, and needs to STAY open
        // until finish_session_setup's own two commit closures (the warm-
        // start one and the late one, auth.rs) have real data ready to
        // reveal underneath it; only THEY should ever close it. This exact
        // pair of lines used to be a silent no-op (the pre-fix threading
        // bug meant this whole block never actually ran during a switch),
        // which is precisely why the picker's spinner correctly stayed
        // visible the whole time before that fix — fixing the threading bug
        // made this code genuinely run for the first time, which then
        // closed the picker itself almost immediately after the token
        // resolved (~0.1-0.3s in), well before finish_session_setup's 1-3s
        // fetch was anywhere near done — revealing the now-correctly-
        // emptied (by the REST of this function) dashboard with nothing
        // covering it, instead of the picker's own loading spinner. The
        // original "Finding 7" reasoning for clearing show-profile-picker
        // here (a stale LEFTOVER picker overlay surviving into Sign Out) is
        // structurally unreachable in practice — every picker raw-key tier
        // consumes all input while shown, so Settings/Sign Out can never be
        // reached with a picker still open in the first place. A stale
        // login-append-source/server-prefill from an abandoned Add-Account
        // flow must still not silently apply to the next thing that opens
        // Login, though, so those three stay cleared.
        g.set_profile_picker_back_focused(false);
        g.set_profile_picker_quit_focused(false);
        g.set_account_picker_quit_focused(false);
        g.set_account_picker_back_focused(false);
        g.set_login_append_source(ss(""));
        g.set_login_server_prefill(ss(""));
        g.set_login_username_prefill(ss(""));
        // login-zone (2026-08-19, full D-pad nav — see app_state.slint's
        // own doc comment) gates input dispatch, not just display, so a
        // stale 3/4 surviving into a fresh Login open would misroute the
        // very first Down/Enter press there — reset alongside the prefills
        // above, per this function's own repeatedly-documented "get these
        // resets right the first time" lesson.
        g.set_login_zone(0);
        // On-screen keyboard (Bonfire Phase 3, 2026-08-22) — same reasoning
        // as login-zone right above: show-onscreen-keyboard gates input
        // dispatch (keys.rs's own top-level gate, checked before ANY
        // screen), so a stray true surviving a sign-out/switch would
        // intercept every subsequent key on whatever screen shows next,
        // not just leave the keyboard visibly stuck open.
        g.set_show_onscreen_keyboard(false);
        g.set_onscreen_keyboard_target(ss(""));
        g.set_onscreen_keyboard_cursor(0);
        // Discover's own overlay screens (Bonfire Phase 1, step 8 audit,
        // 2026-08-09) — a real, pre-existing gap independent of the async-
        // guard work below: these three were never in this function's reset
        // list at all, so they stayed open across BOTH sign-out and a
        // profile switch, the exact "outgoing profile's content still
        // visible" risk the whole reset_session_state extraction exists to
        // prevent for every other content-bearing screen.
        g.set_show_request_detail(false);
        g.set_show_request_options(false);
        g.set_show_calendar(false);
        g.set_show_calendar_day_popup(false);
        // Blocklist + PlaylistPicker (Bonfire Phase 4 review, 2026-08-29) —
        // the identical "outgoing profile's content still visible" gap the
        // three overlays above were already fixed for, found while
        // designing the idle-lock timer: a background timer is far more
        // likely to actually catch a user mid-Blocklist-browse or
        // mid-playlist-add than a deliberate, momentary sign-out click is,
        // so this is worth fixing even though every existing caller
        // (sign-out, switch_to_profile) could in principle already hit it.
        // Both overlays' own main.slint FadeGate mount conditions also
        // needed the matching !show-profile-picker/!show-account-picker
        // exclusion every sibling overlay already has — see main.slint.
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
        // Full D-pad retrofit, 2026-08-17 — only these three, not the whole
        // profile-edit-* set: they gate INPUT DISPATCH (keys.rs's
        // show_profile_edit tier checks text-editing/dropdown-open before
        // routing a key), not just rendering (the overlay Rectangle and the
        // LineEdit's own real focus are already torn down for free by this
        // screen's FadeGate unmount). A stray `true` surviving a sign-out/
        // switch mid-typing or mid-dropdown is the one class of leftover
        // state that could misroute a keypress on whatever screen comes
        // next. The cursor ints (avatar/PIN/checklist/button) are pure
        // display state, fully re-initialized by open_profile_edit_screen
        // before anything can read them again — deliberately not
        // duplicated here.
        g.set_profile_edit_zone(0);
        g.set_profile_edit_text_editing(false);
        g.set_profile_edit_dropdown_open(false);
        // Delete-confirm dialog (2026-08-21) — same reasoning as the three
        // above: keys.rs's own show_profile_edit tier checks this flag
        // BEFORE anything else in that tier, so a stray true surviving a
        // sign-out/switch mid-confirm would intercept every subsequent key
        // on whatever screen shows next, not just leave a dialog visibly
        // open behind an already-torn-down FadeGate.
        g.set_show_profile_edit_delete_confirm(false);
        // BonfireGroupScreen (Bonfire Phase 5, cross-household groups,
        // 2026-08-29) — same reasoning as ManageProfilesScreen/
        // ProfileEditScreen above: a switch mid-open must not leave this
        // showing (or one of its 4 ConfirmDialog gates stuck true, which
        // keys.rs's own show_bonfire_group tier checks BEFORE anything
        // else — a stray true surviving a switch would intercept every
        // subsequent key on whatever screen comes next).
        g.set_show_bonfire_group(false);
        g.set_bonfire_group_zone(0);
        g.set_bonfire_group_join_code(ss(""));
        g.set_show_bonfire_kick_confirm(false);
        g.set_show_bonfire_leave_confirm(false);
        g.set_show_bonfire_delete_group_confirm(false);
        g.set_show_bonfire_lan_bypass_confirm(false);
        // BonfireAdminScreen (Bonfire Phase 6, admin actions, 2026-09-04) —
        // identical reasoning to BonfireGroupScreen right above it, added
        // from the start rather than found live: keys.rs's own
        // show_bonfire_admin raw-key tier is checked the same
        // unconditional way, so a stray true surviving a switch/sign-out
        // would intercept every key on whatever screen comes next.
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
        // Real, more serious gap than anything else in this function, live-
        // reported 2026-08-17 ("the old user shows up for 1-3 s before its
        // changes" during a profile switch) — this function's own doc
        // comments already state its whole purpose is preventing exactly
        // this ("the exact 'outgoing profile's content still visible' risk
        // the whole reset_session_state extraction exists to prevent for
        // every other content-bearing screen"), but the Home dashboard's
        // own MAIN rows — the first, most prominent thing on screen for
        // most sessions — were never actually included in any clearing
        // pass here, on top of the Movies/TV library grid's own backing
        // models (all-movies/all-series/library-display). A stale local
        // Home-dashboard cache warm-starting the NEW profile is a separate,
        // already-understood, comparatively minor cosmetic trade-off (see
        // the Bonfire section of CLAUDE.md); this is the actual root cause
        // — the OUTGOING profile's own watch history/library sat fully
        // rendered and visible the whole time reset_session_state ran,
        // simply because nothing ever told these particular models to go
        // blank. Cleared here, matching the identical pattern every other
        // row in this function already uses.
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
        // Destructive-action confirm dialogs, 2026-08-22 (Sign Out/
        // Disconnect Seerr/Clear Queue/Cancel Request — see each one's own
        // doc comment in app_state.slint) — same "get resets right the
        // first time" discipline as the two Key Bindings dialogs right
        // above: a stray true surviving a sign-out/switch could otherwise
        // silently reopen a dialog for whatever screen comes next (Sign
        // Out/Cancel Request are global, main.slint-level, so they'd
        // render over ANY screen), or leave a raw-key tier intercepting
        // keys with nothing visible to explain why.
        g.set_show_sign_out_confirm(false);
        g.set_show_seerr_disconnect_confirm(false);
        g.set_show_queue_clear_confirm(false);
        g.set_show_cancel_request_confirm(false);
    });
}
