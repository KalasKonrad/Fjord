// ── fjord-app · movies.rs ────────────────────────────────────────────────────
//   LibraryKind                        Movies | Collections | Artists | Albums | Playlists enum;
//                                      get_all/set_all accessors used by apply_cards_preserving_identity
//   push_library_cards                 build decoded cards, apply via apply_cards_preserving_identity;
//                                      reuses each row's existing poster Image if present instead of
//                                      always decoding a new one (Phase 98 — avoids swapping every
//                                      card's texture in one batch even when same_shape=true); routes
//                                      the library-display update through browse::refresh_library_display
//                                      instead of a direct set (Phase 99 — a direct set used raw
//                                      network/decode order, not the sorted order); threads
//                                      unplayed_count through meta/decoded (Phase 100 — was left at
//                                      CardItem::default()'s 0, stomping the metadata-merge pass's
//                                      correct value on the next poster-decode landing); carries
//                                      on_watchlist forward from the old row too (2026-07-20, real
//                                      bug fix, same idiom as the poster preservation right above —
//                                      this is the Library Grid's exclusive landing point for Movies,
//                                      so without this a resync/toggle's live patch onto all_movies
//                                      got silently wiped by the next network refresh)
//   spawn_library_poster_loading       shared async: parallel poster fetch → AppState model
//   spawn_movies_poster_loading        thin wrapper → LibraryKind::Movies
//   spawn_collections_poster_loading   thin wrapper → LibraryKind::Collections
//   spawn_artists_poster_loading       thin wrapper → LibraryKind::Artists
//   spawn_albums_poster_loading        thin wrapper → LibraryKind::Albums
//   spawn_playlists_poster_loading     thin wrapper → LibraryKind::Playlists
//   wire_library           callbacks moved from main() (0.5.0 step 3): lazy library grid + Artists/Albums toggle
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;

use fjord_api::{JellyfinClient, models::MediaItem};
use slint::{Global, Model, ModelRc, SharedString, VecModel};

use crate::AppState;
use crate::poster::{decode_poster_buffer, fetch_poster_cached_tagged};
use crate::{CardItem, MainWindow};

// (id, title, subtitle, year, played, is_favorite, resume_pct, unplayed_count) — raw
// item metadata, pre-poster-decode.
type CardMeta = (String, String, String, i32, bool, bool, f32, i32);
// CardMeta's fields, Slint-ready (SharedString) plus the decoded poster buffer
// (None when that item has no poster or decode failed).
type DecodedCard = (
    SharedString,
    SharedString,
    SharedString,
    i32,
    bool,
    bool,
    f32,
    i32,
    Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
);

#[derive(Copy, Clone)]
enum LibraryKind {
    Movies,
    Collections,
    Artists,
    Albums,
    Playlists,
}

impl LibraryKind {
    fn item_type(self) -> &'static str {
        match self {
            Self::Movies => "Movie",
            Self::Collections => "BoxSet",
            Self::Artists => "MusicArtist",
            Self::Albums => "MusicAlbum",
            Self::Playlists => "Playlist",
        }
    }
    fn active_nav(self) -> i32 {
        match self {
            Self::Movies => 2,
            Self::Collections => 3,
            Self::Artists => 4,
            Self::Albums => 4,
            Self::Playlists => 4,
        }
    }
    fn set_all(self, g: &AppState, model: ModelRc<CardItem>) {
        match self {
            Self::Movies => g.set_all_movies(model),
            Self::Collections => g.set_all_collections(model),
            Self::Artists => g.set_all_artists(model),
            Self::Albums => g.set_all_albums(model),
            Self::Playlists => g.set_all_playlists(model),
        }
    }
    fn get_all(self, g: &AppState) -> ModelRc<CardItem> {
        match self {
            Self::Movies => g.get_all_movies(),
            Self::Collections => g.get_all_collections(),
            Self::Artists => g.get_all_artists(),
            Self::Albums => g.get_all_albums(),
            Self::Playlists => g.get_all_playlists(),
        }
    }
    // For Albums/Artists, only overwrite library-display when the current music view matches.
    fn matches_library_display(self, g: &AppState) -> bool {
        match self {
            Self::Artists => g.get_library_music_view() == 0,
            Self::Albums => g.get_library_music_view() == 1,
            Self::Playlists => g.get_library_music_view() == 2,
            _ => true,
        }
    }
}

// Build decoded cards and push them to AppState from the Slint event loop.
// Called at both the normal completion point and the panic-flush fallback.
fn push_library_cards(
    decoded: Vec<DecodedCard>,
    kind: LibraryKind,
    window_weak: slint::Weak<MainWindow>,
) {
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = window_weak.upgrade() else {
            return;
        };
        let g = AppState::get(&w);
        let old = kind.get_all(&g);
        // Prefer whatever poster the row already has over a freshly-decoded one,
        // even when the new decode succeeded: apply_cards_preserving_identity only
        // avoids destroying/recreating card elements when ids/order match, but a
        // *different* (even if pixel-identical) slint::Image handle still means
        // swapping the texture for every card in the batch — visibly disruptive
        // for a large grid even without any element recreation. Only a row that
        // never had a poster yet takes the fresh decode.
        let old_by_id: std::collections::HashMap<String, CardItem> = (0..old.row_count())
            .filter_map(|i| old.row_data(i))
            .map(|c| (c.id.to_string(), c))
            .collect();
        let items: Vec<CardItem> = decoded
            .into_iter()
            .map(
                |(id, title, subtitle, year, played, is_fav, rpct, upc, buf)| {
                    let mut h = CardItem::default();
                    let existing_poster = old_by_id
                        .get(id.as_str())
                        .filter(|c| c.has_poster)
                        .map(|c| c.poster.clone());
                    let existing_watchlist = old_by_id.get(id.as_str()).map(|c| c.on_watchlist);
                    h.id = id;
                    h.item_type = kind.item_type().into();
                    h.title = title;
                    h.subtitle = subtitle;
                    h.year = year;
                    h.has_played = played;
                    h.is_favorite = is_fav;
                    h.resume_pct = rpct;
                    h.unplayed_count = upc;
                    if let Some(poster) = existing_poster {
                        h.poster = poster;
                        h.has_poster = true;
                    } else if let Some(spb) = buf {
                        h.poster = slint::Image::from_rgba8(spb);
                        h.has_poster = true;
                    }
                    // Carry forward on_watchlist the same way the poster is carried
                    // forward above — this function has no FjordState access (pure
                    // Slint-model merge), so a live patch from
                    // resync_jellyfin_watchlist_stars/discover_toggle_watchlist onto
                    // this row survives the next rebuild instead of silently
                    // resetting to false (real bug, live-reported 2026-07-20 — "the
                    // watch list symbol do not show up on items i the library
                    // screens"; the Library Grid's Movies view is built exclusively
                    // through this function).
                    if let Some(on_watchlist) = existing_watchlist {
                        h.on_watchlist = on_watchlist;
                    }
                    h
                },
            )
            .collect();
        tracing::debug!(
            "push_library_cards[{}]: applying {} card(s)",
            kind.item_type(),
            items.len()
        );
        let model = crate::apply_cards_preserving_identity(&old, items);
        kind.set_all(&g, model);
        // Route through refresh_library_display (like poster.rs::push_decoded_series
        // does for TV) instead of setting library-display directly to `model` — model
        // is in raw network/decode order, not the user's chosen sort. Setting it
        // directly showed items in that raw order first, then any later call to
        // refresh_library_display (e.g. the library-search-clear a NavItem double-click
        // fires) would re-sort by title and visibly reshuffle — a flash TV never had
        // because it already went through the sorted path from the start.
        if g.get_show_library()
            && g.get_active_nav() == kind.active_nav()
            && g.get_library_query().is_empty()
            && kind.matches_library_display(&g)
        {
            crate::browse::refresh_library_display(&w);
        }
    });
}

// ── spawn_library_poster_loading ──────────────────────────────────────────────

fn spawn_library_poster_loading(
    client: Arc<JellyfinClient>,
    items: Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
    kind: LibraryKind,
) {
    rt_handle.spawn(async move {
        use std::collections::HashSet;
        use std::sync::Arc as SArc;

        if items.is_empty() {
            let ww = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    kind.set_all(&AppState::get(&w), ModelRc::new(VecModel::default()));
                }
            });
            return;
        }

        let meta: Vec<CardMeta> = items
            .iter()
            .map(|i| {
                (
                    i.id.clone(),
                    i.card_title(),
                    i.card_subtitle(),
                    i.production_year.unwrap_or(0) as i32,
                    i.user_data.played,
                    i.user_data.is_favorite,
                    i.resume_pct(),
                    i.user_data.unplayed_item_count,
                )
            })
            .collect();
        let mut pending: HashSet<String> = meta
            .iter()
            .map(|(id, _, _, _, _, _, _, _)| id.clone())
            .collect();
        // id → primary image tag for artwork revalidation.
        let tags: std::collections::HashMap<String, String> = items
            .iter()
            .filter_map(|i| i.primary_image_tag().map(|t| (i.id.clone(), t.to_string())))
            .collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for (id, _, _, _, _, _, _, _) in &meta {
            let client = Arc::clone(&client);
            let sem = Arc::clone(&sem);
            let id = id.clone();
            let tag = tags.get(&id).cloned();
            fetch_set.spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else {
                    return (id, None);
                };
                let bytes = fetch_poster_cached_tagged(&client, &id, tag.as_deref())
                    .await
                    .map(SArc::new);
                (id, bytes)
            });
        }

        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();

        while let Some(res) = fetch_set.join_next().await {
            let (id, bytes) = match res {
                Ok(pair) => pair,
                Err(e) => {
                    tracing::warn!("{} poster task panicked: {e}", kind.item_type());
                    continue;
                }
            };
            if let Some(b) = bytes {
                poster_map.insert(id.clone(), b);
            }
            pending.remove(&id);
            if !pending.is_empty() {
                continue;
            }

            let decoded: Vec<DecodedCard> = meta
                .iter()
                .map(|(cid, title, subtitle, year, played, is_fav, rpct, upc)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (
                        SharedString::from(cid.as_str()),
                        SharedString::from(title.as_str()),
                        SharedString::from(subtitle.as_str()),
                        *year,
                        *played,
                        *is_fav,
                        *rpct,
                        *upc,
                        buf,
                    )
                })
                .collect();
            push_library_cards(decoded, kind, window_weak.clone());
        }

        // Post-loop flush: push with partial results if tasks panicked.
        if !pending.is_empty() {
            tracing::warn!(
                "{} poster: {} item(s) never resolved — pushing partial results",
                kind.item_type(),
                pending.len()
            );
            let decoded: Vec<DecodedCard> = meta
                .iter()
                .map(|(cid, title, subtitle, year, played, is_fav, rpct, upc)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (
                        SharedString::from(cid.as_str()),
                        SharedString::from(title.as_str()),
                        SharedString::from(subtitle.as_str()),
                        *year,
                        *played,
                        *is_fav,
                        *rpct,
                        *upc,
                        buf,
                    )
                })
                .collect();
            push_library_cards(decoded, kind, window_weak.clone());
        }
    });
}

pub(crate) fn spawn_movies_poster_loading(
    client: Arc<JellyfinClient>,
    movies: Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    spawn_library_poster_loading(client, movies, window_weak, rt_handle, LibraryKind::Movies);
}

pub(crate) fn spawn_collections_poster_loading(
    client: Arc<JellyfinClient>,
    cols: Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    spawn_library_poster_loading(
        client,
        cols,
        window_weak,
        rt_handle,
        LibraryKind::Collections,
    );
}

pub(crate) fn spawn_artists_poster_loading(
    client: Arc<JellyfinClient>,
    artists: Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    spawn_library_poster_loading(
        client,
        artists,
        window_weak,
        rt_handle,
        LibraryKind::Artists,
    );
}

pub(crate) fn spawn_albums_poster_loading(
    client: Arc<JellyfinClient>,
    albums: Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    spawn_library_poster_loading(client, albums, window_weak, rt_handle, LibraryKind::Albums);
}

pub(crate) fn spawn_playlists_poster_loading(
    client: Arc<JellyfinClient>,
    playlists: Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    spawn_library_poster_loading(
        client,
        playlists,
        window_weak,
        rt_handle,
        LibraryKind::Playlists,
    );
}

// ── wire_library (moved from main(), 0.5.0 step 3) ───────────────────────
/// Wires lazy library grid + Artists/Albums toggle: open_library, library_music_view_changed.
pub(crate) fn wire_library(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── lazy library grid ─────────────────────────────────────────────────────
    {
        let state_ol = Arc::clone(&state);
        let ww_ol = window.as_weak();
        let rth_ol = rt.handle().clone();
        AppState::get(&window).on_open_library(move |nav| {
            // Synchronously initialise sort/filter/query for this library type before any async work.
            {
                let sort_val = {
                    let s = state_ol.lock().unwrap();
                    let cp = s.config.active();
                    match nav {
                        1 => cp.library_series_sort,
                        2 => cp.library_movies_sort,
                        3 => cp.library_collections_sort,
                        4 => match cp.library_music_view {
                            1 => cp.library_albums_sort,
                            2 => cp.library_playlists_sort,
                            _ => cp.library_artists_sort,
                        },
                        _ => 0,
                    }
                };
                if let Some(w) = ww_ol.upgrade() {
                    let g = AppState::get(&w);
                    g.set_library_sort(sort_val as i32);
                    g.set_library_filter_unwatched(false);
                    g.set_library_filter_favorites(false);
                    g.set_library_query("".into());
                    g.set_library_sort_cursor(0);
                    g.set_library_back_focused(false);
                    g.set_library_has_filters(nav != 3 && nav != 4);
                    if nav == 4 {
                        let music_view =
                            state_ol.lock().unwrap().config.active().library_music_view as i32;
                        g.set_library_music_view(music_view);
                    }
                    browse::refresh_library_display(&w);
                }
            }
            spawn_library_fetch(nav, Arc::clone(&state_ol), ww_ol.clone(), rth_ol.clone());
        });
    }

    // ── music library view toggle (Artists ↔ Albums) ─────────────────────────
    {
        let state_mv = Arc::clone(&state);
        let ww_mv = window.as_weak();
        let rth_mv = rt.handle().clone();
        AppState::get(&window).on_library_music_view_changed(move |view| {
            {
                let mut s = state_mv.lock().unwrap();
                let cp = s.config.active_mut();
                cp.library_music_view = view.clamp(0, 2) as u8;
                // Restore the correct sort for the new view.
                let sort_val = match view {
                    1 => cp.library_albums_sort,
                    2 => cp.library_playlists_sort,
                    _ => cp.library_artists_sort,
                };
                let cfg = s.config.clone();
                drop(s);
                crate::config::save_config(&cfg);
                if let Some(w) = ww_mv.upgrade() {
                    let g = AppState::get(&w);
                    g.set_library_music_view(view);
                    g.set_library_sort(sort_val as i32);
                    g.set_library_focused(0);
                    browse::refresh_library_display(&w);
                }
            }
            // Trigger a fetch of the other data source if not yet done.
            let (need_fetch, already_fetched) = {
                let s = state_mv.lock().unwrap();
                match view {
                    1 => (!s.albums_fetched, s.albums_fetched),
                    2 => (!s.playlists_fetched, s.playlists_fetched),
                    _ => (!s.artists_fetched, s.artists_fetched),
                }
            };
            let _ = already_fetched; // suppress unused warning
            if need_fetch {
                let state_f = Arc::clone(&state_mv);
                let ww_f = ww_mv.clone();
                let ww_f2 = ww_mv.clone();
                let Some(client) = state_mv.lock().unwrap().client.as_ref().map(Arc::clone) else {
                    return;
                };
                let client2 = Arc::clone(&client);
                let rth_spawn = rth_mv.clone();
                rth_mv.spawn(async move {
                    let user_id = client.user_id.clone();
                    if view == 2 {
                        match client.get_all_playlists().await {
                            Ok(playlists) => {
                                {
                                    let mut s = state_f.lock().unwrap();
                                    s.all_playlists = playlists.clone();
                                    s.playlists_fetched = true;
                                }
                                save_playlists_cache(&user_id, &playlists);
                                let playlists2 = playlists.clone();
                                let ww_p = ww_f.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = ww_p.upgrade() {
                                        AppState::get(&w).set_all_playlists(items_to_model(
                                            &playlists2,
                                            &std::collections::HashSet::new(),
                                        ));
                                        if AppState::get(&w).get_show_library()
                                            && AppState::get(&w).get_library_music_view() == 2
                                        {
                                            browse::refresh_library_display(&w);
                                        }
                                    }
                                });
                                spawn_playlists_poster_loading(
                                    client2,
                                    playlists,
                                    ww_f2,
                                    rth_spawn.clone(),
                                );
                            }
                            Err(e) => warn!("music view playlists fetch: {:#}", e),
                        }
                    } else if view == 1 {
                        match client.get_all_albums().await {
                            Ok(albums) => {
                                {
                                    let mut s = state_f.lock().unwrap();
                                    s.all_albums = albums.clone();
                                    s.albums_fetched = true;
                                }
                                save_albums_cache(&user_id, &albums);
                                let albums2 = albums.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = ww_f.upgrade() {
                                        AppState::get(&w).set_all_albums(items_to_model(
                                            &albums2,
                                            &std::collections::HashSet::new(),
                                        ));
                                        if AppState::get(&w).get_show_library()
                                            && AppState::get(&w).get_library_music_view() == 1
                                        {
                                            browse::refresh_library_display(&w);
                                        }
                                    }
                                });
                                spawn_albums_poster_loading(
                                    client2,
                                    albums,
                                    ww_f2,
                                    rth_spawn.clone(),
                                );
                            }
                            Err(e) => warn!("music view albums fetch: {:#}", e),
                        }
                    } else {
                        match client.get_album_artists().await {
                            Ok(artists) => {
                                {
                                    let mut s = state_f.lock().unwrap();
                                    s.all_artists = artists.clone();
                                    s.artists_fetched = true;
                                }
                                save_artists_cache(&user_id, &artists);
                                let artists2 = artists.clone();
                                let _ = slint::invoke_from_event_loop(move || {
                                    if let Some(w) = ww_f.upgrade() {
                                        AppState::get(&w).set_all_artists(items_to_model(
                                            &artists2,
                                            &std::collections::HashSet::new(),
                                        ));
                                        if AppState::get(&w).get_show_library()
                                            && AppState::get(&w).get_library_music_view() == 0
                                        {
                                            browse::refresh_library_display(&w);
                                        }
                                    }
                                });
                                spawn_artists_poster_loading(
                                    client2,
                                    artists,
                                    ww_f2,
                                    rth_spawn.clone(),
                                );
                            }
                            Err(e) => warn!("music view artists fetch: {:#}", e),
                        }
                    }
                });
            }
        });
    }
}
