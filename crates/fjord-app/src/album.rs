// ── fjord-app · album.rs ──────────────────────────────────────────────────────
//   media_items_to_tracks  MediaItem → TrackItem, incl. multi-disc separators
//                         (2026-08-17): 2-pass — determines whether the album is
//                         genuinely multi-disc (distinct ParentIndexNumber count),
//                         then computes each row's disc_header ("" or "Disc N",
//                         set on a disc's first track only) and row_y_px (absolute
//                         precomputed Y, accounting for every disc-header's own
//                         extra height above it) — see album.slint's own header
//                         for why the Slint side needs these precomputed rather
//                         than iterating the model itself
//   open_album_screen     → open_music_screen(is_playlist=false)
//   open_playlist_screen  → open_music_screen(is_playlist=true): AlbumScreen reuse —
//                         get_playlist_items, position numbering, entry-id per row,
//                         artist line "Playlist", non-audio entries filtered
//   open_music_screen     reset AppState album props; increment album-open-gen; checks
//                         container_tracks_cache + item_detail_cache (Part 2) — only sets
//                         app-content-loading=true when either is a miss; spawn async: fetch
//                         tracks + cover poster in parallel (cached ones skip their network
//                         call); populate TrackItem model; gen-guarded invoke_from_event_loop
//                         shows page
//   handle_key            keyboard dispatch: Back button / ▶+♥ button row / bio (slot 2) / track list;
//                         Up from track 0 → bio (or button row); C → open-context-menu;
//                         Delete → playlist-remove-entry (playlists only);
//                         Enter on track → play-album-track; Down at last track → returns false
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::playback::fmt_secs;
use crate::poster::{decode_poster_buffer, fetch_poster_cached};
use crate::{AppState, MainWindow};

// ── TrackItem helper ──────────────────────────────────────────────────────────

// Must match album.slint's own `track-h`/`sep-h` literals exactly — no shared
// source of truth between the two files (same dual-side-constant caveat this
// codebase already has for e.g. keys.rs's PIN_VALS vs. VirtualKeyboard's own
// key layout). `SEP_H_PX` matches `SectionHeader`'s own fixed 28px height
// (widgets.slint), which the disc-separator label reuses directly.
const TRACK_ROW_H_PX: f32 = 48.0;
const DISC_SEP_H_PX:  f32 = 28.0;

/// Real bug, live-reported 2026-08-17: multi-disc "Play All" order was
/// fixed server-side (fjord-api::get_album_tracks now sorts
/// ParentIndexNumber,IndexNumber), but the tracklist UI itself gave no
/// visual indication a new disc had started — "now it just hop fron the
/// last track to firs with no explenation." Two-pass: first determine
/// whether the album is genuinely multi-disc at all (a plain single-disc
/// album should never show a "Disc 1" label nobody asked for), then walk
/// the already-correctly-ordered items once, computing each row's
/// `disc_header` (only set on the first track of each new disc) and its
/// absolute `row_y_px` — the position it should render at, already
/// accounting for the extra height every disc-header before it adds. Both
/// computed here, not in Slint, so `kb-track-y`'s scroll-to-view math can
/// do a single O(1) indexed model read (`AppState.album-tracks[i].row-y-px`,
/// the same safe `model[idx]` pattern already used elsewhere in this
/// codebase, e.g. home.slint's `library-alpha-offsets[li]`) instead of
/// iterating the model to sum up preceding separators on every keystroke.
fn media_items_to_tracks(
    items:       &[fjord_api::models::MediaItem],
    is_playlist: bool,
) -> Vec<crate::TrackItem> {
    // Playlists have no disc concept — Jellyfin Playlist entries are an
    // arbitrary ordered collection, ParentIndexNumber (if present at all)
    // reflects whatever the source track's own album disc was, which is
    // meaningless in playlist order.
    let multi_disc = !is_playlist && {
        let discs: std::collections::HashSet<u32> =
            items.iter().map(|m| m.parent_index_number.unwrap_or(1)).collect();
        discs.len() > 1
    };

    let mut y = 0.0f32;
    let mut last_disc: Option<u32> = None;
    items
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let duration_secs = m
                .run_time_ticks
                .map(|t| (t / 10_000_000) as f64)
                .unwrap_or(0.0);
            let disc_header = if multi_disc {
                let disc = m.parent_index_number.unwrap_or(1);
                if Some(disc) != last_disc {
                    last_disc = Some(disc);
                    y += DISC_SEP_H_PX;
                    format!("Disc {disc}")
                } else {
                    String::new()
                }
            } else {
                String::new()
            };
            let row_y_px = y;
            y += TRACK_ROW_H_PX;
            crate::TrackItem {
                id:           m.id.as_str().into(),
                title:        m.name.as_str().into(),
                artist:       m.album_artist.as_deref().unwrap_or("").into(),
                duration:     if duration_secs > 0.0 { fmt_secs(duration_secs) } else { "".into() },
                // Playlists show position, not the track's album-side number.
                track_number: if is_playlist { (i + 1) as i32 } else { m.index_number.unwrap_or(0) as i32 },
                has_played:   m.user_data.played,
                is_favorite:  m.user_data.is_favorite,
                resume_pct:   m.resume_pct(),
                entry_id:     m.playlist_item_id.as_deref().unwrap_or("").into(),
                album_id:     m.album_id.as_deref().unwrap_or("").into(),
                disc_header:  disc_header.into(),
                row_y_px,
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
    open_music_screen(id, title, state, ww, rt, false);
}

// Playlist detail reuses the AlbumScreen (single source of truth): position
// numbering, per-row ✕ remove + Delete key, "Playlist" artist line.
pub(crate) fn open_playlist_screen(
    id:    String,
    title: String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    open_music_screen(id, title, state, ww, rt, true);
}

fn open_music_screen(
    id:          String,
    title:       String,
    state:       Arc<Mutex<FjordState>>,
    ww:          slint::Weak<MainWindow>,
    rt:          tokio::runtime::Handle,
    is_playlist: bool,
) {
    // Screen-open cache (Part 2): skip the loading spinner when both the track
    // list and detail are cached — the remaining work (cover-art fetch) is
    // disk-cached and fast enough to feel instant.
    let (client, cached_tracks, cached_detail) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        (c, s.container_tracks_cache.get(&id), s.item_detail_cache.get(&id))
    };
    let is_cache_hit = cached_tracks.is_some() && cached_detail.is_some();
    tracing::debug!("open_music_screen({id}): cache_hit={is_cache_hit}");

    let gen = if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_album_id(id.as_str().into());
        g.set_album_title(title.as_str().into());
        g.set_album_is_playlist(is_playlist);
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
        if !is_cache_hit {
            g.set_app_content_loading(true);
        }
        let next = g.get_album_open_gen() + 1;
        g.set_album_open_gen(next);
        next
    } else {
        -1
    };

    let id2   = id.clone();
    let ww2   = ww.clone();
    let id_revalidate    = id.clone();
    let state_revalidate = Arc::clone(&state);
    let ww_revalidate    = ww.clone();
    let rt_revalidate    = rt.clone();
    let state_task = state;
    rt.spawn(async move {
        let tracks_fut = async {
            if let Some(v) = cached_tracks { return Ok(v); }
            if is_playlist { client.get_playlist_items(&id2).await } else { client.get_album_tracks(&id2).await }
        };
        let detail_fut = async {
            if let Some(d) = cached_detail { return Ok(d); }
            client.get_item_detail(&id2).await
        };
        let (tracks_res, poster_bytes, detail_res) = tokio::join!(
            tracks_fut,
            fetch_poster_cached(&client, &id2),
            detail_fut,
        );
        if let Ok(d) = &detail_res {
            state_task.lock().unwrap().item_detail_cache.insert(id2.clone(), d.clone());
        }

        // Deleted album: the ParentId track query returns an empty 200 — the
        // ghost is only visible on the detail fetch's 404 (S4).
        if let Err(e) = &detail_res {
            if crate::is_not_found(e) {
                let ww_err = ww2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww_err.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_album_open_gen() == gen {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                crate::purge_deleted_item(&state_task, &ww2, &id2);
                return;
            }
        }

        let tracks = match tracks_res {
            Ok(v) => v,
            Err(e) => {
                warn!("open_music_screen tracks fetch({}): {:#}", id2, e);
                let ww_err = ww2.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww_err.upgrade() {
                        let g = AppState::get(&w);
                        if g.get_album_open_gen() == gen {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                let what = if is_playlist { "playlist" } else { "album" };
                crate::show_toast(ww2, format!("Couldn't load {what} — check your server connection"));
                return;
            }
        };
        state_task.lock().unwrap().container_tracks_cache.insert(id2.clone(), tracks.clone());

        // Playlists can contain non-audio entries on mixed servers — drop them.
        let tracks: Vec<_> = if is_playlist {
            tracks.into_iter().filter(|t| t.item_type == "Audio").collect()
        } else {
            tracks
        };
        let track_items = media_items_to_tracks(&tracks, is_playlist);

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_album_open_gen() != gen { return; }
            // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09) —
            // the gen counter alone doesn't catch a sign-out/profile-switch
            // that happens after this screen was backed out of but before
            // this fetch resolves, since nothing increments it on either
            // path. See collection.rs's own open_collection_screen for the
            // full reasoning (same fix, same shape).
            if !crate::session_current(&state_task, &client) { return; }

            if let Ok(d) = &detail_res {
                // Metadata line: year · N tracks · duration (playlists have no year)
                let year = if is_playlist {
                    String::new()
                } else {
                    d.production_year.map(|y| y.to_string()).unwrap_or_default()
                };
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
                g.set_album_artist(if is_playlist { "Playlist".into() }
                                   else { d.album_artist.as_deref().unwrap_or("").into() });
                g.set_album_overview(crate::strip_html_to_text(d.overview.clone().unwrap_or_default().trim()).into());
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
        spawn_album_revalidate(id_revalidate, gen, state_revalidate, ww_revalidate, rt_revalidate, is_playlist);
    }
}

fn spawn_album_revalidate(
    id:          String,
    gen:         i32,
    state:       Arc<Mutex<FjordState>>,
    ww:          slint::Weak<MainWindow>,
    rt:          tokio::runtime::Handle,
    is_playlist: bool,
) {
    if !crate::should_revalidate(&state, &id) { return; }
    let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
    rt.spawn(async move {
        let tracks_res = if is_playlist { client.get_playlist_items(&id).await } else { client.get_album_tracks(&id).await };
        let detail_res = client.get_item_detail(&id).await;
        let (Ok(tracks), Ok(detail)) = (tracks_res, detail_res) else { return };
        // Sign-out (or a different account signing in on a shared HTPC)
        // mid-fetch must not let this per-user data land in the new session's
        // cache — same guard class as main.rs::session_current's own doc
        // comment (CR11-2).
        if !crate::session_current(&state, &client) { return; }
        {
            let mut s = state.lock().unwrap();
            s.container_tracks_cache.insert(id.clone(), tracks.clone());
            s.item_detail_cache.insert(id.clone(), detail.clone());
        }
        let tracks: Vec<_> = if is_playlist {
            tracks.into_iter().filter(|t| t.item_type == "Audio").collect()
        } else {
            tracks
        };
        let track_items = media_items_to_tracks(&tracks, is_playlist);
        let year = if is_playlist { String::new() } else { detail.production_year.map(|y| y.to_string()).unwrap_or_default() };
        let n_tracks = track_items.len();
        let total_secs: f64 = tracks.iter().filter_map(|t| t.run_time_ticks).map(|t| (t / 10_000_000) as f64).sum();
        let meta = if year.is_empty() {
            format!("{} tracks · {}", n_tracks, fmt_secs(total_secs))
        } else {
            format!("{} · {} tracks · {}", year, n_tracks, fmt_secs(total_secs))
        };
        let artist = if is_playlist { "Playlist".to_string() } else { detail.album_artist.clone().unwrap_or_default() };
        let overview = crate::strip_html_to_text(detail.overview.clone().unwrap_or_default().trim());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_album_open_gen() != gen { return; }
            g.set_album_meta(meta.as_str().into());
            g.set_album_artist(artist.as_str().into());
            g.set_album_overview(overview.as_str().into());
            g.set_album_is_favorite(detail.user_data.is_favorite);
            g.set_album_has_played(detail.user_data.played);
            g.set_album_tracks(ModelRc::new(VecModel::from(track_items)));
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

    // ── ▶ Play All / ♥ button row + bio focused (0=Play All, 1=♥, 2=bio) ─────
    let btn = g.get_album_btn_focused();
    if btn >= 0 {
        return match action {
            Action::Left  => { if btn > 0 && btn <= 1 { g.set_album_btn_focused(btn - 1); } true }
            Action::Right => { if btn < 1             { g.set_album_btn_focused(btn + 1); } true }
            Action::Confirm => {
                match btn {
                    0 => g.invoke_play_album_all(),
                    1 => g.invoke_toggle_album_fav(),
                    _ => g.set_album_overview_expanded(!g.get_album_overview_expanded()),
                }
                true
            }
            Action::Up => {
                if btn == 2 {
                    g.set_album_btn_focused(0); // bio → ▶ Play All
                } else {
                    g.set_album_btn_focused(-1);
                    g.set_album_back_focused(true);
                }
                true
            }
            Action::Down => {
                if btn <= 1 && !g.get_album_overview().is_empty() {
                    g.set_album_btn_focused(2); // buttons → bio
                } else {
                    g.set_album_btn_focused(-1);
                    g.set_album_focused_track(0);
                }
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
            } else if !g.get_album_overview().is_empty() {
                g.set_album_btn_focused(2); // tracks → bio (sits between header and list)
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
        Action::DeleteItem => {
            if g.get_album_is_playlist() && f < len {
                g.invoke_playlist_remove_entry(f);
            }
            true
        }
        Action::OpenContextMenu => {
            if f < len {
                let track = g.get_album_tracks().row_data(f as usize).unwrap();
                g.set_context_menu_title(track.title.clone());
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
