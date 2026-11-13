// ── fjord-app · playback/tracks.rs ────────────────────────────────────────
//   fmt_secs                seconds → "H:MM:SS" / "M:SS"
//   fmt_ends_at             remaining seconds → local wall-clock "HH:MM" (empty when ≤ 0)
//   lang_name / codec_name / channels_name  track names people know: "es-419" → "Spanish (Latin
//                           America)", "subrip" → "SRT", 6 channels → "5.1" (unit-tested)
//   track_label             language · a title that says more · SDH/Forced/Audio description · format
//                           (+ channels) · Default (unit-tested)
//   build_track_model       Vec<TrackInfo> → ModelRc<TrackEntry> (track_label)
//   spawn_audio_quality     the playing song's format + quality (music-bar-quality / -long) from one
//                           item-detail fetch, generation-guarded (start_playback + apply_audio_track)
//   apply_audio_track       music bar + album art (+ its 24px blur for Now Playing's background) +
//                           lyrics + quality for an Audio track a GAPLESS
//                           transition started (condensed mirror of start_playback's is_audio block)
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

// ── Track names (9c T1/T2) ────────────────────────────────────────────────────

/// A track's language code as a name: ISO 639-1 ("en") and 639-2 ("eng", "swe"), plus a region or
/// script ("es-419" → "Spanish (Latin America)", "pt-BR", "zh-Hant"). None for an unknown code
/// (the caller shows the code). Unit-tested.
pub(crate) fn lang_name(code: &str) -> Option<String> {
    let lower = code.to_ascii_lowercase();
    let (base, rest) = match lower.split_once(['-', '_']) {
        Some((b, r)) => (b, Some(r)),
        None => (lower.as_str(), None),
    };
    let name = match base {
        "en" | "eng" => "English",
        "sv" | "swe" => "Swedish",
        "da" | "dan" => "Danish",
        "no" | "nor" | "nb" | "nob" | "nn" | "nno" => "Norwegian",
        "fi" | "fin" => "Finnish",
        "is" | "ice" | "isl" => "Icelandic",
        "de" | "ger" | "deu" => "German",
        "fr" | "fre" | "fra" => "French",
        "es" | "spa" => "Spanish",
        "pt" | "por" => "Portuguese",
        "it" | "ita" => "Italian",
        "nl" | "dut" | "nld" => "Dutch",
        "pl" | "pol" => "Polish",
        "cs" | "cze" | "ces" => "Czech",
        "sk" | "slo" | "slk" => "Slovak",
        "hu" | "hun" => "Hungarian",
        "ro" | "rum" | "ron" => "Romanian",
        "el" | "gre" | "ell" => "Greek",
        "tr" | "tur" => "Turkish",
        "ru" | "rus" => "Russian",
        "uk" | "ukr" => "Ukrainian",
        "bg" | "bul" => "Bulgarian",
        "hr" | "hrv" => "Croatian",
        "sr" | "srp" => "Serbian",
        "sl" | "slv" => "Slovenian",
        "et" | "est" => "Estonian",
        "lv" | "lav" => "Latvian",
        "lt" | "lit" => "Lithuanian",
        "he" | "heb" => "Hebrew",
        "ar" | "ara" => "Arabic",
        "fa" | "per" | "fas" => "Persian",
        "hi" | "hin" => "Hindi",
        "th" | "tha" => "Thai",
        "vi" | "vie" => "Vietnamese",
        "id" | "ind" => "Indonesian",
        "ms" | "may" | "msa" => "Malay",
        "ja" | "jpn" => "Japanese",
        "ko" | "kor" => "Korean",
        "zh" | "chi" | "zho" => "Chinese",
        "ca" | "cat" => "Catalan",
        "eu" | "baq" | "eus" => "Basque",
        "gl" | "glg" => "Galician",
        "ta" | "tam" => "Tamil",
        "te" | "tel" => "Telugu",
        "fil" | "tl" | "tgl" => "Filipino",
        "und" => return Some("Unknown language".into()),
        _ => return None,
    };
    let region = rest.map(|r| match r {
        "419" => "Latin America".to_string(),
        "br" => "Brazil".into(),
        "pt" => "Portugal".into(),
        "es" => "Spain".into(),
        "mx" => "Mexico".into(),
        "us" => "US".into(),
        "gb" => "UK".into(),
        "ca" => "Canada".into(),
        "fr" => "France".into(),
        "be" => "Belgium".into(),
        "ch" => "Switzerland".into(),
        "at" => "Austria".into(),
        "hant" | "tw" | "hk" => "Traditional".into(),
        "hans" | "cn" => "Simplified".into(),
        other => other.to_ascii_uppercase(),
    });
    Some(match region {
        Some(r) => format!("{name} ({r})"),
        None => name.to_string(),
    })
}

/// mpv's codec id as people know it ("subrip" → "SRT", "eac3" → "E-AC3"); a profile that names
/// the format better ("DTS-HD MA") wins. Unknown ids pass through, upper-cased. Unit-tested.
pub(crate) fn codec_name(codec: &str, profile: &str) -> String {
    if codec == "dts" && profile.starts_with("DTS") {
        return profile.to_string();
    }
    match codec {
        "subrip" | "srt" => "SRT",
        "ass" => "ASS",
        "ssa" => "SSA",
        "hdmv_pgs_subtitle" | "pgssub" => "PGS",
        "dvd_subtitle" | "dvdsub" => "VobSub",
        "dvb_subtitle" | "dvbsub" => "DVB",
        "mov_text" => "MP4 text",
        "webvtt" => "WebVTT",
        "aac" => "AAC",
        "ac3" => "AC3",
        "eac3" => "E-AC3",
        "truehd" => "TrueHD",
        "dts" => "DTS",
        "flac" => "FLAC",
        "opus" => "Opus",
        "vorbis" => "Vorbis",
        "mp3" => "MP3",
        "alac" => "ALAC",
        "hevc" => "HEVC",
        "h264" => "H.264",
        "av1" => "AV1",
        "vp9" => "VP9",
        "mpeg2video" => "MPEG-2",
        c if c.starts_with("pcm") => "PCM",
        c => return c.to_ascii_uppercase(),
    }
    .to_string()
}

/// An audio track's channels: "Mono", "Stereo", "5.1", "7.1" — mpv's layout without "(side)",
/// else the count. "" when unknown. Unit-tested.
pub(crate) fn channels_name(count: i64, layout: &str) -> String {
    match count {
        1 => return "Mono".into(),
        2 => return "Stereo".into(),
        _ => {}
    }
    let layout = layout.split('(').next().unwrap_or("").trim();
    if layout.contains('.') {
        return layout.to_string();
    }
    match count {
        6 => "5.1".into(),
        8 => "7.1".into(),
        n if n > 0 => format!("{n} ch"),
        _ => String::new(),
    }
}

/// One track's line in the Sub/Audio/Video panels: language, a title that says more than the
/// language, the flags, then the format — "English  ·  SDH  ·  SRT", "Swedish  ·  Commentary  ·
/// E-AC3 5.1  ·  Default". Unit-tested.
fn track_label(t: &TrackInfo) -> String {
    let mut parts: Vec<String> = Vec::new();
    let lang = if t.lang.is_empty() {
        None
    } else {
        Some(lang_name(&t.lang).unwrap_or_else(|| t.lang.clone()))
    };
    if let Some(l) = &lang {
        parts.push(l.clone());
    }
    // The title, or an external track's file name — unless it only repeats the language.
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
    let title_lower = title.to_ascii_lowercase();
    if !title.is_empty()
        && lang
            .as_ref()
            .is_none_or(|l| !l.eq_ignore_ascii_case(&title))
    {
        parts.push(title);
    }
    if t.track_type == "sub" {
        if t.hearing_impaired && !title_lower.contains("sdh") && !title_lower.contains("hearing") {
            parts.push("SDH".into());
        } else if t.forced && !title_lower.contains("forced") {
            parts.push("Forced".into());
        }
    }
    if t.visual_impaired && !title_lower.contains("descri") {
        parts.push("Audio description".into());
    }
    let format = if t.codec.is_empty() {
        String::new()
    } else {
        codec_name(&t.codec, &t.codec_profile)
    };
    let format = if t.track_type == "audio" {
        let ch = channels_name(t.channel_count, &t.channels);
        [format, ch]
            .into_iter()
            .filter(|s| !s.is_empty())
            .collect::<Vec<_>>()
            .join(" ")
    } else {
        format
    };
    if !format.is_empty() {
        parts.push(format);
    }
    if t.default {
        parts.push("Default".into());
    }
    if parts.is_empty() {
        format!("Track {}", t.id)
    } else {
        parts.join("  ·  ")
    }
}

// ── build_track_model ─────────────────────────────────────────────────────────
pub(crate) fn build_track_model(tracks: &[TrackInfo], kind: &str) -> ModelRc<TrackEntry> {
    let entries: Vec<TrackEntry> = tracks
        .iter()
        .filter(|t| t.track_type == kind)
        .map(|t| TrackEntry {
            id: t.id as i32,
            label: track_label(t).into(),
        })
        .collect();
    ModelRc::new(VecModel::from(entries))
}

// The playing song's format + quality for the music bar and Now Playing (music-bar-quality /
// -long): its Jellyfin streams, from one item-detail fetch (Fields include MediaStreams).
// Generation-guarded like the art and lyrics; empty when the server reports no audio stream.
pub(crate) fn spawn_audio_quality(
    client: Arc<fjord_api::JellyfinClient>,
    item_id: String,
    video: &Arc<Mutex<VideoState>>,
    ww: &slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
    my_gen: u64,
) {
    let video = Arc::clone(video);
    let ww = ww.clone();
    rt.spawn(async move {
        let quality = match client.get_item_detail(&item_id).await {
            Ok(item) => item.audio_quality(),
            Err(e) => {
                debug!("audio quality for {item_id}: {e:#}");
                None
            }
        };
        let (short, long) = quality.map(|q| (q.short(), q.long())).unwrap_or_default();
        debug!("audio quality for {item_id}: {long:?}");
        let _ = slint::invoke_from_event_loop(move || {
            if video.lock().unwrap().playback_generation != my_gen {
                return;
            }
            if let Some(w) = ww.upgrade() {
                let g = AppState::get(&w);
                g.set_music_bar_quality(ss(&short));
                g.set_music_bar_quality_long(ss(&long));
            }
        });
    });
}

// Music-bar UI + album art + lyrics + quality for an Audio track that just started via a
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
        g.set_music_bar_quality("".into());
        g.set_music_bar_quality_long("".into());
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
    spawn_audio_quality(Arc::clone(&client), qi.id.clone(), video, ww, rt, my_gen);

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
                // Now Playing's blurred background (Settings → Audio), from the same bytes.
                let blur = crate::poster::decode_blur_buffer(&bytes);
                let _ = slint::invoke_from_event_loop(move || {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn track(kind: &str) -> TrackInfo {
        TrackInfo {
            id: 3,
            track_type: kind.into(),
            ..TrackInfo::default()
        }
    }

    #[test]
    fn languages_have_names() {
        assert_eq!(lang_name("en").as_deref(), Some("English"));
        assert_eq!(lang_name("swe").as_deref(), Some("Swedish"));
        assert_eq!(
            lang_name("es-419").as_deref(),
            Some("Spanish (Latin America)")
        );
        assert_eq!(lang_name("pt-BR").as_deref(), Some("Portuguese (Brazil)"));
        assert_eq!(
            lang_name("zh-Hant").as_deref(),
            Some("Chinese (Traditional)")
        );
        assert_eq!(lang_name("xx"), None);
    }

    #[test]
    fn formats_and_channels_have_names() {
        assert_eq!(codec_name("subrip", ""), "SRT");
        assert_eq!(codec_name("hdmv_pgs_subtitle", ""), "PGS");
        assert_eq!(codec_name("eac3", ""), "E-AC3");
        assert_eq!(codec_name("dts", "DTS-HD MA"), "DTS-HD MA");
        assert_eq!(codec_name("weird", ""), "WEIRD");
        assert_eq!(channels_name(2, ""), "Stereo");
        assert_eq!(channels_name(6, "5.1(side)"), "5.1");
        assert_eq!(channels_name(8, ""), "7.1");
        assert_eq!(channels_name(0, ""), "");
    }

    #[test]
    fn a_track_reads_like_people_say_it() {
        let mut s = track("sub");
        s.lang = "en".into();
        s.codec = "subrip".into();
        s.hearing_impaired = true;
        assert_eq!(track_label(&s), "English  ·  SDH  ·  SRT");
        // A title that only repeats the language goes; one that says more stays.
        s.hearing_impaired = false;
        s.lang = "es-419".into();
        s.title = "Spanish (Latin America)".into();
        assert_eq!(track_label(&s), "Spanish (Latin America)  ·  SRT");
        let mut a = track("audio");
        a.lang = "eng".into();
        a.title = "Commentary".into();
        a.codec = "eac3".into();
        a.channel_count = 6;
        a.default = true;
        assert_eq!(
            track_label(&a),
            "English  ·  Commentary  ·  E-AC3 5.1  ·  Default"
        );
        assert_eq!(track_label(&track("video")), "Track 3");
    }
}
