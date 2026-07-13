// ── fjord-app · artist.rs ─────────────────────────────────────────────────────
//   open_artist_screen   reset AppState artist props; increment artist-open-gen; checks
//                        artist_albums_cache + item_detail_cache (Part 2) — only sets
//                        app-content-loading=true when either is a miss; spawn async: fetch
//                        artist albums + portrait + detail in parallel (cached ones skip their
//                        network call); build CardItem model with posters, applied via
//                        apply_cards_preserving_identity; gen-guarded invoke_from_event_loop
//                        shows page (show-artist=true)
//   handle_key           keyboard dispatch: Back button / btn row / bio (slot 2) / album grid;
//                        Up from row 0 → bio (or btn row); btn row → Back / bio / grid;
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
    // Screen-open cache (Part 2): skip the loading spinner when both the album
    // list and detail are cached — the remaining work (portrait/album-poster
    // fetch) is disk-cached and fast enough to feel instant.
    let (client, cached_albums, cached_detail) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        (c, s.artist_albums_cache.get(&id), s.item_detail_cache.get(&id))
    };
    let is_cache_hit = cached_albums.is_some() && cached_detail.is_some();
    tracing::debug!("open_artist_screen({id}): cache_hit={is_cache_hit}");

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
        if !is_cache_hit {
            g.set_app_content_loading(true);
        }
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
        let albums_fut = async {
            if let Some(v) = cached_albums { return Ok(v); }
            client.get_artist_albums(&id2).await
        };
        let detail_fut = async {
            if let Some(d) = cached_detail { return Ok(d); }
            client.get_item_detail(&id2).await
        };
        let (albums_res, portrait_bytes, detail_res) = tokio::join!(
            albums_fut,
            fetch_poster_cached(&client, &id2),
            detail_fut,
        );
        if let Ok(d) = &detail_res {
            state_task.lock().unwrap().item_detail_cache.insert(id2.clone(), d.clone());
        }

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
        state_task.lock().unwrap().artist_albums_cache.insert(id2.clone(), albums.clone());

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
                let bytes = fetch_poster_cached_tagged(&client2, &aid, tag.as_deref()).await.map(SArc::new);
                (aid, bytes)
            });
        }
        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();
        while let Some(res) = fetch_set.join_next().await {
            if let Ok((pid, Some(b))) = res { poster_map.insert(pid, b); }
        }

        // Decode album cards (on Tokio worker, before entering the UI thread)
        // (id, title, subtitle, year, played, is_favorite, resume_pct, unplayed_count,
        // decoded poster buffer). Card rows: album name on top, year below (the artist
        // page already names the artist, so the usual album-artist subtitle is redundant).
        type DecodedAlbumCard = (String, String, String, i32, bool, bool, f32, i32,
                                  Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>);
        let album_decoded: Vec<DecodedAlbumCard> = albums.iter()
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
                g.set_artist_overview(d.overview.clone().unwrap_or_default().trim().into());
                g.set_artist_is_favorite(d.user_data.is_favorite);
            }

            if let Some(spb) = portrait_buf {
                g.set_artist_portrait(slint::Image::from_rgba8(spb));
                g.set_artist_has_portrait(true);
            }

            let items: Vec<CardItem> = album_decoded.into_iter().map(|(id, title, subtitle, year, played, is_fav, rpct, upc, buf)| {
                let mut h = CardItem {
                    id:             id.as_str().into(),
                    item_type:      "MusicAlbum".into(),
                    title:          title.as_str().into(),
                    subtitle:       subtitle.as_str().into(),
                    year,
                    has_played:     played,
                    is_favorite:    is_fav,
                    resume_pct:     rpct,
                    unplayed_count: upc,
                    ..Default::default()
                };
                if let Some(spb) = buf { h.poster = slint::Image::from_rgba8(spb); h.has_poster = true; }
                h
            }).collect();

            g.set_artist_albums(crate::apply_cards_preserving_identity(&g.get_artist_albums(), items));
            g.set_artist_focused(0);
            g.set_artist_back_focused(false);
            g.set_app_content_loading(false);
            g.set_show_artist(true);
            w.invoke_grab_keyboard_focus();
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

    // ── ▶/♥ button row + bio focused (0=Play All, 1=♥, 2=bio) ─────────────────
    let btn = g.get_artist_btn_focused();
    if btn >= 0 {
        return match action {
            Action::Left  => { if btn > 0 && btn <= 1 { g.set_artist_btn_focused(btn - 1); } true }
            Action::Right => { if btn < 1             { g.set_artist_btn_focused(btn + 1); } true }
            Action::Confirm => {
                match btn {
                    0 => g.invoke_play_artist_all(),
                    1 => g.invoke_toggle_artist_fav(),
                    _ => g.set_artist_overview_expanded(!g.get_artist_overview_expanded()),
                }
                true
            }
            Action::Up => {
                if btn == 2 {
                    g.set_artist_btn_focused(0); // bio → ▶ Play All
                } else {
                    g.set_artist_btn_focused(-1);
                    g.set_artist_back_focused(true);
                }
                true
            }
            Action::Down => {
                if btn <= 1 && !g.get_artist_overview().is_empty() {
                    g.set_artist_btn_focused(2); // buttons → bio
                } else {
                    g.set_artist_btn_focused(-1); // → album grid
                }
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
                // first row → bio (sits between header and grid) or button row
                if !g.get_artist_overview().is_empty() {
                    g.set_artist_btn_focused(2);
                } else {
                    g.set_artist_btn_focused(0);
                }
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
                    g.set_context_menu_title(card.title.clone());
                    g.invoke_open_context_menu(card.id, card.has_played, card.is_favorite,
                        card.resume_pct, card.item_type, card.series_id);
                }
            }
            true
        }
        _ => false,
    }
}
