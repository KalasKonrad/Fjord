// ── fjord-app · playback/render.rs ────────────────────────────────────────
//   create_fbo / delete_fbo  the two alternating video FBOs (wide = GL_RGB10_A2 for HDR passthrough)
//   wire_rendering_notifier GL thread: FBO render + report_swap() for vsync feedback (no stats — moved to timer)
//                           HDR Stage 5 (2026-10-05): decides each player's path once, at render-ctx
//                           creation — video on the subsurface (video_surface.rs) when Settings →
//                           "Separate video surface" is on and it can be set up, else the FBO path;
//                           subsurface players render via video_surface::render_frame into the spot
//                           Slint shows (VideoSpot rects → to_buffer_rect; skips a frame while a new
//                           spot is unmeasured) and set AppState.video-surface-active; sets is-wayland
//                           once. 2026-10-08: passes Settings → "Use Fjord's own 10-bit buffers" (opt-in)
//                           to ensure_ready; the path line names the plane's mode + mpv depth; mpv gets
//                           the Target's flip/format/depth (in-window path: depth 0 = 8); a frame the
//                           own buffers skipped (none free) requests another redraw
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// A Slint colour as GL clear-colour floats (alpha ignored).
fn color_rgb(c: slint::Color) -> [f32; 3] {
    [
        c.red() as f32 / 255.0,
        c.green() as f32 / 255.0,
        c.blue() as f32 / 255.0,
    ]
}

/// One video FBO (with its texture). `wide` = GL_RGB10_A2 (10 bits per channel in
/// the same 32 bits per pixel as RGBA8, HDR10's precision) — decided per item from
/// VideoState.wide_color_fbo (the HDR passthrough setting; the source isn't known yet).
/// Slint's BorrowedOpenGLTexture works with either: it needs an RGBA *format*, not a
/// particular internal format. The caller's GL context must be current.
pub(crate) unsafe fn create_fbo(w: u32, h: u32, wide: bool) -> Option<(u32, u32)> {
    // SAFETY: the caller's GL context is current (this fn's contract).
    unsafe {
        let mut tex = 0u32;
        gl::GenTextures(1, &mut tex);
        gl::BindTexture(gl::TEXTURE_2D, tex);
        if wide {
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGB10_A2 as i32,
                w as i32,
                h as i32,
                0,
                gl::RGBA,
                gl::UNSIGNED_INT_2_10_10_10_REV,
                std::ptr::null(),
            );
        } else {
            gl::TexImage2D(
                gl::TEXTURE_2D,
                0,
                gl::RGBA as i32,
                w as i32,
                h as i32,
                0,
                gl::RGBA,
                gl::UNSIGNED_BYTE,
                std::ptr::null(),
            );
        }
        gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MIN_FILTER, gl::LINEAR as i32);
        gl::TexParameteri(gl::TEXTURE_2D, gl::TEXTURE_MAG_FILTER, gl::LINEAR as i32);
        gl::BindTexture(gl::TEXTURE_2D, 0);

        let mut fbo = 0u32;
        gl::GenFramebuffers(1, &mut fbo);
        gl::BindFramebuffer(gl::FRAMEBUFFER, fbo);
        gl::FramebufferTexture2D(
            gl::FRAMEBUFFER,
            gl::COLOR_ATTACHMENT0,
            gl::TEXTURE_2D,
            tex,
            0,
        );
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
}

pub(crate) unsafe fn delete_fbo(fbo: u32, tex: u32) {
    // SAFETY: the caller's GL context is current (this fn's contract).
    unsafe {
        if fbo != 0 {
            gl::DeleteFramebuffers(1, &fbo);
        }
        if tex != 0 {
            gl::DeleteTextures(1, &tex);
        }
    }
}

// ── wire_rendering_notifier ───────────────────────────────────────────────────
pub(crate) fn wire_rendering_notifier(window: &MainWindow, video: Arc<Mutex<VideoState>>) {
    let video_rn = video;
    let window_rn = window.as_weak();

    window.window().set_rendering_notifier({
        let mut gl_loaded = false;
        let mut wayland_flag_set = false;

        move |state_rn, api| {
            match state_rn {
                slint::RenderingState::RenderingSetup => {
                    if let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api && !gl_loaded {
                        gl::load_with(|name| {
                            let cname = std::ffi::CString::new(name).unwrap();
                            get_proc_address(cname.as_c_str())
                        });
                        gl_loaded = true;
                        info!("OpenGL loaded");
                    }
                }

                slint::RenderingState::BeforeRendering => {
                    let Some(win) = window_rn.upgrade() else { return; };
                    let slint::GraphicsAPI::NativeOpenGL { get_proc_address } = api else { return; };

                    let mut vs = video_rn.lock().unwrap();
                    vs.did_render = false;
                    let g = AppState::get(&win);

                    // Settings → "Separate video surface" is only offered on
                    // Wayland; activity.rs learns that on the first window
                    // event, before the first frame.
                    if !wayland_flag_set && crate::video_surface::is_wayland() {
                        wayland_flag_set = true;
                        g.set_is_wayland(true);
                    }

                    // HDR Stage 5: no player → the window is opaque again
                    // (reset_playback_ui normally did that already, one frame
                    // earlier — this is the safety net) and the subsurface
                    // gets a plain fill once, so no stale frame can resurface.
                    if vs.player.is_none() {
                        if g.get_video_surface_active() {
                            g.set_video_surface_active(false);
                        }
                        crate::video_surface::idle_fill(color_rgb(g.get_window_bg()));
                    }

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
                        // HDR Stage 5 (2026-10-05): the path is decided here,
                        // once per player — mpv can't move render contexts
                        // mid-file. Video on the subsurface when Settings →
                        // "Separate video surface" is on and the subsurface
                        // can be set up; otherwise (audio, toggle off, X11,
                        // setup failure) the in-window FBO path below.
                        let phys = win.window().size();
                        let want_surface = !vs.current_is_audio && g.get_settings_separate_video_surface();
                        let surface_ctx = if want_surface
                            && crate::video_surface::ensure_ready(
                                (phys.width, phys.height),
                                win.window().scale_factor(),
                                g.get_settings_video_own_buffers(),
                            )
                        {
                            match crate::video_surface::create_render_ctx(vs.player.as_ref().unwrap()) {
                                Ok(ctx) => Some(ctx),
                                Err(e) => {
                                    warn!("mpv render context on the video subsurface failed — in-window path: {e:#}");
                                    crate::video_surface::mark_broken("render context creation failed");
                                    None
                                }
                            }
                        } else {
                            None
                        };
                        vs.video_on_subsurface = surface_ctx.is_some();
                        info!(
                            "video path for this player: {}",
                            if vs.video_on_subsurface {
                                format!("separate video surface ({})", crate::video_surface::present_summary())
                            }
                            else if vs.current_is_audio { "in-window (audio)".into() }
                            else if !g.get_settings_separate_video_surface() { "in-window (Settings: separate video surface off)".into() }
                            else { "in-window (separate video surface unavailable)".into() }
                        );
                        let created = match surface_ctx {
                            Some(ctx) => Ok(ctx),
                            None => {
                                let handle = vs.player.as_ref().unwrap().raw_handle_ptr();
                                unsafe { MpvRenderCtx::new(handle, get_proc_address) }
                            }
                        };
                        match created {
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

                    // Fire the deferred loadfile on any tick where a URL is pending and the render
                    // context exists (same GL thread) — not only right after the context is created:
                    // display sync's pre-decode wait sets pending_load_url later. render_ctx always exists
                    // first (never torn down once created), which is the VO-init race rule.
                    if vs.render_ctx.is_some()
                        && let Some(url) = vs.pending_load_url.take()
                        && let Some(p) = vs.player.as_ref()
                        && let Err(e) = p.load(&url) {
                        error!("Player::load: {:#}", e);
                    }

                    let phys = win.window().size();
                    let w = phys.width.max(1);
                    let h = phys.height.max(1);

                    // HDR Stage 5: this player's video goes to the subsurface —
                    // into whichever spot Slint is showing (fullscreen player,
                    // video-behind-menus layer, mini-player thumbnail), or the
                    // whole plane (window kept opaque) when none is.
                    if vs.video_on_subsurface {
                        let scale = win.window().scale_factor();
                        let spot = crate::video_surface::pick_spot(
                            g.get_is_playing(), g.get_video_behind_ui(), g.get_has_background_player(),
                        );
                        let rect = spot.and_then(|spot| {
                            let r = match spot {
                                crate::video_surface::Spot::Player     => g.get_video_rect_player(),
                                crate::video_surface::Spot::Background => g.get_video_rect_bg(),
                                crate::video_surface::Spot::Thumb      => g.get_video_rect_thumb(),
                            };
                            crate::video_surface::to_buffer_rect((r.x, r.y, r.w, r.h), scale, (w as i32, h as i32))
                        });
                        if spot != vs.video_spot_logged {
                            let drops = vs.player.as_ref().map(|p| p.get_drop_counts()).unwrap_or((0, 0));
                            debug!("video subsurface: spot {:?} at {:?} (frame-drops so far {}, decoder {})", spot, rect, drops.0, drops.1);
                            vs.video_spot_logged = spot;
                        }
                        // A spot that has just appeared (or a window being
                        // resized) isn't measured for a frame: draw nothing
                        // new and leave the window as it is, rather than
                        // flashing it opaque (seen in the first live run).
                        // Ten frames in a row → give up waiting below.
                        if spot.is_some() && rect.is_none() && vs.video_spot_waits < 10 {
                            vs.video_spot_waits += 1;
                            return;
                        }
                        vs.video_spot_waits = 0;
                        // While the subsurface is tagged PQ/BT.2020 (HDR
                        // active), its fill has to be PQ-encoded to look the
                        // same as the sRGB UI around it.
                        let srgb = color_rgb(g.get_window_bg());
                        let fill = if crate::hdr::is_active() {
                            crate::video_surface::pq_fill_from_srgb(srgb)
                        } else {
                            srgb
                        };
                        let ctx = vs.render_ctx.as_ref().unwrap();
                        let result = crate::video_surface::render_frame(
                            (w, h), scale, rect.unwrap_or([0, 0, w as i32, h as i32]), fill,
                            |t| match ctx.render(t.fbo, t.w, t.h, t.flip_y, t.format, t.depth) {
                                Ok(()) => true,
                                Err(e) => { warn!("mpv render: {:#}", e); false }
                            },
                        );
                        use crate::video_surface::FrameOutcome;
                        match result {
                            Ok(FrameOutcome::Drawn) => {
                                vs.did_render = true;
                                if !vs.first_frame_logged && vs.play_start.is_some() {
                                    vs.first_frame_logged = true;
                                    let elapsed = vs.play_start.unwrap().elapsed().as_secs_f64();
                                    info!("first frame rendered {:.3}s after player start (video subsurface)", elapsed);
                                }
                                // Transparent from the next frame on (femtovg
                                // already cleared this one) — the subsurface
                                // frame committed now lands with that frame.
                                let active = rect.is_some();
                                if g.get_video_surface_active() != active {
                                    debug!("video subsurface: window {}", if active { "transparent over the video" } else { "opaque (no video spot showing)" });
                                    g.set_video_surface_active(active);
                                    if active { g.set_video_frame(slint::Image::default()); }
                                }
                            }
                            // Own buffers all held by KWin: this frame is
                            // skipped (mpv not called). Ask for another
                            // redraw — mpv's update callback fired for this
                            // frame already and won't again until it's drawn.
                            Ok(FrameOutcome::Skipped) => win.window().request_redraw(),
                            Ok(FrameOutcome::MpvFailed | FrameOutcome::Unusable) => {
                                if g.get_video_surface_active() { g.set_video_surface_active(false); }
                            }
                            Err(e) => {
                                crate::video_surface::mark_broken(&format!("{e:#}"));
                                g.set_video_surface_active(false);
                            }
                        }
                        return;
                    }
                    if g.get_video_surface_active() {
                        g.set_video_surface_active(false);
                    }

                    if vs.fbos[0] == 0 || vs.fbo_w != w || vs.fbo_h != h {
                        unsafe {
                            delete_fbo(vs.fbos[0], vs.textures[0]);
                            delete_fbo(vs.fbos[1], vs.textures[1]);
                        }
                        let r0 = unsafe { create_fbo(w, h, vs.wide_color_fbo) };
                        let r1 = unsafe { create_fbo(w, h, vs.wide_color_fbo) };
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
                        // hdr branch, Stage 4: 0 (today's exact value) unless
                        // this item's FBO was actually widened — see
                        // create_fbo's own doc comment.
                        let internal_format = if vs.wide_color_fbo { gl::RGB10_A2 as i32 } else { 0 };
                        // Depth 0 (mpv's 8): whatever the FBO, it ends up in
                        // Slint's 8-bit window.
                        if let Err(e) = ctx.render(vs.fbos[b] as i32, w as i32, h as i32, true, internal_format, 0) {
                            warn!("mpv render: {:#}", e);
                        } else {
                            vs.did_render = true;
                            // Only once play_start is set: mpv renders idle frames during the display-sync
                            // wait, which mustn't count as the first frame.
                            if !vs.first_frame_logged && vs.play_start.is_some() {
                                vs.first_frame_logged = true;
                                let elapsed = vs.play_start.unwrap().elapsed().as_secs_f64();
                                info!("first frame rendered {:.3}s after player start", elapsed);
                            }
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
                }

                slint::RenderingState::AfterRendering => {
                    let vs = video_rn.lock().unwrap();
                    if vs.did_render && let Some(ctx) = vs.render_ctx.as_ref() {
                        ctx.report_swap();
                    }
                }

                slint::RenderingState::RenderingTeardown => {
                    let mut vs = video_rn.lock().unwrap();
                    if vs.video_on_subsurface {
                        if let Some(ctx) = vs.render_ctx.take() {
                            crate::video_surface::free_render_ctx(ctx);
                        }
                        vs.video_on_subsurface = false;
                    }
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
