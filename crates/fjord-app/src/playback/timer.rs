// ── fjord-app · playback/timer.rs ─────────────────────────────────────────
//   wire_mpv_timer          the 16 ms tick, everything deferred until the video lock is released:
//                           - position/duration (one read per tick), music-bar pos/elapsed/total, stats
//                           - skip segments (always-skip / ask / ask-timed / never-skip; the ask-timed
//                             countdown freezes while paused) + the fade-to-black seek
//                           - Up Next banner (credits mode) + countdown task (counts unpaused time only);
//                             credits mark-played, reverted on a rewind; natural-end advance via
//                             resolve_true_next_episode, with an EOF-before-fetch fallback
//                           - gapless preload (backs off after a failed append) and gapless commit
//                           - track auto-select at FileLoaded: remembered per-series picks
//                             (state.remembered_tracks) first, then Config sub/audio languages
//                           - stall recovery: no progress in STALL_SECS (FIRST_OPEN_STALL_SECS for a
//                             first open before FileLoaded) → reload at the last good position, budget
//                             by connection health (ws_connected/keep-alive); a failed open uses the same
//                             budget; playback-stalled drives the "Reconnecting…" overlay
//                           - duration guard: an EOF far short of the duration is never a real end
//                           - HDR Stage 3/4 (Branch A) and display sync (Branch B) — mutually exclusive
//                           - video-init diagnostic (no VideoReconfig 5 s after load)
//   loaded_since/loaded_ok  time since mpv's FileLoaded — chapters, tracks, Branch B and the skip-segment
//                           check key off it, not play_start (before FileLoaded time-pos reads a fake 0)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── wire_mpv_timer ────────────────────────────────────────────────────────────
pub(crate) fn wire_mpv_timer(
    window_weak: slint::Weak<MainWindow>,
    video: Arc<Mutex<VideoState>>,
    state: Arc<Mutex<FjordState>>,
    rt_handle: tokio::runtime::Handle,
    controls_show: Arc<AtomicBool>,
    seek_suppress: Arc<AtomicU32>,
) -> slint::Timer {
    let video_timer = video;
    let window_timer = window_weak;
    let state_timer = state;

    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
        let (gapless_enabled, now_playing_auto_open, connection_likely_healthy) = {
            let s = state_timer.lock().unwrap();
            // See MAX_STALL_RELOAD_ATTEMPTS_HEALTHY/_UNHEALTHY's own doc
            // comment for the full reasoning. 45s (vs. the WS's own 30s
            // keep-alive interval) tolerates one delayed/missed tick
            // without immediately treating an otherwise-fine connection as
            // suspect.
            let healthy = s.ws_connected
                && s.ws_last_keepalive_at.is_some_and(|t| t.elapsed() < Duration::from_secs(45));
            (s.config.device.gapless_audio, s.config.active().now_playing_auto_open, healthy)
        };
        let (finished, banner_trigger, gapless_commit, auto_open_now_playing, credits_mark_played,
             hide_next_ep_banner, stalled_now, stall_reload, stall_give_up, trailer_failed) = {
            let mut vs = video_timer.lock().unwrap();
            let mut banner_trigger: Option<(String, Option<Arc<JellyfinClient>>, u32, bool)> = None;
            // Stall auto-recovery outputs — see the doc comment on the check
            // itself, further down. All three dispatched after the lock
            // releases below, same deferred pattern as everything else here.
            let mut stalled_now  = false;
            let mut stall_reload: Option<(QueueItem, Arc<JellyfinClient>, f64)> = None;
            // Some(toast) = stop and say why (stall budget used up, or a
            // file that kept failing to open).
            let mut stall_give_up: Option<&'static str> = None;
            // Some(url) = a trailer failed to open — close with a toast and
            // mark that link unplayable (2026-10-04).
            let mut trailer_failed: Option<String> = None;
            // Set true by the rewind-past-credits revert below when it cancels an
            // in-flight Up Next countdown; acted on after the lock releases, same
            // deferred pattern as everything else in this tuple.
            let mut hide_next_ep_banner = false;
            // Some((item_id, client, mark_played, revert_ticks)) — dispatched after the lock is
            // released, like banner_trigger/gapless_commit. mark_played=true: POST PlayedItems
            // the moment credits are reached; false: DELETE PlayedItems when the user rewinds
            // back before the trigger in the same playback (a Skip-and-rewind corrects itself).
            // On a revert, revert_ticks re-reports the real position right after the DELETE —
            // progress reports are suppressed while credits_auto_marked_played is true, and a
            // position of 0 + unplayed would drop the item from Continue Watching (ws.rs) until
            // the next report. None for mark=true (its POST sets position 0, as wanted).
            let mut credits_mark_played: Option<(String, Arc<JellyfinClient>, bool, Option<i64>)> = None;

            // One position/duration read per tick (an FFI round trip into libmpv), shared by
            // chapter tracking, the skip-segment and Up Next checks and the gapless preload.
            let (live_pos, live_dur): (Option<f64>, Option<f64>) = match vs.player.as_ref() {
                Some(p) => (Some(p.get_position()), Some(p.get_duration())),
                None    => (None, None),
            };
            // How long ago mpv finished opening the file (None while still
            // opening). Before FileLoaded, time-pos reads 0 — a fake position.
            // Checks that need real media data key off this, not play_start:
            // a slow-opening file (library drive waking, ~13 s on the HTPC)
            // otherwise exhausted the chapter poll before the file loaded, and
            // the intro-skip treated the fake 0 as "inside the intro" during
            // display_sync's pre-decode wait, failed its seek and marked the
            // intro handled — so it then played unskipped.
            let loaded_since: Option<Duration> = vs.player.as_ref()
                .and_then(|p| p.file_loaded_at())
                .map(|t| t.elapsed());

            // Skip-fade duration: Config.device.skip_fade_ms × the live
            // settings-animation-speed multiplier, read once per tick — not
            // baked into a constant, so a live Settings change takes effect
            // immediately and the video half (pending_skip_seek, below) and
            // audio half (skip_fade_audio, further below) can never drift
            // out of sync with each other or with player.slint's own
            // `animate` block, which multiplies the identical AppState
            // property by the identical speed multiplier. Only computed
            // when needed — every other tick this is a no-op `None`.
            let skip_fade_wait: Option<Duration> =
                if vs.pending_skip_seek.is_some() || vs.skip_fade_audio.is_some() {
                    window_timer.upgrade().map(|w| {
                        let g = AppState::get(&w);
                        let base_ms = g.get_settings_skip_fade_ms() as f64;
                        let speed   = g.get_settings_animation_speed() as f64;
                        Duration::from_millis((base_ms * speed).max(0.0) as u64)
                    })
                } else {
                    None
                };

            // Fire a deliberately-delayed skip-segment seek once its fade-out
            // has had time to play (see VideoState.pending_skip_seek's own
            // doc comment). Checked unconditionally, every tick, ahead of
            // everything else below — independent of the stall/skip-segment
            // detection logic further down, which only ever ARMS this.
            if let (Some((seg_end, armed_at)), Some(wait)) = (vs.pending_skip_seek, skip_fade_wait)
                && armed_at.elapsed() >= wait {
                if let Some(p) = vs.player.as_ref() {
                    p.seek_to(seg_end);
                    info!("skip segment: faded seek to {:.1}s", seg_end);
                }
                vs.pending_skip_seek = None;
                if let Some(w) = window_timer.upgrade() {
                    AppState::get(&w).set_skip_fade_active(false);
                }
            }

            // Audio-side companion to the fade above — see SkipFadeAudio's
            // own doc comment for the full two-phase design (PCM: a real
            // volume ramp mirroring the visual fade exactly; SPDIF
            // passthrough: mute, since a raw bitstream can't be volume-
            // ramped at all). Spans BOTH halves of the transition (fade-out
            // + fade-in), unlike pending_skip_seek above, which is cleared
            // the instant the seek itself fires — halfway through this
            // window — so this keeps running after that block has already
            // cleared its own state.
            if let (Some(sfa), Some(wait)) = (vs.skip_fade_audio, skip_fade_wait) {
                let elapsed  = sfa.armed_at.elapsed();
                let complete = elapsed >= wait * 2;
                match (vs.player.as_ref(), sfa.passthrough) {
                    (Some(p), true) => {
                        // Binary, held for the whole window rather than
                        // toggled only at the seek instant — silence-then-
                        // resume instead of a raw content-to-content splice.
                        p.set_mute(!complete);
                    }
                    (Some(p), false) => {
                        let target = if elapsed < wait {
                            let progress = elapsed.as_secs_f64() / wait.as_secs_f64().max(0.001);
                            sfa.orig_volume * (1.0 - progress)
                        } else if !complete {
                            let progress = (elapsed - wait).as_secs_f64() / wait.as_secs_f64().max(0.001);
                            sfa.orig_volume * progress
                        } else {
                            sfa.orig_volume
                        };
                        p.set_volume(target);
                    }
                    (None, _) => {} // player gone (stopped mid-fade) — nothing to act on
                }
                if complete {
                    vs.skip_fade_audio = None;
                }
            }

            if vs.player.is_some() {
                // 2 s after the file actually opened (see loaded_since above) —
                // fps estimate settled, chapter/track lists populated.
                let loaded_ok = loaded_since.is_some_and(|d| d >= Duration::from_secs(2));

                // Stall auto-recovery: mpv loaded, unpaused and rendering but the position not
                // advancing (an audio-device handoff, a network outage). Rolling check: no progress
                // in STALL_SECS (5 s — clears normal 4K HEVC hwdec start-up) — FIRST_OPEN_STALL_SECS
                // for an item's first open before FileLoaded. Paused-for-cache doesn't count (real
                // buffering; a reload would make it worse). Recovery is a full reload (tear down +
                // start_playback at the last known-good position): only a new HTTP connection can
                // succeed once the network is back — a seek on the old one produced a false EOF.
                // Capped by MAX_STALL_RELOAD_ATTEMPTS_HEALTHY/_UNHEALTHY through
                // stall_reload_attempts_for (keyed by item id, so it survives the reload), then it
                // stops cleanly. Background: DEVLOG → "Playback resilience".
                if let (Some(pos), Some(start), Some(p)) = (live_pos, vs.play_start, vs.player.as_ref()) {
                    let (buffering, _) = p.get_buffering();
                    let is_paused = p.is_paused();
                    // p (borrows vs.player) not used past this point — safe to
                    // mutate vs.stall_* below.

                    // A position jump far bigger than 16 ms of playback (a seek either way, a gapless
                    // transition, a chapter/skip jump) resets the progress baseline — otherwise a
                    // backward seek left it at the old, higher position and normal playback had to
                    // climb past it before counting as progress.
                    let is_seek_jump = vs.stall_last_tick_pos.is_some_and(|prev| (pos - prev).abs() >= 2.0);
                    vs.stall_last_tick_pos = Some(pos);

                    // Keep the checkpoint fresh while paused or buffering (like is_stalled's own
                    // exclusion): otherwise the whole pause counts as "no progress" and the first tick
                    // after resuming reloads the stream.
                    if is_seek_jump || is_paused || buffering
                        || vs.stall_last_progress_at.is_none() || pos - vs.stall_last_progress_pos >= 1.0
                    {
                        vs.stall_last_progress_pos = pos;
                        vs.stall_last_progress_at  = Some(Instant::now());
                    }
                    let stalled_for = vs.stall_last_progress_at
                        .map(|t| t.elapsed().as_secs_f64())
                        .unwrap_or(0.0);
                    let first_open = loaded_since.is_none()
                        && !vs.stall_reload_attempts_for.as_ref().is_some_and(|(id, n)| {
                            *n > 0 && vs.item_id.as_deref() == Some(id.as_str())
                        });
                    let stall_limit = if first_open { FIRST_OPEN_STALL_SECS } else { STALL_SECS };
                    if first_open && stalled_for >= STALL_SECS && !vs.stall_grace_logged {
                        vs.stall_grace_logged = true;
                        debug!(
                            "still opening after {stalled_for:.1}s (server disks waking up?) — first open of this item, \
                             waiting up to {FIRST_OPEN_STALL_SECS:.0}s before a reload"
                        );
                    }
                    let is_stalled = start.elapsed() >= Duration::from_secs(5)
                        && stalled_for >= stall_limit
                        && !is_paused
                        && !buffering;
                    stalled_now = is_stalled;

                    // Forgive a previously-exhausted reload budget once this
                    // same item has played genuinely healthily for a while
                    // since the last stall event — otherwise two stalls near
                    // the start of a long video permanently disable recovery
                    // for the rest of it. A server that's truly, permanently
                    // down never earns this: playback re-stalls almost
                    // immediately after every reload, so is_stalled never
                    // stays false long enough for the cooldown to complete.
                    if !is_stalled
                        && let Some((id, _)) = &vs.stall_reload_attempts_for
                        && vs.item_id.as_deref() == Some(id.as_str())
                        && vs.stall_last_reload_at.is_some_and(|t| t.elapsed() >= Duration::from_secs(120))
                    {
                        vs.stall_reload_attempts_for = None;
                    }

                    if is_stalled {
                        let why = format!("playback stalled: {stalled_for:.1}s with no progress at {pos:.2}s");
                        match next_stall_step(&mut vs, connection_likely_healthy, &why) {
                            StallStep::Reload(np, cli, resume_secs) => stall_reload = Some((np, cli, resume_secs)),
                            StallStep::GiveUp => stall_give_up = Some(STALL_GIVE_UP_TOAST),
                            StallStep::NotReloadable => {}
                        }
                    }
                }

                // Start-up diagnostic (2026-10-06, HTPC: "picture and sound
                // stop ~1 s in"): mpv's state every ~250 ms for the first 8 s
                // after the file opened — a real stall shows as pos not
                // moving with core-idle/seeking/paused-for-cache telling why.
                if let (Some(d), Some(p)) = (loaded_since, vs.player.as_ref()) {
                    if d <= Duration::from_secs(8) && vs.startup_snapshot_ticks.is_multiple_of(16) {
                        debug!("mpv start-up +{:.2}s: {}", d.as_secs_f64(), p.startup_snapshot());
                    }
                    vs.startup_snapshot_ticks = vs.startup_snapshot_ticks.wrapping_add(1);
                }

                if loaded_ok && !vs.decoder_logged {
                    if let Some(p) = vs.player.as_ref() {
                        p.log_decoder_info();
                        p.apply_auto_vf();
                    }
                    vs.decoder_logged = true;
                }

                // Diagnostic: warns once if a video item has no VideoReconfig 5 s after the file
                // loaded — a rare HTPC case played audio only with the video track selected (not
                // reproducible on demand; a second attempt worked). Only for video items; timed from
                // FileLoaded, so a slow open doesn't trigger it. DEVLOG → "Known platform issues".
                if !vs.current_is_audio
                    && !vs.video_init_checked
                    && loaded_since.is_some_and(|d| d >= Duration::from_secs(5))
                {
                    if let Some(p) = vs.player.as_ref() && !p.has_seen_video_reconfig() {
                        warn!(
                            "no VideoReconfig event {:.1}s after the file loaded on a video item — \
                             video may be stuck audio-only (see CLAUDE.md known issue, 2026-07-29)",
                            loaded_since.unwrap_or_default().as_secs_f64()
                        );
                    }
                    vs.video_init_checked = true;
                }

                // HDR Stage 3 — one-shot Wayland colour-management negotiation, the moment
                // VideoReconfig has happened. Two mutually exclusive branches: A (here; display sync
                // off) and B (below; display sync on). Don't merge them: B needs `loaded_ok` timing
                // (estimated-vf-fps isn't valid at VideoReconfig), which would delay HDR by ~2 s for
                // everyone with display sync off (the default). With display sync on, the subsurface
                // (not the window) is tagged when this player renders there, so the UI stays sRGB.
                let hdr_target = if vs.video_on_subsurface {
                    crate::video_surface::child_surface_addr()
                } else {
                    None
                };
                let (display_sync_enabled, hdr_toggle_enabled) = {
                    let s = state_timer.lock().unwrap();
                    // Trailers switch the display only when Settings → Video
                    // → Sync display for trailers is on (default off,
                    // 2026-10-04) — otherwise they take Branch A like any
                    // video with display sync off.
                    let sync = s.config.device.display_sync_enabled
                        && (!vs.is_trailer || s.config.device.display_sync_trailers);
                    (sync, s.config.device.target_colorspace_hint)
                };

                if !vs.current_is_audio && !vs.hdr_negotiation_attempted && !display_sync_enabled {
                    // Read what's needed from `p` first, inside its own
                    // borrow scope, then set the one-shot flag afterward —
                    // `p` borrows vs.player immutably, so it can't still be
                    // alive when vs.hdr_negotiation_attempted is mutated.
                    let source_meta = vs.player.as_ref().and_then(|p| {
                        p.has_seen_video_reconfig().then(|| p.query_source_hdr_metadata())
                    });
                    if let Some(meta) = source_meta {
                        vs.hdr_negotiation_attempted = true;
                        if hdr_toggle_enabled {
                            crate::hdr::maybe_negotiate(meta, hdr_target);
                        } else {
                            crate::hdr::set_status_disabled();
                        }
                    }
                }

                // Branch B — display sync on: settle the display's mode/HDR/WCG BEFORE HDR
                // negotiates, so a mode-set and a colour-management negotiation never race (DEVLOG →
                // display_sync). Both one-shot flags are claimed synchronously this tick, before
                // spawning anything, so the ~3 s settle can't re-trigger this; only
                // hdr::maybe_negotiate is deferred to the end of the task, which re-checks
                // playback_generation first. When the display was already switched before load (the
                // normal case), HDR is negotiated as soon as mpv knows the format, like Branch A —
                // waiting showed ~1.6 s of SDR on a TV already in HDR; Branch B then only runs its
                // correction check.
                if !vs.current_is_audio && display_sync_enabled && vs.display_presynced
                    && !vs.hdr_negotiation_attempted
                    && vs.player.as_ref().is_some_and(|p| p.has_seen_video_reconfig())
                {
                    let meta = vs.player.as_ref().unwrap().query_source_hdr_metadata();
                    vs.hdr_negotiation_attempted = true;
                    debug!("hdr: display was switched before load — negotiating at the first VideoReconfig");
                    if hdr_toggle_enabled {
                        crate::hdr::maybe_negotiate(meta, hdr_target);
                    } else {
                        crate::hdr::set_status_disabled();
                    }
                }
                if !vs.current_is_audio && !vs.display_sync_attempted && display_sync_enabled {
                    let ready = loaded_ok
                        && vs.player.as_ref().is_some_and(|p| p.has_seen_video_reconfig());
                    let (meta, dims) = if ready {
                        let p = vs.player.as_ref().unwrap();
                        (p.query_source_hdr_metadata(), p.query_video_dimensions())
                    } else {
                        (Default::default(), (0, 0, 0.0))
                    };
                    // Never pick a mode from an unknown frame rate (2026-10-06,
                    // HTPC: fps 0 → "unusual rate" → a 4K HDR film switched to
                    // 1080p59.94 mid-play). Wait for one; 10 s after load
                    // without one, skip the correction and keep the mode.
                    let fps_known = dims.2 > 0.0;
                    let give_up = loaded_since.is_some_and(|d| d >= Duration::from_secs(10));
                    if ready && (fps_known || give_up) {
                        if !fps_known {
                            warn!("display_sync: no frame rate 10 s after load — keeping the current display mode");
                        }
                        vs.display_sync_attempted    = true;
                        // Already negotiated at VideoReconfig when presynced.
                        let negotiate_after = !vs.hdr_negotiation_attempted;
                        vs.hdr_negotiation_attempted = true;
                        let generation = vs.playback_generation;
                        let ds_settings = crate::display_sync::DisplaySyncSettings::from_device_config(
                            &state_timer.lock().unwrap().config.device,
                        );
                        let video2 = Arc::clone(&video_timer);
                        let state2 = Arc::clone(&state_timer);
                        rt_handle.spawn(async move {
                            if fps_known {
                                crate::display_sync::sync_to_source(state2, dims, meta.clone(), ds_settings).await;
                            }
                            if video2.lock().unwrap().playback_generation != generation {
                                // Stopped/replaced while the mode switch was
                                // settling — whatever's playing now already
                                // ran (or will run) its own Branch B trigger.
                                return;
                            }
                            if !negotiate_after {
                                return;
                            }
                            if hdr_toggle_enabled {
                                crate::hdr::maybe_negotiate(meta, hdr_target);
                            } else {
                                crate::hdr::set_status_disabled();
                            }
                        });
                    }
                }

                // HDR Stage 4: once THIS item's negotiation is confirmed Active, switch mpv to real
                // PQ/BT.2020 output. The HDR worker has no notification, so this polls (like the
                // stats overlay), once per item via hdr_output_applied. It's reset in
                // reset_video_state_for_playback with hdr_negotiation_attempted, and
                // hdr::send_command resets the status to Idle on Unset, so a previous item's Active
                // can't apply to a new one.
                if !vs.current_is_audio && !vs.hdr_output_applied && crate::hdr::is_active() {
                    if let Some(p) = vs.player.as_ref() {
                        p.apply_hdr_output();
                    }
                    vs.hdr_output_applied = true;
                }

                // ── Chapter list loading ──────────────────────────────────────
                // Poll chapter-list/count after the 2 s decoder-logged gate.
                // Retry up to 30 ticks (~480 ms) to handle containers where the
                // chapter metadata appears slightly after the first track data.
                // A count of 0 after 30 attempts is treated as "no chapters".
                if loaded_ok && !vs.chapters_loaded && let Some(p) = vs.player.as_ref() {
                    let count = p.get_chapter_count();
                    if count > 0 {
                        let dur      = p.get_duration();
                        let chapters = p.get_chapters();
                        info!("loaded {} chapters", chapters.len());
                        let marks: Vec<f32> = if dur > 0.0 {
                            chapters.iter().map(|(t, _)| (t / dur) as f32).collect()
                        } else {
                            vec![]
                        };
                        if let Some(w) = window_timer.upgrade() {
                            let g = AppState::get(&w);
                            g.set_chapter_marks(
                                ModelRc::new(VecModel::from(marks)),
                            );
                            let entries: Vec<TrackEntry> = chapters.iter().enumerate().map(|(i, (t, title))| {
                                let ts = fmt_secs(*t).to_string();
                                let label = if title.is_empty() {
                                    ts
                                } else {
                                    format!("{ts}  {title}")
                                };
                                TrackEntry { id: i as i32, label: label.into() }
                            }).collect();
                            g.set_chapter_entries(ModelRc::new(VecModel::from(entries)));
                        }
                        vs.chapters = chapters;
                        vs.chapters_loaded = true;
                    } else if vs.chapter_load_attempts >= 30 {
                        debug!("no chapters after 30 attempts");
                        vs.chapters_loaded = true;
                    } else {
                        vs.chapter_load_attempts += 1;
                    }
                }

                // ── Chapter OSD countdown ─────────────────────────────────────
                if vs.chapter_osd_ticks > 0 {
                    vs.chapter_osd_ticks -= 1;
                    if vs.chapter_osd_ticks == 0 && let Some(w) = window_timer.upgrade() {
                        AppState::get(&w).set_chapter_osd_visible(false);
                    }
                }

                // ── Current chapter tracking ─────────────────────────────────
                if vs.chapters_loaded && !vs.chapters.is_empty()
                    && let (Some(pos), Some(w)) = (live_pos, window_timer.upgrade()) {
                    let new_ch = vs.chapters.iter().rposition(|(t, _)| pos >= *t)
                        .map(|i| i as i32).unwrap_or(-1);
                    let g = AppState::get(&w);
                    if g.get_current_chapter() != new_ch {
                        g.set_current_chapter(new_ch);
                    }
                }

                // ── Sub / audio delay OSD countdown ───────────────────────────
                if vs.delay_osd_ticks > 0 {
                    vs.delay_osd_ticks -= 1;
                    if vs.delay_osd_ticks == 0 && let Some(w) = window_timer.upgrade() {
                        AppState::get(&w).set_delay_osd_visible(false);
                    }
                }
                // Track auto-selection runs as soon as the file has loaded,
                // not 2 s later (2026-10-06, HTPC start-up log): enabling a
                // subtitle/audio track mid-playback made mpv drop its whole
                // read-ahead (23 s → 0) and pause ~1.1 s to refill it — only
                // visible on high-bitrate 4K HDR films ("picture and sound stop
                // ~1 s in, only HDR"). At FileLoaded the picture hasn't
                // started yet (a resume is still seeking), so the refill is
                // part of the normal start-up instead.
                if loaded_since.is_some() && !vs.tracks_loaded
                    && let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                    let tracks = p.get_tracks();
                    // Retry next tick if mpv hasn't parsed the track list yet.
                    if !tracks.is_empty() {
                        debug!("track-list ({} entries):", tracks.len());
                        for t in &tracks {
                            debug!("  [{:>2}] {:5}  selected={}  lang={:5}  title={:?}  codec={}",
                                t.id, t.track_type, t.selected, t.lang, t.title, t.codec);
                        }
                        let sub_model   = build_track_model(&tracks, "sub");
                        let audio_model = build_track_model(&tracks, "audio");
                        let video_model = build_track_model(&tracks, "video");
                        let mut cur_sub = tracks.iter().find(|t| t.track_type == "sub" && t.selected).map(|t| t.id).unwrap_or(0);
                        let mut cur_audio = tracks.iter().find(|t| t.track_type == "audio" && t.selected).map(|t| t.id).unwrap_or(1);
                        let cur_video = tracks.iter().find(|t| t.track_type == "video" && t.selected).map(|t| t.id).unwrap_or(1);
                        debug!("active tracks: sub={} audio={} video={}", cur_sub, cur_audio, cur_video);
                        let g = AppState::get(&w);

                        // Per-series remembered track languages (Phase: remember
                        // last manually-picked track) — checked before falling
                        // back to the global Config.sub_lang/audio_lang below.
                        // Session-only lookup, brief nested lock (no I/O), same
                        // pattern as this file's other state_timer reads.
                        let remembered = vs.playing_series_id.as_ref()
                            .and_then(|sid| state_timer.lock().unwrap().remembered_tracks.get(sid).cloned());

                        // Subtitle auto-select: global off → force 0; else try primary then fallback.
                        // Only switch when mpv's own pick (from the
                        // preferences set before load) differs — every
                        // switch makes mpv re-read its buffer (2026-10-06).
                        if !g.get_settings_sub_enabled() {
                            if cur_sub != 0 && let Some(p) = vs.player.as_ref() { p.set_sub_track(0); }
                            cur_sub = 0;
                        } else {
                            let pref1 = g.get_settings_sub_lang().to_string();
                            let pref2 = g.get_settings_sub_lang2().to_string();
                            let sub_type = g.get_settings_sub_type().to_string();
                            // Remembered pick is already a raw mpv lang code (copied
                            // from a real TrackInfo.lang, not a display name), so it
                            // goes in ahead of sub_lang_code()'s translated codes —
                            // takes priority, but a language with no matching track
                            // in THIS episode still falls through to pref1/pref2
                            // rather than leaving mpv's default unchanged.
                            let mut codes: Vec<String> = Vec::new();
                            if let Some(rl) = remembered.as_ref().and_then(|r| r.sub_lang.clone()) {
                                codes.push(rl.to_ascii_lowercase());
                            }
                            codes.extend([pref1.as_str(), pref2.as_str()].iter()
                                .map(|n| sub_lang_code(n)).filter(|c| !c.is_empty()).map(String::from));
                            if !codes.is_empty() {
                                // 0=Normal, 1=SDH, 2=Forced — type priority per preference.
                                let kind_of = |t: &TrackInfo| -> u8 {
                                    if t.hearing_impaired { 1 } else if t.forced { 2 } else { 0 }
                                };
                                let priority: &[u8] = match sub_type.as_str() {
                                    "Forced"           => &[2, 0, 1],
                                    "Hearing Impaired" => &[1, 0, 2],
                                    _                  => &[0, 1, 2], // Normal / Any / empty
                                };
                                // Outer loop: type priority; inner loop: language codes.
                                // A preferred-type match in pref1_lang beats a fallback-type
                                // match in either language.
                                let found = priority.iter().find_map(|&want_kind| {
                                    codes.iter().find_map(|code| {
                                        tracks.iter().find(|t| {
                                            t.track_type == "sub"
                                            && t.lang.to_ascii_lowercase().starts_with(code.as_str())
                                            && kind_of(t) == want_kind
                                        })
                                    })
                                });
                                if let Some(t) = found {
                                    if t.id == cur_sub {
                                        debug!("sub {} (lang={}) already selected by mpv — no switch", t.id, t.lang);
                                    } else {
                                        info!("auto-selected sub {} (lang={} forced={} hi={}) pref_lang={:?}/{:?} pref_type={:?} (mpv had {})",
                                            t.id, t.lang, t.forced, t.hearing_impaired, pref1, pref2, sub_type, cur_sub);
                                        if let Some(p) = vs.player.as_ref() { p.set_sub_track(t.id); }
                                        cur_sub = t.id;
                                    }
                                }
                                // No match → leave mpv default unchanged
                            }
                        }

                        // Audio language auto-select: if preference set, pick first matching track.
                        // Remembered per-series pick (already a raw mpv lang code)
                        // takes priority over the global Config.audio_lang, same
                        // reasoning as the subtitle block above.
                        let audio_lang_pref = g.get_settings_audio_lang().to_string();
                        let audio_code: String = remembered.as_ref().and_then(|r| r.audio_lang.clone())
                            .map(|l| l.to_ascii_lowercase())
                            .unwrap_or_else(|| sub_lang_code(&audio_lang_pref).to_string());
                        if !audio_code.is_empty() {
                            let audio_tracks: Vec<_> = tracks.iter()
                                .filter(|t| t.track_type == "audio").collect();
                            if audio_tracks.len() > 1 {
                                let found = audio_tracks.iter().find(|t| {
                                    t.lang.to_ascii_lowercase().starts_with(audio_code.as_str())
                                });
                                if let Some(t) = found {
                                    if t.id == cur_audio {
                                        debug!("audio {} (lang={}) already selected by mpv — no switch", t.id, t.lang);
                                    } else {
                                        info!("auto-selected audio {} (lang={}) pref={:?} (mpv had {})", t.id, t.lang, audio_lang_pref, cur_audio);
                                        if let Some(p) = vs.player.as_ref() { p.set_audio_track(t.id); }
                                        cur_audio = t.id;
                                    }
                                }
                                // No match → leave mpv default unchanged
                            }
                        }
                        g.set_sub_tracks(sub_model);
                        g.set_audio_tracks(audio_model);
                        g.set_video_tracks(video_model);
                        g.set_current_sub_id(cur_sub as i32);
                        g.set_current_audio_id(cur_audio as i32);
                        g.set_current_video_id(cur_video as i32);
                        vs.tracks_loaded = true;
                    }
                }

                vs.pos_tick = vs.pos_tick.wrapping_add(1);
                if vs.pos_tick.is_multiple_of(30)
                    && let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                    let pos = p.get_position();
                    let dur = p.get_duration();
                    let (buf_active, buf_pct) = p.get_buffering();
                    let buffered_pos = p.get_buffer_end_fraction();
                    // Done with p (releases immutable borrow on vs)
                    let _ = p;
                    // Also show buffering overlay during initial load: player alive but no
                    // video data yet after 500 ms grace period (covers HDD spin-up delays
                    // where paused-for-cache is false because playback hasn't started yet).
                    let initial_stall = vs.play_start
                        .is_some_and(|t| t.elapsed() >= Duration::from_millis(500))
                        && dur == 0.0;
                    // display-mode-prefetch (2026-09-25): play_start stays
                    // None for the whole deferred pre-decode wait, so
                    // initial_stall alone never fires during it — reuse
                    // this same "Loading…" spinner for that wait too
                    // rather than leaving a silent black screen.
                    let buf_active = buf_active || initial_stall || vs.display_sync_prestart_active;
                    if pos > 0.0 { vs.last_known_pos_ticks = (pos * 10_000_000.0) as i64; }
                    let g = AppState::get(&w);
                    // Suppress position updates while a committed seek is settling.
                    // seek_committed stores 3; each timer tick decrements until 0.
                    // This gives mpv ~1440 ms to update time-pos before we read it,
                    // preventing the bar from jumping back to the pre-seek position.
                    let suppressed = {
                        let n = seek_suppress.load(Ordering::Relaxed);
                        if n > 0 { seek_suppress.fetch_sub(1, Ordering::Relaxed); true }
                        else { false }
                    };
                    if !suppressed {
                        let ratio = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
                        g.set_playback_pos(ratio);
                        g.set_playback_time(fmt_secs(pos));
                        g.set_playback_ends_at(fmt_ends_at(dur - pos));
                        // Also drive music bar position when audio-only
                        if g.get_is_audio_playing() {
                            g.set_music_bar_pos(ratio);
                            g.set_music_bar_elapsed(fmt_secs(pos));
                        }
                    }
                    g.set_playback_total(fmt_secs(dur));
                    g.set_playback_total_secs(dur as f32);
                    if g.get_is_audio_playing() {
                        g.set_music_bar_total(fmt_secs(dur));
                    }
                    g.set_buffering_active(buf_active);
                    g.set_buffering_pct(buf_pct);
                    g.set_buffered_pos(buffered_pos);

                    // ── Lyrics active-line tracking ───────────────────────
                    // Runs for either lyrics surface: the standalone LyricsView
                    // overlay (show-lyrics) or the inline panel on the Now
                    // Playing screen (show-now-playing) — without the latter,
                    // lyrics-active-idx never advanced while only Now Playing
                    // was open, so its lyrics panel looked frozen / didn't scroll.
                    if (g.get_show_lyrics() || g.get_show_now_playing()) && g.get_is_audio_playing()
                        && let Some(lyrics) = vs.lyrics.as_ref() {
                        let pos_ms = (pos * 1000.0) as u64;
                        // Find last line whose start_ms ≤ current position.
                        let new_idx = lyrics.iter()
                            .rposition(|(ms, _)| *ms > 0 && *ms <= pos_ms)
                            .map(|i| i as i32)
                            .unwrap_or(-1);
                        if g.get_lyrics_active_idx() != new_idx {
                            g.set_lyrics_active_idx(new_idx);
                        }
                    }

                    // Report progress to Jellyfin every ~10 s. Skipped while
                    // credits_auto_marked_played is true — the credits-trigger
                    // already told Jellyfin this item is played with position 0
                    // (see the trigger block below, and tear_down_player's
                    // identical guard). An ordinary progress report firing before
                    // teardown would silently re-add a nonzero PlaybackPositionTicks
                    // and undo that mark, same as the stop-report used to before
                    // that fix — except this one fires every ~10s throughout the
                    // rest of playback, not just once at teardown, so it can undo
                    // the mark long before the user ever stops or reaches EOF.
                    if vs.pos_tick.is_multiple_of(600) && !vs.credits_auto_marked_played
                        && let (Some(cli), Some(id)) = (vs.client.as_ref().map(Arc::clone), vs.item_id.clone()) {
                        let ticks  = (pos * 10_000_000.0) as i64;
                        let paused = g.get_is_paused();
                        rt_handle.spawn(async move {
                            if let Err(e) = cli.report_playback_progress(&id, ticks, paused).await {
                                warn!("report_playback_progress failed: {e}");
                            }
                        });
                    }
                }

                // ── Stats poll every ~512 ms (CR2-7, CR2-8) ──────────────────
                // Full poll when overlay is visible; 1 read for passthrough only
                // when hidden so the volume-control guard stays current.
                if vs.pos_tick.is_multiple_of(32)
                    && let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                    if AppState::get(&w).get_stats_visible() {
                        let stats = p.poll_stats();
                        update_stats_window(&w, &stats);
                    } else {
                        AppState::get(&w).set_audio_passthrough_active(p.poll_passthrough());
                    }
                }

                // ── Periodic frame-drop log every 5 min ───────────────────────
                if vs.pos_tick > 0 && vs.pos_tick.is_multiple_of(18750) && let Some(p) = vs.player.as_ref() {
                    let (drops, dec_drops) = p.get_drop_counts();
                    let pos = p.get_position();
                    info!("stats at {:.0}s: frame-drops={} decoder-drops={}", pos, drops, dec_drops);
                }

                // ── Skip segment prompt (Intro / Recap / Preview / Commercial) ─
                // Determine active segment in priority order; dispatch by mode:
                //   always-skip → seek immediately, no overlay
                //   ask         → show single "Skip →" button
                //   ask-timed   → show two-button overlay + countdown; auto-seek on expiry
                //   never-skip  → do nothing
                // Only once the file is open — before that live_pos is a fake 0
                // (see loaded_since).
                if let (Some(pos), Some(_)) = (live_pos, loaded_since) {
                    let seg_in = |t: &Option<Segment>| t.as_ref().is_some_and(|s| pos >= s.start && pos < s.end);

                    // (label, end, key) — key used to look up mode/secs from AppState
                    let seg_info: Option<(&str, f64, &str)> =
                        if seg_in(&vs.intro_timestamps) {
                            vs.intro_timestamps.as_ref().map(|s| ("Skip Intro →", s.end, "intro"))
                        } else if seg_in(&vs.recap_timestamps) {
                            vs.recap_timestamps.as_ref().map(|s| ("Skip Recap →", s.end, "recap"))
                        } else if seg_in(&vs.preview_timestamps) {
                            vs.preview_timestamps.as_ref().map(|s| ("Skip Preview →", s.end, "preview"))
                        } else if seg_in(&vs.commercial_timestamps) {
                            vs.commercial_timestamps.as_ref().map(|s| ("Skip Commercial →", s.end, "commercial"))
                        } else {
                            None
                        };

                    if let Some((label, seg_end, seg_key)) = seg_info {
                        vs.skip_segment_end = Some(seg_end);

                        // Read mode + secs from AppState (timer runs on Slint event loop thread)
                        let (mode, prompt_secs) = if let Some(w) = window_timer.upgrade() {
                            let g = AppState::get(&w);
                            match seg_key {
                                "intro"      => (g.get_settings_skip_intro_mode().to_string(),      g.get_settings_skip_intro_secs() as u32),
                                "recap"      => (g.get_settings_skip_recap_mode().to_string(),      g.get_settings_skip_recap_secs() as u32),
                                "preview"    => (g.get_settings_skip_preview_mode().to_string(),    g.get_settings_skip_preview_secs() as u32),
                                "commercial" => (g.get_settings_skip_commercial_mode().to_string(), g.get_settings_skip_commercial_secs() as u32),
                                _            => ("ask".to_string(), 8u32),
                            }
                        } else {
                            ("ask".to_string(), 8u32)
                        };

                        if vs.skip_segment_handled {
                            // Already handled — ensure overlays are hidden
                            if let Some(w) = window_timer.upgrade() {
                                let g = AppState::get(&w);
                                if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                                if g.get_show_skip_timed()   { g.set_show_skip_timed(false); }
                            }
                        } else {
                            match mode.as_str() {
                                "always-skip" => {
                                    vs.skip_segment_handled = true;
                                    info!("always-skip: fading to {:.1}s", seg_end);
                                    if let Some(w) = window_timer.upgrade() {
                                        let g = AppState::get(&w);
                                        arm_skip_fade(&mut vs, &g, seg_end);
                                        if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                                        if g.get_show_skip_timed()   { g.set_show_skip_timed(false); }
                                        g.set_skip_fade_active(true);
                                    } else {
                                        // Window already gone (app quitting) —
                                        // still arm the seek itself so it isn't
                                        // silently dropped; the audio ramp needs
                                        // AppState (for passthrough detection)
                                        // so it's skipped in this edge case.
                                        vs.pending_skip_seek = Some((seg_end, Instant::now()));
                                    }
                                }
                                "ask" => {
                                    if let Some(w) = window_timer.upgrade() {
                                        let g = AppState::get(&w);
                                        if g.get_show_skip_timed() {
                                            g.set_show_skip_timed(false);
                                            vs.skip_timed_shown_at = None;
                                            vs.skip_timed_paused_since = None;
                                        }
                                        if !g.get_show_skip_segment() {
                                            g.set_show_skip_segment(true);
                                            g.set_skip_segment_label(label.into());
                                        }
                                    }
                                }
                                "ask-timed" => {
                                    if let Some(w) = window_timer.upgrade() {
                                        let g = AppState::get(&w);
                                        if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                                        if vs.skip_timed_shown_at.is_none() {
                                            // First tick in segment: start countdown
                                            vs.skip_timed_shown_at    = Some(Instant::now());
                                            vs.skip_timed_prompt_secs = prompt_secs;
                                            vs.skip_timed_paused_since = None;
                                            g.set_skip_timed_label(label.into());
                                            g.set_skip_timed_secs(prompt_secs as i32);
                                            g.set_skip_timed_focused(0);
                                            g.set_show_skip_timed(true);
                                        } else if g.get_is_paused() {
                                            // While paused, freeze the ask-timed countdown: the tick runs every 16 ms whatever
                                            // mpv's pause state, so remember when the pause began instead of letting the
                                            // countdown run out (and auto-skip) during it.
                                            if vs.skip_timed_paused_since.is_none() {
                                                vs.skip_timed_paused_since = Some(Instant::now());
                                            }
                                        } else {
                                            // Un-pausing: fold however long we were paused into the
                                            // anchor so the next elapsed() read picks up exactly where
                                            // the countdown left off, not from real wall-clock time
                                            // that includes the pause.
                                            if let Some(paused_at) = vs.skip_timed_paused_since.take()
                                                && let Some(anchor) = vs.skip_timed_shown_at.as_mut() {
                                                *anchor += paused_at.elapsed();
                                            }
                                            // Update countdown each tick
                                            let elapsed = vs.skip_timed_shown_at.unwrap().elapsed();
                                            let remaining = (vs.skip_timed_prompt_secs as f64 - elapsed.as_secs_f64())
                                                .max(0.0).ceil() as i32;
                                            if remaining != g.get_skip_timed_secs() {
                                                g.set_skip_timed_secs(remaining);
                                            }
                                            if remaining <= 0 {
                                                // Countdown expired — auto-seek
                                                g.set_show_skip_timed(false);
                                                vs.skip_timed_shown_at  = None;
                                                vs.skip_timed_paused_since = None;
                                                vs.skip_segment_handled = true;
                                                arm_skip_fade(&mut vs, &g, seg_end);
                                                g.set_skip_fade_active(true);
                                                info!("ask-timed auto-skip: fading to {:.1}s", seg_end);
                                            }
                                        }
                                    }
                                }
                                _ => { // "never-skip" or unrecognized
                                    if let Some(w) = window_timer.upgrade() {
                                        let g = AppState::get(&w);
                                        if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                                        if g.get_show_skip_timed() {
                                            g.set_show_skip_timed(false);
                                            vs.skip_timed_shown_at = None;
                                            vs.skip_timed_paused_since = None;
                                        }
                                    }
                                }
                            }
                        }

                        vs.intro_skip_shown      = seg_key == "intro"      && mode == "ask" && !vs.skip_segment_handled;
                        vs.recap_skip_shown      = seg_key == "recap"      && mode == "ask" && !vs.skip_segment_handled;
                        vs.preview_skip_shown    = seg_key == "preview"    && mode == "ask" && !vs.skip_segment_handled;
                        vs.commercial_skip_shown = seg_key == "commercial" && mode == "ask" && !vs.skip_segment_handled;
                    } else {
                        // No segment active — clear all state
                        if let Some(w) = window_timer.upgrade() {
                            let g = AppState::get(&w);
                            if g.get_show_skip_segment() { g.set_show_skip_segment(false); }
                            if g.get_show_skip_timed()   { g.set_show_skip_timed(false); }
                        }
                        vs.intro_skip_shown      = false;
                        vs.recap_skip_shown      = false;
                        vs.preview_skip_shown    = false;
                        vs.commercial_skip_shown = false;
                        vs.skip_segment_end      = None;
                        vs.skip_timed_shown_at   = None;
                        vs.skip_timed_paused_since = None;
                        vs.skip_segment_handled  = false;
                    }
                }

                // ── Up Next banner trigger ────────────────────────────────────
                // Fire once per episode when position reaches credits_start or
                // falls within the last 30 s of the runtime (fallback when the
                // Intro Skipper Credits endpoint is unavailable).
                // Respects skip_credits_mode: always-skip → immediate auto-advance,
                // ask → show banner with countdown, never-skip → no trigger.
                if !vs.next_ep_banner_shown && vs.playing_series_id.is_some()
                    && let (Some(pos), Some(dur)) = (live_pos, live_dur) {
                    let credits_fire = vs.credits_start.is_some_and(|c| c > 0.0 && pos >= c);
                    // Require dur >= 60 s so the banner doesn't fire instantly on short clips.
                    let fallback_fire = dur >= 60.0 && pos > 0.0 && dur - pos <= 30.0;
                    if credits_fire || fallback_fire {
                        // Whichever condition(s) actually fired — not always
                        // credits_start, since a short (<30s) end-credits
                        // sequence makes fallback_fire cross first. The
                        // rewind-revert check below must compare against
                        // THIS, or it immediately (same tick) mistakes a
                        // fallback-triggered mark for "already rewound past
                        // it" whenever credits_start sits later than dur-30.
                        let mut fire_threshold = f64::MAX;
                        if credits_fire  { fire_threshold = fire_threshold.min(vs.credits_start.unwrap()); }
                        if fallback_fire { fire_threshold = fire_threshold.min(dur - 30.0); }
                        let (credits_mode, credits_secs) = window_timer.upgrade()
                            .map(|w| {
                                let g = AppState::get(&w);
                                (g.get_settings_skip_credits_mode().to_string(),
                                 g.get_settings_skip_credits_secs() as u32)
                            })
                            .unwrap_or_else(|| ("ask".to_string(), 30u32));
                        if credits_mode != "never-skip" {
                            vs.next_ep_banner_shown = true;
                            // always-skip: secs=0 (countdown loop is empty), no banner shown
                            let (secs, show_banner) = if credits_mode == "always-skip" {
                                (0u32, false)
                            } else {
                                (credits_secs, true)
                            };
                            banner_trigger = Some((
                                vs.playing_series_id.clone().unwrap(),
                                vs.client.as_ref().map(Arc::clone),
                                secs,
                                show_banner,
                            ));
                            if let (Some(id), Some(cli)) = (vs.item_id.clone(), vs.client.as_ref().map(Arc::clone)) {
                                vs.credits_auto_marked_played = true;
                                vs.credits_mark_threshold = Some(fire_threshold);
                                credits_mark_played = Some((id, cli, true, None));
                            }
                        }
                    }
                }

                // Rewind-past-credits revert: if the credits-trigger above already
                // auto-marked this episode played and the position now sits before
                // that trigger point (the user pressed Skip and rewound to keep
                // watching, e.g. to re-see a scene), un-mark it. Runs every tick
                // independent of next_ep_banner_shown, since that guard is already
                // latched true by the time a rewind could happen. Compares against
                // credits_mark_threshold — the position that actually fired the
                // trigger above — not a freshly-recomputed credits_start/dur-30;
                // those two can disagree (a short end-credits sequence makes the
                // dur-30s fallback fire before credits_start is ever reached), and
                // recomputing here used to cause an immediate same-tick self-revert
                // for any such episode, silently dropping the mark_played call.
                if vs.credits_auto_marked_played
                    && let (Some(pos), Some(threshold)) = (live_pos, vs.credits_mark_threshold)
                    && pos < threshold - 1.0 {
                    vs.credits_auto_marked_played = false;
                    vs.credits_mark_threshold = None;
                    // Re-arm the trigger too: after a rewind past the credits point, watching through it
                    // again must mark the episode played again (else stopping before EOF leaves it
                    // unplayed). The banner fires once per un-reverted pass, like
                    // credits_auto_marked_played.
                    vs.next_ep_banner_shown = false;
                    // Also cancel any in-flight Up Next countdown: clearing
                    // next_ep_pending makes the countdown task's own per-second
                    // !pending_ok check bail within ~1s (it already exists for
                    // the Skip button's on_cancel_auto_advance path — see
                    // main.rs — this just reuses the same mechanism from here),
                    // and hide_next_ep_banner tells the UI to hide the banner
                    // immediately rather than leaving it visible for that ~1s.
                    // Without this, rewinding while the banner's countdown is
                    // still running left the ORIGINAL countdown ticking away
                    // untouched — on expiry it would auto-advance to the next
                    // episode out from under a user who was still mid-rewatch
                    // of the current one, regardless of the revert just above.
                    if vs.next_ep_pending.is_some() {
                        vs.next_ep_pending = None;
                        hide_next_ep_banner = true;
                    }
                    if let (Some(id), Some(cli)) = (vs.item_id.clone(), vs.client.as_ref().map(Arc::clone)) {
                        let ticks = (pos * 10_000_000.0) as i64;
                        credits_mark_played = Some((id, cli, false, Some(ticks)));
                    }
                }

                // Seek accumulation debounce (~480 ms = 30 × 16 ms)
                if vs.seek_pending_ticks > 0 {
                    vs.seek_pending_ticks -= 1;
                    if vs.seek_pending_ticks == 0 {
                        let pending = vs.seek_pending_secs;
                        vs.seek_pending_secs = 0.0;
                        if pending.abs() > 0.001 {
                            if let Some(p) = vs.player.as_ref() {
                                // Logged at debug with on_seek_acc's own line, so a log shows whether a keyboard
                                // seek both accumulated and reached mpv.
                                debug!("seek_acc: executing debounced seek of {pending:+.1}s");
                                if pending > 0.0 { p.seek_forward(pending); }
                                else             { p.seek_backward(-pending); }
                            } else {
                                debug!("seek_acc: debounce fired with {pending:+.1}s pending but no player — dropped");
                            }
                        }
                        if let Some(w) = window_timer.upgrade() {
                            let g = AppState::get(&w);
                            g.set_seek_osd_visible(false);
                            g.set_seek_bar_pos(0.0);
                            g.set_seek_bar_time("".into());
                            g.set_seek_delta_text("".into());
                        }
                    }
                }

                if controls_show.swap(false, Ordering::Relaxed) {
                    vs.controls_idle_ticks = 0;
                } else {
                    vs.controls_idle_ticks = vs.controls_idle_ticks.saturating_add(1);
                }
                if vs.controls_idle_ticks == 187 && let Some(w) = window_timer.upgrade() {
                    let g = AppState::get(&w);
                    g.set_controls_visible(false);
                    // Force Slint to re-evaluate the cursor at the last-known position.
                    // Slint only calls set_cursor_visible() during mouse event processing;
                    // dispatching PointerMoved at the same coordinates triggers that path
                    // without changing mouse-x/y (so show-controls won't fire).
                    let cx = g.get_player_cursor_x();
                    let cy = g.get_player_cursor_y();
                    w.window().dispatch_event(WindowEvent::PointerMoved {
                        position: LogicalPosition::new(cx, cy),
                    });
                }
            }

            // Idle-ticks + auto-open Now Playing (Settings → Audio → MUSIC,
            // default on; fixed 30 s threshold). Pinned to 0 while the screen
            // IS open so any close path — keyboard, mouse click, Confirm on a
            // control — needs a fresh 30 s of idle before it can pop again.
            // The actual invoke happens AFTER this block releases `vs` (below,
            // alongside gapless_commit/banner_trigger) — invoke_open_now_playing
            // synchronously calls refresh-queue-display, which locks this same
            // mutex; firing it while `vs` is still held self-deadlocked the UI
            // thread (mpv's own audio thread kept playing regardless, which is
            // why music continued while the interface froze solid).
            let mut auto_open_now_playing = false;
            if vs.current_is_audio && vs.player.is_some() {
                if let Some(w) = window_timer.upgrade() {
                    let g = AppState::get(&w);
                    if g.get_show_now_playing() {
                        vs.music_idle_ticks = 0;
                    } else {
                        vs.music_idle_ticks = vs.music_idle_ticks.saturating_add(1);
                        if now_playing_auto_open && vs.music_idle_ticks == 1875 {
                            auto_open_now_playing = true;
                        }
                    }
                }
            } else {
                vs.music_idle_ticks = 0;
            }

            // Gapless preload: near the end of an audio track, append what
            // natural end will play next into the SAME mpv instance so the
            // transition happens without a player rebuild (no audible gap).
            if vs.gapless_retry_cooldown > 0 {
                vs.gapless_retry_cooldown -= 1;
            } else if gapless_enabled && vs.current_is_audio && vs.preloaded_next.is_none() {
                let (pos, dur) = (live_pos.unwrap_or(0.0), live_dur.unwrap_or(0.0));
                if dur > 1.0 && pos > 0.0 && dur - pos < 12.0 {
                    let next = peek_natural_next(&vs).filter(|q| q.item_type == "Audio");
                    if let Some(qi) = next {
                        let url = vs.client.as_ref().map(|c| c.direct_play_url(&qi.id));
                        if let (Some(url), Some(p)) = (url, vs.player.as_mut()) {
                            if p.append_gapless(&url).is_ok() {
                                info!("gapless: preloaded next track {}", qi.id);
                                vs.preloaded_next = Some(qi);
                            } else {
                                warn!("gapless: append_gapless failed for {}, backing off ~1s", qi.id);
                                vs.gapless_retry_cooldown = 62; // ~1s at 16ms/tick
                            }
                        }
                    }
                }
            }

            let poll = if let Some(player) = vs.player.as_mut() {
                player.poll()
            } else {
                PollResult::Running
            };
            let finished = matches!(poll, PollResult::Finished);

            // A file that failed to open/play (2026-10-04 — used to be
            // ignored, leaving a black player until Stop). Library items go
            // through the same reload budget as a stall (a server hiccup
            // often clears on a fresh request — see the stall-reload
            // successes in the HTPC logs); trailers close at once, since a
            // blocked/removed YouTube video won't come back.
            // (Skipped if the stall watchdog already acted this tick, so
            // one failure never uses up two reload attempts.)
            if let (PollResult::Failed(code), None, None) = (&poll, &stall_reload, stall_give_up) {
                let code = *code;
                if vs.is_trailer {
                    trailer_failed = Some(vs.trailer_url.clone().unwrap_or_default());
                } else {
                    let why = format!("file failed to open/play (mpv error {code})");
                    match next_stall_step(&mut vs, connection_likely_healthy, &why) {
                        StallStep::Reload(np, cli, resume_secs) => stall_reload = Some((np, cli, resume_secs)),
                        StallStep::GiveUp | StallStep::NotReloadable => stall_give_up = Some(FAILED_OPEN_TOAST),
                    }
                }
            }

            // Gapless transition: mpv already plays the preloaded entry — commit
            // the bookkeeping and hand the UI/report work to the code below.
            let mut gapless_commit: Option<(QueueItem, u64, Option<String>, i64)> = None;
            if matches!(poll, PollResult::TrackChanged) && let Some(qi) = vs.preloaded_next.take() {
                commit_natural_next(&mut vs, &qi);
                let old_id    = vs.item_id.clone();
                let old_ticks = vs.last_known_pos_ticks;
                vs.playback_generation = vs.playback_generation.wrapping_add(1);
                let generation = vs.playback_generation;
                vs.item_id              = Some(qi.id.clone());
                vs.now_playing          = Some(qi.clone());
                vs.current_is_audio     = true;
                vs.lyrics               = None;
                vs.lyrics_available     = false;
                vs.last_known_pos_ticks = 0;
                gapless_commit = Some((qi, generation, old_id, old_ticks));
            }

            (finished, banner_trigger, gapless_commit, auto_open_now_playing, credits_mark_played,
             hide_next_ep_banner, stalled_now, stall_reload, stall_give_up, trailer_failed)
        };

        if hide_next_ep_banner && let Some(w) = window_timer.upgrade() {
            AppState::get(&w).set_show_next_ep_banner(false);
        }

        // Stall indicator — distinct from the cache-buffering spinner
        // (buffering-active), which only reflects mpv's own paused-for-cache
        // state and stayed false throughout the real outage that motivated
        // this (mpv was actively erroring/retrying, not calmly waiting for
        // cache) — see CLAUDE.md's Playback resilience section. Set every
        // tick so it clears the moment position resumes advancing.
        if let Some(w) = window_timer.upgrade() {
            AppState::get(&w).set_playback_stalled(stalled_now);
        }

        // Stall recovery: reload the same item fresh at the last known-good
        // position — a new HTTP connection, not a seek within one mpv may
        // have already abandoned. Dispatched here, after `vs` is released,
        // since start_playback re-locks video_timer itself.
        if let Some((np, cli, resume_secs)) = stall_reload {
            let mut config = state_timer.lock().unwrap().player_config();
            config.start_position_secs = if resume_secs > 0.0 { Some(resume_secs) } else { None };
            let url = cli.direct_play_url(&np.id);
            start_playback(url, np.id, &np.item_type, np.title, config, cli,
                           np.series_id, np.audio_meta, &video_timer, &window_timer, &rt_handle,
                           &state_timer, None);
        }

        // Stall recovery gave up (the applicable MAX_STALL_RELOAD_ATTEMPTS_* budget exhausted, still
        // no progress) — stop cleanly rather than leave the video frozen
        // forever with no feedback. do_stop_playback already reports the
        // correct resume position (last_known_pos_ticks, preserved even
        // though the live position may itself be reading 0 by this point)
        // and never advances to anything else.
        if let Some(msg) = stall_give_up {
            do_stop_playback(&video_timer, &window_timer, &rt_handle, &state_timer);
            crate::show_toast(window_timer.clone(), msg.to_string());
        }
        if let Some(url) = trailer_failed {
            do_stop_playback(&video_timer, &window_timer, &rt_handle, &state_timer);
            crate::show_toast(window_timer.clone(), TRAILER_FAILED_TOAST.to_string());
            crate::discover::mark_trailer_unplayable(&state_timer, &window_timer, &rt_handle, url);
        }

        // Auto-open Now Playing: fires here, after `vs` is released, so its
        // callback chain (refresh-queue-display → push_queue_display) can
        // safely re-lock VideoState without deadlocking this thread.
        if auto_open_now_playing && let Some(w) = window_timer.upgrade() {
            AppState::get(&w).invoke_open_now_playing();
        }

        // Credits-trigger auto mark-played / rewind-revert (see the block above
        // where credits_mark_played is set). Best-effort, matching every other
        // playback-reporting call in this file: log and move on on failure.
        if let Some((id, cli, played, revert_ticks)) = credits_mark_played {
            rt_handle.spawn(async move {
                let result = if played { cli.mark_played(&id).await } else { cli.mark_unplayed(&id).await };
                if let Err(e) = result {
                    warn!("credits-trigger mark_played({played}) failed: {e:#}");
                    return;
                }
                // Revert only: mark_unplayed resets the server's position to 0,
                // same as mark_played does — but here the user is actively
                // rewatching, not starting over, so immediately correct it to
                // the real position rather than leaving it at 0 for up to ~10s
                // until the next ordinary progress tick (suppressed the whole
                // time credits_auto_marked_played was true — see the gate on
                // report_playback_progress below). Closes the exact window
                // where ws.rs's UserDataChanged handling would otherwise
                // misread position=0+unplayed as "untouched" and drop the row
                // from Continue Watching mid-rewatch.
                if let Some(ticks) = revert_ticks
                    && let Err(e) = cli.report_playback_progress(&id, ticks, false).await {
                    warn!("credits-trigger revert position correction failed: {e:#}");
                }
            });
        }

        // ── Gapless transition: update UI + progress reports, no teardown ─────
        if let Some((qi, generation, old_id, old_ticks)) = gapless_commit {
            info!("gapless: now playing {} — {}", qi.id, qi.title);
            apply_audio_track(&video_timer, &window_timer, &rt_handle, &qi, generation);
            if let Some(w) = window_timer.upgrade() {
                crate::push_queue_display(&video_timer.lock().unwrap(), &AppState::get(&w));
            }
            // Preserved-by-poster-id art (push_queue_display, above) covers a same-album
            // advance for free; this covers the remaining case — a poster-id never seen
            // in the Up Next strip before (e.g. the queue crosses into a different album).
            if let Some(cli) = video_timer.lock().unwrap().client.as_ref().map(Arc::clone) {
                crate::spawn_queue_poster_loading(cli, window_timer.clone(), rt_handle.clone());
            }
            let client = video_timer.lock().unwrap().client.as_ref().map(Arc::clone);
            if let Some(cli) = client {
                let new_id = qi.id.clone();
                rt_handle.spawn(async move {
                    if let Some(old) = old_id && let Err(e) = cli.report_playback_stopped(&old, old_ticks).await {
                        warn!("gapless stop report: {e:#}");
                    }
                    if let Err(e) = cli.report_playback_start(&new_id).await {
                        warn!("gapless start report: {e:#}");
                    }
                });
            }
        }

        // ── Spawn Up Next countdown task when trigger fired ───────────────────
        if let Some((series_id, Some(cli), credits_secs, show_banner)) = banner_trigger {
            let state2  = Arc::clone(&state_timer);
            let video2  = Arc::clone(&video_timer);
            let ww2     = window_timer.clone();
            let rt2     = rt_handle.clone();
            // Capture generation so rapid episode skips cancel the old task immediately
            // instead of waiting up to 1 s for the next loop tick (CR2-10).
            let my_gen          = video_timer.lock().unwrap().playback_generation;
            let current_item_id = video_timer.lock().unwrap().item_id.clone();
            rt_handle.spawn(async move {
                // Resolve against the series' ordered episode list, not /Shows/NextUp — see
                // resolve_true_next_episode (NextUp can return the current episode or suggest a
                // rewatch once the credits mark has landed).
                let Some(current_id) = current_item_id else { return; };
                let Some(next) = resolve_true_next_episode(&cli, &series_id, &current_id).await else { return; };
                info!("up-next: queued {} (secs={} banner={})", next.id, credits_secs, show_banner);

                // Check generation and set next_ep_pending in one lock scope — holding the lock
                // across both prevents start_playback from incrementing the generation and
                // clearing next_ep_pending between the guard and the write.
                {
                    let mut vs = video2.lock().unwrap();
                    if vs.player.is_none() || vs.playback_generation != my_gen { return; }
                    vs.next_ep_pending = Some(next.clone());
                }

                if show_banner {
                    let title_str = next.display_name();
                    let t = SharedString::from(title_str.as_str());
                    let next_ep_secs = next.run_time_ticks.unwrap_or(0) as f64 / 10_000_000.0;
                    let ends_at = fmt_ends_at(next_ep_secs);
                    let _ = slint::invoke_from_event_loop({
                        let ww = ww2.clone();
                        move || {
                            if let Some(w) = ww.upgrade() {
                                let g = AppState::get(&w);
                                g.set_next_ep_title(t);
                                g.set_next_ep_ends_at(ends_at);
                                g.set_next_ep_secs(credits_secs as i32);
                                g.set_next_ep_banner_focused(0);
                                g.set_show_next_ep_banner(true);
                            }
                        }
                    });
                }

                // Count credits_secs down to 0 in real UNPAUSED seconds, polling every 250 ms for
                // cancellation and pause state — a pause freezes the countdown (like the ask-timed
                // skip countdown). credits_secs == 0 (always-skip): the loop doesn't run.
                let mut remaining_secs = credits_secs as f64;
                let mut last_tick      = Instant::now();
                let mut last_shown     = credits_secs as i32;
                while remaining_secs > 0.0 {
                    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
                    let now = Instant::now();
                    let dt  = now.duration_since(last_tick).as_secs_f64();
                    last_tick = now;

                    let (still_playing, pending_ok, gen_ok, paused) = {
                        let vs = video2.lock().unwrap();
                        (vs.player.is_some(), vs.next_ep_pending.is_some(),
                         vs.playback_generation == my_gen,
                         vs.player.as_ref().is_some_and(|p| p.is_paused()))
                    };
                    if !still_playing || !pending_ok || !gen_ok {
                        // !still_playing: video ended naturally — let the natural-end path in
                        //   the 16 ms timer take() next_ep_pending and advance. Clearing it here
                        //   would race with that path and silently drop the episode advance.
                        // !gen_ok: start_playback already cleared next_ep_pending.
                        // !pending_ok: user pressed Skip/cancel, already cleared.
                        return;
                    }
                    if paused {
                        // Frozen: this tick's elapsed time is deliberately not consumed —
                        // just keep polling for a resume (or cancellation) at 250ms.
                        continue;
                    }
                    remaining_secs = (remaining_secs - dt).max(0.0);
                    let shown = remaining_secs.ceil() as i32;
                    if show_banner && shown != last_shown {
                        last_shown = shown;
                        let _ = slint::invoke_from_event_loop({
                            let ww = ww2.clone();
                            move || {
                                if let Some(w) = ww.upgrade() {
                                    AppState::get(&w).set_next_ep_secs(shown);
                                }
                            }
                        });
                    }
                }

                // Countdown reached 0 (or was 0 for always-skip) — play next now.
                let next = video2.lock().unwrap().next_ep_pending.take();
                let Some(next) = next else { return; };

                let config = state2.lock().unwrap().player_config();
                let cli2   = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                let Some(cli2) = cli2 else { return; };

                let url        = cli2.direct_play_url(&next.id);
                let title      = next.display_name();
                let ep_id      = next.id.clone();
                let series_id2 = next.series_id.clone();
                let video_info = next.video_stream_info();
                info!("up-next countdown expired, starting {}", ep_id);

                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww2.upgrade() {
                        AppState::get(&w).set_show_next_ep_banner(false);
                        start_playback(url, ep_id, "Episode", title, config, cli2,
                                       series_id2, None, &video2, &ww2, &rt2,
                                       &state2, video_info);
                    }
                });
            });
        }

        if finished {
            let (dropped, dec_dropped) = video_timer.lock().unwrap().player.as_ref()
                .map(|p| p.get_drop_counts()).unwrap_or((0, 0));
            info!("playback finished: frame-drops={} decoder-drops={}", dropped, dec_dropped);
            // Raw position/duration read BEFORE tear_down_player — deliberately
            // not final_ticks (below), which credits_auto_marked_played can
            // force to 0 for an episode that reached a genuine physical EOF
            // well after already being marked played; using mpv's own live
            // position here avoids that special case entirely.
            let (had_series, advance_series_id, live_pos_at_eof, live_dur_at_eof) = {
                let vs = video_timer.lock().unwrap();
                let (pos, dur) = vs.player.as_ref()
                    .map(|p| (p.get_position(), p.get_duration()))
                    .unwrap_or((0.0, 0.0));
                (vs.playing_series_id.is_some(), vs.playing_series_id.clone(), pos, dur)
            };
            let (item_id, client, ss_cookie, final_ticks) = {
                let mut vs = video_timer.lock().unwrap();
                vs.playing_series_id = None;
                tear_down_player(&mut vs)
            };
            let finished_item_id = item_id.clone();
            uninhibit_screensaver(ss_cookie);

            // Whether something plays next is decided up to 3 ways below (synchronously,
            // a deferred start_playback this tick, or async via resolve_true_next_episode), so
            // capture playback_generation now — bumped only by a real new-item start, never by
            // tear_down_player — and let a deferred check decide once every path has resolved.
            let display_sync_gen = video_timer.lock().unwrap().playback_generation;
            {
                let state2 = Arc::clone(&state_timer);
                let video2 = Arc::clone(&video_timer);
                rt_handle.spawn(async move {
                    // Generous enough to cover the slowest real path (the
                    // async fallback's own network round trip) without
                    // leaving the display wrong for long when genuinely
                    // nothing is next.
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                    if video2.lock().unwrap().playback_generation != display_sync_gen {
                        // Something genuinely started next — that new
                        // item's own display_sync trigger already handles
                        // whatever the display needs, don't fight it.
                        return;
                    }
                    crate::display_sync::revert_to_default(state2).await;
                });
            }

            // Duration guard: a real natural end lands at (or very near) the duration. An EOF
            // more than 60 s short is a broken stream (e.g. a stall reload hitting a dead
            // connection) — never mark played or advance; stop cleanly with a toast instead.
            // Background: DEVLOG → "Playback resilience" (an EOF at 0.0 s of a 25-min episode).
            let premature = live_dur_at_eof > 0.0 && live_dur_at_eof - live_pos_at_eof > 60.0;
            if premature {
                warn!(
                    "playback ended at {:.1}s of {:.1}s — too far from the real duration to be a natural end, not advancing",
                    live_pos_at_eof, live_dur_at_eof
                );
                crate::show_toast(window_timer.clone(), "Playback stopped — lost connection to server".to_string());
            }

            if let Some(w) = window_timer.upgrade() { reset_playback_ui(&w); }

            // Stop report then home refresh, sequenced so Jellyfin has processed the stop
            // before we fetch continue-watching.
            if let (Some(id), Some(cli)) = (item_id, client) {
                let ww_home    = window_timer.clone();
                let rth_home   = rt_handle.clone();
                let state_home = Arc::clone(&state_timer);
                rt_handle.spawn(async move {
                    if let Err(e) = cli.report_playback_stopped(&id, final_ticks).await {
                        warn!("report_playback_stopped (natural end) failed: {e}");
                    }
                    let home_data = fetch_home_data(&cli, true).await;
                    let sections  = home_data_sections(&home_data);
                    let ww2       = ww_home.clone();
                    let watchlist = state_home.lock().unwrap().jellyfin_watchlist_ids.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() { push_home_data(&w, &home_data, &watchlist); }
                    });
                    spawn_poster_loading(cli, sections, ww_home, rth_home, state_home);
                });
            }

            // Playlist/queue advance when not a series item. Suppressed on a
            // premature "end" (see the duration guard above) — an EOF that
            // isn't a genuine finish must never start whatever's next.
            if !had_series && !premature {
                let next_item: Option<QueueItem> = {
                    let mut vs = video_timer.lock().unwrap();
                    // Class-gated advance: audio only follows audio, video only
                    // follows video — a movie ending must not start queued music.
                    let ended_audio = vs.current_is_audio;
                    let queue_head_matches = vs.queue.first()
                        .map(|q| (q.item_type == "Audio") == ended_audio)
                        .unwrap_or(false);
                    if let Some(q) = repeat_one_target(&vs) {
                        // Repeat One: replay the song that just ended, with or
                        // without a playlist loaded (see repeat_one_target).
                        info!("repeat one: replaying {}", q.id);
                        Some(q)
                    } else if repeat_all_ring(&vs) {
                        // Repeat All, no album playlist: rotate the queue ring.
                        let next = take_repeat_all_ring_next(&mut vs);
                        if let Some(q) = &next {
                            info!("repeat all: next {} ({} song(s) in the loop)", q.id, vs.queue.len() + 1);
                        }
                        next
                    } else if ended_audio && !vs.playlist.is_empty() {
                        // Playlist mode (album/artist): advance with repeat/shuffle logic.
                        let len      = vs.playlist.len();
                        let next_idx = match vs.repeat_mode {
                            RepeatMode::One => Some(vs.playlist_index), // restart same track
                            RepeatMode::Off | RepeatMode::All => {
                                if vs.shuffle && !vs.shuffle_order.is_empty() {
                                    let cur_pos = vs.shuffle_order.iter()
                                        .position(|&i| i == vs.playlist_index)
                                        .unwrap_or(0);
                                    let next_pos = cur_pos + 1;
                                    match vs.repeat_mode {
                                        RepeatMode::Off => vs.shuffle_order.get(next_pos).copied(),
                                        RepeatMode::All => Some(vs.shuffle_order[next_pos % len]),
                                        RepeatMode::One => unreachable!(),
                                    }
                                } else {
                                    let next = vs.playlist_index + 1;
                                    match vs.repeat_mode {
                                        RepeatMode::Off => if next < len { Some(next) } else { None },
                                        RepeatMode::All => Some(next % len),
                                        RepeatMode::One => unreachable!(),
                                    }
                                }
                            }
                        };
                        if let Some(idx) = next_idx {
                            vs.playlist_index = idx;
                            Some(vs.playlist[idx].clone())
                        } else if queue_head_matches {
                            // Playlist exhausted (Repeat Off) — queued audio plays next.
                            Some(vs.queue.remove(0))
                        } else {
                            None
                        }
                    } else if queue_head_matches {
                        // Context-menu queue: pop from front (same media class only).
                        Some(vs.queue.remove(0))
                    } else {
                        None
                    }
                };

                if let Some(q) = next_item {
                    let config = state_timer.lock().unwrap().player_config();
                    let cli    = state_timer.lock().unwrap().client.as_ref().map(Arc::clone);
                    if let Some(cli) = cli {
                        let remaining = upcoming_count(&video_timer.lock().unwrap());
                        let audio_m  = q.audio_meta.clone();
                        let url      = cli.direct_play_url(&q.id);
                        let ww_q     = window_timer.clone();
                        let vid_q    = Arc::clone(&video_timer);
                        let rt_q     = rt_handle.clone();
                        info!("playlist/queue advance: starting {} ({} remaining)", q.id, remaining);
                        let vid_rq = Arc::clone(&vid_q);
                        let cli2 = Arc::clone(&cli);
                        let ww_q2 = ww_q.clone();
                        let rt_q2 = rt_q.clone();
                        let state_q = Arc::clone(&state_timer);
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww_q.upgrade() {
                                let g = AppState::get(&w);
                                crate::push_queue_display(&vid_rq.lock().unwrap(), &g);
                                // See the gapless-commit site above for why this pairs
                                // with push_queue_display: only a genuinely new poster-id
                                // (crossing into a different album) needs this network
                                // fetch — a same-album advance is already covered by
                                // push_queue_display's own poster-id preservation.
                                crate::spawn_queue_poster_loading(cli2, ww_q2, rt_q2);
                                start_playback(url, q.id, &q.item_type, q.title, config, cli,
                                               q.series_id, audio_m, &vid_q, &ww_q, &rt_q,
                                               &state_q, None);
                            }
                        });
                    }
                }
            }

            if had_series && !premature {
                let next = video_timer.lock().unwrap().next_ep_pending.take();
                if let Some(next) = next {
                    let config = state_timer.lock().unwrap().player_config();
                    let cli    = state_timer.lock().unwrap().client.as_ref().map(Arc::clone);
                    if let Some(cli) = cli {
                        let url        = cli.direct_play_url(&next.id);
                        let title      = next.display_name();
                        let ep_id      = next.id.clone();
                        let series_id  = next.series_id.clone();
                        let video_info = next.video_stream_info();
                        info!("natural end with pending next-ep, starting {}", ep_id);
                        if let Some(w) = window_timer.upgrade() {
                            AppState::get(&w).set_show_next_ep_banner(false);
                        }
                        start_playback(url, ep_id, "Episode", title, config, cli,
                                       series_id, None, &video_timer, &window_timer, &rt_handle,
                                       &state_timer, video_info);
                    }
                } else if let (Some(sid), Some(current_id)) = (advance_series_id, finished_item_id) {
                    // EOF arrived before the background next-up fetch completed.
                    // The countdown task bails when player.is_none(), so next_ep_pending was
                    // never set. Spawn a fresh fetch as a fallback — but only when the credits
                    // mode actually wants an advance (never-skip means stop here).
                    let skip_mode = state_timer.lock().unwrap().config.active().skip_credits_mode.clone();
                    if skip_mode != "never-skip" {
                        let end_gen = video_timer.lock().unwrap().playback_generation;
                        let video2  = Arc::clone(&video_timer);
                        let state2  = Arc::clone(&state_timer);
                        let ww2     = window_timer.clone();
                        let rt2     = rt_handle.clone();
                        rt_handle.spawn(async move {
                            let cli = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                            let Some(cli) = cli else { return; };
                            // Resolve against the ordered episode list, not /Shows/NextUp — by natural EOF the
                            // credits mark has usually landed, and NextUp would suggest a rewatch of the last
                            // episode instead of "nothing next".
                            let Some(next) = resolve_true_next_episode(&cli, &sid, &current_id).await else { return; };
                            // Bail if the user started watching something else.
                            if video2.lock().unwrap().playback_generation != end_gen { return; }
                            let config = state2.lock().unwrap().player_config();
                            let cli2   = state2.lock().unwrap().client.as_ref().map(Arc::clone);
                            let Some(cli2) = cli2 else { return; };
                            let url   = cli2.direct_play_url(&next.id);
                            let title = next.display_name();
                            let ep_id = next.id.clone();
                            let sid2  = next.series_id.clone();
                            let video_info = next.video_stream_info();
                            info!("natural-end fallback advance: starting {}", ep_id);
                            let _ = slint::invoke_from_event_loop(move || {
                                if ww2.upgrade().is_some() {
                                    start_playback(url, ep_id, "Episode", title, config, cli2,
                                                   sid2, None, &video2, &ww2, &rt2,
                                                   &state2, video_info);
                                }
                            });
                        });
                    }
                }
            }
        }
    });
    timer
}
