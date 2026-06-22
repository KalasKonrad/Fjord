// ── fjord-app · detail.rs ────────────────────────────────────────────────────
//   DetailCtx           shared context for the three parallel detail fetch tasks
//     spawn_main        fetch item detail, poster, backdrop, cast portraits
//     spawn_similar     fetch similar items row
//     spawn_collection  fetch BoxSet siblings row (retries while movie_collections builds)
//   open_detail      routes by item_type ("Series" → open_series_screen, else detail page);
//                    resets UI state; spawns all three DetailCtx tasks
//   handle_key       keyboard dispatch for the detail page
//   fetch_card_posters  async: parallel poster fetch for a slice of MediaItems; returns pixel buffers
//   items_to_cards      build Vec<CardItem> from items + pre-fetched buffers (call on UI thread)
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

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Fetch poster images for `items` in parallel (up to 6 concurrent).
/// Returns one entry per item; `None` means no image or decode failed.
pub(crate) async fn fetch_card_posters(
    client: &Arc<fjord_api::JellyfinClient>,
    items:  &[fjord_api::models::MediaItem],
) -> Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> {
    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut tasks: JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> = JoinSet::new();
    for (idx, item) in items.iter().enumerate() {
        let c   = client.clone();
        let s   = sem.clone();
        let iid = item.id.clone();
        tasks.spawn(async move {
            let _permit = s.acquire_owned().await.ok();
            let bytes = fetch_poster_cached(&c, &iid).await;
            (idx, bytes.as_deref().and_then(decode_poster_buffer))
        });
    }
    let mut bufs = vec![None; items.len()];
    while let Some(res) = tasks.join_next().await {
        if let Ok((idx, buf)) = res { bufs[idx] = buf; }
    }
    bufs
}

/// Build `Vec<CardItem>` from items + pre-fetched pixel buffers. Call on the UI thread.
pub(crate) fn items_to_cards(
    items: &[fjord_api::models::MediaItem],
    bufs:  Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>>,
) -> Vec<CardItem> {
    items.iter().zip(bufs).map(|(i, buf)| {
        let mut c = CardItem::default();
        c.id             = i.id.as_str().into();
        c.item_type      = i.item_type.as_str().into();
        c.title          = i.name.as_str().into();
        c.year           = i.production_year.unwrap_or(0) as i32;
        c.has_played     = i.user_data.played;
        c.is_favorite    = i.user_data.is_favorite;
        c.resume_pct     = i.resume_pct();
        c.unplayed_count = i.user_data.unplayed_item_count;
        if let Some(spb) = buf {
            c.poster     = slint::Image::from_rgba8(spb);
            c.has_poster = true;
        }
        c
    }).collect()
}

// ── DetailCtx — shared context for the three parallel fetch tasks ─────────────

struct DetailCtx {
    id:     String,
    client: Arc<fjord_api::JellyfinClient>,
    ww:     slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
    state:  Arc<Mutex<FjordState>>,
}

impl DetailCtx {
    fn spawn_main(&self) {
        let id     = self.id.clone();
        let client = Arc::clone(&self.client);
        let ww     = self.ww.clone();
        let rt     = self.rt.clone();
        rt.spawn(async move {
            let detail = match client.get_item_detail(&id).await {
                Ok(d)  => d,
                Err(e) => { warn!("get_item_detail {}: {:#}", id, e); return; }
            };
            debug!("detail fetched: {} | genres={:?} | people={}", detail.name, detail.genres, detail.people.len());

            let poster_bytes   = fetch_poster_cached(&client, &id).await;
            let backdrop_bytes = if detail.backdrop_image_tags.is_empty() {
                None
            } else {
                fetch_backdrop_cached(&client, &id).await
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
            let person_ids: Vec<(usize, String)> = cast_data.iter()
                .enumerate()
                .filter(|(_, (pid, _, _))| !pid.is_empty())
                .map(|(idx, (pid, _, _))| (idx, pid.clone()))
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

            let id_c = id.clone();
            let ww2  = ww.clone();
            slint::invoke_from_event_loop(move || {
                let Some(w) = ww2.upgrade() else { return };
                if AppState::get(&w).get_detail_id().as_str() != id_c { return; }
                let g = AppState::get(&w);
                g.set_detail_title(detail.name.as_str().into());
                g.set_detail_series_label(series_label.as_str().into());
                g.set_detail_series_id(series_id_for_detail.as_str().into());
                g.set_detail_meta(meta.as_str().into());
                g.set_detail_genres(genres.as_str().into());
                g.set_detail_overview(overview.as_str().into());
                g.set_detail_rating_label(rating_label.as_str().into());
                g.set_detail_item_type(detail.item_type.as_str().into());
                g.set_detail_resume_secs(resume_secs as f32);
                g.set_detail_can_resume(resume_secs > 0.0);
                g.set_detail_resume_label(fmt_resume_label(resume_secs).into());
                let cast: Vec<CastMember> = cast_data.into_iter()
                    .map(|(cid, name, role)| CastMember {
                        id:        cid.as_str().into(),
                        name:      name.as_str().into(),
                        role:      role.as_str().into(),
                        photo:     Default::default(),
                        has_photo: false,
                    })
                    .collect();
                g.set_detail_cast(ModelRc::new(VecModel::from(cast)));
                g.set_detail_tagline(tagline.as_str().into());
                g.set_detail_studio(studio.as_str().into());
                g.set_detail_is_favorite(detail.user_data.is_favorite);
                g.set_detail_has_played(detail.user_data.played);
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
            for (model_idx, pid) in person_ids {
                let c2  = client.clone();
                let s2  = sem.clone();
                portrait_tasks.spawn(async move {
                    let _permit = s2.acquire_owned().await.ok();
                    let bytes = fetch_poster_cached(&c2, &pid).await;
                    (model_idx, bytes.as_deref().and_then(decode_poster_buffer))
                });
            }
            while let Some(res) = portrait_tasks.join_next().await {
                let Ok((idx, Some(buf))) = res else { continue };
                let ww_p  = ww.clone();
                let id_p  = id.clone();
                slint::invoke_from_event_loop(move || {
                    let Some(w) = ww_p.upgrade() else { return };
                    if AppState::get(&w).get_detail_id().as_str() != id_p { return; }
                    let cast_model = AppState::get(&w).get_detail_cast();
                    if let Some(mut member) = cast_model.row_data(idx) {
                        member.photo     = slint::Image::from_rgba8(buf);
                        member.has_photo = true;
                        cast_model.set_row_data(idx, member);
                    }
                }).ok();
            }
        });
    }

    fn spawn_similar(&self) {
        let id     = self.id.clone();
        let client = Arc::clone(&self.client);
        let ww     = self.ww.clone();
        self.rt.spawn(async move {
            let similar = match client.get_similar_items(&id).await {
                Ok(v)  => v,
                Err(e) => { warn!("get_similar_items {}: {:#}", id, e); return; }
            };
            if similar.is_empty() { return; }
            let bufs = fetch_card_posters(&client, &similar).await;
            let id_c = id.clone();
            slint::invoke_from_event_loop(move || {
                let Some(w) = ww.upgrade() else { return };
                if AppState::get(&w).get_detail_id().as_str() != id_c { return; }
                AppState::get(&w).set_detail_similar(
                    ModelRc::new(VecModel::from(items_to_cards(&similar, bufs)))
                );
            }).ok();
        });
    }

    fn spawn_collection(&self) {
        let id     = self.id.clone();
        let client = Arc::clone(&self.client);
        let ww     = self.ww.clone();
        let state  = Arc::clone(&self.state);
        self.rt.spawn(async move {
            // movie_collections is populated async after login; retry until the map is built.
            // Both facts (hit + is_empty) are read under a single lock hold to avoid the TOCTOU
            // race where the map could be populated between two separate lock acquisitions.
            let boxset = {
                let mut retries = 0u32;
                loop {
                    let (result, map_empty) = {
                        let s = state.lock().unwrap();
                        (s.movie_collections.get(&id).cloned(), s.movie_collections.is_empty())
                    };
                    if let Some(bs) = result { break Some(bs); }
                    if !map_empty || retries >= 10 { break None; }
                    retries += 1;
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            };
            let Some((bs_id, bs_name)) = boxset else { return };
            let items = match client.get_boxset_items(&bs_id).await {
                Ok(v)  => v,
                Err(e) => { warn!("get_boxset_items {}: {:#}", bs_id, e); return; }
            };
            let items: Vec<_> = items.into_iter().filter(|i| i.id != id).collect();
            if items.is_empty() { return; }
            let bufs  = fetch_card_posters(&client, &items).await;
            let id_c  = id.clone();
            slint::invoke_from_event_loop(move || {
                let Some(w) = ww.upgrade() else { return };
                if AppState::get(&w).get_detail_id().as_str() != id_c { return; }
                let g = AppState::get(&w);
                g.set_detail_collection_title(bs_name.as_str().into());
                g.set_detail_collection(ModelRc::new(VecModel::from(items_to_cards(&items, bufs))));
            }).ok();
        });
    }
}

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
        drop(s);
        open_series_screen(id, state, ww, rt_handle);
        return;
    }
    drop(s);

    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_show_detail(true);
        g.set_detail_id(id.as_str().into());
        w.invoke_grab_keyboard_focus();
        g.set_detail_scroll(0.0);
        g.set_detail_item_type("".into());
        g.set_detail_series_id("".into());
        g.set_detail_loading(true);
        g.set_detail_has_backdrop(false);
        g.set_detail_focused_btn(0);
        g.set_detail_cast(ModelRc::new(VecModel::<CastMember>::default()));
        g.set_detail_focused_row(0);
        g.set_detail_cast_focused(-1);
        g.set_detail_collection_focused(-1);
        g.set_detail_similar_focused(-1);
        g.set_detail_tagline("".into());
        g.set_detail_studio("".into());
        g.set_detail_is_favorite(false);
        g.set_detail_has_played(false);
        g.set_detail_similar(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_detail_collection_title("".into());
        g.set_detail_collection(ModelRc::new(VecModel::<CardItem>::default()));
    }

    let ctx = DetailCtx { id, client, ww, rt: rt_handle, state };
    ctx.spawn_main();
    ctx.spawn_similar();
    ctx.spawn_collection();
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;
    let cast_len = g.get_detail_cast().row_count() as i32;
    let coll_len = g.get_detail_collection().row_count() as i32;
    let sim_len  = g.get_detail_similar().row_count() as i32;
    let row      = g.get_detail_focused_row();
    let bg       = g.get_has_background_player();

    let scroll_for = |r: i32| -> f32 {
        const BASE: f32    = 600.0;
        const SECTION: f32 = 280.0;
        match r {
            0 => 0.0,
            1 => BASE,
            2 => BASE + if cast_len > 0 { SECTION } else { 0.0 },
            3 => BASE + if cast_len > 0 { SECTION } else { 0.0 }
                      + if coll_len > 0 { SECTION } else { 0.0 },
            _ => 0.0,
        }
    };

    match action {
        Action::Back => {
            if bg {
                g.set_playback_from_detail(false);
            }
            g.set_detail_focused_row(0);
            g.set_detail_cast_focused(-1);
            g.set_detail_collection_focused(-1);
            g.set_detail_similar_focused(-1);
            g.set_detail_scroll(0.0);
            g.set_detail_focused_btn(0);
            g.set_detail_collection_title("".into());
            g.set_detail_collection(ModelRc::new(VecModel::<CardItem>::default()));
            g.set_detail_similar(ModelRc::new(VecModel::<CardItem>::default()));
            g.invoke_close_detail();
            true
        }
        Action::Up => {
            match row {
                0 => { g.set_detail_scroll((g.get_detail_scroll() - 120.0).max(0.0)); }
                1 => {
                    g.set_detail_focused_row(0);
                    g.set_detail_cast_focused(-1);
                    g.set_detail_scroll(0.0);
                }
                2 => {
                    g.set_detail_collection_focused(-1);
                    if cast_len > 0 { g.set_detail_focused_row(1); g.set_detail_cast_focused(0); }
                    else             { g.set_detail_focused_row(0); }
                    g.set_detail_scroll(scroll_for(g.get_detail_focused_row()));
                }
                _ => {
                    g.set_detail_similar_focused(-1);
                    if coll_len > 0 {
                        g.set_detail_focused_row(2);
                        g.set_detail_collection_focused(0);
                    } else if cast_len > 0 {
                        g.set_detail_focused_row(1);
                        g.set_detail_cast_focused(0);
                    } else {
                        g.set_detail_focused_row(0);
                    }
                    g.set_detail_scroll(scroll_for(g.get_detail_focused_row()));
                }
            }
            true
        }
        Action::Down => {
            let old_row = row;
            match row {
                0 => {
                    if cast_len > 0 {
                        g.set_detail_focused_row(1); g.set_detail_cast_focused(0);
                    } else if coll_len > 0 {
                        g.set_detail_focused_row(2); g.set_detail_collection_focused(0);
                    } else if sim_len > 0 {
                        g.set_detail_focused_row(3); g.set_detail_similar_focused(0);
                    } else {
                        g.set_detail_scroll(g.get_detail_scroll() + 120.0);
                    }
                }
                1 => {
                    if coll_len > 0 {
                        g.set_detail_cast_focused(-1);
                        g.set_detail_focused_row(2); g.set_detail_collection_focused(0);
                    } else if sim_len > 0 {
                        g.set_detail_cast_focused(-1);
                        g.set_detail_focused_row(3); g.set_detail_similar_focused(0);
                    }
                    // else: nowhere to go; stay in cast row with current focus intact
                }
                2 => {
                    if sim_len > 0 {
                        g.set_detail_focused_row(3);
                        g.set_detail_similar_focused(0);
                        g.set_detail_collection_focused(-1);
                    }
                }
                _ => {} // already at bottom
            }
            if g.get_detail_focused_row() != old_row {
                g.set_detail_scroll(scroll_for(g.get_detail_focused_row()));
            }
            true
        }
        Action::Left | Action::Right => {
            let dir = if *action == Action::Right { 1i32 } else { -1 };
            match row {
                0 => {
                    // Fixed slots: 0=Play, 1=Resume (cond), 2=Series (cond), 3=Fav, 4=Watched
                    let has_resume = g.get_detail_can_resume();
                    let has_series = !g.get_detail_series_id().is_empty();
                    let cur        = g.get_detail_focused_btn();
                    let mut next   = (cur + dir).clamp(0, 4);
                    if next == 1 && !has_resume { next = if dir > 0 { 2 } else { 0 }; }
                    if next == 2 && !has_series { next = if dir > 0 { 3 } else { if has_resume { 1 } else { 0 } }; }
                    g.set_detail_focused_btn(next.clamp(0, 4));
                }
                1 => {
                    let fi = g.get_detail_cast_focused();
                    g.set_detail_cast_focused((fi + dir).clamp(0, cast_len - 1));
                }
                2 => {
                    let fi = g.get_detail_collection_focused();
                    g.set_detail_collection_focused((fi + dir).clamp(0, coll_len - 1));
                }
                3 => {
                    let fi = g.get_detail_similar_focused();
                    g.set_detail_similar_focused((fi + dir).clamp(0, sim_len - 1));
                }
                _ => {}
            }
            true
        }
        Action::Confirm | Action::OpenDetail => {
            match row {
                0 => {
                    match g.get_detail_focused_btn() {
                        1 if g.get_detail_can_resume() => { g.invoke_resume_detail(); }
                        2 if !g.get_detail_series_id().is_empty() => {
                            let sid = g.get_detail_series_id().to_string();
                            g.set_detail_focused_row(0);
                            g.set_detail_cast_focused(-1);
                            g.set_detail_collection_focused(-1);
                            g.set_detail_similar_focused(-1);
                            g.set_detail_scroll(0.0);
                            g.set_detail_focused_btn(0);
                            g.invoke_close_detail();
                            g.invoke_open_series(sid.as_str().into());
                        }
                        3 => { g.invoke_toggle_detail_fav(); }
                        4 => { g.invoke_toggle_detail_played(); }
                        _ => { g.invoke_play_detail(); }
                    }
                }
                1 => {} // cast: no action on Enter
                2 => {
                    let fi = g.get_detail_collection_focused();
                    if fi >= 0 && fi < coll_len {
                        let card = g.get_detail_collection().row_data(fi as usize).unwrap();
                        g.invoke_open_detail(card.id, card.item_type);
                    }
                }
                3 => {
                    let fi = g.get_detail_similar_focused();
                    if fi >= 0 && fi < sim_len {
                        let card = g.get_detail_similar().row_data(fi as usize).unwrap();
                        g.invoke_open_detail(card.id, card.item_type);
                    }
                }
                _ => {}
            }
            true
        }
        Action::OpenContextMenu => {
            match row {
                2 => {
                    let fi = g.get_detail_collection_focused();
                    if fi >= 0 && fi < coll_len {
                        let card = g.get_detail_collection().row_data(fi as usize).unwrap();
                        g.invoke_open_context_menu(card.id, card.has_played, card.is_favorite,
                            card.resume_pct, card.item_type, card.series_id);
                    }
                }
                3 => {
                    let fi = g.get_detail_similar_focused();
                    if fi >= 0 && fi < sim_len {
                        let card = g.get_detail_similar().row_data(fi as usize).unwrap();
                        g.invoke_open_context_menu(card.id, card.has_played, card.is_favorite,
                            card.resume_pct, card.item_type, card.series_id);
                    }
                }
                _ => {}
            }
            true
        }
        Action::ResumePlayer => {
            if bg { g.invoke_resume_player(); }
            else if g.get_detail_can_resume() { g.invoke_resume_detail(); }
            true
        }
        Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
        Action::Quit       => { g.invoke_quit(); true }
        _ => false
    }
}
