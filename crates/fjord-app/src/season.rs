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
//   wire_season            callbacks moved from main() (0.5.0 step 3): season detail
//   wire_season_toggles    callbacks moved from main() (0.5.0 step 3): season favourite / played
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tokio::task::JoinSet;
use tracing::warn;

use crate::AppState;
use crate::config::FjordState;
use crate::poster::{
    decode_backdrop_buffer, decode_poster_buffer, fetch_backdrop_cached,
    fetch_backdrop_cached_tagged, fetch_poster_cached,
};
use crate::{CastMember, MainWindow};

// ── open_season_screen ────────────────────────────────────────────────────────

pub(crate) fn open_season_screen(
    season_id: String,
    series_id: String,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else {
        return;
    };
    // Screen-open cache (Part 2): skip the loading spinner on a cache hit — the
    // remaining work (poster/backdrop/cast-portrait fetch) is disk-cached and fast.
    let cached_detail = s.item_detail_cache.get(&season_id);
    drop(s);
    tracing::debug!(
        "open_season_screen({season_id}): cache_hit={}",
        cached_detail.is_some()
    );

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

    let is_cache_hit = cached_detail.is_some();
    spawn_season_fetch(SeasonFetchArgs {
        sid: season_id.clone(),
        series_id: series_id.clone(),
        client: Arc::clone(&client),
        state: Arc::clone(&state),
        ww: ww.clone(),
        cached_detail,
        revalidate: false,
        rt: rt.clone(),
    });

    // Cache-hit only: the screen above already showed instantly from cached
    // data. Real gap, live-reported: Jellyfin's WebSocket only delivers
    // LibraryChanged to the most-recently-connected client when multiple
    // clients share a session (JELLYFIN.md) — this can silently starve Fjord
    // of the event, leaving item_detail_cache stale indefinitely with no
    // other fallback. This revalidation is what closes that gap for
    // whatever's actually on screen right now.
    if is_cache_hit && crate::should_revalidate(&state, &season_id) {
        spawn_season_fetch(SeasonFetchArgs {
            sid: season_id,
            series_id,
            client,
            state,
            ww,
            cached_detail: None,
            revalidate: true,
            rt,
        });
    }
}

struct SeasonFetchArgs {
    sid: String,
    series_id: String,
    client: Arc<fjord_api::JellyfinClient>,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    cached_detail: Option<fjord_api::models::MediaItem>,
    revalidate: bool,
    rt: tokio::runtime::Handle,
}

fn spawn_season_fetch(args: SeasonFetchArgs) {
    let SeasonFetchArgs {
        sid,
        series_id,
        client,
        state: state2,
        ww: ww_ui,
        cached_detail,
        revalidate,
        rt,
    } = args;
    rt.spawn(async move {
        let detail_fut = async {
            if let Some(d) = cached_detail {
                return Ok(d);
            }
            client.get_item_detail(&sid).await
        };
        let (detail_res, poster_bytes) =
            tokio::join!(detail_fut, fetch_poster_cached(&client, &sid),);
        // Sign-out (or a different account signing in on a shared HTPC)
        // mid-fetch must not let this per-user data land in the new session's
        // cache — same guard class as main.rs::session_current's own doc
        // comment (CR11-2). Applies to both the original open and a
        // background revalidate call alike.
        if let Ok(d) = &detail_res {
            if !crate::session_current(&state2, &client) {
                return;
            }
            state2
                .lock()
                .unwrap()
                .item_detail_cache
                .insert(sid.clone(), d.clone());
        }
        // Use season backdrop if available, else fall back to series backdrop.
        let backdrop_bytes = match &detail_res {
            Ok(d) if !d.backdrop_image_tags.is_empty() => {
                fetch_backdrop_cached_tagged(
                    &client,
                    &sid,
                    d.backdrop_image_tags.first().map(String::as_str),
                )
                .await
            }
            _ if !series_id.is_empty() => fetch_backdrop_cached(&client, &series_id).await,
            _ => None,
        };

        let (title, overview, meta, is_fav, has_played, cast_data) = match detail_res {
            Ok(ref d) => {
                let mut meta_parts: Vec<String> = vec![];
                if let Some(y) = d.production_year {
                    meta_parts.push(y.to_string());
                }
                if let Some(ref r) = d.official_rating {
                    meta_parts.push(r.clone());
                }
                let meta = meta_parts.join(" · ");

                let mut seen: std::collections::HashSet<String> = Default::default();
                let mut cast: Vec<(String, String, String)> = vec![];
                for p in d
                    .people
                    .iter()
                    .filter(|p| p.person_type == "Director")
                    .take(2)
                {
                    if seen.insert(p.id.clone()) {
                        cast.push((p.id.clone(), p.name.clone(), "Director".to_string()));
                    }
                }
                for p in d
                    .people
                    .iter()
                    .filter(|p| p.person_type == "Writer")
                    .take(3)
                {
                    if seen.insert(p.id.clone()) {
                        cast.push((p.id.clone(), p.name.clone(), "Writer".to_string()));
                    }
                }
                for p in d
                    .people
                    .iter()
                    .filter(|p| p.person_type == "Actor")
                    .take(12)
                {
                    if seen.insert(p.id.clone()) {
                        cast.push((p.id.clone(), p.name.clone(), p.role.clone()));
                    }
                }
                let is_fav = d.user_data.is_favorite;
                let has_played = d.user_data.played;
                (
                    d.name.clone(),
                    crate::strip_html_to_text(d.overview.clone().unwrap_or_default().trim()),
                    meta,
                    is_fav,
                    has_played,
                    cast,
                )
            }
            Err(e) => {
                warn!("get_item_detail season {}: {:#}", sid, e);
                (
                    String::new(),
                    String::new(),
                    String::new(),
                    false,
                    false,
                    vec![],
                )
            }
        };

        // Emit 50% progress — metadata + poster ready, about to fetch portraits.
        // Skipped on a background revalidate: no loading bar is showing.
        if !revalidate {
            let _ = slint::invoke_from_event_loop({
                let ww = ww_ui.clone();
                let sid = sid.clone();
                move || {
                    let Some(w) = ww.upgrade() else { return };
                    if AppState::get(&w).get_season_id().as_str() != sid {
                        return;
                    }
                    AppState::get(&w).set_app_loading_progress(0.5);
                }
            });
        }

        // Fetch ALL cast portraits in parallel before showing the page (no trickle-in).
        let person_ids: Vec<(usize, String)> = cast_data
            .iter()
            .enumerate()
            .filter(|(_, (pid, _, _))| !pid.is_empty())
            .map(|(idx, (pid, _, _))| (idx, pid.clone()))
            .collect();

        let sem = Arc::new(tokio::sync::Semaphore::new(6));
        let mut portrait_tasks: JoinSet<(
            usize,
            Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>,
        )> = JoinSet::new();
        for (model_idx, pid) in person_ids {
            let c2 = client.clone();
            let s2 = sem.clone();
            portrait_tasks.spawn(async move {
                let _permit = s2.acquire_owned().await.ok();
                let bytes = fetch_poster_cached(&c2, &pid).await;
                (model_idx, bytes.as_deref().and_then(decode_poster_buffer))
            });
        }
        let mut portraits: std::collections::HashMap<
            usize,
            slint::SharedPixelBuffer<slint::Rgba8Pixel>,
        > = std::collections::HashMap::new();
        while let Some(res) = portrait_tasks.join_next().await {
            if let Ok((idx, Some(buf))) = res {
                portraits.insert(idx, buf);
            }
        }

        // Single invoke — set everything and show the page with portraits already populated.
        let sid2 = sid.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_ui.upgrade() else { return };
            if AppState::get(&w).get_season_id().as_str() != sid2 {
                return;
            }
            // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09) —
            // same reasoning as series.rs's own spawn_main: the id check
            // above (helped by reset_session_state now clearing season-id
            // on a switch/sign-out) doesn't cover an Err(detail) path or a
            // coincidental same-id reopen under a new profile.
            if !crate::session_current(&state2, &client) {
                return;
            }
            let g = AppState::get(&w);
            if !title.is_empty() {
                g.set_season_title(title.as_str().into());
            }
            if !overview.is_empty() {
                g.set_season_overview(overview.as_str().into());
            }
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
            let cast_members: Vec<CastMember> = cast_data
                .into_iter()
                .enumerate()
                .map(|(idx, (cid, name, role))| {
                    let (photo, has_photo) = portraits
                        .remove(&idx)
                        .map(|buf| (slint::Image::from_rgba8(buf), true))
                        .unwrap_or_default();
                    CastMember {
                        id: cid.as_str().into(),
                        name: name.as_str().into(),
                        role: role.as_str().into(),
                        photo,
                        has_photo,
                    }
                })
                .collect();
            g.set_season_cast(ModelRc::new(VecModel::from(cast_members)));
            if !revalidate {
                g.set_app_content_loading(false);
                g.set_show_season(true);
                w.invoke_grab_keyboard_focus();
            }
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
                if (1..=2).contains(&btn) {
                    g.set_season_focused_btn(btn - 1);
                }
                true
            }
            Action::Right => {
                match btn {
                    0 => {
                        g.set_season_focused_btn(1);
                    }
                    1 => {
                        g.set_season_focused_btn(2);
                    }
                    _ => {}
                }
                true
            }
            Action::Up => {
                match btn {
                    0 => {
                        return false;
                    } // Back — let focus_bar_on_up handle it
                    3 => {
                        g.set_season_focused_btn(1);
                    } // Overview → ♥ fav
                    _ => {
                        g.set_season_focused_btn(0);
                    } // ♥/✓ → Back
                }
                true
            }
            Action::Down => {
                match btn {
                    0 => {
                        g.set_season_focused_btn(1);
                    } // Back → ♥ fav
                    3 => {
                        g.set_season_focused_btn(-1);
                    } // Overview → episodes
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
                    0 => {
                        g.set_season_focused_btn(-1);
                        g.invoke_close_season_detail();
                    }
                    1 => {
                        g.invoke_toggle_season_fav();
                    }
                    2 => {
                        g.invoke_toggle_season_played();
                    }
                    3 => {
                        g.set_season_overview_expanded(!g.get_season_overview_expanded());
                    }
                    _ => {}
                }
                true
            }
            Action::Fullscreen => {
                g.invoke_toggle_fullscreen();
                true
            }
            Action::Quit => {
                g.invoke_quit();
                true
            }
            _ => false,
        };
    }

    let in_cast = g.get_season_cast_focused() >= 0;

    // ── Cast row ──────────────────────────────────────────────────────────────
    if in_cast {
        return match action {
            Action::Left => {
                let idx = g.get_season_cast_focused();
                if idx > 0 {
                    g.set_season_cast_focused(idx - 1);
                }
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
                if idx >= 0
                    && let Some(c) = g.get_season_cast().row_data(idx as usize)
                {
                    g.invoke_open_person(c.id, c.name);
                }
                true
            }
            Action::Fullscreen => {
                g.invoke_toggle_fullscreen();
                true
            }
            Action::Quit => {
                g.invoke_quit();
                true
            }
            _ => false,
        };
    }

    // ── Episode row (default) ─────────────────────────────────────────────────
    match action {
        Action::Left => {
            let ep = g.get_season_focused_ep();
            if ep > 0 {
                g.set_season_focused_ep(ep - 1);
            }
            true
        }
        Action::Right => {
            let ep = g.get_season_focused_ep();
            let max = g.get_series_episode_cards().row_count() as i32 - 1;
            if ep < max {
                g.set_season_focused_ep(ep + 1);
            }
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
            if cards.row_count() > 0
                && let Some(card) = cards.row_data(g.get_season_focused_ep() as usize)
            {
                g.invoke_play_series_episode(card.id);
            }
            true
        }
        Action::OpenDetail => {
            let cards = g.get_series_episode_cards();
            if cards.row_count() > 0
                && let Some(card) = cards.row_data(g.get_season_focused_ep() as usize)
            {
                g.invoke_open_detail(card.id, "Episode".into());
            }
            true
        }
        Action::OpenContextMenu => {
            let cards = g.get_series_episode_cards();
            if cards.row_count() > 0
                && let Some(card) = cards.row_data(g.get_season_focused_ep() as usize)
            {
                g.set_context_menu_title(card.title.clone());
                g.invoke_open_context_menu(
                    card.id,
                    card.has_played,
                    card.is_favorite,
                    card.resume_pct,
                    card.item_type,
                    card.series_id,
                );
            }
            true
        }
        Action::Fullscreen => {
            g.invoke_toggle_fullscreen();
            true
        }
        Action::Quit => {
            g.invoke_quit();
            true
        }
        _ => false,
    }
}

// ── wire_season (moved from main(), 0.5.0 step 3) ────────────────────────
/// Wires season detail: open_season_detail, close_season_detail.
pub(crate) fn wire_season(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── season detail ─────────────────────────────────────────────────────────
    {
        let state_osd = Arc::clone(&state);
        let ww_osd = window.as_weak();
        let rth_osd = rt.handle().clone();
        AppState::get(&window).on_open_season_detail(move |season_id, series_id| {
            season::open_season_screen(
                season_id.to_string(),
                series_id.to_string(),
                state_osd.clone(),
                ww_osd.clone(),
                rth_osd.clone(),
            );
        });
    }
    {
        let ww_csd = window.as_weak();
        AppState::get(&window).on_close_season_detail(move || {
            if let Some(w) = ww_csd.upgrade() {
                let g = AppState::get(&w);
                // Closing season detail returns to series screen — clear only the
                // season restore flag; series screen will still show (or restore on stop).
                if g.get_has_background_player() {
                    g.set_playback_from_season(false);
                }
                g.set_show_season(false);
                g.set_season_id("".into());
                g.set_season_cast_focused(-1);
            }
        });
    }
}

// ── wire_season_toggles (moved from main(), 0.5.0 step 3) ────────────────
/// Wires season favourite / played: toggle_season_fav, toggle_season_played.
pub(crate) fn wire_season_toggles(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── season fav / played toggles ───────────────────────────────────────────
    {
        let state2 = Arc::clone(&state);
        let ww2 = window.as_weak();
        let rt2 = rt.handle().clone();
        AppState::get(&window).on_toggle_season_fav(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id = AppState::get(&w).get_season_id().to_string();
            let cur_fav = AppState::get(&w).get_season_is_favorite();
            let s = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else {
                return;
            };
            drop(s);
            let ww3 = ww2.clone();
            let state3 = Arc::clone(&state2);
            rt2.spawn(async move {
                let result = if cur_fav {
                    client.unset_favorite(&id).await
                } else {
                    client.set_favorite(&id).await
                };
                if let Err(e) = result {
                    warn!("toggle-season-fav: {e}");
                    return;
                }
                let new_fav = !cur_fav;
                state3
                    .lock()
                    .unwrap()
                    .update_item_user_state(&id, None, Some(new_fav));
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww3.upgrade()
                        && AppState::get(&w).get_season_id().as_str() == id
                    {
                        AppState::get(&w).set_season_is_favorite(new_fav);
                    }
                });
            });
        });
    }
    {
        let state2 = Arc::clone(&state);
        let ww2 = window.as_weak();
        let rt2 = rt.handle().clone();
        AppState::get(&window).on_toggle_season_played(move || {
            let Some(w) = ww2.upgrade() else { return };
            let id = AppState::get(&w).get_season_id().to_string();
            let cur_play = AppState::get(&w).get_season_has_played();
            // Capture the parent series_id so the series Next Up row can be refreshed.
            let sid = AppState::get(&w).get_series_id().to_string();
            let s = state2.lock().unwrap();
            let Some(client) = s.client.as_ref().map(Arc::clone) else {
                return;
            };
            drop(s);
            let ww3 = ww2.clone();
            let state3 = Arc::clone(&state2);
            let rt3 = rt2.clone();
            rt2.spawn(async move {
                let result = if cur_play {
                    client.mark_unplayed(&id).await
                } else {
                    client.mark_played(&id).await
                };
                if let Err(e) = result {
                    warn!("toggle-season-played: {e}");
                    return;
                }
                let new_play = !cur_play;
                state3
                    .lock()
                    .unwrap()
                    .update_item_user_state(&id, Some(new_play), None);
                let client2 = Arc::clone(&client);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww3.upgrade() {
                        if AppState::get(&w).get_season_id().as_str() == id {
                            AppState::get(&w).set_season_has_played(new_play);
                        }
                        if !sid.is_empty() {
                            crate::series::refresh_series_next_up(sid, client2, ww3.clone(), rt3);
                        }
                    }
                });
            });
        });
    }
}
