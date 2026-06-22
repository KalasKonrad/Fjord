// ── fjord-app · series.rs ────────────────────────────────────────────────────
//   ep_to_card              MediaItem (Episode) → CardItem (title "S01E02 · Title")
//   spawn_episode_thumb_loading  parallel episode thumbnail fetch → series-episode-cards
//   SeriesCtx               shared context for background fetch tasks
//     spawn_main    fetch detail+seasons+first-eps in parallel; push metadata,
//                   cast, episode cards; fetch cast portraits; call spawn_next_up/spawn_similar
//     spawn_next_up fetch next unwatched episode for this series; set series-has-next-up + thumb
//     spawn_similar fetch similar series; push series-similar SectionRow
//   open_series_screen      reset AppState, build SeriesCtx, spawn_main
//   handle_key              keyboard dispatch for the series screen
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, Model, ModelRc, VecModel};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::config::FjordState;
use crate::AppState;
use crate::detail::{fetch_card_posters, items_to_cards};
use crate::poster::{fetch_poster_cached, fetch_backdrop_cached, decode_poster_buffer};
use crate::{CardItem, CastMember, SeasonEntry, MainWindow};

// ── ep_to_card ────────────────────────────────────────────────────────────────

pub(crate) fn ep_to_card(ep: &MediaItem) -> CardItem {
    let s   = ep.parent_index_number.unwrap_or(0);
    let e   = ep.index_number.unwrap_or(0);
    let lbl = if s > 0 || e > 0 {
        format!("S{:02}E{:02} · {}", s, e, ep.name)
    } else {
        ep.name.clone()
    };
    let resume_pct = if let Some(ticks) = ep.run_time_ticks {
        if ticks > 0 {
            (ep.user_data.playback_position_ticks as f32 / ticks as f32).clamp(0.0, 1.0)
        } else { 0.0 }
    } else { 0.0 };
    let series_id = ep.series_id.clone().unwrap_or_default();
    CardItem {
        id:             ep.id.as_str().into(),
        series_id:      series_id.as_str().into(),
        item_type:      "Episode".into(),
        title:          lbl.as_str().into(),
        year:           ep.production_year.unwrap_or(0) as i32,
        has_played:     ep.user_data.played,
        is_favorite:    ep.user_data.is_favorite,
        resume_pct,
        has_poster:     false,
        poster:         Default::default(),
        unplayed_count: 0,
    }
}

// ── spawn_episode_thumb_loading ───────────────────────────────────────────────

pub(crate) fn spawn_episode_thumb_loading(
    client:      Arc<JellyfinClient>,
    episodes:    Vec<MediaItem>,
    series_id:   String,
    window_weak: slint::Weak<MainWindow>,
    rt_handle:   tokio::runtime::Handle,
) {
    if episodes.is_empty() { return; }
    rt_handle.spawn(async move {
        let sem = Arc::new(tokio::sync::Semaphore::new(6));
        let mut tasks: JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> =
            JoinSet::new();
        for (idx, ep) in episodes.iter().enumerate() {
            let c2  = Arc::clone(&client);
            let s2  = Arc::clone(&sem);
            let id  = ep.id.clone();
            tasks.spawn(async move {
                let _permit = s2.acquire_owned().await.ok();
                let bytes = fetch_poster_cached(&*c2, &id).await;
                (idx, bytes.as_deref().and_then(|b| decode_poster_buffer(b)))
            });
        }
        while let Some(res) = tasks.join_next().await {
            let Ok((idx, Some(buf))) = res else { continue };
            let ww  = window_weak.clone();
            let sid = series_id.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww.upgrade() else { return };
                if AppState::get(&w).get_series_id().as_str() != sid { return; }
                let model = AppState::get(&w).get_series_episode_cards();
                if let Some(mut card) = model.row_data(idx) {
                    card.poster     = slint::Image::from_rgba8(buf);
                    card.has_poster = true;
                    model.set_row_data(idx, card);
                }
            });
        }
    });
}

// ── SeriesCtx ─────────────────────────────────────────────────────────────────

struct SeriesCtx {
    id:     String,
    client: Arc<JellyfinClient>,
    ww:     slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
    state:  Arc<Mutex<FjordState>>,
}

impl SeriesCtx {
    fn spawn_main(&self) {
        let id     = self.id.clone();
        let client = Arc::clone(&self.client);
        let ww_ui  = self.ww.clone();
        let ww_ep  = self.ww.clone();
        let ww_cas = self.ww.clone();
        let state  = Arc::clone(&self.state);
        let rth    = self.rt.clone();
        self.rt.spawn(async move {
            let (detail_res, poster_bytes, seasons_res) = tokio::join!(
                client.get_item_detail(&id),
                fetch_poster_cached(&client, &id),
                client.get_seasons(&id),
            );
            let backdrop_bytes = match &detail_res {
                Ok(d) if !d.backdrop_image_tags.is_empty() => fetch_backdrop_cached(&client, &id).await,
                _ => None,
            };
            let seasons = seasons_res.unwrap_or_else(|e| { warn!("get_seasons {}: {:#}", id, e); vec![] });
            debug!("series {} — {} season(s)", id, seasons.len());

            let season_ids: Vec<String> = seasons.iter().map(|s| s.id.clone()).collect();
            {
                let mut s = state.lock().unwrap();
                s.series_open_id    = id.clone();
                s.series_season_ids = season_ids;
                s.series_episode_cache.clear();
                s.series_season_generation = 0;
            }

            let first_season_id = seasons.first().map(|s| s.id.clone());
            let first_eps = if let Some(ref fid) = first_season_id {
                client.get_season_episodes(&id, fid).await.unwrap_or_else(|e| {
                    warn!("get_season_episodes {} {}: {:#}", id, fid, e);
                    vec![]
                })
            } else { vec![] };
            debug!("series {} season 0 — {} episode(s)", id, first_eps.len());
            {
                let mut s = state.lock().unwrap();
                s.series_episode_items = first_eps.clone();
                if let Some(fid) = first_season_id {
                    s.series_episode_cache.insert(fid, first_eps.clone());
                }
            }

            // Build metadata from detail response.
            let detail_name     = detail_res.as_ref().map(|d| d.name.clone()).ok().unwrap_or_default();
            let detail_overview = detail_res.as_ref().ok().and_then(|d| d.overview.clone()).unwrap_or_default();

            // Extended metadata only when detail fetch succeeded.
            let (meta, genres, rating_label, cast_data) = if let Ok(ref d) = detail_res {
                let mut meta_parts: Vec<String> = vec![];
                if let Some(y) = d.production_year { meta_parts.push(y.to_string()); }
                if let Some(ref r) = d.official_rating { meta_parts.push(r.clone()); }
                if let Some(ref rt_str) = d.runtime_string() { meta_parts.push(rt_str.clone()); }
                let meta = meta_parts.join(" · ");
                let genres = d.genres.join(", ");
                let rating = d.community_rating.map(|r| format!("★ {:.1}", r)).unwrap_or_default();

                let mut seen: std::collections::HashSet<String> = Default::default();
                let mut cast: Vec<(String, String, String)> = vec![];
                for p in d.people.iter().filter(|p| p.person_type == "Director").take(2) {
                    if seen.insert(p.id.clone()) {
                        cast.push((p.id.clone(), p.name.clone(), "Director".to_string()));
                    }
                }
                for p in d.people.iter().filter(|p| p.person_type == "Writer").take(3) {
                    if seen.insert(p.id.clone()) {
                        cast.push((p.id.clone(), p.name.clone(), "Writer".to_string()));
                    }
                }
                for p in d.people.iter().filter(|p| p.person_type == "Actor").take(12) {
                    if seen.insert(p.id.clone()) {
                        cast.push((p.id.clone(), p.name.clone(), p.role.clone()));
                    }
                }
                (meta, genres, rating, cast)
            } else {
                (String::new(), String::new(), String::new(), vec![])
            };

            let person_ids: Vec<(usize, String)> = cast_data.iter()
                .enumerate()
                .filter(|(_, (pid, _, _))| !pid.is_empty())
                .map(|(idx, (pid, _, _))| (idx, pid.clone()))
                .collect();

            let season_entries: Vec<SeasonEntry> = seasons.iter()
                .map(|s| SeasonEntry { id: s.id.as_str().into(), name: s.name.as_str().into() })
                .collect();
            // Pass Vec<MediaItem> (Send) into the closure; build Vec<CardItem> (!Send) inside.
            let eps_for_cards = first_eps.clone();

            let id_guard = id.clone();
            let id_cast  = id.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww_ui.upgrade() else { return };
                if AppState::get(&w).get_series_id().as_str() != id_guard { return; }
                let g = AppState::get(&w);
                if !detail_name.is_empty()     { g.set_series_title(detail_name.as_str().into()); }
                if !detail_overview.is_empty() { g.set_series_overview(detail_overview.as_str().into()); }
                g.set_series_meta(meta.as_str().into());
                g.set_series_genres(genres.as_str().into());
                g.set_series_rating_label(rating_label.as_str().into());
                g.set_series_seasons(ModelRc::new(VecModel::from(season_entries)));
                let ep_cards: Vec<CardItem> = eps_for_cards.iter().map(ep_to_card).collect();
                g.set_series_episode_cards(ModelRc::new(VecModel::from(ep_cards)));
                g.set_series_loading(false);
                let cast_members: Vec<CastMember> = cast_data.into_iter()
                    .map(|(cid, name, role)| CastMember {
                        id:        cid.as_str().into(),
                        name:      name.as_str().into(),
                        role:      role.as_str().into(),
                        photo:     Default::default(),
                        has_photo: false,
                    })
                    .collect();
                g.set_series_cast(ModelRc::new(VecModel::from(cast_members)));
                if let Some(buf) = poster_bytes.as_deref().and_then(decode_poster_buffer) {
                    g.set_series_poster(slint::Image::from_rgba8(buf));
                    g.set_series_has_poster(true);
                }
                if let Some(buf) = backdrop_bytes.as_deref().and_then(decode_poster_buffer) {
                    g.set_series_backdrop(slint::Image::from_rgba8(buf));
                    g.set_series_has_backdrop(true);
                }
            });

            // Portrait fetches — cast model queued ahead in the event loop.
            let sem = Arc::new(tokio::sync::Semaphore::new(6));
            let mut portrait_tasks: JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> =
                JoinSet::new();
            for (model_idx, pid) in person_ids {
                let c2 = client.clone();
                let s2 = sem.clone();
                portrait_tasks.spawn(async move {
                    let _permit = s2.acquire_owned().await.ok();
                    let bytes = fetch_poster_cached(&c2, &pid).await;
                    (model_idx, bytes.as_deref().and_then(decode_poster_buffer))
                });
            }
            while let Some(res) = portrait_tasks.join_next().await {
                let Ok((idx, Some(buf))) = res else { continue };
                let ww_p = ww_cas.clone();
                let id_p = id_cast.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww_p.upgrade() else { return };
                    if AppState::get(&w).get_series_id().as_str() != id_p { return; }
                    let cast_model = AppState::get(&w).get_series_cast();
                    if let Some(mut member) = cast_model.row_data(idx) {
                        member.photo     = slint::Image::from_rgba8(buf);
                        member.has_photo = true;
                        cast_model.set_row_data(idx, member);
                    }
                });
            }

            spawn_episode_thumb_loading(client, first_eps, id, ww_ep, rth);
        });
    }

    fn spawn_next_up(&self) {
        let id     = self.id.clone();
        let client = Arc::clone(&self.client);
        let ww     = self.ww.clone();
        self.rt.spawn(async move {
            let ep = match client.get_next_up_for_series(&id).await {
                Ok(Some(ep)) => ep,
                Ok(None)     => return,
                Err(e)       => { warn!("get_next_up_for_series {}: {:#}", id, e); return; }
            };
            let thumb_bytes = fetch_poster_cached(&client, &ep.id).await;
            let ep_id     = ep.id.clone();
            let ep_title  = {
                let s = ep.parent_index_number.unwrap_or(0);
                let e = ep.index_number.unwrap_or(0);
                if s > 0 || e > 0 { format!("S{:02}E{:02} · {}", s, e, ep.name) } else { ep.name.clone() }
            };
            let resume_pct = if let Some(ticks) = ep.run_time_ticks {
                if ticks > 0 {
                    (ep.user_data.playback_position_ticks as f32 / ticks as f32).clamp(0.0, 1.0)
                } else { 0.0 }
            } else { 0.0 };
            let has_played = ep.user_data.played;
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww.upgrade() else { return };
                if AppState::get(&w).get_series_id().as_str() != id { return; }
                let g = AppState::get(&w);
                g.set_series_has_next_up(true);
                g.set_series_next_up_id(ep_id.as_str().into());
                g.set_series_next_up_title(ep_title.as_str().into());
                g.set_series_next_up_resume_pct(resume_pct);
                g.set_series_next_up_has_played(has_played);
                if let Some(buf) = thumb_bytes.as_deref().and_then(decode_poster_buffer) {
                    g.set_series_next_up_thumb(slint::Image::from_rgba8(buf));
                    g.set_series_next_up_has_thumb(true);
                }
            });
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
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww.upgrade() else { return };
                if AppState::get(&w).get_series_id().as_str() != id { return; }
                AppState::get(&w).set_series_similar(
                    ModelRc::new(VecModel::from(items_to_cards(&similar, bufs)))
                );
            });
        });
    }
}

// ── open_series_screen ────────────────────────────────────────────────────────

pub(crate) fn open_series_screen(
    id:        String,
    state:     Arc<Mutex<FjordState>>,
    ww:        slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    let s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
    let basic = s.all_series.iter().find(|i| i.id == id).cloned();
    drop(s);

    info!("open_series: id={} name={:?}", id, basic.as_ref().map(|i| i.name.as_str()));

    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_show_series(true);
        w.invoke_grab_keyboard_focus();
        g.set_series_id(id.as_str().into());
        g.set_series_loading(true);
        g.set_series_in_season_row(true);      // start with season tabs focused
        g.set_series_next_up_focused(false);
        g.set_series_season_idx(0);
        g.set_series_focused_ep(0);
        g.set_series_seasons(ModelRc::new(VecModel::<SeasonEntry>::default()));
        g.set_series_episode_cards(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_series_has_backdrop(false);
        g.set_series_has_poster(false);
        g.set_series_meta("".into());
        g.set_series_genres("".into());
        g.set_series_rating_label("".into());
        g.set_series_cast(ModelRc::new(VecModel::<CastMember>::default()));
        g.set_series_cast_focused(-1);
        g.set_series_similar(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_series_similar_focused(-1);
        g.set_series_has_next_up(false);
        g.set_series_next_up_id("".into());
        g.set_series_next_up_title("".into());
        g.set_series_next_up_resume_pct(0.0);
        g.set_series_next_up_has_played(false);
        g.set_series_next_up_has_thumb(false);
        if let Some(ref item) = basic {
            g.set_series_title(item.name.as_str().into());
            g.set_series_overview(item.overview.clone().unwrap_or_default().as_str().into());
        }
    }

    let ctx = SeriesCtx { id: id.clone(), client: client.clone(), ww: ww.clone(), rt: rt_handle.clone(), state: Arc::clone(&state) };
    ctx.spawn_main();
    let ctx_nu = SeriesCtx { id: id.clone(), client: client.clone(), ww: ww.clone(), rt: rt_handle.clone(), state: Arc::clone(&state) };
    ctx_nu.spawn_next_up();
    let ctx_si = SeriesCtx { id, client, ww, rt: rt_handle, state };
    ctx_si.spawn_similar();
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &crate::AppState) -> bool {
    use crate::keys::Action;
    if *action == Action::Back {
        g.set_series_cast_focused(-1);
        g.set_series_similar_focused(-1);
        g.set_series_next_up_focused(false);
        g.invoke_close_series();
        return true;
    }

    // ── Next Up row ───────────────────────────────────────────────────────────
    if g.get_series_next_up_focused() {
        return match action {
            Action::Down => {
                g.set_series_next_up_focused(false);
                g.set_series_in_season_row(true);
                true
            }
            Action::Up => true, // already at the top
            Action::Confirm => {
                g.invoke_play_series_episode(g.get_series_next_up_id());
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }

    // ── Season row ────────────────────────────────────────────────────────────
    if g.get_series_in_season_row() {
        return match action {
            Action::Left => {
                let idx = g.get_series_season_idx();
                if idx > 0 {
                    g.set_series_season_idx(idx - 1);
                    g.invoke_series_select_season(idx - 1);
                    g.set_series_focused_ep(0);
                }
                true
            }
            Action::Right => {
                let idx = g.get_series_season_idx();
                if idx < g.get_series_seasons().row_count() as i32 - 1 {
                    g.set_series_season_idx(idx + 1);
                    g.invoke_series_select_season(idx + 1);
                    g.set_series_focused_ep(0);
                }
                true
            }
            Action::Up => {
                if g.get_series_has_next_up() {
                    g.set_series_in_season_row(false);
                    g.set_series_next_up_focused(true);
                }
                true
            }
            Action::Down => {
                g.set_series_in_season_row(false);
                true
            }
            // Enter or I on a season tab → open season detail page
            Action::Confirm | Action::OpenDetail => {
                let idx = g.get_series_season_idx() as usize;
                if let Some(season) = g.get_series_seasons().row_data(idx) {
                    g.invoke_open_season_detail(season.id, g.get_series_id());
                }
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }

    // Derive which row we're in from the existing state properties.
    let in_cast    = g.get_series_cast_focused()    >= 0;
    let in_similar = g.get_series_similar_focused() >= 0;

    // ── Cast row ─────────────────────────────────────────────────────────────
    if in_cast {
        return match action {
            Action::Left => {
                let idx = g.get_series_cast_focused();
                if idx > 0 { g.set_series_cast_focused(idx - 1); }
                true
            }
            Action::Right => {
                let idx = g.get_series_cast_focused();
                if idx < g.get_series_cast().row_count() as i32 - 1 {
                    g.set_series_cast_focused(idx + 1);
                }
                true
            }
            Action::Up => {
                g.set_series_cast_focused(-1);   // back to episode row
                true
            }
            Action::Down => {
                if g.get_series_similar().row_count() > 0 {
                    g.set_series_cast_focused(-1);
                    g.set_series_similar_focused(0);
                }
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }

    // ── More Like This (similar) row ──────────────────────────────────────────
    if in_similar {
        return match action {
            Action::Left => {
                let idx = g.get_series_similar_focused();
                if idx > 0 { g.set_series_similar_focused(idx - 1); }
                true
            }
            Action::Right => {
                let idx = g.get_series_similar_focused();
                if idx < g.get_series_similar().row_count() as i32 - 1 {
                    g.set_series_similar_focused(idx + 1);
                }
                true
            }
            Action::Up => {
                g.set_series_similar_focused(-1);
                if g.get_series_cast().row_count() > 0 {
                    g.set_series_cast_focused(0);  // back up to cast row
                }
                true
            }
            Action::Confirm => {
                let idx = g.get_series_similar_focused() as usize;
                if let Some(card) = g.get_series_similar().row_data(idx) {
                    g.invoke_open_detail(card.id, card.item_type);
                }
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }

    // ── Episode row ───────────────────────────────────────────────────────────
    match action {
        Action::Left => {
            let ep = g.get_series_focused_ep();
            if ep > 0 { g.set_series_focused_ep(ep - 1); }
            true
        }
        Action::Right => {
            let ep  = g.get_series_focused_ep();
            let max = g.get_series_episode_cards().row_count() as i32 - 1;
            if ep < max { g.set_series_focused_ep(ep + 1); }
            true
        }
        Action::Up => {
            g.set_series_in_season_row(true);
            true
        }
        Action::Down => {
            if g.get_series_cast().row_count() > 0 {
                g.set_series_cast_focused(0);
            } else if g.get_series_similar().row_count() > 0 {
                g.set_series_similar_focused(0);
            }
            true
        }
        Action::Confirm => {
            let cards = g.get_series_episode_cards();
            if cards.row_count() > 0 {
                if let Some(card) = cards.row_data(g.get_series_focused_ep() as usize) {
                    g.invoke_play_series_episode(card.id);
                }
            }
            true
        }
        Action::OpenDetail => {
            let cards = g.get_series_episode_cards();
            if cards.row_count() > 0 {
                if let Some(card) = cards.row_data(g.get_series_focused_ep() as usize) {
                    g.invoke_open_detail(card.id, "Episode".into());
                }
            }
            true
        }
        Action::OpenContextMenu => {
            let cards = g.get_series_episode_cards();
            if cards.row_count() > 0 {
                if let Some(card) = cards.row_data(g.get_series_focused_ep() as usize) {
                    g.invoke_open_context_menu(
                        card.id, card.has_played, card.is_favorite, card.resume_pct,
                        card.item_type, card.series_id,
                    );
                }
            }
            true
        }
        Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
        Action::Quit       => { g.invoke_quit(); true }
        _ => false
    }
}
