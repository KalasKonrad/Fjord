// ── fjord-app · movies.rs ────────────────────────────────────────────────────
//   LibraryKind                        Movies | Collections | Artists | Albums enum
//   spawn_library_poster_loading       shared async: parallel poster fetch → AppState model
//   spawn_movies_poster_loading        thin wrapper → LibraryKind::Movies
//   spawn_collections_poster_loading   thin wrapper → LibraryKind::Collections
//   spawn_artists_poster_loading       thin wrapper → LibraryKind::Artists
//   spawn_albums_poster_loading        thin wrapper → LibraryKind::Albums
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;

use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, ModelRc, SharedString, VecModel};

use crate::AppState;
use crate::poster::{fetch_poster_cached, decode_poster_buffer};
use crate::{CardItem, MainWindow};

#[derive(Copy, Clone)]
enum LibraryKind {
    Movies,
    Collections,
    Artists,
    Albums,
}

impl LibraryKind {
    fn item_type(self) -> &'static str {
        match self {
            Self::Movies      => "Movie",
            Self::Collections => "BoxSet",
            Self::Artists     => "MusicArtist",
            Self::Albums      => "MusicAlbum",
        }
    }
    fn active_nav(self) -> i32 {
        match self {
            Self::Movies      => 2,
            Self::Collections => 3,
            Self::Artists     => 4,
            Self::Albums      => 4,
        }
    }
    fn set_all(self, g: &AppState, model: ModelRc<CardItem>) {
        match self {
            Self::Movies      => g.set_all_movies(model),
            Self::Collections => g.set_all_collections(model),
            Self::Artists     => g.set_all_artists(model),
            Self::Albums      => g.set_all_albums(model),
        }
    }
    // For Albums/Artists, only overwrite library-display when the current music view matches.
    fn matches_library_display(self, g: &AppState) -> bool {
        match self {
            Self::Artists => g.get_library_music_view() == 0,
            Self::Albums  => g.get_library_music_view() == 1,
            _             => true,
        }
    }
}

// Build decoded cards and push them to AppState from the Slint event loop.
// Called at both the normal completion point and the panic-flush fallback.
fn push_library_cards(
    decoded:     Vec<(SharedString, SharedString, i32, bool, bool, f32, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)>,
    kind:        LibraryKind,
    window_weak: slint::Weak<MainWindow>,
) {
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = window_weak.upgrade() else { return };
        let items: Vec<CardItem> = decoded.into_iter().map(|(id, title, year, played, is_fav, rpct, buf)| {
            let mut h = CardItem::default();
            h.id          = id;
            h.item_type   = kind.item_type().into();
            h.title       = title;
            h.year        = year;
            h.has_played  = played;
            h.is_favorite = is_fav;
            h.resume_pct  = rpct;
            if let Some(spb) = buf { h.poster = slint::Image::from_rgba8(spb); h.has_poster = true; }
            h
        }).collect();
        let model = ModelRc::new(VecModel::from(items));
        let g = AppState::get(&w);
        kind.set_all(&g, model.clone());
        if g.get_show_library() && g.get_active_nav() == kind.active_nav()
           && g.get_library_query().is_empty() && kind.matches_library_display(&g) {
            g.set_library_display(model);
        }
    });
}

// ── spawn_library_poster_loading ──────────────────────────────────────────────

fn spawn_library_poster_loading(
    client:      Arc<JellyfinClient>,
    items:       Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
    kind:        LibraryKind,
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

        let meta: Vec<(String, String, i32, bool, bool, f32)> = items.iter()
            .map(|i| (i.id.clone(), i.display_name(), i.production_year.unwrap_or(0) as i32, i.user_data.played, i.user_data.is_favorite, i.resume_pct()))
            .collect();
        let mut pending: HashSet<String> = meta.iter().map(|(id, _, _, _, _, _)| id.clone()).collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut fetch_set: tokio::task::JoinSet<(String, Option<SArc<Vec<u8>>>)> =
            tokio::task::JoinSet::new();
        for (id, _, _, _, _, _) in &meta {
            let client = Arc::clone(&client);
            let sem    = Arc::clone(&sem);
            let id     = id.clone();
            fetch_set.spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else { return (id, None) };
                let bytes = fetch_poster_cached(&*client, &id).await.map(SArc::new);
                (id, bytes)
            });
        }

        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();

        while let Some(res) = fetch_set.join_next().await {
            let (id, bytes) = match res {
                Ok(pair) => pair,
                Err(e) => { tracing::warn!("{} poster task panicked: {e}", kind.item_type()); continue; }
            };
            if let Some(b) = bytes { poster_map.insert(id.clone(), b); }
            pending.remove(&id);
            if !pending.is_empty() { continue; }

            type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
            let decoded: Vec<(SharedString, SharedString, i32, bool, bool, f32, Option<Buf>)> =
                meta.iter().map(|(cid, title, year, played, is_fav, rpct)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (SharedString::from(cid.as_str()), SharedString::from(title.as_str()), *year, *played, *is_fav, *rpct, buf)
                }).collect();
            push_library_cards(decoded, kind, window_weak.clone());
        }

        // Post-loop flush: push with partial results if tasks panicked.
        if !pending.is_empty() {
            tracing::warn!("{} poster: {} item(s) never resolved — pushing partial results", kind.item_type(), pending.len());
            type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
            let decoded: Vec<(SharedString, SharedString, i32, bool, bool, f32, Option<Buf>)> =
                meta.iter().map(|(cid, title, year, played, is_fav, rpct)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (SharedString::from(cid.as_str()), SharedString::from(title.as_str()), *year, *played, *is_fav, *rpct, buf)
                }).collect();
            push_library_cards(decoded, kind, window_weak.clone());
        }
    });
}

pub(crate) fn spawn_movies_poster_loading(
    client:      Arc<JellyfinClient>,
    movies:      Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    spawn_library_poster_loading(client, movies, window_weak, rt_handle, LibraryKind::Movies);
}

pub(crate) fn spawn_collections_poster_loading(
    client:      Arc<JellyfinClient>,
    cols:        Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    spawn_library_poster_loading(client, cols, window_weak, rt_handle, LibraryKind::Collections);
}

pub(crate) fn spawn_artists_poster_loading(
    client:      Arc<JellyfinClient>,
    artists:     Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    spawn_library_poster_loading(client, artists, window_weak, rt_handle, LibraryKind::Artists);
}

pub(crate) fn spawn_albums_poster_loading(
    client:      Arc<JellyfinClient>,
    albums:      Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    spawn_library_poster_loading(client, albums, window_weak, rt_handle, LibraryKind::Albums);
}
