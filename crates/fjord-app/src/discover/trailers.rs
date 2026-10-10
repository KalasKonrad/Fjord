// ── fjord-app · discover/trailers.rs ─────────────────────────────────────────
//   trailer_url_allowed         https YouTube only — every trailer URL passes it before yt-dlp/mpv
//                               (2026-10-09 security review; unit-tested)
//   trailer_candidates          MovieDetails/TvDetails.relatedVideos -> trailer URLs, best first
//   start_trailer_check         background yt-dlp check of those candidates → request-detail-
//                               trailer-state "checking"/"ok"/"none" + -trailer-url (2026-10-04)
//   mark_trailer_unplayable     a trailer that failed to play → remembered, re-check the rest
//                              (prefers Trailer, falls back to Teaser, else None)
//   wire_trailers          callbacks moved from main() (0.5.0 step 3): yt-dlp detection (once) + Watch Trailer
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// The videos to offer as "Watch Trailer", best first: every `Trailer`, then
/// every `Teaser` (a shorter preview, still trailer-like); a `Clip`/
/// `Featurette`/etc. isn't what "Watch Trailer" implies. Several, not one
/// (2026-10-04): TMDB keeps listing videos YouTube has since blocked or
/// removed, so start_trailer_check walks this list until one actually plays.
/// Capped at 4 to bound the check. `url` is already a fully-formed YouTube
/// watch-page link — see `Video`'s own doc comment in fjord-seerr for why
/// only `kind`/`url` are modeled at all.
/// A trailer URL Fjord will hand to yt-dlp or mpv: `https` on YouTube only
/// (2026-10-09 security review). The URL comes from the server: anything
/// starting with `-` would be read by yt-dlp as an option (`--exec=…` runs
/// a command), and other schemes would let mpv open local files or other
/// protocols.
pub(crate) fn trailer_url_allowed(url: &str) -> bool {
    let Ok(u) = url::Url::parse(url) else {
        return false;
    };
    u.scheme() == "https"
        && u.username().is_empty()
        && u.password().is_none()
        && matches!(
            u.host_str(),
            Some("www.youtube.com" | "youtube.com" | "m.youtube.com" | "youtu.be")
        )
}

pub(crate) fn trailer_candidates(videos: &[fjord_seerr::Video]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for kind in ["Trailer", "Teaser"] {
        for v in videos.iter().filter(|v| v.kind == kind) {
            if !trailer_url_allowed(&v.url) {
                debug!(
                    "trailer candidate skipped (not an https YouTube URL): {:?}",
                    v.url
                );
                continue;
            }
            if !out.contains(&v.url) {
                out.push(v.url.clone());
            }
        }
    }
    out.truncate(4);
    out
}

/// Decides what the RequestDetail Trailer button shows (2026-10-04, live-
/// reported: a trailer that won't play shouldn't look playable). TMDB lists
/// trailers YouTube has since blocked for this region or removed, and only
/// yt-dlp can tell — YouTube's public oEmbed answers 200 for the very video
/// yt-dlp reports "Video unavailable" (checked live). So: answer from the
/// session cache when possible, otherwise show greyed "Checking…" and run
/// `yt-dlp --simulate` (no download) per candidate, best first, until one
/// resolves. Sets request-detail-trailer-state "ok" (+ -trailer-url) or
/// "none". UI thread only. `generation` = request-detail-open-gen of the screen
/// this is for; a later open of another title discards the result.
pub(crate) fn start_trailer_check(
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    generation: i32,
    candidates: Vec<String>,
) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    let (known_ok, all_known_bad, ytdl_format) = {
        let mut s = state.lock().unwrap();
        s.request_detail_trailers = candidates.clone();
        let known_ok = candidates
            .iter()
            .find(|c| s.trailer_playable.get(*c) == Some(&true))
            .cloned();
        let all_known_bad = candidates
            .iter()
            .all(|c| s.trailer_playable.get(c) == Some(&false));
        (
            known_ok,
            all_known_bad,
            crate::trailer_ytdl_format(&s.config.active().trailer_quality),
        )
    };
    if let Some(url) = known_ok {
        debug!("trailer check: cached playable {url}");
        g.set_request_detail_trailer_url(url.as_str().into());
        g.set_request_detail_trailer_state("ok".into());
        return;
    }
    if all_known_bad || !g.get_yt_dlp_available() {
        debug!(
            "trailer check: {} candidate(s), none playable (yt-dlp available={})",
            candidates.len(),
            g.get_yt_dlp_available()
        );
        g.set_request_detail_trailer_url("".into());
        g.set_request_detail_trailer_state("none".into());
        fix_detail_btn_focus(&g);
        return;
    }
    g.set_request_detail_trailer_url("".into());
    g.set_request_detail_trailer_state("checking".into());
    fix_detail_btn_focus(&g);
    let state = Arc::clone(state);
    let ww = ww.clone();
    rt.spawn(async move {
        let mut found: Option<String> = None;
        for url in &candidates {
            match state.lock().unwrap().trailer_playable.get(url) {
                Some(true) => {
                    found = Some(url.clone());
                    break;
                }
                Some(false) => continue,
                None => {}
            }
            let ok = trailer_plays(url, ytdl_format.as_deref()).await;
            state
                .lock()
                .unwrap()
                .trailer_playable
                .insert(url.clone(), ok);
            if ok {
                found = Some(url.clone());
                break;
            }
        }
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_request_detail_open_gen() != generation || !g.get_show_request_detail() {
                return; // another title (or none) is showing by now
            }
            match found {
                Some(url) => {
                    g.set_request_detail_trailer_url(url.as_str().into());
                    g.set_request_detail_trailer_state("ok".into());
                }
                None => {
                    g.set_request_detail_trailer_url("".into());
                    g.set_request_detail_trailer_state("none".into());
                    fix_detail_btn_focus(&g);
                }
            }
        });
    });
}

/// `yt-dlp --simulate`: resolves the video and the exact format mpv would
/// ask for, without downloading. 20 s cap; the process is killed on timeout.
/// It can't catch a 403 that only happens once the download starts — those
/// land in mark_trailer_unplayable after a failed play.
async fn trailer_plays(url: &str, ytdl_format: Option<&str>) -> bool {
    if !trailer_url_allowed(url) {
        return false;
    }
    let mut cmd = tokio::process::Command::new("yt-dlp");
    cmd.args(["--simulate", "--quiet", "--no-warnings", "--no-playlist"]);
    if let Some(f) = ytdl_format {
        cmd.args(["-f", f]);
    }
    // `--`: the URL can never be read as an option, whatever it contains.
    cmd.arg("--")
        .arg(url)
        .kill_on_drop(true)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped());
    let started = std::time::Instant::now();
    match tokio::time::timeout(std::time::Duration::from_secs(20), cmd.output()).await {
        Ok(Ok(out)) if out.status.success() => {
            debug!(
                "trailer check: {url} plays ({:.1}s)",
                started.elapsed().as_secs_f64()
            );
            true
        }
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            info!(
                "trailer check: {url} won't play: {}",
                err.trim().lines().last().unwrap_or("")
            );
            false
        }
        Ok(Err(e)) => {
            warn!("trailer check: couldn't run yt-dlp: {e}");
            false
        }
        Err(_) => {
            info!("trailer check: {url} timed out after 20s");
            false
        }
    }
}

/// A trailer failed to play even though the check passed (e.g. YouTube
/// answered 403 once the download started — HTPC log, 2026-10-04). Remember
/// it for the session and, if its detail screen is still open, re-check the
/// remaining candidates (greyed "Checking…", then the next playable one or
/// "No trailer"). UI thread only.
pub(crate) fn mark_trailer_unplayable(
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    url: String,
) {
    let candidates = {
        let mut s = state.lock().unwrap();
        s.trailer_playable.insert(url.clone(), false);
        s.request_detail_trailers.clone()
    };
    info!("trailer {url} failed to play — marked unplayable for this session");
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    if g.get_show_request_detail() && candidates.contains(&url) {
        start_trailer_check(state, ww, rt, g.get_request_detail_open_gen(), candidates);
    }
}

// ── wire_trailers (moved from main(), 0.5.0 step 3) ──────────────────────
/// Wires yt-dlp detection (once) + Watch Trailer: play_trailer.
pub(crate) fn wire_trailers(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    video: &std::sync::Arc<std::sync::Mutex<crate::playback::VideoState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    let video = std::sync::Arc::clone(video);
    // ── yt-dlp detection: fetch once at startup ────────────────────────────────
    // Gates the Watch Trailer button's visibility (request-detail-trailer-url
    // alone isn't enough to guarantee playback will actually work — see
    // CLAUDE.md's Seerr integration section). A pure local-machine fact, not
    // tied to Seerr connection state like Streaming Region/Trailer Quality
    // above, so no gating on seerr_enabled/seerr_connected here.
    {
        let state_yt = Arc::clone(&state);
        let ww_yt = window.as_weak();
        rt.spawn(async move {
            let available = tokio::task::spawn_blocking(detect_yt_dlp)
                .await
                .unwrap_or(false);
            state_yt.lock().unwrap().yt_dlp_available = available;
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww_yt.upgrade() {
                    AppState::get(&w).set_yt_dlp_available(available);
                }
            });
        });
    }

    // ── Watch Trailer (Discover / RequestDetailScreen only) ───────────────────
    // Registered here, not inside discover::wire_discover — that function
    // never receives `video` (VideoState), and every other module that needs
    // to start playback gets it the same way: `video` created once in main()
    // and cloned locally right before the specific callback that needs it,
    // not threaded as a parameter into other modules' wire_X functions.
    {
        let state_pt = Arc::clone(&state);
        let video_pt = Arc::clone(&video);
        let ww_pt = window.as_weak();
        let rt_pt = rt.handle().clone();
        AppState::get(&window).on_play_trailer(move || {
            let Some(w) = ww_pt.upgrade() else { return };
            let g = AppState::get(&w);
            let url = g.get_request_detail_trailer_url().to_string();
            if url.is_empty() {
                return;
            }
            let title = format!("Trailer — {}", g.get_request_detail_title());
            let (mut config, quality) = {
                let s = state_pt.lock().unwrap();
                (s.player_config(), s.config.active().trailer_quality.clone())
            };
            config.ytdl_format = trailer_ytdl_format(&quality);
            config.start_position_secs = None; // no resume concept for a trailer
            playback::play_trailer(url, title, config, &video_pt, &ww_pt, &rt_pt);
        });
    }
}

#[cfg(test)]
mod tests {
    use super::trailer_url_allowed;

    #[test]
    fn only_https_youtube_trailers() {
        for ok in [
            "https://www.youtube.com/watch?v=GSycMV-_Csw",
            "https://youtube.com/watch?v=GSycMV-_Csw",
            "https://m.youtube.com/watch?v=GSycMV-_Csw",
            "https://youtu.be/GSycMV-_Csw",
        ] {
            assert!(trailer_url_allowed(ok), "{ok}");
        }
        for bad in [
            "--exec=touch /tmp/x",
            "-o /tmp/x",
            "",
            "http://www.youtube.com/watch?v=x", // not https
            "file:///etc/passwd",
            "ytdl://x",
            "av://lavfi:sine",
            "https://evil.example/watch?v=x",
            "https://www.youtube.com.evil.example/watch?v=x",
            "https://user:pw@www.youtube.com/watch?v=x",
            "https://evil.example@www.youtube.com/watch?v=x",
        ] {
            assert!(!trailer_url_allowed(bad), "{bad}");
        }
    }
}
