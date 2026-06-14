// ── fjord-app · movies.rs ────────────────────────────────────────────────────
//   spawn_movies_poster_loading  parallel poster fetch for all movies → AppState.all-movies
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;

use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, ModelRc, SharedString, VecModel};

use crate::AppState;
use crate::poster::{fetch_poster_cached, decode_poster_buffer};
use crate::{CardItem, MainWindow};

pub(crate) fn spawn_movies_poster_loading(
    client:      Arc<JellyfinClient>,
    movies:      Vec<MediaItem>,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    rt_handle.spawn(async move {
        use std::collections::HashSet;
        use std::sync::Arc as SArc;

        let meta: Vec<(String, String, i32, bool, bool, f32)> = movies.iter()
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
                let _permit = sem.acquire_owned().await.ok();
                let bytes   = fetch_poster_cached(&*client, &id).await.map(SArc::new);
                (id, bytes)
            });
        }

        let mut poster_map: std::collections::HashMap<String, SArc<Vec<u8>>> = Default::default();

        while let Some(res) = fetch_set.join_next().await {
            let Ok((id, bytes)) = res else { continue };
            if let Some(b) = bytes { poster_map.insert(id.clone(), b); }
            pending.remove(&id);
            if !pending.is_empty() { continue; }

            type Buf = slint::SharedPixelBuffer<slint::Rgba8Pixel>;
            let decoded: Vec<(SharedString, SharedString, i32, bool, bool, f32, Option<Buf>)> =
                meta.iter().map(|(cid, title, year, played, is_fav, rpct)| {
                    let buf = poster_map.get(cid).and_then(|b| decode_poster_buffer(b));
                    (SharedString::from(cid.as_str()), SharedString::from(title.as_str()), *year, *played, *is_fav, *rpct, buf)
                }).collect();
            let ww = window_weak.clone();
            let _ = slint::invoke_from_event_loop(move || {
                if let Some(w) = ww.upgrade() {
                    let items: Vec<CardItem> = decoded.into_iter().map(|(id, title, year, played, is_fav, rpct, buf)| {
                        let mut h = CardItem::default();
                        h.id          = id;
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
                    g.set_all_movies(model.clone());
                    if g.get_show_library() && g.get_library_query().is_empty() {
                        g.set_library_display(model);
                    }
                }
            });
        }
    });
}
