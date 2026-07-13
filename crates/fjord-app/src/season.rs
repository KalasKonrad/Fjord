// ── fjord-app · season.rs ────────────────────────────────────────────────────
//   open_season_screen  reset AppState season props; checks item_detail_cache (Part 2) —
//                       only sets app-content-loading=true on a cache miss; pre-fill title
//                       from series model; spawn async fetch for detail (skipped on cache
//                       hit) + poster + backdrop + ALL cast portraits; defers
//                       set_show_season until all data is ready (no trickle-in)
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
use crate::poster::{fetch_poster_cached, fetch_backdrop_cached, fetch_backdrop_cached_tagged, decode_backdrop_buffer, decode_poster_buffer};
use crate::{CastMember, MainWindow};

// ── open_season_screen ────────────────────────────────────────────────────────

pub(crate) fn open_season_screen(
    season_id: String,
    series_id: String,
    state:     Arc<Mutex<FjordState>>,
    ww:        slint::Weak<MainWindow>,
    rt:        tokio::runtime::Handle,
) {
    let s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
    // Screen-open cache (Part 2): skip the loading spinner on a cache hit — the
    // remaining work (poster/backdrop/cast-portrait fetch) is disk-cached and fast.
    let cached_detail = s.item_detail_cache.get(&season_id);
    drop(s);
    tracing::debug!("open_season_screen({season_id}): cache_hit={}", cached_detail.is_some());

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

        g.set_season_id(season_id.as_str().into());
        g.set_season_title(season_name.as_str().into());
        g.set_season_overview("".into());
        g.set_season_meta("".into());
        g.set_season_has_poster(false);
        g.set_season_has_backdrop(false);
        g.set_season_cast(ModelRc::new(VecModel::<CastMember>::default()));
        g.set_season_cast_focused(-1);
        g.set_season_focused_ep(0);
        g.set_season_focused_btn(-1);
        g.set_season_overview_expanded(false);
        g.set_season_is_favorite(false);
        g.set_season_has_played(false);
        g.set_season_loading(false);
        g.set_app_loading_progress(0.0);
        if cached_detail.is_none() {
            g.set_app_content_loading(true);
        }
        // show_season is deferred until the async task has all data ready
    }

    let sid    = season_id.clone();
    let ww_ui  = ww.clone();
    let state2 = Arc::clone(&state);
    rt.spawn(async move {
        let detail_fut = async {
            if let Some(d) = cached_detail { return Ok(d); }
            client.get_item_detail(&sid).await
        };
        let (detail_res, poster_bytes) = tokio::join!(
            detail_fut,
            fetch_poster_cached(&client, &sid),
        );
        if let Ok(d) = &detail_res {
            state2.lock().unwrap().item_detail_cache.insert(sid.clone(), d.clone());
        }
        // Use season backdrop if available, else fall back to series backdrop.
        let backdrop_bytes = match &detail_res {
            Ok(d) if !d.backdrop_image_tags.is_empty() =>
                fetch_backdrop_cached_tagged(&client, &sid, d.backdrop_image_tags.first().map(String::as_str)).await,
            _ if !series_id.is_empty() => fetch_backdrop_cached(&client, &series_id).await,
            _ => None,
        };

        let (title, overview, meta, is_fav, has_played, cast_data) = match detail_res {
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
                let is_fav     = d.user_data.is_favorite;
                let has_played = d.user_data.played;
                (d.name.clone(), d.overview.clone().unwrap_or_default().trim().to_string(), meta, is_fav, has_played, cast)
            }
            Err(e) => {
                warn!("get_item_detail season {}: {:#}", sid, e);
                (String::new(), String::new(), String::new(), false, false, vec![])
            }
        };

        // Emit 50% progress — metadata + poster ready, about to fetch portraits.
        let _ = slint::invoke_from_event_loop({
            let ww  = ww_ui.clone();
            let sid = sid.clone();
            move || {
                let Some(w) = ww.upgrade() else { return };
                if AppState::get(&w).get_season_id().as_str() != sid { return; }
                AppState::get(&w).set_app_loading_progress(0.5);
            }
        });

        // Fetch ALL cast portraits in parallel before showing the page (no trickle-in).
        let person_ids: Vec<(usize, String)> = cast_data.iter()
            .enumerate()
            .filter(|(_, (pid, _, _))| !pid.is_empty())
            .map(|(idx, (pid, _, _))| (idx, pid.clone()))
            .collect();

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
        let mut portraits: std::collections::HashMap<usize, slint::SharedPixelBuffer<slint::Rgba8Pixel>> =
            std::collections::HashMap::new();
        while let Some(res) = portrait_tasks.join_next().await {
            if let Ok((idx, Some(buf))) = res { portraits.insert(idx, buf); }
        }

        // Single invoke — set everything and show the page with portraits already populated.
        let sid2 = sid.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_ui.upgrade() else { return };
            if AppState::get(&w).get_season_id().as_str() != sid2 { return; }
            let g = AppState::get(&w);
            if !title.is_empty()    { g.set_season_title(title.as_str().into()); }
            if !overview.is_empty() { g.set_season_overview(overview.as_str().into()); }
            g.set_season_meta(meta.as_str().into());
            g.set_season_is_favorite(is_fav);
            g.set_season_has_played(has_played);
            if let Some(buf) = poster_bytes.as_deref().and_then(decode_poster_buffer) {
                g.set_season_poster(slint::Image::from_rgba8(buf));
                g.set_season_has_poster(true);
            }
            if let Some(buf) = backdrop_bytes.as_deref().and_then(decode_backdrop_buffer) {
                g.set_season_backdrop(slint::Image::from_rgba8(buf));
                g.set_season_has_backdrop(true);
            }
            let cast_members: Vec<CastMember> = cast_data.into_iter()
                .enumerate()
                .map(|(idx, (cid, name, role))| {
                    let (photo, has_photo) = portraits.remove(&idx)
                        .map(|buf| (slint::Image::from_rgba8(buf), true))
                        .unwrap_or_default();
                    CastMember {
                        id: cid.as_str().into(), name: name.as_str().into(),
                        role: role.as_str().into(), photo, has_photo,
                    }
                })
                .collect();
            g.set_season_cast(ModelRc::new(VecModel::from(cast_members)));
            g.set_app_content_loading(false);
            g.set_show_season(true);
            w.invoke_grab_keyboard_focus();
        });
    });
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &crate::AppState) -> bool {
    use crate::keys::Action;

    if *action == Action::Back {
        g.set_season_cast_focused(-1);
        g.set_season_focused_btn(-1);
        g.invoke_close_season_detail();
        return true;
    }

    let btn = g.get_season_focused_btn();

    // ── Header buttons (Back=0 / ♥=1 / ✓=2 / Overview=3) ────────────────────
    if btn >= 0 {
        return match action {
            Action::Left => {
                if (1..=2).contains(&btn) { g.set_season_focused_btn(btn - 1); }
                true
            }
            Action::Right => {
                match btn {
                    0 => { g.set_season_focused_btn(1); }
                    1 => { g.set_season_focused_btn(2); }
                    _ => {}
                }
                true
            }
            Action::Up => {
                match btn {
                    0 => { return false; } // Back — let focus_bar_on_up handle it
                    3 => { g.set_season_focused_btn(1); } // Overview → ♥ fav
                    _ => { g.set_season_focused_btn(0); } // ♥/✓ → Back
                }
                true
            }
            Action::Down => {
                match btn {
                    0 => { g.set_season_focused_btn(1); } // Back → ♥ fav
                    3 => { g.set_season_focused_btn(-1); } // Overview → episodes
                    _ => {
                        // ♥/✓ → Overview if present, else episodes
                        if !g.get_season_overview().is_empty() {
                            g.set_season_focused_btn(3);
                        } else {
                            g.set_season_focused_btn(-1);
                        }
                    }
                }
                true
            }
            Action::Confirm => {
                match btn {
                    0 => { g.set_season_focused_btn(-1); g.invoke_close_season_detail(); }
                    1 => { g.invoke_toggle_season_fav(); }
                    2 => { g.invoke_toggle_season_played(); }
                    3 => { g.set_season_overview_expanded(!g.get_season_overview_expanded()); }
                    _ => {}
                }
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
            Action::Confirm => {
                let idx = g.get_season_cast_focused();
                if idx >= 0 {
                    if let Some(c) = g.get_season_cast().row_data(idx as usize) {
                        g.invoke_open_person(c.id, c.name);
                    }
                }
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
            if !g.get_season_overview().is_empty() {
                g.set_season_focused_btn(3); // → Overview
            } else {
                g.set_season_focused_btn(1); // → ♥ fav
            }
            true
        }
        Action::Down => {
            if g.get_season_cast().row_count() > 0 {
                g.set_season_cast_focused(0);
                true
            } else {
                false // nothing below — let focus_bar_on_down reach the bars (CR10-19)
            }
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
                    g.set_context_menu_title(card.title.clone());
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
