// ── fjord-app · detail.rs ────────────────────────────────────────────────────
//   open_detail  routes by item_type ("Series" → open_series_screen, else detail page);
//                fetches item detail, poster, backdrop, similar items, collection row (parallel);
//                extracts crew+cast (Director/Writer/Actor) with portrait photos, tagline, studio;
//                collection row: lookup in FjordState.movie_collections; retries if map not yet built;
//                populates detail-series-id (Episodes → "Series →" button)
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tokio::task::JoinSet;
use tracing::{debug, warn};

use crate::config::{FjordState, fmt_resume_label};
use crate::AppState;
use crate::poster::{decode_poster_buffer, fetch_backdrop_cached, fetch_poster_cached};
use crate::series::open_series_screen;
use crate::{CardItem, CastMember, MainWindow};

pub(crate) fn open_detail(
    id:        String,
    item_type: String,
    state:     Arc<Mutex<FjordState>>,
    ww:        slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    let s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };

    if item_type == "Series" {
        let state3 = state.clone();
        let ww3    = ww.clone();
        let rth3   = rt_handle.clone();
        drop(s);
        open_series_screen(id, state3, ww3, rth3);
        return;
    }

    drop(s);

    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_show_detail(true);
        g.set_detail_id(id.as_str().into());
        w.invoke_grab_keyboard_focus();
        g.set_detail_series_id("".into());
        g.set_detail_loading(true);
        g.set_detail_has_backdrop(false);
        g.set_detail_focused_btn(0);
        g.set_detail_cast(ModelRc::new(VecModel::<CastMember>::default()));
        g.set_detail_cast_focused(-1);
        g.set_detail_tagline("".into());
        g.set_detail_studio("".into());
        g.set_detail_similar(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_detail_collection_title("".into());
        g.set_detail_collection(ModelRc::new(VecModel::<CardItem>::default()));
    }

    // ── Main detail task ──────────────────────────────────────────────────────
    let id2        = id.clone();
    let ww2        = ww.clone();
    let client2    = client.clone();
    rt_handle.spawn(async move {
        let detail = match client2.get_item_detail(&id2).await {
            Ok(d)  => d,
            Err(e) => { warn!("get_item_detail {}: {:#}", id2, e); return; }
        };
        debug!("detail fetched: {} | genres={:?} | people={}", detail.name, detail.genres, detail.people.len());

        let poster_bytes   = fetch_poster_cached(&client2, &id2).await;
        let backdrop_bytes = if detail.backdrop_image_tags.is_empty() {
            None
        } else {
            fetch_backdrop_cached(&client2, &id2).await
        };

        // Build crew+cast as (id, name, role_label) — directors first, writers, then actors.
        // Vec<CastMember> is !Send because image is !Send so we carry raw tuples here.
        let mut seen_ids: std::collections::HashSet<String> = Default::default();
        let mut cast_data: Vec<(String, String, String)> = vec![];
        for p in detail.people.iter().filter(|p| p.person_type == "Director").take(2) {
            if seen_ids.insert(p.id.clone()) {
                cast_data.push((p.id.clone(), p.name.clone(), "Director".to_string()));
            }
        }
        for p in detail.people.iter().filter(|p| p.person_type == "Writer").take(3) {
            if seen_ids.insert(p.id.clone()) {
                cast_data.push((p.id.clone(), p.name.clone(), "Writer".to_string()));
            }
        }
        for p in detail.people.iter().filter(|p| p.person_type == "Actor").take(12) {
            if seen_ids.insert(p.id.clone()) {
                cast_data.push((p.id.clone(), p.name.clone(), p.role.clone()));
            }
        }
        // (model_row_index, person_id) — only entries with non-empty IDs
        let person_ids: Vec<(usize, String)> = cast_data.iter()
            .enumerate()
            .filter(|(_, (id, _, _))| !id.is_empty())
            .map(|(idx, (id, _, _))| (idx, id.clone()))
            .collect();

        let tagline = detail.taglines.first().cloned().unwrap_or_default();
        let studio  = detail.studios.first().map(|s| s.name.clone()).unwrap_or_default();

        let mut meta_parts: Vec<String> = vec![];
        if let Some(y) = detail.production_year { meta_parts.push(y.to_string()); }
        if let Some(ref r) = detail.official_rating { meta_parts.push(r.clone()); }
        if let Some(ref rt_str) = detail.runtime_string() { meta_parts.push(rt_str.clone()); }
        let meta = meta_parts.join(" • ");

        let genres       = detail.genres.join(", ");
        let overview     = detail.overview.clone().unwrap_or_default();
        let rating_label = detail.community_rating
            .map(|r| format!("★ {:.1}", r))
            .unwrap_or_default();
        let (series_label, series_id_for_detail) = if detail.item_type == "Episode" {
            let s      = detail.parent_index_number.unwrap_or(0);
            let e      = detail.index_number.unwrap_or(0);
            let series = detail.series_name.as_deref().unwrap_or("");
            let label  = format!("{} — S{:02}E{:02}", series, s, e);
            let sid    = detail.series_id.clone().unwrap_or_default();
            (label, sid)
        } else {
            (String::new(), String::new())
        };
        let resume_secs = detail.resume_position_secs().unwrap_or(0.0);

        let id2c = id2.clone();
        let ww3  = ww2.clone();
        slint::invoke_from_event_loop(move || {
            let Some(w) = ww3.upgrade() else { return };
            if AppState::get(&w).get_detail_id().as_str() != id2c { return; }

            let g = AppState::get(&w);
            g.set_detail_title(detail.name.as_str().into());
            g.set_detail_series_label(series_label.as_str().into());
            g.set_detail_series_id(series_id_for_detail.as_str().into());
            g.set_detail_meta(meta.as_str().into());
            g.set_detail_genres(genres.as_str().into());
            g.set_detail_overview(overview.as_str().into());
            g.set_detail_rating_label(rating_label.as_str().into());
            g.set_detail_can_resume(resume_secs > 0.0);
            g.set_detail_resume_label(fmt_resume_label(resume_secs).into());
            let cast: Vec<CastMember> = cast_data.into_iter()
                .map(|(id, name, role)| CastMember {
                    id:        id.as_str().into(),
                    name:      name.as_str().into(),
                    role:      role.as_str().into(),
                    photo:     Default::default(),
                    has_photo: false,
                })
                .collect();
            g.set_detail_cast(ModelRc::new(VecModel::from(cast)));
            g.set_detail_tagline(tagline.as_str().into());
            g.set_detail_studio(studio.as_str().into());
            g.set_detail_loading(false);

            if let Some(bytes) = poster_bytes {
                if let Some(buf) = decode_poster_buffer(&bytes) {
                    g.set_detail_poster(slint::Image::from_rgba8(buf));
                    g.set_detail_has_poster(true);
                }
            }
            if let Some(bytes) = backdrop_bytes {
                if let Some(buf) = decode_poster_buffer(&bytes) {
                    g.set_detail_backdrop(slint::Image::from_rgba8(buf));
                    g.set_detail_has_backdrop(true);
                }
            }
        }).ok();

        // Portrait fetches — cast model is queued ahead in the event loop
        let sem = Arc::new(tokio::sync::Semaphore::new(6));
        let mut portrait_tasks: JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> = JoinSet::new();
        for (model_idx, pid) in person_ids.into_iter() {
            let client_p = client2.clone();
            let sem_p    = sem.clone();
            portrait_tasks.spawn(async move {
                let _permit = sem_p.acquire_owned().await.ok();
                let bytes = fetch_poster_cached(&client_p, &pid).await;
                (model_idx, bytes.as_deref().and_then(decode_poster_buffer))
            });
        }
        while let Some(res) = portrait_tasks.join_next().await {
            let Ok((idx, Some(buf))) = res else { continue };
            let ww_p  = ww2.clone();
            let id2_p = id2.clone();
            slint::invoke_from_event_loop(move || {
                let Some(w) = ww_p.upgrade() else { return };
                if AppState::get(&w).get_detail_id().as_str() != id2_p { return; }
                let cast_model = AppState::get(&w).get_detail_cast();
                if let Some(mut member) = cast_model.row_data(idx) {
                    member.photo     = slint::Image::from_rgba8(buf);
                    member.has_photo = true;
                    cast_model.set_row_data(idx, member);
                }
            }).ok();
        }
    });

    // ── Similar items task (independent of main task) ─────────────────────────
    let id_sim      = id.clone();
    let ww_sim      = ww.clone();
    let client_sim  = client.clone();
    rt_handle.spawn(async move {
        let similar = match client_sim.get_similar_items(&id_sim).await {
            Ok(v)  => v,
            Err(e) => { warn!("get_similar_items {}: {:#}", id_sim, e); return; }
        };
        if similar.is_empty() { return; }

        let meta: Vec<(String, String, String, i32, bool, bool, f32, i32)> = similar.iter()
            .map(|i| (i.id.clone(), i.item_type.clone(), i.name.clone(),
                      i.production_year.unwrap_or(0) as i32,
                      i.user_data.played, i.user_data.is_favorite,
                      i.resume_pct(), i.user_data.unplayed_item_count))
            .collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(6));
        let mut tasks: JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> = JoinSet::new();
        for (idx, item) in similar.iter().enumerate() {
            let client_s = client_sim.clone();
            let sem_s    = sem.clone();
            let iid      = item.id.clone();
            tasks.spawn(async move {
                let _permit = sem_s.acquire_owned().await.ok();
                let bytes = fetch_poster_cached(&client_s, &iid).await;
                (idx, bytes.as_deref().and_then(decode_poster_buffer))
            });
        }

        let mut bufs: Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> = vec![None; similar.len()];
        while let Some(res) = tasks.join_next().await {
            if let Ok((idx, buf)) = res { bufs[idx] = buf; }
        }

        let id_sim_c = id_sim.clone();
        slint::invoke_from_event_loop(move || {
            let Some(w) = ww_sim.upgrade() else { return };
            if AppState::get(&w).get_detail_id().as_str() != id_sim_c { return; }
            let items: Vec<CardItem> = meta.into_iter().zip(bufs)
                .map(|((id, itype, title, year, played, is_fav, rpct, upc), buf)| {
                    let mut c = CardItem::default();
                    c.id             = id.as_str().into();
                    c.item_type      = itype.as_str().into();
                    c.title          = title.as_str().into();
                    c.year           = year;
                    c.has_played     = played;
                    c.is_favorite    = is_fav;
                    c.resume_pct     = rpct;
                    c.unplayed_count = upc;
                    if let Some(spb) = buf {
                        c.poster     = slint::Image::from_rgba8(spb);
                        c.has_poster = true;
                    }
                    c
                }).collect();
            AppState::get(&w).set_detail_similar(ModelRc::new(VecModel::from(items)));
        }).ok();
    });

    // ── Collection row task (independent; retries map lookup if still building at call time) ──
    {
        let movie_id = id.clone();
        let ww_bs    = ww.clone();
        let client_b = client;
        let state_bs = state.clone();
        rt_handle.spawn(async move {
            // movie_collections is populated async after login; retry until map is built.
            let boxset = {
                let mut retries = 0u32;
                loop {
                    let result = state_bs.lock().unwrap().movie_collections.get(&movie_id).cloned();
                    if let Some(bs) = result {
                        break Some(bs);
                    }
                    let map_empty = state_bs.lock().unwrap().movie_collections.is_empty();
                    if !map_empty || retries >= 10 {
                        break None;
                    }
                    retries += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            };
            let Some((bs_id, bs_name)) = boxset else { return };

            let items = match client_b.get_boxset_items(&bs_id).await {
                Ok(v)  => v,
                Err(e) => { warn!("get_boxset_items {}: {:#}", bs_id, e); return; }
            };
            // Exclude the movie currently being viewed
            let items: Vec<_> = items.into_iter().filter(|i| i.id != movie_id).collect();
            if items.is_empty() { return; }

            let meta: Vec<(String, String, String, i32, bool, bool, f32, i32)> = items.iter()
                .map(|i| (i.id.clone(), i.item_type.clone(), i.name.clone(),
                          i.production_year.unwrap_or(0) as i32,
                          i.user_data.played, i.user_data.is_favorite,
                          i.resume_pct(), i.user_data.unplayed_item_count))
                .collect();

            let sem = Arc::new(tokio::sync::Semaphore::new(6));
            let mut tasks: JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> = JoinSet::new();
            for (idx, item) in items.iter().enumerate() {
                let client_p = client_b.clone();
                let sem_p    = sem.clone();
                let iid      = item.id.clone();
                tasks.spawn(async move {
                    let _permit = sem_p.acquire_owned().await.ok();
                    let bytes = fetch_poster_cached(&client_p, &iid).await;
                    (idx, bytes.as_deref().and_then(decode_poster_buffer))
                });
            }
            let mut bufs: Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> = vec![None; items.len()];
            while let Some(res) = tasks.join_next().await {
                if let Ok((idx, buf)) = res { bufs[idx] = buf; }
            }

            let movie_id_c = movie_id.clone();
            slint::invoke_from_event_loop(move || {
                let Some(w) = ww_bs.upgrade() else { return };
                if AppState::get(&w).get_detail_id().as_str() != movie_id_c { return; }
                let card_items: Vec<CardItem> = meta.into_iter().zip(bufs)
                    .map(|((id, itype, title, year, played, is_fav, rpct, upc), buf)| {
                        let mut c = CardItem::default();
                        c.id             = id.as_str().into();
                        c.item_type      = itype.as_str().into();
                        c.title          = title.as_str().into();
                        c.year           = year;
                        c.has_played     = played;
                        c.is_favorite    = is_fav;
                        c.resume_pct     = rpct;
                        c.unplayed_count = upc;
                        if let Some(spb) = buf {
                            c.poster     = slint::Image::from_rgba8(spb);
                            c.has_poster = true;
                        }
                        c
                    }).collect();
                let g = AppState::get(&w);
                g.set_detail_collection_title(bs_name.as_str().into());
                g.set_detail_collection(ModelRc::new(VecModel::from(card_items)));
            }).ok();
        });
    }
}
