// ── fjord-app · playback/tracks.rs ────────────────────────────────────────
//   fmt_secs                seconds → "H:MM:SS" / "M:SS"
//   fmt_ends_at             remaining seconds → local wall-clock "HH:MM" (empty when ≤ 0)
//   build_track_model       Vec<TrackInfo> → ModelRc<TrackEntry>; title preferred, falls back to external filename base
//   apply_audio_track       music bar + album art + lyrics for an Audio track a GAPLESS transition
//                           started (condensed mirror of start_playback's is_audio block)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── fmt_secs ──────────────────────────────────────────────────────────────────
pub(crate) fn fmt_secs(secs: f64) -> SharedString {
    if secs <= 0.0 {
        return "0:00".into();
    }
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

// ── fmt_ends_at ───────────────────────────────────────────────────────────────
pub(crate) fn fmt_ends_at(remaining_secs: f64) -> SharedString {
    if remaining_secs <= 0.0 {
        return "".into();
    }
    let ends = Local::now() + chrono::Duration::seconds(remaining_secs as i64);
    SharedString::from(ends.format("%H:%M").to_string().as_str())
}

// ── sub_lang_code ────────────────────────────────────────────────────────────
pub(crate) fn sub_lang_code(name: &str) -> &str {
    match name {
        "English" => "en",
        "German" => "de",
        "French" => "fr",
        "Japanese" => "ja",
        "Spanish" => "es",
        "Italian" => "it",
        "Portuguese" => "pt",
        "Russian" => "ru",
        "Korean" => "ko",
        "Chinese" => "zh",
        "Dutch" => "nl",
        "Swedish" => "sv",
        "Polish" => "pl",
        "Czech" => "cs",
        "Arabic" => "ar",
        "Turkish" => "tr",
        "Finnish" => "fi",
        "Danish" => "da",
        "Norwegian" => "no",
        _ => "",
    }
}

// ── build_track_model ─────────────────────────────────────────────────────────
pub(crate) fn build_track_model(tracks: &[TrackInfo], kind: &str) -> ModelRc<TrackEntry> {
    let entries: Vec<TrackEntry> = tracks
        .iter()
        .filter(|t| t.track_type == kind)
        .map(|t| {
            let mut label = String::new();

            // Title first: prefer embedded title, fall back to base filename for external tracks.
            let title = if !t.title.is_empty() {
                t.title.clone()
            } else if !t.external_filename.is_empty() {
                std::path::Path::new(&t.external_filename)
                    .file_name()
                    .map(|f| f.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                String::new()
            };
            if !title.is_empty() {
                label.push_str(&title);
            }

            // Append type tag for subtitle tracks when the flag is set but the
            // title doesn't already contain a hint (avoids "English (SDH) [SDH]").
            if kind == "sub" {
                let title_lower = title.to_ascii_lowercase();
                if t.hearing_impaired
                    && !title_lower.contains("sdh")
                    && !title_lower.contains("hearing")
                {
                    if !label.is_empty() {
                        label.push(' ');
                    }
                    label.push_str("[SDH]");
                } else if t.forced && !title_lower.contains("forced") {
                    if !label.is_empty() {
                        label.push(' ');
                    }
                    label.push_str("[Forced]");
                }
            }

            // Language code after title.
            if !t.lang.is_empty() {
                if !label.is_empty() {
                    label.push(' ');
                }
                label.push_str(&t.lang);
            }

            // Codec last.
            if !t.codec.is_empty() {
                label.push_str(&format!(" ({})", t.codec));
            }
            if label.is_empty() {
                label = format!("Track {}", t.id);
            }
            TrackEntry {
                id: t.id as i32,
                label: label.into(),
            }
        })
        .collect();
    ModelRc::new(VecModel::from(entries))
}

// Music-bar UI + album art + lyrics for an Audio track that just started via a
// GAPLESS transition (same mpv instance). Condensed mirror of start_playback's
// is_audio block — keep the two in sync when changing music-bar behaviour.
// `my_gen` guards the async art/lyrics pushes against later track changes.
pub(crate) fn apply_audio_track(
    video: &Arc<Mutex<VideoState>>,
    ww: &slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    qi: &QueueItem,
    my_gen: u64,
) {
    let (artist, art_id) = qi.audio_meta.clone().unwrap_or_default();
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_playing_title(ss(&qi.title));
        g.set_music_bar_title(ss(&qi.title));
        g.set_music_bar_artist(ss(&artist));
        g.set_music_bar_album_id(ss(&art_id));
        g.set_music_bar_has_art(false);
        g.set_music_bar_pos(0.0);
        g.set_music_bar_elapsed("0:00".into());
        g.set_lyrics_available(false);
        g.set_show_lyrics(false);
        g.set_lyrics_active_idx(-1);
        g.set_lyrics_lines(ModelRc::new(VecModel::<crate::LyricEntry>::default()));
        if g.get_music_bar_focused() == 9 {
            g.set_music_bar_focused(8);
        }
    }
    let client = video.lock().unwrap().client.as_ref().map(Arc::clone);
    let Some(client) = client else { return };

    // Album art (generation-guarded)
    {
        let ww_art = ww.clone();
        let vid_art = Arc::clone(video);
        let art_fetch = if art_id.is_empty() {
            qi.id.clone()
        } else {
            art_id
        };
        let cli_art = Arc::clone(&client);
        rt.spawn(async move {
            if let Some(bytes) = crate::poster::fetch_poster_cached(&cli_art, &art_fetch).await
                && let Some(spb) = crate::poster::decode_poster_buffer(&bytes)
            {
                let _ = slint::invoke_from_event_loop(move || {
                    if vid_art.lock().unwrap().playback_generation != my_gen {
                        return;
                    }
                    if let Some(w) = ww_art.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_is_audio_playing() {
                            g.set_music_bar_art(slint::Image::from_rgba8(spb));
                            g.set_music_bar_has_art(true);
                        }
                    }
                });
            }
        });
    }
    // Lyrics (generation-guarded, single lock scope)
    {
        let ww_lyr = ww.clone();
        let video_lyr = Arc::clone(video);
        let id_lyr = qi.id.clone();
        rt.spawn(async move {
            if let Ok(Some(lines)) = client.get_lyrics(&id_lyr).await {
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
                    if vid_ui.lock().unwrap().playback_generation != my_gen {
                        return;
                    }
                    if let Some(w) = ww_lyr.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_is_audio_playing() {
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
        });
    }
}
