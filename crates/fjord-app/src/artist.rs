// ── fjord-app · artist.rs ─────────────────────────────────────────────────────
//   open_artist_screen   reset AppState artist props; increment artist-open-gen; checks
//                        artist_albums_cache + item_detail_cache (Part 2) — only sets
//                        app-content-loading=true when either is a miss; spawn async: fetch
//                        artist albums + portrait + detail in parallel (cached ones skip their
//                        network call); build CardItem model with posters, applied via
//                        apply_cards_preserving_identity; generation-guarded invoke_from_event_loop
//                        shows page (show-artist=true)
//   handle_key           keyboard dispatch: Back button / btn row / bio (slot 2) / album grid;
//                        Up from row 0 → bio (or btn row); btn row → Back / bio / grid;
//                        Enter on album → open-album; C → context menu
//   wire_artist            callbacks moved from main() (0.5.0 step 3): artist screen
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::poster::{decode_poster_buffer, fetch_poster_cached, fetch_poster_cached_tagged};
use crate::{AppState, CardItem, MainWindow};

// ── open_artist_screen ────────────────────────────────────────────────────────

pub(crate) fn open_artist_screen(
    id: String,
    title: String,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    // Screen-open cache (Part 2): skip the loading spinner when both the album
    // list and detail are cached — the remaining work (portrait/album-poster
    // fetch) is disk-cached and fast enough to feel instant.
    let (client, cached_albums, cached_detail) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else {
            return;
        };
        (
            c,
            s.artist_albums_cache.get(&id),
            s.item_detail_cache.get(&id),
        )
    };
    let is_cache_hit = cached_albums.is_some() && cached_detail.is_some();
    tracing::debug!("open_artist_screen({id}): cache_hit={is_cache_hit}");

    let generation = if let Some(w) = ww.upgrade() {
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
    let id_revalidate = id.clone();
    let state_revalidate = Arc::clone(&state);
    let ww_revalidate = ww.clone();
    let rt_revalidate = rt.clone();
    let state_task = state;
    rt.spawn(async move {
        let albums_fut = async {
            if let Some(v) = cached_albums {
                return Ok(v);
            }
            client.get_artist_albums(&id2).await
        };
        let detail_fut = async {
            if let Some(d) = cached_detail {
                return Ok(d);
            }
            client.get_item_detail(&id2).await
        };
        let (albums_res, portrait_bytes, detail_res) =
            tokio::join!(albums_fut, fetch_poster_cached(&client, &id2), detail_fut,);
        if let Ok(d) = &detail_res {
            state_task
                .lock()
                .unwrap()
                .item_detail_cache
                .insert(id2.clone(), d.clone());
        }

        // Deleted artist: the ArtistIds album query returns an empty 200 — the
        // ghost is only visible on the detail fetch's 404 (S4).
        if let Err(e) = &detail_res
            && crate::is_not_found(e)
        {
            let ww_err = ww2.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww_err.upgrade() {
                    let g = AppState::get(&w);
                    if g.get_artist_open_gen() == generation {
                        g.set_app_content_loading(false);
                    }
                }
            });
            crate::purge_deleted_item(&state_task, &ww2, &id2);
            return;
        }

        let albums = match albums_res {
            Ok(v) => v,
            Err(e) => {
                warn!("open_artist_screen get_artist_albums({}): {:#}", id2, e);
                crate::show_toast(
                    ww2,
                    "Couldn't load artist — check your server connection".into(),
                );
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_artist_open_gen() == generation {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                return;
            }
        };
        state_task
            .lock()
            .unwrap()
            .artist_albums_cache
            .insert(id2.clone(), albums.clone());

        let album_count = albums.len();
        let meta = format!(
            "{} album{}",
            album_count,
            if album_count == 1 { "" } else { "s" }
        );

        // Fetch album posters in parallel (semaphore 8)
        use std::sync::Arc as SArc;
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for album in &albums {
            let client2 = Arc::clone(&client);
            let sem2 = Arc::clone(&sem);
            let aid = album.id.clone();
            let tag = album.primary_image_tag().map(str::to_string);
            fetch_set.spawn(async move {
                let Ok(_permit) = sem2.acquire_owned().await else {
                    return (aid, None);
                };
                let bytes = fetch_poster_cached_tagged(&client2, &aid, tag.as_deref())
                    .await
                    .map(SArc::new);
                (aid, bytes)
            });
        }
        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();
        while let Some(res) = fetch_set.join_next().await {
            if let Ok((pid, Some(b))) = res {
                poster_map.insert(pid, b);
            }
        }

        // Decode album cards (on Tokio worker, before entering the UI thread)
        // (id, title, subtitle, year, played, is_favorite, resume_pct, unplayed_count,
        // decoded poster buffer). Card rows: album name on top, year below (the artist
        // page already names the artist, so the usual album-artist subtitle is redundant).
        type DecodedAlbumCard = (
            String,
            String,
            String,
            i32,
            bool,
            bool,
            f32,
            i32,
            Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
        );
        let album_decoded: Vec<DecodedAlbumCard> = albums
            .iter()
            .map(|a| {
                let buf = poster_map.get(&a.id).and_then(|b| decode_poster_buffer(b));
                (
                    a.id.clone(),
                    a.name.clone(),
                    a.production_year.map(|y| y.to_string()).unwrap_or_default(),
                    a.production_year.unwrap_or(0) as i32,
                    a.user_data.played,
                    a.user_data.is_favorite,
                    a.resume_pct(),
                    a.user_data.unplayed_item_count,
                    buf,
                )
            })
            .collect();

        let portrait_buf = portrait_bytes.and_then(|b| decode_poster_buffer(&b));
        let meta2 = meta.clone();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_artist_open_gen() != generation {
                return;
            }
            // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09) —
            // see collection.rs's own open_collection_screen for the full
            // reasoning (same generation-counter-alone gap, same fix).
            if !crate::session_current(&state_task, &client) {
                return;
            }

            g.set_artist_meta(meta2.as_str().into());

            if let Ok(d) = &detail_res {
                g.set_artist_overview(
                    crate::strip_html_to_text(d.overview.clone().unwrap_or_default().trim()).into(),
                );
                g.set_artist_is_favorite(d.user_data.is_favorite);
            }

            if let Some(spb) = portrait_buf {
                g.set_artist_portrait(slint::Image::from_rgba8(spb));
                g.set_artist_has_portrait(true);
            }

            let items: Vec<CardItem> = album_decoded
                .into_iter()
                .map(
                    |(id, title, subtitle, year, played, is_fav, rpct, upc, buf)| {
                        let mut h = CardItem {
                            id: id.as_str().into(),
                            item_type: "MusicAlbum".into(),
                            title: title.as_str().into(),
                            subtitle: subtitle.as_str().into(),
                            year,
                            has_played: played,
                            is_favorite: is_fav,
                            resume_pct: rpct,
                            unplayed_count: upc,
                            ..Default::default()
                        };
                        if let Some(spb) = buf {
                            h.poster = slint::Image::from_rgba8(spb);
                            h.has_poster = true;
                        }
                        h
                    },
                )
                .collect();

            g.set_artist_albums(crate::apply_cards_preserving_identity(
                &g.get_artist_albums(),
                items,
            ));
            g.set_artist_focused(0);
            g.set_artist_back_focused(false);
            g.set_app_content_loading(false);
            g.set_show_artist(true);
            w.invoke_grab_keyboard_focus();
        });
    });

    // Cache-hit only: the screen above already showed instantly from cached
    // data. Real gap, live-reported: Jellyfin's WebSocket only delivers
    // LibraryChanged to the most-recently-connected client when multiple
    // clients share a session (JELLYFIN.md) — this can silently starve Fjord
    // of the event, leaving these caches stale indefinitely with no other
    // fallback. This revalidation is what closes that gap for whatever's
    // actually on screen right now.
    if is_cache_hit {
        spawn_artist_revalidate(
            id_revalidate,
            generation,
            state_revalidate,
            ww_revalidate,
            rt_revalidate,
        );
    }
}

fn spawn_artist_revalidate(
    id: String,
    generation: i32,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    if !crate::should_revalidate(&state, &id) {
        return;
    }
    let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else {
        return;
    };
    rt.spawn(async move {
        let (albums_res, detail_res) =
            tokio::join!(client.get_artist_albums(&id), client.get_item_detail(&id));
        let (Ok(albums), Ok(detail)) = (albums_res, detail_res) else {
            return;
        };
        // Sign-out (or a different account signing in on a shared HTPC)
        // mid-fetch must not let this per-user data land in the new session's
        // cache — same guard class as main.rs::session_current's own doc
        // comment (CR11-2).
        if !crate::session_current(&state, &client) {
            return;
        }
        {
            let mut s = state.lock().unwrap();
            s.artist_albums_cache.insert(id.clone(), albums.clone());
            s.item_detail_cache.insert(id.clone(), detail.clone());
        }
        let meta = format!(
            "{} album{}",
            albums.len(),
            if albums.len() == 1 { "" } else { "s" }
        );
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<Arc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for album in &albums {
            let client2 = Arc::clone(&client);
            let sem2 = Arc::clone(&sem);
            let aid = album.id.clone();
            let tag = album.primary_image_tag().map(str::to_string);
            fetch_set.spawn(async move {
                let Ok(_permit) = sem2.acquire_owned().await else {
                    return (aid, None);
                };
                let bytes = fetch_poster_cached_tagged(&client2, &aid, tag.as_deref())
                    .await
                    .map(Arc::new);
                (aid, bytes)
            });
        }
        let mut poster_map: std::collections::HashMap<String, Arc<Vec<u8>>> = Default::default();
        while let Some(res) = fetch_set.join_next().await {
            if let Ok((pid, Some(b))) = res {
                poster_map.insert(pid, b);
            }
        }
        // Send-safe decode on the worker thread — CardItem (carries slint::Image,
        // !Send) must only ever be constructed inside invoke_from_event_loop.
        type DecodedAlbumCard = (
            String,
            String,
            String,
            i32,
            bool,
            bool,
            f32,
            i32,
            Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
        );
        let album_decoded: Vec<DecodedAlbumCard> = albums
            .iter()
            .map(|a| {
                let buf = poster_map.get(&a.id).and_then(|b| decode_poster_buffer(b));
                (
                    a.id.clone(),
                    a.name.clone(),
                    a.production_year.map(|y| y.to_string()).unwrap_or_default(),
                    a.production_year.unwrap_or(0) as i32,
                    a.user_data.played,
                    a.user_data.is_favorite,
                    a.resume_pct(),
                    a.user_data.unplayed_item_count,
                    buf,
                )
            })
            .collect();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_artist_open_gen() != generation {
                return;
            }
            g.set_artist_meta(meta.as_str().into());
            g.set_artist_overview(
                crate::strip_html_to_text(detail.overview.clone().unwrap_or_default().trim())
                    .into(),
            );
            g.set_artist_is_favorite(detail.user_data.is_favorite);
            let items: Vec<CardItem> = album_decoded
                .into_iter()
                .map(
                    |(id, title, subtitle, year, played, is_fav, rpct, upc, buf)| {
                        let mut h = CardItem {
                            id: id.as_str().into(),
                            item_type: "MusicAlbum".into(),
                            title: title.as_str().into(),
                            subtitle: subtitle.as_str().into(),
                            year,
                            has_played: played,
                            is_favorite: is_fav,
                            resume_pct: rpct,
                            unplayed_count: upc,
                            ..Default::default()
                        };
                        if let Some(spb) = buf {
                            h.poster = slint::Image::from_rgba8(spb);
                            h.has_poster = true;
                        }
                        h
                    },
                )
                .collect();
            g.set_artist_albums(crate::apply_cards_preserving_identity(
                &g.get_artist_albums(),
                items,
            ));
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
            Action::Left => {
                if btn > 0 && btn <= 1 {
                    g.set_artist_btn_focused(btn - 1);
                }
                true
            }
            Action::Right => {
                if btn < 1 {
                    g.set_artist_btn_focused(btn + 1);
                }
                true
            }
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
    let cols = g.get_library_cols();
    let total = g.get_artist_albums().row_count() as i32;
    let f = g.get_artist_focused();

    match action {
        Action::Back => {
            g.invoke_close_artist();
            true
        }
        Action::Right => {
            if f + 1 < total {
                g.set_artist_focused(f + 1);
            }
            true
        }
        Action::Left => {
            if f > 0 {
                g.set_artist_focused(f - 1);
            }
            true
        }
        Action::Down => {
            let next = f + cols;
            if next < total {
                g.set_artist_focused(next);
                true
            } else {
                false
            } // at last row — let focus_bar_on_down handle it
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
            if f < total
                && let Some(card) = g.get_artist_albums().row_data(f as usize)
            {
                g.invoke_open_album(card.id, card.title);
            }
            true
        }
        Action::OpenContextMenu => {
            if f < total
                && let Some(card) = g.get_artist_albums().row_data(f as usize)
            {
                g.set_context_menu_title(card.title.clone());
                g.invoke_open_context_menu(
                    card.id,
                    card.has_played,
                    card.is_favorite,
                    card.resume_pct,
                    card.item_type,
                    card.series_id,
                );
            }
            true
        }
        _ => false,
    }
}

// ── wire_artist (moved from main(), 0.5.0 step 3) ────────────────────────
/// Wires artist screen: open_artist, close_artist, toggle_artist_fav, play_artist_all.
pub(crate) fn wire_artist(
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
    // ── artist screen ─────────────────────────────────────────────────────────
    {
        let state_art = Arc::clone(&state);
        let ww = window.as_weak();
        let rt_handle = rt.handle().clone();
        AppState::get(&window).on_open_artist(move |id, title| {
            artist::open_artist_screen(
                id.to_string(),
                title.to_string(),
                Arc::clone(&state_art),
                ww.clone(),
                rt_handle.clone(),
            );
        });
    }
    {
        let ww_art = window.as_weak();
        AppState::get(&window).on_close_artist(move || {
            if let Some(w) = ww_art.upgrade() {
                AppState::get(&w).set_show_artist(false);
            }
        });
    }
    {
        let state_taf = Arc::clone(&state);
        let ww_taf = window.as_weak();
        // Capture the runtime handle — Handle::current() panics on the Slint
        // event-loop thread because main() never enters the Tokio runtime.
        let rt_taf = rt.handle().clone();
        AppState::get(&window).on_toggle_artist_fav(move || {
            let Some(w) = ww_taf.upgrade() else { return };
            let g = AppState::get(&w);
            let id = g.get_artist_id().to_string();
            let new_fav = !g.get_artist_is_favorite();
            g.set_artist_is_favorite(new_fav);
            let s = state_taf.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else {
                return;
            };
            let ww3 = ww_taf.clone();
            drop(s);
            let rth = rt_taf.clone();
            let state_rf = Arc::clone(&state_taf);
            rt_taf.spawn(async move {
                let result = if new_fav {
                    client.set_favorite(&id).await
                } else {
                    client.unset_favorite(&id).await
                };
                if let Err(e) = result {
                    warn!("toggle_artist_fav: {e}");
                    crate::show_toast(ww3, format!("Favourite error: {e}"));
                    return;
                }
                let ww4 = ww3.clone();
                let id2 = id.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww4.upgrade() {
                        crate::context_menu::update_card_in_all_models(
                            &w,
                            &id2,
                            None,
                            Some(new_fav),
                        );
                    }
                });
                crate::home::refresh_favorites(client, ww3, rth, state_rf);
            });
        });
    }
    {
        let state_paa = Arc::clone(&state);
        let video_paa = Arc::clone(&video);
        let ww_paa = window.as_weak();
        let rt_paa = rt.handle().clone();
        AppState::get(&window).on_play_artist_all(move || {
            let Some(w) = ww_paa.upgrade() else { return };
            let g = AppState::get(&w);
            let albums = g.get_artist_albums();
            if albums.row_count() == 0 {
                return;
            }

            let album_ids: Vec<String> = (0..albums.row_count())
                .filter_map(|i| albums.row_data(i))
                .map(|c| c.id.to_string())
                .collect();
            let artist = g.get_artist_title().to_string();

            let s = state_paa.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else {
                return;
            };
            let mut config = s.player_config();
            config.start_position_secs = None;
            drop(s);

            let video2 = Arc::clone(&video_paa);
            let ww3 = ww_paa.clone();
            let state3 = Arc::clone(&state_paa);

            rt_paa.spawn(async move {
                // Fetch tracks for every album in order; track (id, title, album_id)
                let mut all_tracks: Vec<(String, String, String)> = Vec::new();
                for album_id in &album_ids {
                    if let Ok(tracks) = client.get_album_tracks(album_id).await {
                        for t in tracks {
                            all_tracks.push((t.id, t.name, album_id.clone()));
                        }
                    }
                }
                if all_tracks.is_empty() {
                    return;
                }

                let (first_id, first_title, first_alb_id) = all_tracks[0].clone();
                let first_url = client.direct_play_url(&first_id);
                let rt3 = tokio::runtime::Handle::current();

                let _ = slint::invoke_from_event_loop(move || {
                    {
                        let mut vs = video2.lock().unwrap();
                        // Rebuild the playlist but keep vs.queue — Play All plays
                        // now; previously queued items follow after (Phase 56).
                        vs.playlist.clear();
                        vs.playlist_index = 0;
                        vs.shuffle_order.clear();
                        for (id, title, alb_id) in &all_tracks {
                            vs.playlist.push(crate::playback::QueueItem {
                                id: id.clone(),
                                item_type: "Audio".into(),
                                series_id: None,
                                title: title.clone(),
                                audio_meta: Some((artist.clone(), alb_id.clone())),
                            });
                        }
                        crate::playback::rebuild_shuffle_order(&mut vs);
                        if let Some(w) = ww3.upgrade() {
                            push_queue_display(&vs, &AppState::get(&w));
                        }
                    }
                    start_playback(
                        first_url,
                        first_id,
                        "Audio",
                        first_title,
                        config,
                        client,
                        None,
                        Some((artist, first_alb_id)),
                        &video2,
                        &ww3,
                        &rt3,
                        &state3,
                        None,
                    );
                });
            });
        });
    }
}
