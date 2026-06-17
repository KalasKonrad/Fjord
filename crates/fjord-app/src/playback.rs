// ── fjord-app · playback.rs ──────────────────────────────────────────────────
//   VideoState              mpv Player + MpvRenderCtx, GL FBOs, playback metadata
//   fmt_secs                seconds → "H:MM:SS" / "M:SS"
//   build_track_model       Vec<TrackInfo> → ModelRc<TrackEntry> for Slint
//   inhibit_screensaver     call org.freedesktop.ScreenSaver.Inhibit via busctl → cookie
//   uninhibit_screensaver   call UnInhibit(cookie) to release inhibitor
//   tear_down_player        capture ticks, drop render_ctx then player (mpv invariant), return stop data
//   quit_cleanup            synchronous stop report + screensaver release called after window.run() exits
//   start_playback          tear down any existing player, open URL in mpv, report to Jellyfin, spawn intro-skip task
//   wire_rendering_notifier GL thread: FBO render + report_swap() for vsync feedback
//   wire_mpv_timer          16 ms timer: position, stats, intro skip, auto-advance
// ─────────────────────────────────────────────────────────────────────────────
use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use fjord_api::{models::IntroTimestamps, JellyfinClient};
use fjord_player::{MpvRenderCtx, Player, PlayerConfig, PollResult, TrackInfo};
use slint::{ComponentHandle, Global, LogicalPosition, ModelRc, SharedString, VecModel};
use slint::platform::WindowEvent;
use tracing::{debug, error, info, warn};

use crate::config::FjordState;
use crate::home::{fetch_home_data, home_data_sections, push_home_data};
use crate::poster::spawn_poster_loading;
use crate::AppState;
use crate::stats::update_stats_window;
use crate::MainWindow;
use crate::TrackEntry;

fn ss(s: &str) -> SharedString { SharedString::from(s) }

// ── screensaver inhibitor ─────────────────────────────────────────────────────

fn inhibit_screensaver() -> Option<u32> {
    let out = std::process::Command::new("busctl")
        .args(["call", "--session",
               "org.freedesktop.ScreenSaver",
               "/org/freedesktop/ScreenSaver",
               "org.freedesktop.ScreenSaver",
               "Inhibit", "ss",
               "Fjord", "Video playback"])
        .output()
        .ok()?;
    // busctl output is "u <cookie>\n"
    let stdout = String::from_utf8_lossy(&out.stdout);
    let cookie = stdout.trim().strip_prefix("u ").and_then(|s| s.parse().ok());
    if let Some(c) = cookie {
        info!("screensaver inhibited (cookie={})", c);
    } else {
        debug!("screensaver inhibit failed or busctl unavailable");
    }
    cookie
}

pub(crate) fn uninhibit_screensaver(cookie: u32) {
    let _ = std::process::Command::new("busctl")
        .args(["call", "--session",
               "org.freedesktop.ScreenSaver",
               "/org/freedesktop/ScreenSaver",
               "org.freedesktop.ScreenSaver",
               "UnInhibit", "u",
               &cookie.to_string()])
        .status();
    info!("screensaver uninhibited (cookie={})", cookie);
}

// ── VideoState ────────────────────────────────────────────────────────────────
pub(crate) struct VideoState {
    pub player:     Option<Player>,
    pub render_ctx: Option<MpvRenderCtx>,
    pub fbos:       [u32; 2],
    pub textures:   [u32; 2],
    pub fbo_w:      u32,
    pub fbo_h:      u32,
    pub back:       usize,
    pub item_id:          Option<String>,
    pub playing_series_id: Option<String>,
    pub client:           Option<Arc<JellyfinClient>>,
    pub play_start:     Option<Instant>,
    pub decoder_logged:     bool,
    pub tracks_loaded:      bool,
    pub pos_tick:           u32,
    pub controls_idle_ticks: u32,
    pub intro_timestamps:    Option<IntroTimestamps>,
    pub intro_skip_shown:    bool,
    pub did_render:          bool,
    pub user_stopped:        bool,
    pub screensaver_cookie:  Option<u32>,
}

impl Default for VideoState {
    fn default() -> Self {
        Self {
            player: None, render_ctx: None,
            fbos: [0; 2], textures: [0; 2],
            fbo_w: 0, fbo_h: 0, back: 0,
            item_id: None, playing_series_id: None, client: None,
            play_start: None, decoder_logged: false,
            tracks_loaded: false, pos_tick: 0,
            controls_idle_ticks: 0,
            intro_timestamps: None, intro_skip_shown: false,
            did_render: false, user_stopped: false, screensaver_cookie: None,
        }
    }
}

// ── fmt_secs ──────────────────────────────────────────────────────────────────
pub(crate) fn fmt_secs(secs: f64) -> SharedString {
    if secs <= 0.0 { return "0:00".into(); }
    let s = secs as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let s = s % 60;
    if h > 0 {
        SharedString::from(format!("{}:{:02}:{:02}", h, m, s).as_str())
    } else {
        SharedString::from(format!("{}:{:02}", m, s).as_str())
    }
}

// ── build_track_model ─────────────────────────────────────────────────────────
pub(crate) fn build_track_model(tracks: &[TrackInfo], kind: &str) -> ModelRc<TrackEntry> {
    let entries: Vec<TrackEntry> = tracks.iter()
        .filter(|t| t.track_type == kind)
        .map(|t| {
            let mut label = String::new();
            if !t.lang.is_empty()  { label.push_str(&t.lang); }
            if !t.title.is_empty() {
                if !label.is_empty() { label.push(' '); }
                label.push_str(&t.title);
            }
            if !t.codec.is_empty() {
                label.push_str(&format!(" ({})", t.codec));
            }
            if label.is_empty() { label = format!("Track {}", t.id); }
            TrackEntry { id: t.id as i32, label: label.into() }
        })
        .collect();
    ModelRc::new(VecModel::from(entries))
}

pub(crate) unsafe fn create_fbo(w: u32, h: u32) -> Option<(u32, u32)> {
    let mut tex = 0u32;
    gl::GenTextures(1, &mut tex);
    gl::BindTexture(gl::TEXTURE_2D, tex);
    gl::TexImage2D(
        gl::TEXTURE_2D, 0, gl::RGBA as i32,
        w as i32, h as i32, 0,
        gl::RGBA, gl::UNSIGNED_BYTE, std::ptr::null(),
    );
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
    gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
    gl::BindTexture(gl::TEXTURE_2D, 0);

    let mut fbo = 0u32;
    gl::GenFramebuffers(1, &mut fbo);
    gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
    gl::FramebufferTexture2D(gl::FRAMEBUFFER, gl::COLOR_ATTACHMENT0, gl::TEXTURE_2D, tex, 0);
    let status = gl::CheckFramebufferStatus(gl::FRAMEBUFFER);
    gl::BindFramebuffer(gl::FRAMEBUFFER, 0);

    if status != gl::FRAMEBUFFER_COMPLETE {
        tracing::error!("FBO not complete: {:#x}", status);
        gl::DeleteFramebuffers(1, &fbo);
        gl::DeleteTextures(1, &tex);
        return None;
    }
    Some((fbo, tex))
}

pub(crate) unsafe fn delete_fbo(fbo: u32, tex: u32) {
    if fbo != 0 { gl::DeleteFramebuffers(1, &fbo); }
    if tex != 0 { gl::DeleteTextures(1, &tex); }
}

// ── tear_down_player ──────────────────────────────────────────────────────────
// Capture the final playback position then drop render_ctx before player
// (mpv invariant: MpvRenderCtx must be freed before mpv_terminate_destroy).
// Returns (item_id, client, screensaver_cookie, final_ticks) so the caller
// can send the stop report and release the screensaver inhibitor.
// Call this every time playback ends — normal finish, user stop, or replacement.
pub(crate) fn tear_down_player(vs: &mut VideoState)
    -> (Option<String>, Option<Arc<JellyfinClient>>, Option<u32>, i64)
{
    let ticks = vs.player.as_ref()
        .map(|p| (p.get_position() * 10_000_000.0) as i64)
        .unwrap_or(0);
    vs.render_ctx = None;
    vs.player     = None;
    (vs.item_id.take(), vs.client.take(), vs.screensaver_cookie.take(), ticks)
}

// ── quit_cleanup ──────────────────────────────────────────────────────────────
// Called from main() after window.run() returns (i.e. the user quit).
// The 16 ms timer has stopped so tear_down_player will never run via the
// normal finished path. We do it here synchronously so the stop report
// reaches Jellyfin before the runtime drops and cancels in-flight tasks.
pub(crate) fn quit_cleanup(video: &Arc<Mutex<VideoState>>, rt: &tokio::runtime::Runtime) {
    let (item_id, client, ss_cookie, final_ticks) = tear_down_player(&mut video.lock().unwrap());
    if let Some(cookie) = ss_cookie { uninhibit_screensaver(cookie); }
    if let (Some(id), Some(cli)) = (item_id, client) {
        info!("quit: sending stop report for {} at {:.1}s", id, final_ticks as f64 / 10_000_000.0);
        rt.block_on(async move {
            let _ = cli.report_playback_stopped(&id, final_ticks).await;
        });
    }
}

// ── start_playback ────────────────────────────────────────────────────────────
pub(crate) fn start_playback(
    url:         String,
    item_id:     String,
    item_type:   &str,
    title:       String,
    config:      PlayerConfig,
    client:      Arc<JellyfinClient>,
    video:       &Arc<Mutex<VideoState>>,
    window_weak: &slint::Weak<MainWindow>,
    rt_handle:   &tokio::runtime::Handle,
) {
    info!("starting playback: {} — {}", item_id, url);

    {
        let client2  = Arc::clone(&client);
        let item_id2 = item_id.clone();
        rt_handle.spawn(async move {
            let _ = client2.report_playback_start(&item_id2).await;
        });
    }

    if item_type == "Episode" {
        let client_ts  = Arc::clone(&client);
        let video_ts   = Arc::clone(video);
        let item_id_ts = item_id.clone();
        rt_handle.spawn(async move {
            match client_ts.get_intro_timestamps(&item_id_ts).await {
                Ok(Some(ts)) => {
                    debug!(
                        "intro timestamps: show_at={:.1}s hide_at={:.1}s end={:.1}s",
                        ts.show_skip_prompt_at, ts.hide_skip_prompt_at, ts.intro_end
                    );
                    video_ts.lock().unwrap().intro_timestamps = Some(ts);
                }
                Ok(None) => debug!("no intro timestamps for {}", item_id_ts),
                Err(e)   => debug!("intro timestamps unavailable: {:#}", e),
            }
        });
    }

    let (prev_item_id, prev_client, prev_cookie, prev_ticks) = {
        tear_down_player(&mut video.lock().unwrap())
    };
    if let Some(cookie) = prev_cookie { uninhibit_screensaver(cookie); }
    if let (Some(id), Some(cli)) = (prev_item_id, prev_client) {
        rt_handle.spawn(async move {
            let _ = cli.report_playback_stopped(&id, prev_ticks).await;
        });
    }

    match Player::new(&url, &config) {
        Ok(player) => {
            {
                let mut vs             = video.lock().unwrap();
                vs.player              = Some(player);
                vs.item_id             = Some(item_id);
                vs.client              = Some(client);
                vs.play_start          = Some(Instant::now());
                vs.decoder_logged      = false;
                vs.tracks_loaded       = false;
                vs.pos_tick            = 0;
                vs.controls_idle_ticks = 0;
                vs.intro_timestamps    = None;
                vs.intro_skip_shown    = false;
                vs.screensaver_cookie  = inhibit_screensaver();
            }
            if let Some(w) = window_weak.upgrade() {
                let g = AppState::get(&w);
                g.set_playing_title(ss(&title));
                g.set_is_playing(true);
                g.set_has_background_player(false);
                g.set_video_behind_ui(false);
                g.set_is_paused(false);
            }
        }
        Err(e) => error!("player init failed: {:#}", e),
    }
}

// ── wire_rendering_notifier ───────────────────────────────────────────────────
pub(crate) fn wire_rendering_notifier(
    window: &MainWindow,
    video:  Arc<Mutex<VideoState>>,
) {
    let video_rn  = video;
    let window_rn = window.as_weak();

    window.window().set_rendering_notifier({
        let mut gl_loaded  = false;
        let mut last_stats = Instant::now();

        move |state_rn, api| {
            match state_rn {
                slint::RenderingState::RenderingSetup => {
                    if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api {
                        if !gl_loaded {
                            gl::load_with(|name| {
                                let cname = std::ffi::CString::new(name).unwrap();
                                get_proc_address(cname.as_c_str())
                            });
                            gl_loaded = true;
                            info!("OpenGL loaded");
                        }
                    }
                }

                slint::RenderingState::BeforeRendering => {
                    let Some(win) = window_rn.upgrade() else { return; };
                    let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api else { return; };

                    let mut vs = video_rn.lock().unwrap();
                    vs.did_render = false;

                    if vs.fbos[0] != 0 && vs.player.is_none() {
                        unsafe {
                            delete_fbo(vs.fbos[0], vs.textures[0]);
                            delete_fbo(vs.fbos[1], vs.textures[1]);
                        }
                        vs.fbos = [0; 2]; vs.textures = [0; 2];
                        vs.fbo_w = 0; vs.fbo_h = 0;
                        return;
                    }

                    if vs.player.is_none() { return; }

                    if vs.render_ctx.is_none() {
                        let handle = vs.player.as_ref().unwrap().raw_handle_ptr();
                        match unsafe { MpvRenderCtx::new(handle, get_proc_address) } {
                            Ok(mut ctx) => {
                                let ww = window_rn.clone();
                                ctx.set_update_callback(move || {
                                    let ww2 = ww.clone();
                                    let _ = slint::invoke_from_event_loop(move || {
                                        if let Some(w) = ww2.upgrade() {
                                            w.window().request_redraw();
                                        }
                                    });
                                });
                                vs.render_ctx = Some(ctx);
                                info!("mpv render context created");
                            }
                            Err(e) => { error!("MpvRenderCtx::new: {:#}", e); return; }
                        }
                    }

                    let phys = win.window().size();
                    let w = phys.width.max(1);
                    let h = phys.height.max(1);

                    if vs.fbos[0] == 0 || vs.fbo_w != w || vs.fbo_h != h {
                        unsafe {
                            delete_fbo(vs.fbos[0], vs.textures[0]);
                            delete_fbo(vs.fbos[1], vs.textures[1]);
                        }
                        let r0 = unsafe { create_fbo(w, h) };
                        let r1 = unsafe { create_fbo(w, h) };
                        match (r0, r1) {
                            (Some((f0, t0)), Some((f1, t1))) => {
                                vs.fbos = [f0, f1]; vs.textures = [t0, t1];
                                vs.fbo_w = w; vs.fbo_h = h; vs.back = 0;
                            }
                            (p0, p1) => {
                                if let Some((f, t)) = p0 { unsafe { delete_fbo(f, t); } }
                                if let Some((f, t)) = p1 { unsafe { delete_fbo(f, t); } }
                                vs.fbos = [0; 2]; vs.textures = [0; 2];
                                return;
                            }
                        }
                    }

                    if let Some(ctx) = vs.render_ctx.as_ref() {
                        let b = vs.back;
                        if let Err(e) = ctx.render(vs.fbos[b] as i32, w as i32, h as i32, true) {
                            warn!("mpv render: {:#}", e);
                        } else {
                            vs.did_render = true;
                        }

                        if let Some(tex_id) = NonZeroU32::new(vs.textures[b]) {
                            let size = euclid::default::Size2D::new(w, h);
                            let img = unsafe {
                                slint::BorrowedOpenGLTextureBuilder::new_gl_2d_rgba_texture(tex_id, size)
                                    .origin(slint::BorrowedOpenGLTextureOrigin::BottomLeft)
                                    .build()
                            };
                            AppState::get(&win).set_video_frame(img);
                        }

                        vs.back = 1 - b;
                    }

                    if last_stats.elapsed() >= Duration::from_millis(500) {
                        if let Some(player) = vs.player.as_ref() {
                            let stats = player.poll_stats();
                            if let Some(w) = window_rn.upgrade() {
                                update_stats_window(&w, &stats);
                            }
                        }
                        last_stats = Instant::now();
                    }
                }

                slint::RenderingState::AfterRendering => {
                    let vs = video_rn.lock().unwrap();
                    if vs.did_render {
                        if let Some(ctx) = vs.render_ctx.as_ref() {
                            ctx.report_swap();
                        }
                    }
                }

                slint::RenderingState::RenderingTeardown => {
                    let mut vs = video_rn.lock().unwrap();
                    vs.render_ctx = None;
                    unsafe {
                        delete_fbo(vs.fbos[0], vs.textures[0]);
                        delete_fbo(vs.fbos[1], vs.textures[1]);
                    }
                    vs.fbos = [0; 2]; vs.textures = [0; 2];
                }

                _ => {}
            }
        }
    }).ok();
}

// ── wire_mpv_timer ────────────────────────────────────────────────────────────
pub(crate) fn wire_mpv_timer(
    window_weak:   slint::Weak<MainWindow>,
    video:         Arc<Mutex<VideoState>>,
    state:         Arc<Mutex<FjordState>>,
    rt_handle:     tokio::runtime::Handle,
    controls_show: Arc<AtomicBool>,
) -> slint::Timer {
    let video_timer  = video;
    let window_timer = window_weak;
    let state_timer  = state;

    let timer = slint::Timer::default();
    timer.start(slint::TimerMode::Repeated, Duration::from_millis(16), move || {
        let finished = {
            let mut vs = video_timer.lock().unwrap();

            if vs.player.is_some() {
                let elapsed_ok = vs.play_start.map_or(false, |t| t.elapsed() >= Duration::from_secs(2));

                if elapsed_ok && !vs.decoder_logged {
                    if let Some(p) = vs.player.as_ref() {
                        p.log_decoder_info();
                        p.apply_auto_vf();
                    }
                    vs.decoder_logged = true;
                }
                if elapsed_ok && !vs.tracks_loaded {
                    if let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
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
                            let cur_sub   = tracks.iter().find(|t| t.track_type == "sub"   && t.selected).map(|t| t.id).unwrap_or(0);
                            let cur_audio = tracks.iter().find(|t| t.track_type == "audio" && t.selected).map(|t| t.id).unwrap_or(1);
                            let cur_video = tracks.iter().find(|t| t.track_type == "video" && t.selected).map(|t| t.id).unwrap_or(1);
                            debug!("active tracks: sub={} audio={} video={}", cur_sub, cur_audio, cur_video);
                            let g = AppState::get(&w);
                            g.set_sub_tracks(sub_model);
                            g.set_audio_tracks(audio_model);
                            g.set_video_tracks(video_model);
                            g.set_current_sub_id(cur_sub as i32);
                            g.set_current_audio_id(cur_audio as i32);
                            g.set_current_video_id(cur_video as i32);
                            vs.tracks_loaded = true;
                        }
                    }
                }

                vs.pos_tick = vs.pos_tick.wrapping_add(1);
                if vs.pos_tick % 30 == 0 {
                    if let (Some(p), Some(w)) = (vs.player.as_ref(), window_timer.upgrade()) {
                        let pos = p.get_position();
                        let dur = p.get_duration();
                        let ratio = if dur > 0.0 { (pos / dur) as f32 } else { 0.0 };
                        let g = AppState::get(&w);
                        g.set_playback_pos(ratio);
                        g.set_playback_time(fmt_secs(pos));
                        g.set_playback_total(fmt_secs(dur));

                        // Report progress to Jellyfin every ~10 s.
                        if vs.pos_tick % 600 == 0 {
                            if let (Some(cli), Some(id)) = (vs.client.as_ref().map(Arc::clone), vs.item_id.clone()) {
                                let ticks = (pos * 10_000_000.0) as i64;
                                rt_handle.spawn(async move {
                                    let _ = cli.report_playback_progress(&id, ticks).await;
                                });
                            }
                        }
                    }
                }

                if let Some(ref ts) = vs.intro_timestamps {
                    if let Some(p) = vs.player.as_ref() {
                        let pos = p.get_position();
                        let should_show = pos >= ts.show_skip_prompt_at
                            && pos < ts.hide_skip_prompt_at;
                        if should_show != vs.intro_skip_shown {
                            vs.intro_skip_shown = should_show;
                            if let Some(w) = window_timer.upgrade() {
                                AppState::get(&w).set_show_skip_intro(should_show);
                            }
                        }
                    }
                }

                if controls_show.swap(false, Ordering::Relaxed) {
                    vs.controls_idle_ticks = 0;
                } else {
                    vs.controls_idle_ticks = vs.controls_idle_ticks.saturating_add(1);
                }
                if vs.controls_idle_ticks == 187 {
                    if let Some(w) = window_timer.upgrade() {
                        let g = AppState::get(&w);
                        g.set_controls_visible(false);
                        // Force Slint to re-evaluate the cursor at the last-known position.
                        // Slint only calls set_cursor_visible() during mouse event processing;
                        // dispatching PointerMoved at the same coordinates triggers that path
                        // without changing mouse-x/y (so show-controls won't fire).
                        let cx = g.get_player_cursor_x();
                        let cy = g.get_player_cursor_y();
                        w.window().dispatch_event(WindowEvent::PointerMoved {
                            position: LogicalPosition::new(cx as f32, cy as f32),
                        });
                    }
                }
            }

            if let Some(player) = vs.player.as_mut() {
                matches!(player.poll(), PollResult::Finished)
            } else {
                false
            }
        };

        if finished {
            info!("playback finished — tearing down");
            let (item_id, playing_series_id, client, user_stopped, ss_cookie, final_ticks) = {
                let mut vs = video_timer.lock().unwrap();
                let playing_series_id = vs.playing_series_id.take();
                let user_stopped = vs.user_stopped;
                vs.user_stopped = false;
                let (item_id, client, ss_cookie, ticks) = tear_down_player(&mut vs);
                (item_id, playing_series_id, client, user_stopped, ss_cookie, ticks)
            };
            if let Some(cookie) = ss_cookie { uninhibit_screensaver(cookie); }

            if let Some(w) = window_timer.upgrade() {
                let g = AppState::get(&w);
                g.set_is_playing(false);
                g.set_has_background_player(false);
                g.set_video_behind_ui(false);
                g.set_is_paused(false);
                g.set_stats_visible(false);
                g.set_playback_pos(0.0);
                g.set_playback_time("0:00".into());
                g.set_playback_total("0:00".into());
                g.set_sub_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
                g.set_audio_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
                g.set_video_tracks(ModelRc::new(VecModel::<TrackEntry>::default()));
                g.set_player_open_panel(0);
                g.set_controls_visible(true);
                g.set_show_skip_intro(false);
            }

            let client_home = client.as_ref().map(Arc::clone);

            if let Some(id) = item_id.as_deref() {
                if let Some(cli) = client.as_ref().map(Arc::clone) {
                    let id2 = id.to_string();
                    rt_handle.spawn(async move {
                        let _ = cli.report_playback_stopped(&id2, final_ticks).await;
                    });
                }
            }

            if let Some(cli) = client_home {
                let ww_home = window_timer.clone();
                let rth_home = rt_handle.clone();
                rt_handle.spawn(async move {
                    let home_data = fetch_home_data(&cli).await;
                    let sections  = home_data_sections(&home_data);
                    let ww2       = ww_home.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        if let Some(w) = ww2.upgrade() {
                            push_home_data(&w, &home_data);
                        }
                    });
                    spawn_poster_loading(cli, sections, ww_home, rth_home);
                });
            }

            if !user_stopped {
            if let Some(series_id) = playing_series_id {
                if let Some(cli) = client {
                    let state_adv  = Arc::clone(&state_timer);
                    let video_adv  = Arc::clone(&video_timer);
                    let ww_adv     = window_timer.clone();
                    let rt_adv     = rt_handle.clone();
                    rt_handle.spawn(async move {
                        let Ok(Some(next)) = cli.get_next_up_for_series(&series_id).await else {
                            return;
                        };
                        info!("auto-advance: next episode is {}", next.id);

                        state_adv.lock().unwrap().next_ep_pending = Some(next.clone());

                        let title_str = next.display_name();
                        let ww1 = ww_adv.clone();
                        let t1   = SharedString::from(title_str.as_str());
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww1.upgrade() {
                                let g = AppState::get(&w);
                                g.set_next_ep_title(t1);
                                g.set_next_ep_secs(5);
                                g.set_show_next_ep_banner(true);
                            }
                        });

                        for remaining in (0i32..5).rev() {
                            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                            if state_adv.lock().unwrap().next_ep_pending.is_none() {
                                return;
                            }
                            let ww2 = ww_adv.clone();
                            let _ = slint::invoke_from_event_loop(move || {
                                if let Some(w) = ww2.upgrade() {
                                    AppState::get(&w).set_next_ep_secs(remaining);
                                }
                            });
                        }

                        let next = state_adv.lock().unwrap().next_ep_pending.take();
                        let Some(next) = next else { return; };

                        let config = state_adv.lock().unwrap().player_config();
                        let cli2   = state_adv.lock().unwrap().client.as_ref().map(Arc::clone);
                        let Some(cli2) = cli2 else { return; };

                        let url   = cli2.direct_play_url(&next.id);
                        let title = next.display_name();
                        let id    = next.id.clone();
                        info!("auto-advancing to: {}", id);

                        let series_id2 = next.series_id.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            if let Some(w) = ww_adv.upgrade() {
                                AppState::get(&w).set_show_next_ep_banner(false);
                            }
                            start_playback(url, id, "Episode", title, config, cli2,
                                           &video_adv, &ww_adv, &rt_adv);
                            video_adv.lock().unwrap().playing_series_id = series_id2;
                        });
                    });
                }
            }
            }
        }
    });
    timer
}
