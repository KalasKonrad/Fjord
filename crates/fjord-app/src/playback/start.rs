// ── fjord-app · playback/start.rs ────────────────────────────────────────────
//   start_playback          stop-report the previous item first, then open the URL; Audio → music bar
//                           (cover + its 24px blur for Now Playing's background),
//                           no fullscreen player; playing music inserts at the top of the queue; with
//                           display sync on (video), holds back pending_load_url/play_start while a task
//                           switches the display first (display_sync::sync_before_load, 20 s cap;
//                           skipped when replacing a live player for the same item; a stop mid-switch
//                           reverts the display); a video starts trickplay::spawn_fetch
//                           (VideoState.trickplay, cleared by every reset/teardown). One function per
//                           step, in order:
//     route_audio_output    music → the normal device without S/PDIF; video → the passthrough device
//     bump_generation       playback_generation + 1 (the stale-result guard of every task below)
//     apply_play_origin     from_detail/-series → which page reset_playback_ui restores on stop
//     set_now_playing       current_is_audio + now_playing (the queue survives a new play)
//     spawn_segment_fetch   Episode: clear the skip segments, fetch them (intro/recap/preview/
//                           commercial/credits)
//     replace_previous_player  same-item-live check, drop counts, tear_down_player + its stop report
//     spawn_start_report    report_playback_start (after the previous stop was dispatched)
//     install_player        reset_video_state_for_playback, track preferences, ids/client, and
//                           whether the display-sync pre-decode wait applies (else load at once)
//     apply_track_preferences  sub/audio languages to mpv before the file loads
//     spawn_display_sync_prestart  the pre-decode display switch (20 s cap), then the deferred load
//     show_playing_ui       the music bar (audio) or the fullscreen player (video)
//     spawn_music_cover / spawn_lyrics  a song's cover (+ blur) and synced lyrics
//     player_init_failed    clear segments, reset the UI, toast
//   prestart_still_current  the prestart task's staleness check — generation unchanged AND a player
//                           still set
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// display-mode-prefetch: is the deferred pre-decode task spawned by
/// `start_playback` (generation `my_gen`) still the live playback? False once
/// a newer item started (generation bumped) or playback was stopped
/// (`vs.player` gone — Stop does not bump the generation).
fn prestart_still_current(video: &Arc<Mutex<VideoState>>, my_gen: u64) -> bool {
    let vs = video.lock().unwrap();
    vs.playback_generation == my_gen && vs.player.is_some()
}

// ── start_playback ────────────────────────────────────────────────────────────
// Called from ~30 sites across the app (dashboards, library grids, detail/series/
// season/album/artist/collection screens, context menu, queue/playlist advance,
// gapless commit) — bundling params into a struct would touch all of them for no
// behavior change, so the arg count is accepted rather than "fixed".
#[allow(clippy::too_many_arguments)]
pub(crate) fn start_playback(
    url: String,
    item_id: String,
    item_type: &str,
    title: String,
    config: PlayerConfig,
    client: Arc<JellyfinClient>,
    series_id: Option<String>,
    // (artist, album_art_id) — populated for Audio items; drives the music bar
    audio_meta: Option<(String, String)>,
    video: &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
    // For Config.device.display_sync_enabled and sync_before_load's FjordState-backed
    // cache (every caller already holds it: it builds `config` via s.player_config()).
    state: &Arc<Mutex<FjordState>>,
    // Already-known width/fps/is_hdr for THIS item, when the caller happens
    // to have a MediaItem with Fields=MediaStreams on hand (avoids a
    // redundant fetch); None falls back to an internal item_detail_cache
    // lookup / fresh fetch inside the deferred-load task below. Either way,
    // has no effect at all unless display_sync is enabled and item_type
    // isn't Audio.
    video_info: Option<fjord_api::models::VideoStreamInfo>,
) {
    info!(
        "starting playback: {} — {}",
        item_id,
        fjord_player::redact_api_key(&url)
    );

    let config = route_audio_output(config, item_type);
    let my_gen = bump_generation(video);
    apply_play_origin(video, window_weak);
    set_now_playing(video, &item_id, item_type, &series_id, &title, &audio_meta);

    if item_type == "Episode" {
        spawn_segment_fetch(video, &client, &item_id, my_gen, rt_handle);
    }

    let (same_item_live, keep_presync) = replace_previous_player(video, &item_id, rt_handle);
    spawn_start_report(&client, &item_id, rt_handle);

    let client_art = Arc::clone(&client);
    let item_id_art = item_id.clone();
    let is_audio = item_type == "Audio";
    // display-mode-prefetch: cloned here, BEFORE install_player moves the originals into
    // vs.client/vs.item_id — mirrors client_art/item_id_art immediately above for the
    // identical reason.
    let client_ds = Arc::clone(&client);
    let item_id_ds = item_id.clone();
    let url_ds = url.clone();

    match Player::new(&config) {
        Ok(player) => {
            let eligible = install_player(
                video,
                player,
                &config,
                item_type,
                &url,
                keep_presync,
                same_item_live,
                item_id,
                series_id,
                client,
                state,
            );
            if !is_audio {
                crate::trickplay::spawn_fetch(
                    Arc::clone(&client_art),
                    item_id_art.clone(),
                    my_gen,
                    video,
                    window_weak,
                    rt_handle,
                );
            }
            if eligible {
                spawn_display_sync_prestart(
                    state, video, client_ds, item_id_ds, url_ds, video_info, my_gen, rt_handle,
                );
            }
            show_playing_ui(window_weak, &title, is_audio, &audio_meta);
            // For audio tracks: fetch album art for music bar (and player background), and the
            // song's format + quality.
            if is_audio {
                crate::playback::spawn_audio_quality(
                    Arc::clone(&client_art),
                    item_id_art.clone(),
                    video,
                    window_weak,
                    rt_handle,
                    my_gen,
                );
                spawn_music_cover(
                    client_art,
                    &item_id_art,
                    &audio_meta,
                    video,
                    window_weak,
                    rt_handle,
                    my_gen,
                );
                spawn_lyrics(&item_id_art, video, window_weak, rt_handle, my_gen);
            }
        }
        Err(e) => player_init_failed(video, window_weak, e),
    }
}

// ── route_audio_output ────────────────────────────────────────────────────────
/// Route audio output by content type: music always plays PCM on the normal
/// device (no audio-spdif options); video uses the dedicated passthrough
/// device when SPDIF is enabled and one is configured.
fn route_audio_output(config: PlayerConfig, item_type: &str) -> PlayerConfig {
    let mut config = config;
    if item_type == "Audio" {
        config.audio_spdif_formats.clear();
    } else if !config.audio_spdif_formats.is_empty() && !config.audio_device_passthrough.is_empty()
    {
        config.audio_device = config.audio_device_passthrough.clone();
    }
    config
}

// ── bump_generation ───────────────────────────────────────────────────────────
/// Increment generation before spawning tasks so stale responses from a prior
/// episode can be detected and discarded even if they arrive after Player::new.
fn bump_generation(video: &Arc<Mutex<VideoState>>) -> u64 {
    let mut vs = video.lock().unwrap();
    vs.playback_generation = vs.playback_generation.wrapping_add(1);
    vs.playback_generation
}

// ── apply_play_origin ─────────────────────────────────────────────────────────
/// Track whether this play started from the detail/series/season page so reset_playback_ui
/// can restore the correct screen on stop.
fn apply_play_origin(video: &Arc<Mutex<VideoState>>, window_weak: &slint::Weak<MainWindow>) {
    let (from_detail, from_series) = {
        let mut vs = video.lock().unwrap();
        let fd = vs.from_detail;
        vs.from_detail = false;
        let fs = vs.from_series;
        vs.from_series = false;
        vs.from_season = false;
        (fd, fs)
    };
    if let Some(w) = window_weak.upgrade() {
        let g = AppState::get(&w);
        if !from_detail {
            g.set_show_detail(false);
        }
        // Series/season have no inline-video slot — always hide.
        // on_play_series_episode already set playback_from_series/season directly on the
        // UI thread; only clear them when this is NOT a series play (from_series = false
        // means a different source, e.g. home screen or context menu).
        g.set_show_series(false);
        g.set_show_season(false);
        g.set_playback_from_detail(from_detail);
        if !from_series {
            g.set_playback_from_series(false);
            g.set_playback_from_season(false);
        }
    }
}

// ── set_now_playing ───────────────────────────────────────────────────────────
/// The playlist and queue always survive a new play (Phase 56): playing music
/// while items are queued means "insert at the top of the queue" — the new item
/// plays now and the previously upcoming items continue after it. A video play
/// leaves the queue dormant; the class-gated natural-end advance below makes
/// sure a movie ending never auto-starts queued music.
fn set_now_playing(
    video: &Arc<Mutex<VideoState>>,
    item_id: &str,
    item_type: &str,
    series_id: &Option<String>,
    title: &str,
    audio_meta: &Option<(String, String)>,
) {
    let mut vs = video.lock().unwrap();
    vs.current_is_audio = item_type == "Audio";
    vs.now_playing = Some(QueueItem {
        id: item_id.to_string(),
        item_type: item_type.to_string(),
        series_id: series_id.clone(),
        title: title.to_string(),
        audio_meta: audio_meta.clone(),
    });
}

// ── spawn_segment_fetch ───────────────────────────────────────────────────────
/// An episode's skip segments: reset the intro/credits state, then fetch them.
fn spawn_segment_fetch(
    video: &Arc<Mutex<VideoState>>,
    client: &Arc<JellyfinClient>,
    item_id: &str,
    my_gen: u64,
    rt_handle: &tokio::runtime::Handle,
) {
    // Reset intro/credits state before spawning fetch tasks so that if the response
    // arrives before Player::new completes the result is not wiped by the init block.
    {
        let mut vs = video.lock().unwrap();
        vs.intro_timestamps = None;
        vs.recap_timestamps = None;
        vs.preview_timestamps = None;
        vs.commercial_timestamps = None;
        vs.intro_skip_shown = false;
        vs.recap_skip_shown = false;
        vs.preview_skip_shown = false;
        vs.commercial_skip_shown = false;
        vs.skip_segment_end = None;
        vs.credits_start = None;
    }

    // Intro + credits timestamps (Intro Skipper v2+: single call returns both)
    let client_ts = Arc::clone(client);
    let video_ts = Arc::clone(video);
    let item_id_ts = item_id.to_string();
    rt_handle.spawn(async move {
        match client_ts.get_episode_timestamps(&item_id_ts).await {
            Ok(Some(ts)) => {
                let mut vs = video_ts.lock().unwrap();
                if vs.playback_generation != my_gen {
                    debug!(
                        "episode timestamps for {} arrived late — discarding",
                        item_id_ts
                    );
                    return;
                }
                let mut any = false;
                if ts.introduction.valid() {
                    info!(
                        "intro: start={:.1}s end={:.1}s",
                        ts.introduction.start, ts.introduction.end
                    );
                    vs.intro_timestamps = Some(ts.introduction.clone());
                    any = true;
                }
                if ts.recap.valid() {
                    info!(
                        "recap: start={:.1}s end={:.1}s",
                        ts.recap.start, ts.recap.end
                    );
                    vs.recap_timestamps = Some(ts.recap.clone());
                    any = true;
                }
                if ts.preview.valid() {
                    info!(
                        "preview: start={:.1}s end={:.1}s",
                        ts.preview.start, ts.preview.end
                    );
                    vs.preview_timestamps = Some(ts.preview.clone());
                    any = true;
                }
                if ts.commercial.valid() {
                    info!(
                        "commercial: start={:.1}s end={:.1}s",
                        ts.commercial.start, ts.commercial.end
                    );
                    vs.commercial_timestamps = Some(ts.commercial.clone());
                    any = true;
                }
                if ts.credits.valid() {
                    info!("credits start: {:.1}s", ts.credits.start);
                    vs.credits_start = Some(ts.credits.start);
                    any = true;
                }
                if !any {
                    info!(
                        "no segments for {} (plugin absent or episode not analyzed)",
                        item_id_ts
                    );
                }
            }
            Ok(None) => info!(
                "no episode timestamps for {} (plugin absent or episode not analyzed)",
                item_id_ts
            ),
            Err(e) => warn!("episode timestamps fetch failed: {:#}", e),
        }
    });
}

// ── replace_previous_player ───────────────────────────────────────────────────
/// Tears down the previous player (and reports its stop). Returns (same_item_live,
/// keep_presync).
fn replace_previous_player(
    video: &Arc<Mutex<VideoState>>,
    item_id: &str,
    rt_handle: &tokio::runtime::Handle,
) -> (bool, bool) {
    // Replacing a LIVE player for this same item (a stall reload, or re-picking what's
    // playing): the display is already in this item's mode, so skip the pre-decode wait
    // (during an outage its item-detail fetch would likely hang up to the 20 s cap).
    // keep_presync: the display stays in that mode across such a reload, so HDR can still
    // be negotiated at the first VideoReconfig instead of switching late mid-playback.
    let (same_item_live, keep_presync) = {
        let vs = video.lock().unwrap();
        let same = vs.player.is_some() && vs.item_id.as_deref() == Some(item_id);
        (same, same && vs.display_presynced)
    };

    let (dropped, dec_dropped) = video
        .lock()
        .unwrap()
        .player
        .as_ref()
        .map(|p| p.get_drop_counts())
        .unwrap_or((0, 0));
    info!(
        "playback replaced: frame-drops={} decoder-drops={}",
        dropped, dec_dropped
    );
    let (prev_item_id, prev_client, prev_cookie, prev_ticks) =
        { tear_down_player(&mut video.lock().unwrap()) };
    uninhibit_screensaver(prev_cookie);
    if let (Some(id), Some(cli)) = (prev_item_id, prev_client) {
        rt_handle.spawn(async move {
            if let Err(e) = cli.report_playback_stopped(&id, prev_ticks).await {
                warn!("report_playback_stopped (replaced) failed: {e}");
            }
        });
    }
    (same_item_live, keep_presync)
}

// ── spawn_start_report ────────────────────────────────────────────────────────
/// Send start report only after the previous stop has been dispatched (CR-3).
fn spawn_start_report(
    client: &Arc<JellyfinClient>,
    item_id: &str,
    rt_handle: &tokio::runtime::Handle,
) {
    let client2 = Arc::clone(client);
    let item_id2 = item_id.to_string();
    rt_handle.spawn(async move {
        if let Err(e) = client2.report_playback_start(&item_id2).await {
            warn!("report_playback_start failed: {e}");
        }
    });
}

// ── install_player ────────────────────────────────────────────────────────────
/// The new player into VideoState (the shared fresh-playback reset, track preferences, ids,
/// client). Returns whether the display-sync pre-decode wait applies.
#[allow(clippy::too_many_arguments)]
fn install_player(
    video: &Arc<Mutex<VideoState>>,
    player: Player,
    config: &PlayerConfig,
    item_type: &str,
    url: &str,
    keep_presync: bool,
    same_item_live: bool,
    item_id: String,
    series_id: Option<String>,
    client: Arc<JellyfinClient>,
    state: &Arc<Mutex<FjordState>>,
) -> bool {
    let is_audio = item_type == "Audio";
    let mut vs = video.lock().unwrap();
    reset_video_state_for_playback(&mut vs, player, config, item_type == "Episode", url);
    vs.display_presynced = keep_presync;
    apply_track_preferences(&vs, state, &series_id);
    vs.item_id = Some(item_id);
    vs.playing_series_id = series_id;
    vs.client = Some(client);

    // Audio never takes part (no display mode for music). When eligible, hold back
    // pending_load_url/play_start until the task below has set the display's mode/HDR/WCG
    // ahead of decode; otherwise load at once and stamp play_start now.
    let eligible = !is_audio && !same_item_live && {
        let s = state.lock().unwrap();
        s.config.device.display_sync_enabled
    };
    if eligible {
        vs.pending_load_url = None;
        vs.display_sync_prestart_active = true;
    } else {
        vs.pending_load_url = Some(url.to_string());
        vs.play_start = Some(Instant::now());
    }
    eligible
}

// ── apply_track_preferences ───────────────────────────────────────────────────
/// Subtitle/audio language preferences go to mpv BEFORE the file loads, so it enables
/// those tracks from the first byte; the auto-select at FileLoaded only corrects a
/// different pick (switching tracks after reading starts makes mpv re-read its buffer).
fn apply_track_preferences(
    vs: &VideoState,
    state: &Arc<Mutex<FjordState>>,
    series_id: &Option<String>,
) {
    let s = state.lock().unwrap();
    let a = s.config.active();
    let remembered = series_id
        .as_ref()
        .and_then(|id| s.remembered_tracks.get(id));
    let mut slang: Vec<String> = Vec::new();
    if let Some(l) = remembered.and_then(|r| r.sub_lang.clone()) {
        slang.push(l.to_ascii_lowercase());
    }
    for name in [a.sub_lang.as_str(), a.sub_lang2.as_str()] {
        let code = sub_lang_code(name);
        if !code.is_empty() && !slang.iter().any(|c| c == code) {
            slang.push(code.to_string());
        }
    }
    let alang: Vec<String> = match remembered.and_then(|r| r.audio_lang.clone()) {
        Some(l) => vec![l.to_ascii_lowercase()],
        None => Some(sub_lang_code(&a.audio_lang))
            .filter(|c| !c.is_empty())
            .map(|c| vec![c.to_string()])
            .unwrap_or_default(),
    };
    if let Some(p) = vs.player.as_ref() {
        p.set_track_preferences(&slang, &alang, a.sub_enabled);
    }
}

// ── spawn_display_sync_prestart ───────────────────────────────────────────────
/// Switches the display (mode/HDR/WCG) before decode, then releases the deferred load.
#[allow(clippy::too_many_arguments)]
fn spawn_display_sync_prestart(
    state: &Arc<Mutex<FjordState>>,
    video: &Arc<Mutex<VideoState>>,
    client_ds: Arc<JellyfinClient>,
    item_id_ds: String,
    url_ds: String,
    video_info: Option<fjord_api::models::VideoStreamInfo>,
    my_gen: u64,
    rt_handle: &tokio::runtime::Handle,
) {
    let state_ds = Arc::clone(state);
    let video_ds = Arc::clone(video);
    rt_handle.spawn(async move {
        // Caught on plan re-check — wrapping only sync_before_load
        // in a timeout would leave the fallback item-detail fetch
        // (the None branch below) unprotected on its own; combined
        // with get_item_detail's own 30s reqwest timeout, a
        // pathological case could stack toward ~45s before ever
        // giving up. Wrapping the WHOLE body (fetch AND switch) in
        // one outer timeout gives a single, honest cap instead —
        // a fetch still in flight when it fires is fine to
        // abandon, the item just starts playing without the
        // pre-decode switch, falling back to Branch B's existing
        // post-decode behavior exactly as it already does today
        // for every one of the bare-id call sites.
        let prestart = async {
            let resolved = match video_info {
                Some(vi) => Some(vi),
                None => {
                    // A cache hit only counts if it actually carries
                    // stream info — entries written by older builds
                    // (screen_caches.json) have no MediaStreams.
                    let cached = state_ds.lock().unwrap().item_detail_cache
                        .get(&item_id_ds)
                        .and_then(|i| i.video_stream_info());
                    match cached {
                        Some(vi) => Some(vi),
                        None => client_ds.get_item_detail(&item_id_ds).await.ok()
                            .and_then(|i| i.video_stream_info()),
                    }
                }
            };
            let Some(vi) = resolved else { return false };
            // Cheap, early staleness check — shrinks (does not
            // eliminate) the window where a rapid second
            // start_playback call for a DIFFERENT item could spawn
            // a second, concurrent prestart task while this one is
            // still resolving video_info. See DEVLOG.md's
            // display-mode-prefetch section for the residual race
            // this doesn't fully close (accepted, self-healing).
            // Also bail if playback was stopped (not replaced)
            // meanwhile: Stop doesn't bump playback_generation, so
            // vs.player being gone is the only signal — without
            // it, a Play-then-quick-Stop would still switch the
            // display after the stop's own revert already ran.
            if !prestart_still_current(&video_ds, my_gen) { return false; }
            let cfg = crate::display_sync::DisplaySyncSettings::from_device_config(
                &state_ds.lock().unwrap().config.device,
            );
            crate::display_sync::sync_before_load(Arc::clone(&state_ds), vi, cfg).await;
            true
        };
        // Hang-guard, not an expected-case budget — Branch B
        // today runs sync_to_source with NO timeout at all, so
        // even a generous value here is strictly safer than the
        // status quo. 20s covers a legitimately-slow-but-real
        // fetch plus sync_to_source's own real settle path
        // (get_supported_modes + mode-set + a genuine 3s sleep +
        // optional color-apply, realistically ~3.5-4.5s) with
        // real margin — deliberately NOT tight, since a timeout
        // firing WHILE inside that 3s sleep drops the future
        // after the real kscreen-doctor mode-set already ran but
        // before FjordState.display_sync_current_mode gets
        // cached, which would make Branch B (post-decode,
        // unconditionally still running) see mode_changed==true
        // and re-issue a second real mode-set AFTER video has
        // already started rendering — reintroducing the exact
        // blink this feature exists to remove, just moved later.
        let presynced = match tokio::time::timeout(Duration::from_secs(20), prestart).await {
            Ok(synced) => synced,
            Err(_) => {
                warn!("display_sync prestart timed out — starting playback at whatever mode is current");
                false
            }
        };
        // Stopped (same generation, player gone) while the switch
        // was in flight: sync_to_source only records the new mode
        // after its 3 s settle, so the stop's own revert_to_default
        // saw the old mode and did nothing. Revert here instead —
        // a cheap no-op if nothing was actually switched.
        let stopped = {
            let vs = video_ds.lock().unwrap();
            vs.playback_generation == my_gen && vs.player.is_none()
        };
        if stopped {
            info!("display_sync prestart: playback stopped during the switch — reverting");
            crate::display_sync::revert_to_default(Arc::clone(&state_ds)).await;
            return;
        }
        if !prestart_still_current(&video_ds, my_gen) { return; }
        let _ = slint::invoke_from_event_loop(move || {
            let mut vs = video_ds.lock().unwrap();
            // Re-check on the UI thread: a new Play (or a Stop)
            // can be processed between the check above and this
            // closure running — applying then would load THIS
            // item's URL into the next item's session.
            if vs.playback_generation != my_gen || vs.player.is_none() { return; }
            vs.pending_load_url = Some(url_ds);
            vs.play_start = Some(Instant::now());
            vs.display_sync_prestart_active = false;
            vs.display_presynced = presynced;
        });
    });
}

// ── show_playing_ui ───────────────────────────────────────────────────────────
/// The music bar for a song, the fullscreen player for a video.
fn show_playing_ui(
    window_weak: &slint::Weak<MainWindow>,
    title: &str,
    is_audio: bool,
    audio_meta: &Option<(String, String)>,
) {
    if let Some(w) = window_weak.upgrade() {
        let g = AppState::get(&w);
        g.set_playing_title(ss(title));
        g.set_trickplay_has_thumb(false);
        if is_audio {
            // Audio-only: show music bar, not the fullscreen player.
            let (artist, album_art_id) = audio_meta
                .as_ref()
                .map(|(a, i)| (a.as_str(), i.as_str()))
                .unwrap_or(("", ""));
            g.set_is_audio_playing(true);
            g.set_is_playing(false);
            g.set_has_background_player(false);
            g.set_video_behind_ui(false);
            g.set_music_bar_title(ss(title));
            g.set_music_bar_artist(artist.into());
            g.set_music_bar_album_id(album_art_id.into());
            g.set_music_bar_has_art(false);
            g.set_music_bar_quality("".into());
            g.set_music_bar_quality_long("".into());
            g.set_music_bar_paused(false);
            g.set_music_bar_pos(0.0);
            g.set_music_bar_elapsed("0:00".into());
            g.set_music_bar_total("0:00".into());
            // Clear lyrics for the new track; lyrics fetch will re-populate.
            g.set_lyrics_available(false);
            g.set_show_lyrics(false);
            g.set_lyrics_active_idx(-1);
            g.set_lyrics_lines(ModelRc::new(VecModel::<crate::LyricEntry>::default()));
            // The ♪ button (slot 9) un-renders while lyrics-available is
            // false — move focus off it so it can't sit on a hidden button.
            if g.get_music_bar_focused() == 9 {
                g.set_music_bar_focused(8);
            }
        } else {
            // Video: fullscreen player as before.
            g.set_is_audio_playing(false);
            g.set_is_playing(true);
            g.set_has_background_player(false);
            g.set_video_behind_ui(false);
            g.set_is_paused(false);
            g.set_controls_visible(false);
        }
    }
}

// ── spawn_music_cover ─────────────────────────────────────────────────────────
/// A song's cover for the music bar, and Now Playing's blurred background from the same bytes.
fn spawn_music_cover(
    client_art: Arc<JellyfinClient>,
    item_id_art: &str,
    audio_meta: &Option<(String, String)>,
    video: &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
    my_gen: u64,
) {
    let ww_art = window_weak.clone();
    let vid_art = Arc::clone(video);
    let art_id = audio_meta
        .as_ref()
        .map(|(_, i)| i.clone())
        .unwrap_or_else(|| item_id_art.to_string());
    rt_handle.spawn(async move {
        if let Some(bytes) = crate::poster::fetch_poster_cached(&client_art, &art_id).await
            && let Some(spb) = crate::poster::decode_poster_buffer(&bytes)
        {
            // Now Playing's blurred background (Settings → Audio), from the same bytes.
            let blur = crate::poster::decode_blur_buffer(&bytes);
            let _ = slint::invoke_from_event_loop(move || {
                // Generation guard: on fast track skips the previous
                // track's cover could land on the new track's bar.
                if vid_art.lock().unwrap().playback_generation != my_gen {
                    return;
                }
                if let Some(w) = ww_art.upgrade() {
                    let g = AppState::get(&w);
                    if g.get_is_audio_playing() {
                        g.set_music_bar_art(slint::Image::from_rgba8(spb));
                        if let Some(b) = blur {
                            g.set_music_bar_art_blur(slint::Image::from_rgba8(b));
                        }
                        g.set_music_bar_has_art(true);
                    }
                }
            });
        }
    });
}

// ── spawn_lyrics ──────────────────────────────────────────────────────────────
/// Fetch lyrics (Jellyfin 10.9+; gracefully absent when 404).
fn spawn_lyrics(
    item_id_art: &str,
    video: &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle: &tokio::runtime::Handle,
    my_gen: u64,
) {
    let client_lyr = Arc::clone(
        video
            .lock()
            .unwrap()
            .client
            .as_ref()
            .expect("client just set"),
    );
    let item_id_lyr = item_id_art.to_string();
    let video_lyr = Arc::clone(video);
    let ww_lyr = window_weak.clone();
    rt_handle.spawn(async move {
        match client_lyr.get_lyrics(&item_id_lyr).await {
            Ok(Some(lines)) => {
                // Check generation and write in ONE lock scope — a
                // separate check/write pair let a new start_playback
                // slip between them and get the old track's lyrics.
                {
                    let mut vs = video_lyr.lock().unwrap();
                    if vs.playback_generation != my_gen {
                        return;
                    }
                    vs.lyrics = Some(lines.clone());
                    vs.lyrics_available = true;
                }
                let vid_ui = Arc::clone(&video_lyr);
                let _ = slint::invoke_from_event_loop(move || {
                    // Same guard for the UI push (is-audio-playing alone
                    // can't tell one track from the next).
                    if vid_ui.lock().unwrap().playback_generation != my_gen {
                        return;
                    }
                    if let Some(w) = ww_lyr.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_is_audio_playing() {
                            use slint::{ModelRc, VecModel};
                            let entries: Vec<crate::LyricEntry> = lines
                                .into_iter()
                                .map(|(ms, text)| crate::LyricEntry {
                                    text: text.as_str().into(),
                                    start_ms: ms as i32,
                                })
                                .collect();
                            g.set_lyrics_lines(ModelRc::new(VecModel::from(entries)));
                            g.set_lyrics_available(true);
                            g.set_lyrics_active_idx(-1);
                        }
                    }
                });
            }
            Ok(None) => {
                debug!(
                    "no lyrics for {} (not found or server too old)",
                    item_id_lyr
                );
            }
            Err(e) => {
                debug!("lyrics fetch failed for {}: {:#}", item_id_lyr, e);
            }
        }
    });
}

// ── player_init_failed ────────────────────────────────────────────────────────
/// Player::new failed: nothing plays — clear what the segment fetch may set, reset the UI, toast.
fn player_init_failed(
    video: &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    e: anyhow::Error,
) {
    error!("player init failed: {:#}", e);
    // Clear timestamp fields so a fast async response for this failed item
    // can't leave stale segment data for a subsequent play.
    {
        let mut vs = video.lock().unwrap();
        vs.intro_timestamps = None;
        vs.recap_timestamps = None;
        vs.preview_timestamps = None;
        vs.commercial_timestamps = None;
        vs.credits_start = None;
    }
    if let Some(w) = window_weak.upgrade() {
        reset_playback_ui(&w);
    }
    crate::show_toast(
        window_weak.clone(),
        "Couldn't start playback — check your server connection".to_string(),
    );
}
