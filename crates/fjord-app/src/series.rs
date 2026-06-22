// ── fjord-app · series.rs ────────────────────────────────────────────────────
//   EpisodeRaw              intermediate episode data before Slint EpisodeEntry
//   make_episode_raw        MediaItem → EpisodeRaw (resume_pct, runtime, etc.)
//   raw_to_entry            EpisodeRaw → Slint EpisodeEntry (no image yet)
//   spawn_episode_thumb_loading  parallel episode thumbnail fetch → series model
//   SeriesCtx               shared context for the background fetch task
//     spawn_main            fetch seasons + first-season episodes + poster + backdrop
//   open_series_screen      reset AppState, build SeriesCtx, spawn_main
//   handle_key              keyboard dispatch for the series screen
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, Model, ModelRc, VecModel};
use tracing::{debug, info, warn};

use crate::config::FjordState;
use crate::AppState;
use crate::poster::{fetch_poster_cached, fetch_backdrop_cached, decode_poster_buffer};
use crate::{EpisodeEntry, SeasonEntry, MainWindow};

pub(crate) struct EpisodeRaw {
    pub id:          String,
    pub title:       String,
    pub ep_num:      i32,
    pub season_num:  i32,
    pub overview:    String,
    pub has_played:  bool,
    pub is_favorite: bool,
    pub resume_pct:  f32,
    pub runtime:     String,
}

pub(crate) fn make_episode_raw(ep: &MediaItem) -> EpisodeRaw {
    let resume_pct = if let Some(ticks) = ep.run_time_ticks {
        let pos = ep.user_data.playback_position_ticks;
        if ticks > 0 { (pos as f32 / ticks as f32).clamp(0.0, 1.0) } else { 0.0 }
    } else { 0.0 };
    EpisodeRaw {
        id:         ep.id.clone(),
        title:      ep.name.clone(),
        ep_num:     ep.index_number.unwrap_or(0) as i32,
        season_num: ep.parent_index_number.unwrap_or(0) as i32,
        overview:   ep.overview.clone().unwrap_or_default(),
        has_played:  ep.user_data.played,
        is_favorite: ep.user_data.is_favorite,
        resume_pct,
        runtime:     ep.runtime_string().unwrap_or_default(),
    }
}

pub(crate) fn raw_to_entry(r: EpisodeRaw) -> EpisodeEntry {
    EpisodeEntry {
        id:         r.id.as_str().into(),
        title:      r.title.as_str().into(),
        ep_num:     r.ep_num,
        season_num: r.season_num,
        overview:   r.overview.as_str().into(),
        has_played:  r.has_played,
        is_favorite: r.is_favorite,
        resume_pct:  r.resume_pct,
        runtime:    r.runtime.as_str().into(),
        has_thumb:  false,
        thumb:      Default::default(),
    }
}

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
        let mut tasks: tokio::task::JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> =
            tokio::task::JoinSet::new();
        for (idx, ep) in episodes.iter().enumerate() {
            let client2 = Arc::clone(&client);
            let sem2    = Arc::clone(&sem);
            let id      = ep.id.clone();
            tasks.spawn(async move {
                let _permit = sem2.acquire_owned().await.ok();
                let bytes = fetch_poster_cached(&*client2, &id).await;
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
                let eps = AppState::get(&w).get_series_episodes();
                if let Some(mut ep) = eps.row_data(idx) {
                    ep.thumb     = slint::Image::from_rgba8(buf);
                    ep.has_thumb = true;
                    eps.set_row_data(idx, ep);
                }
            });
        }
    });
}

// ── SeriesCtx ────────────────────────────────────────────────────────────────

struct SeriesCtx {
    id:    String,
    client: Arc<JellyfinClient>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
    state: Arc<Mutex<FjordState>>,
}

impl SeriesCtx {
    fn spawn_main(&self) {
        let id     = self.id.clone();
        let client = Arc::clone(&self.client);
        let ww_ui  = self.ww.clone();
        let ww_ep  = self.ww.clone();
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

            let season_entries: Vec<SeasonEntry> = seasons.iter()
                .map(|s| SeasonEntry { id: s.id.as_str().into(), name: s.name.as_str().into() })
                .collect();
            let ep_raws: Vec<EpisodeRaw> = first_eps.iter().map(make_episode_raw).collect();

            let detail_name     = detail_res.as_ref().map(|d| d.name.clone()).ok().unwrap_or_default();
            let detail_overview = detail_res.as_ref().ok().and_then(|d| d.overview.clone()).unwrap_or_default();
            let id_guard = id.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww_ui.upgrade() else { return };
                if AppState::get(&w).get_series_id().as_str() != id_guard { return; }
                let g = AppState::get(&w);
                if !detail_name.is_empty()     { g.set_series_title(detail_name.as_str().into()); }
                if !detail_overview.is_empty() { g.set_series_overview(detail_overview.as_str().into()); }
                g.set_series_seasons(ModelRc::new(VecModel::from(season_entries)));
                let ep_entries: Vec<EpisodeEntry> = ep_raws.into_iter().map(raw_to_entry).collect();
                g.set_series_episodes(ModelRc::new(VecModel::from(ep_entries)));
                g.set_series_loading(false);
                if let Some(buf) = poster_bytes.as_deref().and_then(decode_poster_buffer) {
                    AppState::get(&w).set_series_poster(slint::Image::from_rgba8(buf));
                    AppState::get(&w).set_series_has_poster(true);
                }
                if let Some(buf) = backdrop_bytes.as_deref().and_then(decode_poster_buffer) {
                    AppState::get(&w).set_series_backdrop(slint::Image::from_rgba8(buf));
                    AppState::get(&w).set_series_has_backdrop(true);
                }
            });
            spawn_episode_thumb_loading(client, first_eps, id, ww_ep, rth);
        });
    }
}

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
        g.set_series_in_season_row(false);
        g.set_series_season_idx(0);
        g.set_series_focused_ep(0);
        g.set_series_seasons(ModelRc::new(VecModel::<SeasonEntry>::default()));
        g.set_series_episodes(ModelRc::new(VecModel::<EpisodeEntry>::default()));
        g.set_series_has_backdrop(false);
        g.set_series_has_poster(false);
        if let Some(ref item) = basic {
            g.set_series_title(item.name.as_str().into());
            g.set_series_overview(item.overview.clone().unwrap_or_default().as_str().into());
        }
    }

    let ctx = SeriesCtx { id, client, ww, rt: rt_handle, state };
    ctx.spawn_main();
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &crate::AppState) -> bool {
    use crate::keys::Action;
    if *action == Action::Back { g.invoke_close_series(); return true; }
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
            Action::Down | Action::Confirm => { g.set_series_in_season_row(false); true }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }
    match action {
        Action::Up => {
            let ep = g.get_series_focused_ep();
            if ep > 0 { g.set_series_focused_ep(ep - 1); }
            else { g.set_series_in_season_row(true); }
            true
        }
        Action::Down => {
            let ep = g.get_series_focused_ep();
            if ep < g.get_series_episodes().row_count() as i32 - 1 {
                g.set_series_focused_ep(ep + 1);
            }
            true
        }
        Action::Confirm => {
            if g.get_series_episodes().row_count() > 0 {
                let ep = g.get_series_episodes()
                    .row_data(g.get_series_focused_ep() as usize).unwrap();
                g.invoke_play_series_episode(ep.id);
            }
            true
        }
        Action::OpenContextMenu => {
            if g.get_series_episodes().row_count() > 0 {
                let ep = g.get_series_episodes()
                    .row_data(g.get_series_focused_ep() as usize).unwrap();
                g.invoke_open_context_menu_series_ep(
                    ep.id, ep.has_played, ep.is_favorite, ep.resume_pct,
                    g.get_series_id(),
                );
            }
            true
        }
        Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
        Action::Quit       => { g.invoke_quit(); true }
        _ => false
    }
}
