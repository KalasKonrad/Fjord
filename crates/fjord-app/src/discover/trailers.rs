// ── fjord-app · discover/trailers.rs ────────────────────────────────────────
//   trailer_url_allowed      https YouTube only — every trailer URL passes it before yt-dlp/mpv
//                            (unit-tested)
//   TrailerScreen            RequestDetail | Detail (movie page) | Series: each screen's trailer
//                            state/url pair, current key (open-gen / item id) and focus fix
//   trailer_candidates       relatedVideos → trailer URLs, best first (Trailer, Teaser; max 4)
//   library_trailer_candidates  Jellyfin RemoteTrailers → URLs, best first (Trailer, Teaser,
//                            unnamed, the rest; max 4; unit-tested)
//   start_library_trailer_check  movie/series page: a trailer file on the server first
//                            (get_local_trailers), else start_trailer_check on the YouTube links
//   start_trailer_check      yt-dlp --simulate over the candidates → the screen's trailer state
//                            "checking"/"ok"/"none" + url (session cache); stale results dropped
//   trailer_plays            one yt-dlp check (URL after `--`)
//   mark_trailer_unplayable  a trailer that failed to play → remembered; re-check the rest on every
//                            screen still showing a title that listed it
//   detect_yt_dlp / trailer_ytdl_format  is yt-dlp installed; the format string trailers use
//   wire_trailers            callbacks moved from main() (0.5.0 step 3): yt-dlp detection (once),
//                            Watch Trailer; play-detail-/play-series-trailer → play_library_trailer
//                            (the server file as a stream, or YouTube)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

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

/// The videos to offer as "Watch Trailer", best first: every `Trailer`, then every
/// `Teaser` (a `Clip`/`Featurette` isn't what "Watch Trailer" means). Several, not one:
/// TMDB keeps listing videos YouTube has since blocked or removed, so
/// start_trailer_check walks the list until one plays. Capped at 4; only URLs passing
/// `trailer_url_allowed`. `url` is a full YouTube watch link (see `Video` in fjord-seerr).
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

/// Which screen a trailer check belongs to. Each has its own AppState state/url pair and its
/// own "still showing the same title" key: request-detail-open-gen for RequestDetail, the item
/// id for the library screens (movie detail page, series page).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub(crate) enum TrailerScreen {
    RequestDetail,
    Detail,
    Series,
}

impl TrailerScreen {
    const ALL: [TrailerScreen; 3] = [Self::RequestDetail, Self::Detail, Self::Series];

    /// What this screen shows right now; None when it isn't showing.
    pub(crate) fn current_key(self, g: &AppState) -> Option<String> {
        match self {
            Self::RequestDetail => g
                .get_show_request_detail()
                .then(|| g.get_request_detail_open_gen().to_string()),
            Self::Detail => g.get_show_detail().then(|| g.get_detail_id().to_string()),
            Self::Series => g.get_show_series().then(|| g.get_series_id().to_string()),
        }
    }

    /// Sets the button: "checking" | "ok" (+ the YouTube url, "" for a server file) | "none".
    pub(crate) fn set(self, g: &AppState, state: &str, url: &str) {
        match self {
            Self::RequestDetail => {
                g.set_request_detail_trailer_url(url.into());
                g.set_request_detail_trailer_state(state.into());
            }
            Self::Detail => {
                g.set_detail_trailer_url(url.into());
                g.set_detail_trailer_state(state.into());
            }
            Self::Series => {
                g.set_series_trailer_url(url.into());
                g.set_series_trailer_state(state.into());
            }
        }
        if state != "ok" {
            // A greyed button isn't a D-pad stop.
            match self {
                Self::RequestDetail => fix_detail_btn_focus(g),
                Self::Detail => crate::detail::fix_trailer_focus(g),
                Self::Series => crate::series::fix_trailer_focus(g),
            }
        }
    }
}

/// RemoteTrailers (Jellyfin) → candidate URLs, best first: names with "Trailer", then
/// "Teaser", then unnamed (series entries have no names), then the rest (TV spots, promos —
/// most of the other names on the verified server). Stable within a group; only URLs passing
/// `trailer_url_allowed`; capped at 4 like Discover.
pub(crate) fn library_trailer_candidates(
    trailers: &[fjord_api::models::RemoteTrailer],
) -> Vec<String> {
    let rank = |t: &fjord_api::models::RemoteTrailer| {
        let name = t.name.as_deref().unwrap_or("").to_lowercase();
        if name.is_empty() {
            2
        } else if name.contains("teaser") {
            1
        } else if name.contains("trailer") {
            0
        } else {
            3
        }
    };
    let mut ranked: Vec<&fjord_api::models::RemoteTrailer> = trailers.iter().collect();
    ranked.sort_by_key(|t| rank(t));
    let mut out: Vec<String> = Vec::new();
    for t in ranked {
        if !trailer_url_allowed(&t.url) {
            debug!(
                "library trailer skipped (not an https YouTube URL): {:?}",
                t.url
            );
            continue;
        }
        if !out.contains(&t.url) {
            out.push(t.url.clone());
        }
        if out.len() == 4 {
            break;
        }
    }
    out
}

/// Trailer button for a library screen (movie detail page / series page), from its item: a
/// trailer file on the server first (LocalTrailerCount > 0 → get_local_trailers; plays as a
/// server stream, no yt-dlp needed), else the RemoteTrailers YouTube links through
/// start_trailer_check. UI thread only.
pub(crate) fn start_library_trailer_check(
    screen: TrailerScreen,
    item: &fjord_api::models::MediaItem,
    client: &Arc<fjord_api::JellyfinClient>,
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
) {
    let key = item.id.clone();
    let candidates = library_trailer_candidates(&item.remote_trailers);
    state.lock().unwrap().local_trailers.remove(&screen);
    debug!(
        "library trailer check ({screen:?}): {} remote candidate(s), {} local trailer(s)",
        candidates.len(),
        item.local_trailer_count
    );
    if item.local_trailer_count == 0 {
        start_trailer_check(state, ww, rt, screen, key, candidates);
        return;
    }
    let Some(w) = ww.upgrade() else { return };
    screen.set(&AppState::get(&w), "checking", "");
    let (client, state, ww, rt) = (
        Arc::clone(client),
        Arc::clone(state),
        ww.clone(),
        rt.clone(),
    );
    let rt2 = rt.clone();
    rt.spawn(async move {
        let local = match client.get_local_trailers(&key).await {
            // Only an id that's a safe cache name (32 hex) — it becomes part of a stream URL.
            Ok(items) => items
                .into_iter()
                .map(|i| i.id)
                .find(|id| crate::config::safe_cache_name(id).is_some()),
            Err(e) => {
                warn!("local trailers of {key}: {e:#}");
                None
            }
        };
        debug!("library trailer check ({screen:?}): local trailer {local:?}");
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if screen.current_key(&g).as_deref() != Some(key.as_str())
                || !crate::session_current(&state, &client)
            {
                return;
            }
            match local {
                Some(id) => {
                    state
                        .lock()
                        .unwrap()
                        .local_trailers
                        .insert(screen, (key, id));
                    screen.set(&g, "ok", "");
                }
                None => start_trailer_check(&state, &ww, &rt2, screen, key, candidates),
            }
        });
    });
}

/// Decides what a Trailer button shows: TMDB/Jellyfin list trailers YouTube has since blocked
/// or removed, and only yt-dlp can tell (YouTube's oEmbed answers 200 for videos yt-dlp reports
/// "Video unavailable"). From the session cache when possible; otherwise greyed "Checking…"
/// while `yt-dlp --simulate` (no download) tries the candidates best first until one resolves.
/// Ends "ok" (+ url) or "none". UI thread only. `key` = TrailerScreen::current_key of the title
/// this is for; a result for a title no longer showing is dropped.
pub(crate) fn start_trailer_check(
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    screen: TrailerScreen,
    key: String,
    candidates: Vec<String>,
) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    let (known_ok, all_known_bad, ytdl_format) = {
        let mut s = state.lock().unwrap();
        s.trailer_candidates
            .insert(screen, (key.clone(), candidates.clone()));
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
        debug!("trailer check ({screen:?}): cached playable {url}");
        screen.set(&g, "ok", &url);
        return;
    }
    if all_known_bad || !g.get_yt_dlp_available() {
        debug!(
            "trailer check ({screen:?}): {} candidate(s), none playable (yt-dlp available={})",
            candidates.len(),
            g.get_yt_dlp_available()
        );
        screen.set(&g, "none", "");
        return;
    }
    screen.set(&g, "checking", "");
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
            if screen.current_key(&g).as_deref() != Some(key.as_str()) {
                return; // another title (or none) is showing by now
            }
            match found {
                Some(url) => screen.set(&g, "ok", &url),
                None => screen.set(&g, "none", ""),
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
/// it for the session and, on every screen still showing a title that listed
/// it, re-check the remaining candidates (greyed "Checking…", then the next
/// playable one or "No trailer"). `url` "" = a trailer file from the server
/// (nothing to remember). UI thread only.
pub(crate) fn mark_trailer_unplayable(
    state: &Arc<Mutex<FjordState>>,
    ww: &Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    url: String,
) {
    if url.is_empty() {
        return;
    }
    let checks = {
        let mut s = state.lock().unwrap();
        s.trailer_playable.insert(url.clone(), false);
        s.trailer_candidates.clone()
    };
    info!("trailer {url} failed to play — marked unplayable for this session");
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    for screen in TrailerScreen::ALL {
        if let Some((key, candidates)) = checks.get(&screen)
            && candidates.contains(&url)
            && screen.current_key(&g).as_deref() == Some(key.as_str())
        {
            start_trailer_check(state, ww, rt, screen, key.clone(), candidates.clone());
        }
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
            playback::play_trailer(
                url,
                title,
                config,
                playback::TrailerSource::YouTube,
                &video_pt,
                &ww_pt,
                &rt_pt,
            );
        });
    }

    // ── Trailer on the movie detail page / series page ────────────────────────
    for screen in [TrailerScreen::Detail, TrailerScreen::Series] {
        let state = Arc::clone(&state);
        let video = Arc::clone(&video);
        let ww = window.as_weak();
        let rt = rt.handle().clone();
        let play = move || play_library_trailer(screen, &state, &video, &ww, &rt);
        match screen {
            TrailerScreen::Detail => AppState::get(&window).on_play_detail_trailer(play),
            _ => AppState::get(&window).on_play_series_trailer(play),
        }
    }
}

/// Plays the trailer a library screen's button found: the server's trailer file (a stream URL
/// Fjord builds for its validated id) or the YouTube link through yt-dlp.
fn play_library_trailer(
    screen: TrailerScreen,
    state: &Arc<Mutex<FjordState>>,
    video: &Arc<Mutex<crate::playback::VideoState>>,
    ww: &Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    let Some(key) = screen.current_key(&g) else {
        return;
    };
    let (title, url) = match screen {
        TrailerScreen::Detail => (g.get_detail_title(), g.get_detail_trailer_url()),
        _ => (g.get_series_title(), g.get_series_trailer_url()),
    };
    let title = format!("Trailer — {title}");
    let (mut config, quality, local, client) = {
        let s = state.lock().unwrap();
        (
            s.player_config(),
            s.config.active().trailer_quality.clone(),
            s.local_trailers
                .get(&screen)
                .filter(|(k, _)| *k == key)
                .map(|(_, id)| id.clone()),
            s.client.clone(),
        )
    };
    config.start_position_secs = None; // no resume concept for a trailer
    if let (Some(id), Some(client)) = (local, client) {
        debug!("library trailer ({screen:?}): playing the server's trailer file {id}");
        let url = client.direct_play_url(&id);
        crate::playback::play_trailer(
            url,
            title,
            config,
            crate::playback::TrailerSource::Server,
            video,
            ww,
            rt,
        );
        return;
    }
    if url.is_empty() {
        return;
    }
    config.ytdl_format = trailer_ytdl_format(&quality);
    crate::playback::play_trailer(
        url.to_string(),
        title,
        config,
        crate::playback::TrailerSource::YouTube,
        video,
        ww,
        rt,
    );
}

// ── yt-dlp detection (Watch Trailer) ──────────────────────────────────────────
// Same shape as fetch_audio_devices/fetch_system_fonts above — shells out
// once at startup to check for a local tool. mpv's bundled ytdl_hook can
// resolve a YouTube watch-page URL into a playable stream, but only when
// yt-dlp (or youtube-dl) is actually installed; this proactively gates the
// Watch Trailer button's visibility so a click never guarantees failure —
// see CLAUDE.md's Seerr integration section for the full reasoning.
pub(crate) fn detect_yt_dlp() -> bool {
    std::process::Command::new("yt-dlp")
        .arg("--version")
        .output()
        .map(|out| out.status.success())
        .unwrap_or(false)
}

/// Settings → Integrations → Trailer Quality -> mpv `ytdl-format` string.
/// `quality` is the display-ready value stored directly in
/// `Config.trailer_quality` (same idiom as `Config.sub_color`/
/// `SUB_COLOR_MODEL`) — "Best" (and anything unrecognized) leaves it
/// `None`, yt-dlp's own default selection, no override. The height cap is
/// yt-dlp's own documented format-selector idiom for exactly this ("cap
/// resolution") use case.
pub(crate) fn trailer_ytdl_format(quality: &str) -> Option<String> {
    let height = match quality {
        "1080p" => 1080,
        "720p" => 720,
        "480p" => 480,
        _ => return None,
    };
    Some(format!(
        "bestvideo[height<={height}]+bestaudio/best[height<={height}]"
    ))
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

    #[test]
    fn library_candidates_ranked_and_filtered() {
        use super::library_trailer_candidates;
        use fjord_api::models::RemoteTrailer;
        let t = |url: &str, name: Option<&str>| RemoteTrailer {
            url: url.into(),
            name: name.map(str::to_string),
        };
        let yt = |id: &str| format!("https://www.youtube.com/watch?v={id}");
        let list = vec![
            t(&yt("spot"), Some("TV Spot: Big Game")),
            t(&yt("teaser"), Some("Teaser")),
            t("--exec=touch /tmp/x", Some("Official Trailer")), // not a YouTube URL: dropped
            t(&yt("main"), Some("Official Trailer")),
            t(&yt("unnamed"), None),
            t(&yt("main"), Some("Official Trailer 2")), // duplicate URL: once
            t(&yt("final"), Some("Final Trailer")),
        ];
        assert_eq!(
            library_trailer_candidates(&list),
            vec![yt("main"), yt("final"), yt("teaser"), yt("unnamed")] // capped at 4
        );
        // Series entries have no names: their own order is kept.
        let series = vec![t(&yt("a"), None), t(&yt("b"), None)];
        assert_eq!(library_trailer_candidates(&series), vec![yt("a"), yt("b")]);
        assert!(library_trailer_candidates(&[]).is_empty());
    }
}
