// ── fjord-app · album.rs ──────────────────────────────────────────────────────
//   open_album_screen   reset AppState album props; increment album-open-gen;
//                       set app-content-loading=true; spawn async: fetch album
//                       tracks + cover poster in parallel; populate TrackItem model;
//                       gen-guarded invoke_from_event_loop shows page
//   handle_key          keyboard dispatch: Back button / ♥✓ button row / track list;
//                       Up from track 0 → ♥✓ row; C → open-context-menu;
//                       Enter on track → play-album-track; Down at last track → returns false
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::playback::fmt_secs;
use crate::poster::{decode_poster_buffer, fetch_poster_cached};
use crate::{AppState, MainWindow};

// ── TrackItem helper ──────────────────────────────────────────────────────────

fn media_items_to_tracks(
    items: &[fjord_api::models::MediaItem],
) -> Vec<crate::TrackItem> {
    items
        .iter()
        .map(|m| {
            let duration_secs = m
                .run_time_ticks
                .map(|t| (t / 10_000_000) as f64)
                .unwrap_or(0.0);
            crate::TrackItem {
                id:           m.id.as_str().into(),
                title:        m.name.as_str().into(),
                artist:       m.album_artist.as_deref().unwrap_or("").into(),
                duration:     if duration_secs > 0.0 { fmt_secs(duration_secs).into() } else { "".into() },
                track_number: m.index_number.unwrap_or(0) as i32,
                has_played:   m.user_data.played,
                is_favorite:  m.user_data.is_favorite,
                resume_pct:   m.resume_pct(),
            }
        })
        .collect()
}

// ── open_album_screen ─────────────────────────────────────────────────────────

pub(crate) fn open_album_screen(
    id:    String,
    title: String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    let client = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        c
    };

    let gen = if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_album_id(id.as_str().into());
        g.set_album_title(title.as_str().into());
        g.set_album_artist("".into());
        g.set_album_meta("".into());
        g.set_album_overview("".into());
        g.set_album_has_poster(false);
        g.set_album_is_favorite(false);
        g.set_album_has_played(false);
        g.set_album_btn_focused(-1);
        g.set_album_overview_expanded(false);
        g.set_album_tracks(ModelRc::new(VecModel::default()));
        g.set_album_focused_track(0);
        g.set_album_back_focused(false);
        g.set_app_loading_progress(0.0);
        g.set_app_content_loading(true);
        let next = g.get_album_open_gen() + 1;
        g.set_album_open_gen(next);
        next
    } else {
        -1
    };

    let id2   = id.clone();
    let ww2   = ww.clone();
    rt.spawn(async move {
        let (tracks_res, poster_bytes, detail_res) = tokio::join!(
            client.get_album_tracks(&id2),
            fetch_poster_cached(&client, &id2),
            client.get_item_detail(&id2),
        );

        let tracks = match tracks_res {
            Ok(v) => v,
            Err(e) => {
                warn!("open_album_screen get_album_tracks({}): {:#}", id2, e);
                let ww_err = ww2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww_err.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_album_open_gen() == gen {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                crate::show_toast(ww2, "Couldn't load album — check your server connection".into());
                return;
            }
        };

        let track_items = media_items_to_tracks(&tracks);

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_album_open_gen() != gen { return; }

            if let Ok(d) = &detail_res {
                // Metadata line: year · N tracks · duration
                let year = d
                    .production_year
                    .map(|y| y.to_string())
                    .unwrap_or_default();
                let n_tracks = track_items.len();
                let total_secs: f64 = tracks
                    .iter()
                    .filter_map(|t| t.run_time_ticks)
                    .map(|t| (t / 10_000_000) as f64)
                    .sum();
                let meta = if year.is_empty() {
                    format!("{} tracks · {}", n_tracks, fmt_secs(total_secs))
                } else {
                    format!("{} · {} tracks · {}", year, n_tracks, fmt_secs(total_secs))
                };
                g.set_album_meta(meta.as_str().into());
                g.set_album_artist(d.album_artist.as_deref().unwrap_or("").into());
                g.set_album_overview(d.overview.clone().unwrap_or_default().as_str().into());
                g.set_album_is_favorite(d.user_data.is_favorite);
                g.set_album_has_played(d.user_data.played);
            }

            if let Some(bytes) = poster_bytes {
                if let Some(spb) = decode_poster_buffer(&bytes) {
                    g.set_album_poster(slint::Image::from_rgba8(spb));
                    g.set_album_has_poster(true);
                }
            }

            g.set_album_tracks(ModelRc::new(VecModel::from(track_items)));
            g.set_album_focused_track(0);
            g.set_album_back_focused(false);
            g.set_app_content_loading(false);
            g.set_show_album(true);
        });
    });
}

// ── handle_key ────────────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;

    // ── Back button focused ────────────────────────────────────────────────────
    if g.get_album_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_album(false);
                true
            }
            Action::Down => {
                g.set_album_back_focused(false);
                g.set_album_btn_focused(0);
                true
            }
            Action::Up => false, // allow focus_bar_on_up
            _ => true,
        };
    }

    // ── ♥/✓ button row focused ─────────────────────────────────────────────────
    let btn = g.get_album_btn_focused();
    if btn >= 0 {
        return match action {
            Action::Left  => { g.set_album_btn_focused((btn - 1).max(0)); true }
            Action::Right => { g.set_album_btn_focused((btn + 1).min(1)); true }
            Action::Confirm => {
                if btn == 0 { g.invoke_toggle_album_fav(); }
                else        { g.invoke_toggle_album_played(); }
                true
            }
            Action::Up => {
                g.set_album_btn_focused(-1);
                g.set_album_back_focused(true);
                true
            }
            Action::Down => {
                g.set_album_btn_focused(-1);
                g.set_album_focused_track(0);
                true
            }
            Action::Back => {
                g.set_album_btn_focused(-1);
                g.set_show_album(false);
                true
            }
            _ => true,
        };
    }

    // ── Track list ─────────────────────────────────────────────────────────────
    let f   = g.get_album_focused_track();
    let len = g.get_album_tracks().row_count() as i32;

    match action {
        Action::Back => {
            g.set_show_album(false);
            true
        }
        Action::Up => {
            if f > 0 {
                g.set_album_focused_track(f - 1);
            } else {
                g.set_album_btn_focused(0);
            }
            true
        }
        Action::Down => {
            if f + 1 < len {
                g.set_album_focused_track(f + 1);
                true
            } else {
                false // at last track — let focus_bar_on_down handle it
            }
        }
        Action::Left | Action::Right => true, // absorb
        Action::Confirm => {
            if f < len {
                let track = g.get_album_tracks().row_data(f as usize).unwrap();
                g.invoke_play_album_track(track.id);
            }
            true
        }
        Action::OpenContextMenu => {
            if f < len {
                let track = g.get_album_tracks().row_data(f as usize).unwrap();
                g.invoke_open_context_menu(
                    track.id,
                    track.has_played,
                    track.is_favorite,
                    track.resume_pct,
                    "Audio".into(),
                    "".into(),
                );
            }
            true
        }
        _ => false,
    }
}
