// ── fjord-app · season.rs ────────────────────────────────────────────────────
//   open_season_screen  reset AppState season props; pre-fill title from series
//                       model; spawn async fetch for detail + poster + backdrop +
//                       cast portraits; push via invoke_from_event_loop
//   handle_key          keyboard dispatch for the season detail screen:
//                       episode row (default) ↔ cast row (when cast-focused ≥ 0);
//                       Enter plays focused episode; I opens episode detail;
//                       C opens context menu; Back closes season detail
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tokio::task::JoinSet;
use tracing::warn;

use crate::config::FjordState;
use crate::AppState;
use crate::poster::{fetch_poster_cached, fetch_backdrop_cached, decode_poster_buffer};
use crate::{CastMember, MainWindow};

// ── open_season_screen ────────────────────────────────────────────────────────

pub(crate) fn open_season_screen(
    season_id: String,
    state:     Arc<Mutex<FjordState>>,
    ww:        slint::Weak<MainWindow>,
    rt:        tokio::runtime::Handle,
) {
    let s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
    drop(s);

    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);

        // Pre-fill title from the seasons model already in AppState.
        let season_name = {
            let seasons = g.get_series_seasons();
            (0..seasons.row_count())
                .filter_map(|i| seasons.row_data(i))
                .find(|s| s.id.as_str() == season_id)
                .map(|s| s.name.to_string())
                .unwrap_or_default()
        };

        g.set_show_season(true);
        w.invoke_grab_keyboard_focus();
        g.set_season_id(season_id.as_str().into());
        g.set_season_title(season_name.as_str().into());
        g.set_season_overview("".into());
        g.set_season_meta("".into());
        g.set_season_has_poster(false);
        g.set_season_has_backdrop(false);
        g.set_season_cast(ModelRc::new(VecModel::<CastMember>::default()));
        g.set_season_cast_focused(-1);
        g.set_season_focused_ep(0);
        g.set_season_focused_back(false);
        g.set_season_loading(false);
    }

    let sid     = season_id.clone();
    let ww_ui   = ww.clone();
    let ww_cast = ww.clone();
    rt.spawn(async move {
        let (detail_res, poster_bytes) = tokio::join!(
            client.get_item_detail(&sid),
            fetch_poster_cached(&client, &sid),
        );
        let backdrop_bytes = match &detail_res {
            Ok(d) if !d.backdrop_image_tags.is_empty() => fetch_backdrop_cached(&client, &sid).await,
            _ => None,
        };

        let (title, overview, meta, cast_data) = match detail_res {
            Ok(ref d) => {
                let mut meta_parts: Vec<String> = vec![];
                if let Some(y) = d.production_year { meta_parts.push(y.to_string()); }
                if let Some(ref r) = d.official_rating { meta_parts.push(r.clone()); }
                let meta = meta_parts.join(" · ");

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
                (d.name.clone(), d.overview.clone().unwrap_or_default(), meta, cast)
            }
            Err(e) => {
                warn!("get_item_detail season {}: {:#}", sid, e);
                (String::new(), String::new(), String::new(), vec![])
            }
        };

        let person_ids: Vec<(usize, String)> = cast_data.iter()
            .enumerate()
            .filter(|(_, (pid, _, _))| !pid.is_empty())
            .map(|(idx, (pid, _, _))| (idx, pid.clone()))
            .collect();

        let sid2 = sid.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_ui.upgrade() else { return };
            if AppState::get(&w).get_season_id().as_str() != sid2 { return; }
            let g = AppState::get(&w);
            if !title.is_empty()    { g.set_season_title(title.as_str().into()); }
            if !overview.is_empty() { g.set_season_overview(overview.as_str().into()); }
            g.set_season_meta(meta.as_str().into());
            if let Some(buf) = poster_bytes.as_deref().and_then(decode_poster_buffer) {
                g.set_season_poster(slint::Image::from_rgba8(buf));
                g.set_season_has_poster(true);
            }
            if let Some(buf) = backdrop_bytes.as_deref().and_then(decode_poster_buffer) {
                g.set_season_backdrop(slint::Image::from_rgba8(buf));
                g.set_season_has_backdrop(true);
            }
            let cast_members: Vec<CastMember> = cast_data.into_iter()
                .map(|(cid, name, role)| CastMember {
                    id:        cid.as_str().into(),
                    name:      name.as_str().into(),
                    role:      role.as_str().into(),
                    photo:     Default::default(),
                    has_photo: false,
                })
                .collect();
            g.set_season_cast(ModelRc::new(VecModel::from(cast_members)));
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
            let ww_p  = ww_cast.clone();
            let sid_p = sid.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww_p.upgrade() else { return };
                if AppState::get(&w).get_season_id().as_str() != sid_p { return; }
                let cast_model = AppState::get(&w).get_season_cast();
                if let Some(mut member) = cast_model.row_data(idx) {
                    member.photo     = slint::Image::from_rgba8(buf);
                    member.has_photo = true;
                    cast_model.set_row_data(idx, member);
                }
            });
        }
    });
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &crate::AppState) -> bool {
    use crate::keys::Action;

    if *action == Action::Back {
        g.set_season_cast_focused(-1);
        g.set_season_focused_back(false);
        g.invoke_close_season_detail();
        return true;
    }

    // ── Back button focus ─────────────────────────────────────────────────────
    if g.get_season_focused_back() {
        return match action {
            Action::Down | Action::Right => {
                g.set_season_focused_back(false);
                true
            }
            Action::Confirm => {
                g.set_season_focused_back(false);
                g.set_season_cast_focused(-1);
                g.invoke_close_season_detail();
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }

    let in_cast = g.get_season_cast_focused() >= 0;

    // ── Cast row ──────────────────────────────────────────────────────────────
    if in_cast {
        return match action {
            Action::Left => {
                let idx = g.get_season_cast_focused();
                if idx > 0 { g.set_season_cast_focused(idx - 1); }
                true
            }
            Action::Right => {
                let idx = g.get_season_cast_focused();
                if idx < g.get_season_cast().row_count() as i32 - 1 {
                    g.set_season_cast_focused(idx + 1);
                }
                true
            }
            Action::Up => {
                g.set_season_cast_focused(-1); // back to episode row
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }

    // ── Episode row (default) ─────────────────────────────────────────────────
    match action {
        Action::Left => {
            let ep = g.get_season_focused_ep();
            if ep > 0 { g.set_season_focused_ep(ep - 1); }
            true
        }
        Action::Right => {
            let ep  = g.get_season_focused_ep();
            let max = g.get_series_episode_cards().row_count() as i32 - 1;
            if ep < max { g.set_season_focused_ep(ep + 1); }
            true
        }
        Action::Up => {
            g.set_season_focused_back(true);
            true
        }
        Action::Down => {
            if g.get_season_cast().row_count() > 0 {
                g.set_season_cast_focused(0);
            }
            true
        }
        Action::Confirm => {
            let cards = g.get_series_episode_cards();
            if cards.row_count() > 0 {
                if let Some(card) = cards.row_data(g.get_season_focused_ep() as usize) {
                    g.invoke_play_series_episode(card.id);
                }
            }
            true
        }
        Action::OpenDetail => {
            let cards = g.get_series_episode_cards();
            if cards.row_count() > 0 {
                if let Some(card) = cards.row_data(g.get_season_focused_ep() as usize) {
                    g.invoke_open_detail(card.id, "Episode".into());
                }
            }
            true
        }
        Action::OpenContextMenu => {
            let cards = g.get_series_episode_cards();
            if cards.row_count() > 0 {
                if let Some(card) = cards.row_data(g.get_season_focused_ep() as usize) {
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
