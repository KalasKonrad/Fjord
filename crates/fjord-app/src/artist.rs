// ── fjord-app · artist.rs ─────────────────────────────────────────────────────
//   open_artist_screen   reset AppState artist props; increment artist-open-gen;
//                        spawn async: fetch artist albums + portrait + detail in parallel;
//                        build CardItem model with posters; gen-guarded
//                        invoke_from_event_loop shows page (show-artist=true)
//   handle_key           keyboard dispatch: Back button / btn row / album grid;
//                        Up from row 0 → btn row; btn row → Back / grid;
//                        Enter on album → open-album; C → context menu
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::poster::{decode_poster_buffer, fetch_poster_cached, fetch_poster_cached_tagged};
use crate::{AppState, CardItem, MainWindow};

// ── open_artist_screen ────────────────────────────────────────────────────────

pub(crate) fn open_artist_screen(
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
        g.set_artist_id(id.as_str().into());
        g.set_artist_title(title.as_str().into());
        g.set_artist_overview("".into());
        g.set_artist_meta("".into());
        g.set_artist_has_portrait(false);
        g.set_artist_albums(ModelRc::new(VecModel::default()));
        g.set_artist_focused(0);
        g.set_artist_back_focused(false);
        g.set_artist_btn_focused(-1);
        g.set_artist_is_favorite(false);
        g.set_artist_overview_expanded(false);
        g.set_app_loading_progress(0.0);
        g.set_app_content_loading(true);
        let next = g.get_artist_open_gen() + 1;
        g.set_artist_open_gen(next);
        next
    } else {
        return;
    };

    let id2 = id.clone();
    let ww2 = ww.clone();
    let state_task = state;
    rt.spawn(async move {
        let (albums_res, portrait_bytes, detail_res) = tokio::join!(
            client.get_artist_albums(&id2),
            fetch_poster_cached(&client, &id2),
            client.get_item_detail(&id2),
        );

        // Deleted artist: the ArtistIds album query returns an empty 200 — the
        // ghost is only visible on the detail fetch's 404 (S4).
        if let Err(e) = &detail_res {
            if crate::is_not_found(e) {
                let ww_err = ww2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww_err.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_artist_open_gen() == gen {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                crate::purge_deleted_item(&state_task, &ww2, &id2);
                return;
            }
        }

        let albums = match albums_res {
            Ok(v) => v,
            Err(e) => {
                warn!("open_artist_screen get_artist_albums({}): {:#}", id2, e);
                crate::show_toast(ww2, "Couldn't load artist — check your server connection".into());
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_artist_open_gen() == gen {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                return;
            }
        };

        let album_count = albums.len();
        let meta = format!("{} album{}", album_count, if album_count == 1 { "" } else { "s" });

        // Fetch album posters in parallel (semaphore 8)
        use std::sync::Arc as SArc;
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for album in &albums {
            let client2 = Arc::clone(&client);
            let sem2    = Arc::clone(&sem);
            let aid     = album.id.clone();
            let tag     = album.primary_image_tag().map(str::to_string);
            fetch_set.spawn(async move {
                let Ok(_permit) = sem2.acquire_owned().await else { return (aid, None) };
                let bytes = fetch_poster_cached_tagged(&*client2, &aid, tag.as_deref()).await.map(SArc::new);
                (aid, bytes)
            });
        }
        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();
        while let Some(res) = fetch_set.join_next().await {
            if let Ok((pid, Some(b))) = res { poster_map.insert(pid, b); }
        }

        // Decode album cards (on Tokio worker, before entering the UI thread)
        type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
        // Card rows: album name on top, year below (the artist page already
        // names the artist, so the usual album-artist subtitle is redundant).
        let album_decoded: Vec<(String, String, String, i32, bool, bool, f32, i32, Option<Buf>)> = albums.iter()
            .map(|a| {
                let buf = poster_map.get(&a.id).and_then(|b| decode_poster_buffer(b));
                (a.id.clone(), a.name.clone(),
                 a.production_year.map(|y| y.to_string()).unwrap_or_default(),
                 a.production_year.unwrap_or(0) as i32,
                 a.user_data.played, a.user_data.is_favorite, a.resume_pct(),
                 a.user_data.unplayed_item_count, buf)
            })
            .collect();

        let portrait_buf = portrait_bytes.and_then(|b| decode_poster_buffer(&b));
        let meta2 = meta.clone();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_artist_open_gen() != gen { return; }

            g.set_artist_meta(meta2.as_str().into());

            if let Ok(d) = &detail_res {
                g.set_artist_overview(d.overview.clone().unwrap_or_default().as_str().into());
                g.set_artist_is_favorite(d.user_data.is_favorite);
            }

            if let Some(spb) = portrait_buf {
                g.set_artist_portrait(slint::Image::from_rgba8(spb));
                g.set_artist_has_portrait(true);
            }

            let items: Vec<CardItem> = album_decoded.into_iter().map(|(id, title, subtitle, year, played, is_fav, rpct, upc, buf)| {
                let mut h = CardItem::default();
                h.id           = id.as_str().into();
                h.item_type    = "MusicAlbum".into();
                h.title        = title.as_str().into();
                h.subtitle     = subtitle.as_str().into();
                h.year         = year;
                h.has_played   = played;
                h.is_favorite  = is_fav;
                h.resume_pct   = rpct;
                h.unplayed_count = upc;
                if let Some(spb) = buf { h.poster = slint::Image::from_rgba8(spb); h.has_poster = true; }
                h
            }).collect();

            g.set_artist_albums(ModelRc::new(VecModel::from(items)));
            g.set_artist_focused(0);
            g.set_artist_back_focused(false);
            g.set_app_content_loading(false);
            g.set_show_artist(true);
        });
    });
}

// ── handle_key ────────────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;

    // ── Back button focused ────────────────────────────────────────────────────
    if g.get_artist_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.invoke_close_artist();
                true
            }
            Action::Down => {
                g.set_artist_back_focused(false);
                g.set_artist_btn_focused(0);
                true
            }
            Action::Up => false, // allow focus_bar_on_down
            _ => true,
        };
    }

    // ── ▶/♥ button row focused ─────────────────────────────────────────────────
    let btn = g.get_artist_btn_focused();
    if btn >= 0 {
        return match action {
            Action::Left  => { if btn > 0 { g.set_artist_btn_focused(btn - 1); } true }
            Action::Right => { if btn < 1 { g.set_artist_btn_focused(btn + 1); } true }
            Action::Confirm => {
                if btn == 0 { g.invoke_play_artist_all(); }
                else        { g.invoke_toggle_artist_fav(); }
                true
            }
            Action::Up => {
                g.set_artist_btn_focused(-1);
                g.set_artist_back_focused(true);
                true
            }
            Action::Down => {
                g.set_artist_btn_focused(-1);
                true
            }
            Action::Back => {
                g.set_artist_btn_focused(-1);
                g.invoke_close_artist();
                true
            }
            _ => true,
        };
    }

    // ── Album grid ─────────────────────────────────────────────────────────────
    let cols  = g.get_library_cols();
    let total = g.get_artist_albums().row_count() as i32;
    let f     = g.get_artist_focused();

    match action {
        Action::Back => {
            g.invoke_close_artist();
            true
        }
        Action::Right => {
            if f + 1 < total { g.set_artist_focused(f + 1); }
            true
        }
        Action::Left => {
            if f > 0 { g.set_artist_focused(f - 1); }
            true
        }
        Action::Down => {
            let next = f + cols;
            if next < total { g.set_artist_focused(next); true }
            else { false } // at last row — let focus_bar_on_down handle it
        }
        Action::Up => {
            if f < cols {
                // first row → button row
                g.set_artist_btn_focused(0);
                true
            } else {
                g.set_artist_focused(f - cols);
                true
            }
        }
        Action::Confirm => {
            if f < total {
                if let Some(card) = g.get_artist_albums().row_data(f as usize) {
                    g.invoke_open_album(card.id, card.title);
                }
            }
            true
        }
        Action::OpenContextMenu => {
            if f < total {
                if let Some(card) = g.get_artist_albums().row_data(f as usize) {
                    g.invoke_open_context_menu(card.id, card.has_played, card.is_favorite,
                        card.resume_pct, card.item_type, card.series_id);
                }
            }
            true
        }
        _ => false,
    }
}
