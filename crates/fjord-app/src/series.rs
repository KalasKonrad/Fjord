// ── fjord-app · series.rs ────────────────────────────────────────────────────
//   ep_to_card              MediaItem (Episode) → CardItem (title "S01E02 · Title")
//   spawn_episode_thumb_loading  parallel episode thumbnail fetch → series-episode-cards
//   SeriesCtx               shared context for background fetch tasks;
//                           cached_detail: Option<MediaItem> — Part 2 screen-open cache hit, if any
//                           (only spawn_main uses it; spawn_next_up/spawn_similar always get None)
//     spawn_main    detail(skips network on cache hit)+poster+seasons in parallel — seasons/
//                   first-season-episodes are deliberately NOT cached, to avoid interacting with
//                   the CR10-20 stale-fetch guards; caches the fetched/reused detail; backdrop;
//                   first eps; all cast portraits (fetched before show); emits
//                   app-loading-progress=0.5 at midpoint; single invoke shows page with all data +
//                   portraits ready; sets app-content-loading=false + show-series=true; spawns
//                   episode thumb loading after
//     spawn_next_up fetch next unwatched episode for this series (always fresh — inherently
//                   dynamic); set series-has-next-up + thumb
//     spawn_similar fetch similar series (FjordState.similar_items_cache, keyed by this series'
//                   id); push series-similar SectionRow via apply_cards_preserving_identity
//   spawn_recommended       "Recommended" row (2026-07-29, Deep Seerr integration) — standalone fn,
//                           not a SeriesCtx method (needs only Seerr + state, not the Jellyfin
//                           client); TMDB recommendations via Seerr filtered to not-already-owned
//                           titles, shown below More Like This
//   spawn_missing_seasons   "Missing Seasons" row (2026-07-29, Deep Seerr integration) — any TMDB
//                           season number (excluding 0/Specials) not present locally, any status
//                           regardless of Continuing/Ended; per-season request-status pill via
//                           discover::season_request_status; placed between Episodes and Cast
//   activate_missing_season Confirm/click on a missing-season card — no covering request opens
//                           Request Options preselected to all still-unrequested missing seasons;
//                           an already-covered one opens RequestDetailScreen normally instead
//                           (its own ⋮ More button handles Edit/Cancel); called from main.rs's
//                           on_series_missing_season_activate wiring (needs state/rt)
//   refresh_series_next_up  re-fetch Next Up after an episode is marked played; no focus-stealing
//   open_series_screen      reset AppState; checks item_detail_cache (Part 2) — only sets
//                           app-content-loading=true on a cache miss; (show-series deferred until
//                           spawn_main completes), build SeriesCtx, spawn tasks (incl. Recommended +
//                           Missing Seasons, 2026-07-29)
//   handle_key              keyboard dispatch for the series screen; row order top-to-bottom:
//                           episodes → missing-seasons → cast → similar → recommended (2026-07-29)
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use fjord_api::{models::MediaItem, JellyfinClient};
use slint::{Global, Model, ModelRc, VecModel};
use tokio::task::JoinSet;
use tracing::{debug, info, warn};

use crate::config::FjordState;
use crate::AppState;
use crate::detail::{fetch_card_posters, items_to_cards};
use crate::poster::{fetch_poster_cached, fetch_poster_cached_tagged, fetch_backdrop_cached_tagged, decode_backdrop_buffer, decode_poster_buffer};
use crate::{CardItem, CastMember, SeasonEntry, MainWindow};

// ── ep_to_card ────────────────────────────────────────────────────────────────

pub(crate) fn ep_to_card(ep: &MediaItem) -> CardItem {
    // Inside a series screen the show is known — title row is the episode name,
    // subtitle row the Jellyfin-style episode number.
    let s   = ep.parent_index_number.unwrap_or(0);
    let e   = ep.index_number.unwrap_or(0);
    let sub = if s > 0 || e > 0 { format!("S{}:E{}", s, e) } else { String::new() };
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
        title:          ep.name.as_str().into(),
        subtitle:       sub.as_str().into(),
        year:           ep.production_year.unwrap_or(0) as i32,
        has_played:     ep.user_data.played,
        is_favorite:    ep.user_data.is_favorite,
        resume_pct,
        has_poster:     false,
        poster:         Default::default(),
        unplayed_count: 0,
        availability:   "".into(),
        requested_4k:   false,
        other_tier_available: false,
        other_tier_requested: false,
        request_id: "".into(),
        request_pending: false,
        request_mine: false,
        on_watchlist: false,
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
            let tag = ep.primary_image_tag().map(str::to_string);
            tasks.spawn(async move {
                let _permit = s2.acquire_owned().await.ok();
                let bytes = fetch_poster_cached_tagged(&c2, &id, tag.as_deref()).await;
                (idx, bytes.as_deref().and_then(decode_poster_buffer))
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
    id:            String,
    client:        Arc<JellyfinClient>,
    ww:            slint::Weak<MainWindow>,
    rt:            tokio::runtime::Handle,
    state:         Arc<Mutex<FjordState>>,
    cached_detail: Option<MediaItem>,
    // See detail.rs::DetailCtx.revalidate's doc comment — same pattern,
    // background-only second call fired after a cache-hit already showed
    // the page, patches fields without touching show-series/loading state.
    revalidate:    bool,
}

impl SeriesCtx {
    fn spawn_main(&self) {
        let id     = self.id.clone();
        let client = Arc::clone(&self.client);
        let ww     = self.ww.clone();
        let ww_ep  = self.ww.clone();
        let state  = Arc::clone(&self.state);
        let rth    = self.rt.clone();
        let cached = self.cached_detail.clone();
        let revalidate = self.revalidate;
        self.rt.spawn(async move {
            let detail_fut = async {
                if let Some(d) = cached { return Ok(d); }
                client.get_item_detail(&id).await
            };
            let (detail_res, poster_bytes, seasons_res) = tokio::join!(
                detail_fut,
                fetch_poster_cached(&client, &id),
                client.get_seasons(&id),
            );
            // Sign-out (or a different account signing in on a shared HTPC)
            // mid-fetch must not let this per-user data land in the new
            // session's cache — same guard class as main.rs::session_current's
            // own doc comment (CR11-2). Applies to both the original open and
            // a background revalidate call alike.
            if let Ok(d) = &detail_res {
                if !crate::session_current(&state, &client) { return; }
                state.lock().unwrap().item_detail_cache.insert(id.clone(), d.clone());
            }
            // Ghost series (deleted server-side): clean up and bail before the
            // page shows — otherwise the loading overlay gives way to an empty
            // shell built from error fallbacks (S4).
            if let Err(e) = &detail_res {
                if crate::is_not_found(e) {
                    if !revalidate {
                        let ww_err = ww.clone();
                        let id_err = id.clone();
                        let _ = slint::invoke_from_event_loop(move || {
                            let Some(w) = ww_err.upgrade() else { return };
                            if AppState::get(&w).get_series_id().as_str() != id_err { return; }
                            let g = AppState::get(&w);
                            g.set_app_content_loading(false);
                            g.set_series_id("".into());
                        });
                        crate::purge_deleted_item(&state, &ww, &id);
                    }
                    return;
                }
            }
            let backdrop_bytes = match &detail_res {
                Ok(d) if !d.backdrop_image_tags.is_empty() =>
                    fetch_backdrop_cached_tagged(&client, &id, d.backdrop_image_tags.first().map(String::as_str)).await,
                _ => None,
            };
            let seasons = seasons_res.unwrap_or_else(|e| { warn!("get_seasons {}: {:#}", id, e); vec![] });
            debug!("series {} — {} season(s)", id, seasons.len());

            // Season list + season-0 episodes: skipped on revalidate for the
            // same reason the later set_series_episode_cards write already
            // skips itself (real bug, caught in review before shipping — the
            // only pre-existing guard here, series_open_id (CR10-20), catches
            // switching to a *different series* or closing the screen, but
            // says nothing about switching *seasons* within the same series.
            // Before revalidate existed this was safe by construction — the
            // screen wasn't interactive until this one call finished, so
            // there was no window for the user to tab to a different season
            // mid-fetch. Revalidate changes that: the cache-hit path shows
            // the screen instantly while this background call is still
            // running, so a season switch during that window would get
            // silently clobbered back to season 0 the moment this call
            // finishes, with playback still starting fine but the episode
            // title falling back to the raw id (series_episode_items lookup
            // miss). The purpose-built guard for this exact race,
            // series_season_generation, exists for on_series_select_season —
            // simplest correct fix is just not touching this state at all
            // during a revalidate pass, matching the UI-side precedent.
            let season_ids: Vec<String> = seasons.iter().map(|s| s.id.clone()).collect();
            if !revalidate {
                let mut s = state.lock().unwrap();
                // Superseded by another open (or the screen was closed) — bail (CR10-20).
                if s.series_open_id != id { return; }
                s.series_season_ids = season_ids;
            }

            let first_season_id = seasons.first().map(|s| s.id.clone());
            let first_eps = if !revalidate {
                if let Some(ref fid) = first_season_id {
                    client.get_season_episodes(&id, fid).await.unwrap_or_else(|e| {
                        warn!("get_season_episodes {} {}: {:#}", id, fid, e);
                        vec![]
                    })
                } else { vec![] }
            } else { vec![] };
            debug!("series {} season 0 — {} episode(s)", id, first_eps.len());
            if !revalidate {
                let mut s = state.lock().unwrap();
                if s.series_open_id != id { return; } // superseded (CR10-20)
                s.series_episode_items = first_eps.clone();
                if let Some(fid) = first_season_id {
                    s.series_episode_cache.insert(fid, first_eps.clone());
                }
            }

            // Build metadata from detail response.
            let detail_name     = detail_res.as_ref().map(|d| d.name.clone()).ok().unwrap_or_default();
            let detail_overview = detail_res.as_ref().ok().and_then(|d| d.overview.clone()).unwrap_or_default().trim().to_string();

            // Extended metadata only when detail fetch succeeded.
            let (meta, genres, rating_label, tagline, studio, is_favorite, series_played, cast_data) = if let Ok(ref d) = detail_res {
                let mut meta_parts: Vec<String> = vec![];
                if let Some(y) = d.production_year { meta_parts.push(y.to_string()); }
                if let Some(ref r) = d.official_rating { meta_parts.push(r.clone()); }
                let season_count = seasons.len();
                if season_count > 0 {
                    let ep_count = d.recursive_item_count.unwrap_or(0);
                    let s_label = if season_count == 1 { "Season".to_string() } else { "Seasons".to_string() };
                    let e_label = if ep_count == 1 { "Episode".to_string() } else { "Episodes".to_string() };
                    if ep_count > 0 {
                        meta_parts.push(format!("{} {} · {} {}", season_count, s_label, ep_count, e_label));
                    } else {
                        meta_parts.push(format!("{} {}", season_count, s_label));
                    }
                }
                let meta = meta_parts.join(" · ");
                let genres = d.genres.join(", ");
                let rating = d.community_rating.map(|r| format!("★ {:.1}", r)).unwrap_or_default();
                let tagline    = d.taglines.first().cloned().unwrap_or_default();
                let studio     = d.studios.first().map(|s| s.name.clone()).unwrap_or_default();
                let is_fav     = d.user_data.is_favorite;
                let has_played = d.user_data.played;

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
                (meta, genres, rating, tagline, studio, is_fav, has_played, cast)
            } else {
                (String::new(), String::new(), String::new(), String::new(), String::new(), false, false, vec![])
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

            // Main data ready — emit 50% progress so the bar shows movement.
            // Skipped on a background revalidate: no loading bar is showing.
            if !revalidate {
                let ww2  = ww.clone();
                let id_c = id.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww2.upgrade() else { return };
                    if AppState::get(&w).get_series_id().as_str() != id_c { return; }
                    AppState::get(&w).set_app_loading_progress(0.5);
                });
            }

            // Fetch all cast portraits before showing the page so they never trickle in.
            let sem = Arc::new(tokio::sync::Semaphore::new(6));
            let mut portrait_tasks: JoinSet<(usize, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>)> =
                JoinSet::new();
            for (model_idx, pid) in &person_ids {
                let c2    = client.clone();
                let s2    = sem.clone();
                let pid_c = pid.clone();
                let midx  = *model_idx;
                portrait_tasks.spawn(async move {
                    let _permit = s2.acquire_owned().await.ok();
                    let bytes = fetch_poster_cached(&c2, &pid_c).await;
                    (midx, bytes.as_deref().and_then(decode_poster_buffer))
                });
            }
            let mut portrait_bufs: Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> =
                vec![None; cast_data.len()];
            while let Some(res) = portrait_tasks.join_next().await {
                let Ok((idx, buf)) = res else { continue };
                portrait_bufs[idx] = buf;
            }

            // All data ready — show the series screen in a single event-loop call.
            let id_guard = id.clone();
            // client is still needed below (spawn_episode_thumb_loading) after
            // this closure — clone rather than let the closure's own capture
            // move it away.
            let client_guard = client.clone();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww.upgrade() else { return };
                if AppState::get(&w).get_series_id().as_str() != id_guard { return; }
                // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09):
                // the id check above now catches most of this
                // (reset_session_state clears series-id on a switch/sign-
                // out), but the early session_current check further up
                // this fn only runs on the Ok(detail) branch — an Err path,
                // or a coincidental same-id reopen under a NEW profile
                // before this stale fetch resolves, would still slip
                // through an id check alone.
                if !crate::session_current(&state, &client_guard) { return; }
                let g = AppState::get(&w);
                if !detail_name.is_empty()     { g.set_series_title(detail_name.as_str().into()); }
                if !detail_overview.is_empty() { g.set_series_overview(detail_overview.as_str().into()); }
                g.set_series_meta(meta.as_str().into());
                g.set_series_genres(genres.as_str().into());
                g.set_series_rating_label(rating_label.as_str().into());
                g.set_series_tagline(tagline.as_str().into());
                g.set_series_studio(studio.as_str().into());
                g.set_series_is_favorite(is_favorite);
                g.set_series_has_played(series_played);
                g.set_series_seasons(ModelRc::new(VecModel::from(season_entries)));
                // Skipped on revalidate: the user may have since tabbed to a
                // different season (season.rs owns that via its own
                // series_episode_cache lookup) — overwriting the visible
                // episode row here would snap it back to season 0 under them.
                if !revalidate {
                    let ep_cards: Vec<CardItem> = eps_for_cards.iter().map(ep_to_card).collect();
                    g.set_series_episode_cards(ModelRc::new(VecModel::from(ep_cards)));
                    g.set_series_loading(false);
                }
                // Build cast with portraits already fetched — no trickle-in.
                let cast_members: Vec<CastMember> = cast_data.into_iter().zip(portrait_bufs)
                    .map(|((cid, name, role), buf)| {
                        let (photo, has_photo) = if let Some(b) = buf {
                            (slint::Image::from_rgba8(b), true)
                        } else {
                            (Default::default(), false)
                        };
                        CastMember {
                            id:        cid.as_str().into(),
                            name:      name.as_str().into(),
                            role:      role.as_str().into(),
                            photo,
                            has_photo,
                        }
                    })
                    .collect();
                g.set_series_cast(ModelRc::new(VecModel::from(cast_members)));
                if let Some(buf) = poster_bytes.as_deref().and_then(decode_poster_buffer) {
                    g.set_series_poster(slint::Image::from_rgba8(buf));
                    g.set_series_has_poster(true);
                }
                if let Some(buf) = backdrop_bytes.as_deref().and_then(decode_backdrop_buffer) {
                    g.set_series_backdrop(slint::Image::from_rgba8(buf));
                    g.set_series_has_backdrop(true);
                }
                // Show the series screen and clear the loading overlay.
                if !revalidate {
                    g.set_show_series(true);
                    g.set_app_content_loading(false);
                    g.set_app_loading_progress(0.0);
                    w.invoke_grab_keyboard_focus();
                }
            });

            if !revalidate {
                spawn_episode_thumb_loading(client, first_eps, id, ww_ep, rth);
            }
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
            let ep_id    = ep.id.clone();
            let ep_title = ep.name.clone();
            let ep_sub   = {
                let s = ep.parent_index_number.unwrap_or(0);
                let e = ep.index_number.unwrap_or(0);
                if s > 0 || e > 0 { format!("S{}:E{}", s, e) } else { String::new() }
            };
            let runtime_secs = ep.run_time_ticks.unwrap_or(0) as f64 / 10_000_000.0;
            let resume_secs  = ep.user_data.playback_position_ticks as f64 / 10_000_000.0;
            let remaining    = if resume_secs > 0.0 { runtime_secs - resume_secs } else { runtime_secs };
            let ends_at      = crate::playback::fmt_ends_at(remaining);
            let section_title: slint::SharedString = if ends_at.is_empty() {
                "Next Up".into()
            } else {
                format!("Next Up  ·  Ends {}", ends_at).as_str().into()
            };
            let resume_pct = if runtime_secs > 0.0 {
                (resume_secs / runtime_secs).clamp(0.0, 1.0) as f32
            } else { 0.0 };
            let has_played  = ep.user_data.played;
            // Decode poster outside the closure (SharedPixelBuffer is Send; Image::from_rgba8 is not).
            let thumb_buf   = thumb_bytes.as_deref().and_then(decode_poster_buffer);
            let has_thumb   = thumb_buf.is_some();
            let _ = slint::invoke_from_event_loop(move || {
                let Some(w) = ww.upgrade() else { return };
                if AppState::get(&w).get_series_id().as_str() != id { return; }
                let g = AppState::get(&w);
                g.set_series_has_next_up(true);
                // Steal focus to Next Up only if user hasn't navigated away from default state.
                // series_focused_btn >= 0 means user is already on Back/♥/✓ — don't yank focus.
                if !g.get_series_in_season_row() && !g.get_series_next_up_focused()
                    && g.get_series_cast_focused() < 0 && g.get_series_similar_focused() < 0
                    && g.get_series_focused_btn() < 0 {
                    g.set_series_next_up_focused(true);
                }
                g.set_series_next_up_id(ep_id.as_str().into());
                g.set_series_next_up_section_title(section_title);
                g.set_series_next_up_resume_pct(resume_pct);
                g.set_series_next_up_has_played(has_played);
                // Build the CardItem here on the UI thread (Image::from_rgba8 requires it).
                // Passing an inline struct literal to SectionRow's `in property <[CardItem]>`
                // triggers Slint's recursion detector during component init — always use a model.
                let poster = thumb_buf.map(slint::Image::from_rgba8).unwrap_or_default();
                let card = CardItem {
                    id:             ep_id.as_str().into(),
                    series_id:      id.as_str().into(),
                    item_type:      "Episode".into(),
                    title:          ep_title.as_str().into(),
                    subtitle:       ep_sub.as_str().into(),
                    year:           0,
                    has_played,
                    is_favorite:    ep.user_data.is_favorite,
                    resume_pct,
                    has_poster:     has_thumb,
                    poster,
                    unplayed_count: 0,
                    availability:   "".into(),
                    requested_4k:   false,
                    other_tier_available: false,
                    other_tier_requested: false,
                    request_id: "".into(),
                    request_pending: false,
                    request_mine: false,
                    on_watchlist: false,
                };
                g.set_series_next_up_cards(ModelRc::new(VecModel::from(vec![card])));
            });
        });
    }

}

// ── refresh_series_next_up ────────────────────────────────────────────────────

/// Re-fetch the Next Up episode for a series after any played-state change.
/// Leaves the old card visible until the response arrives (no disappear-reappear flash).
/// On Ok(None) (series fully watched): clears the row and redirects focus to season tabs.
/// Same logic as SeriesCtx::spawn_next_up but does NOT steal keyboard focus.
pub(crate) fn refresh_series_next_up(
    series_id: String,
    client:    Arc<fjord_api::JellyfinClient>,
    ww:        slint::Weak<MainWindow>,
    rt:        tokio::runtime::Handle,
) {
    rt.spawn(async move {
        let ep = match client.get_next_up_for_series(&series_id).await {
            Ok(Some(ep)) => ep,
            Ok(None) => {
                // Series fully watched — clear the row now that we have confirmation.
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    if AppState::get(&w).get_series_id().as_str() != series_id { return; }
                    let g = AppState::get(&w);
                    let was_focused = g.get_series_next_up_focused();
                    g.set_series_has_next_up(false);
                    g.set_series_next_up_focused(false);
                    g.set_series_next_up_cards(slint::ModelRc::new(slint::VecModel::<CardItem>::default()));
                    if was_focused {
                        g.set_series_in_season_row(true);
                    }
                });
                return;
            }
            Err(e) => { warn!("refresh_series_next_up {}: {:#}", series_id, e); return; }
        };
        let thumb_bytes  = fetch_poster_cached(&client, &ep.id).await;
        let ep_id        = ep.id.clone();
        let ep_title     = ep.name.clone();
        let ep_sub       = {
            let s = ep.parent_index_number.unwrap_or(0);
            let e = ep.index_number.unwrap_or(0);
            if s > 0 || e > 0 { format!("S{}:E{}", s, e) } else { String::new() }
        };
        let runtime_secs = ep.run_time_ticks.unwrap_or(0) as f64 / 10_000_000.0;
        let resume_secs  = ep.user_data.playback_position_ticks as f64 / 10_000_000.0;
        let remaining    = if resume_secs > 0.0 { runtime_secs - resume_secs } else { runtime_secs };
        let ends_at      = crate::playback::fmt_ends_at(remaining);
        let section_title: slint::SharedString = if ends_at.is_empty() {
            "Next Up".into()
        } else {
            format!("Next Up  ·  Ends {}", ends_at).as_str().into()
        };
        let resume_pct  = if runtime_secs > 0.0 { (resume_secs / runtime_secs).clamp(0.0, 1.0) as f32 } else { 0.0 };
        let has_played  = ep.user_data.played;
        let thumb_buf   = thumb_bytes.as_deref().and_then(decode_poster_buffer);
        let has_thumb   = thumb_buf.is_some();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            if AppState::get(&w).get_series_id().as_str() != series_id { return; }
            let g = AppState::get(&w);
            let poster = thumb_buf.map(slint::Image::from_rgba8).unwrap_or_default();
            let card = CardItem {
                id:             ep_id.as_str().into(),
                series_id:      series_id.as_str().into(),
                item_type:      "Episode".into(),
                title:          ep_title.as_str().into(),
                subtitle:       ep_sub.as_str().into(),
                year:           0,
                has_played,
                is_favorite:    ep.user_data.is_favorite,
                resume_pct,
                has_poster:     has_thumb,
                poster,
                unplayed_count: 0,
                availability:   "".into(),
                requested_4k:   false,
                other_tier_available: false,
                other_tier_requested: false,
                request_id: "".into(),
                request_pending: false,
                request_mine: false,
                on_watchlist: false,
            };
            g.set_series_next_up_section_title(section_title);
            g.set_series_next_up_id(ep_id.as_str().into());
            g.set_series_next_up_resume_pct(resume_pct);
            g.set_series_next_up_has_played(has_played);
            g.set_series_next_up_cards(ModelRc::new(VecModel::from(vec![card])));
            g.set_series_has_next_up(true);
        });
    });
}

// ── SeriesCtx continued ───────────────────────────────────────────────────────

impl SeriesCtx {
    fn spawn_similar(&self) {
        let id     = self.id.clone();
        let client = Arc::clone(&self.client);
        let ww     = self.ww.clone();
        let state  = Arc::clone(&self.state);
        let cached = state.lock().unwrap().similar_items_cache.get(&id);
        let is_hit = cached.is_some();
        let ww2    = self.ww.clone();
        self.rt.spawn(async move {
            let similar = match cached {
                Some(v) => v,
                None => match client.get_similar_items(&id).await {
                    Ok(v)  => v,
                    Err(e) => { warn!("get_similar_items {}: {:#}", id, e); return; }
                },
            };
            state.lock().unwrap().similar_items_cache.insert(id.clone(), similar.clone());
            if !similar.is_empty() {
                let bufs = fetch_card_posters(&client, &similar).await;
                let id_c = id.clone();
                let _ = slint::invoke_from_event_loop(move || {
                    let Some(w) = ww.upgrade() else { return };
                    if AppState::get(&w).get_series_id().as_str() != id_c { return; }
                    let g = AppState::get(&w);
                    let fresh = items_to_cards(&similar, bufs);
                    g.set_series_similar(crate::apply_cards_preserving_identity(&g.get_series_similar(), fresh));
                });
            }
            // Cache-hit only: shown instantly above; silently revalidate and
            // patch if changed (same staleness gap as detail.rs::spawn_similar).
            if is_hit {
                if let Ok(fresh_similar) = client.get_similar_items(&id).await {
                    if !crate::session_current(&state, &client) { return; }
                    state.lock().unwrap().similar_items_cache.insert(id.clone(), fresh_similar.clone());
                    let bufs = fetch_card_posters(&client, &fresh_similar).await;
                    let id_c = id.clone();
                    let _ = slint::invoke_from_event_loop(move || {
                        let Some(w) = ww2.upgrade() else { return };
                        if AppState::get(&w).get_series_id().as_str() != id_c { return; }
                        let g = AppState::get(&w);
                        let fresh = items_to_cards(&fresh_similar, bufs);
                        g.set_series_similar(crate::apply_cards_preserving_identity(&g.get_series_similar(), fresh));
                    });
                }
            }
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
    let mut s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };
    let basic = s.all_series.iter().find(|i| i.id == id).cloned();
    // Screen-open cache (Part 2): only the get_item_detail fetch is cached here —
    // seasons/first-season-episodes are always refetched fresh, since they
    // interact with the CR10-20 stale-fetch guards (series_open_id/
    // series_episode_cache, cleared below on every open) and caching them too
    // would risk reintroducing exactly the kind of subtle bug those guards exist
    // to prevent. Detail alone still skips the slowest single call (full
    // metadata + cast list) in spawn_main's join.
    let cached_detail = s.item_detail_cache.get(&id);
    debug!("open_series_screen({id}): cache_hit={}", cached_detail.is_some());
    // Claim the canonical series slot synchronously (CR10-20). spawn_main's
    // async writes are guarded by series_open_id == id, so a slow task for a
    // previously opened series can no longer overwrite the state of the one
    // now on screen after rapid A -> B navigation.
    s.series_open_id = id.clone();
    s.series_season_ids.clear();
    s.series_episode_items.clear();
    s.series_episode_cache.clear();
    s.series_season_generation = 0;
    drop(s);

    info!("open_series: id={} name={:?}", id, basic.as_ref().map(|i| i.name.as_str()));

    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        // Don't show the series screen yet — spawn_main will set show_series=true
        // and clear app-content-loading once metadata + poster + backdrop + episodes are ready.
        if cached_detail.is_none() {
            g.set_app_content_loading(true);
        }
        g.set_app_loading_progress(0.0);
        g.set_series_id(id.as_str().into());
        g.set_series_loading(true);
        g.set_series_in_season_row(false);     // default: episode row (Next Up steals focus when it loads)
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
        g.set_series_tagline("".into());
        g.set_series_studio("".into());
        g.set_series_is_favorite(false);
        g.set_series_has_played(false);
        g.set_series_unplayed_count(0);
        g.set_series_cast(ModelRc::new(VecModel::<CastMember>::default()));
        g.set_series_cast_focused(-1);
        g.set_series_similar(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_series_similar_focused(-1);
        g.set_series_recommended(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_series_recommended_focused(-1);
        g.set_series_missing_seasons(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_series_missing_seasons_focused(-1);
        g.set_series_focused_btn(-1);
        g.set_series_overview_expanded(false);
        g.set_series_has_next_up(false);
        g.set_series_next_up_id("".into());
        g.set_series_next_up_section_title("Next Up".into());
        g.set_series_next_up_resume_pct(0.0);
        g.set_series_next_up_has_played(false);
        g.set_series_next_up_cards(ModelRc::new(VecModel::<CardItem>::default()));
        if let Some(ref item) = basic {
            g.set_series_title(item.name.as_str().into());
            g.set_series_overview(item.overview.clone().unwrap_or_default().trim().into());
            g.set_series_is_favorite(item.user_data.is_favorite);
            g.set_series_has_played(item.user_data.played);
            g.set_series_unplayed_count(item.user_data.unplayed_item_count);
        }
    }

    let is_detail_cache_hit = cached_detail.is_some();
    let ctx = SeriesCtx { id: id.clone(), client: client.clone(), ww: ww.clone(), rt: rt_handle.clone(), state: Arc::clone(&state), cached_detail, revalidate: false };
    ctx.spawn_main();
    if is_detail_cache_hit && crate::should_revalidate(&state, &id) {
        let ctx_revalidate = SeriesCtx { id: id.clone(), client: client.clone(), ww: ww.clone(), rt: rt_handle.clone(), state: Arc::clone(&state), cached_detail: None, revalidate: true };
        ctx_revalidate.spawn_main();
    }
    let ctx_nu = SeriesCtx { id: id.clone(), client: client.clone(), ww: ww.clone(), rt: rt_handle.clone(), state: Arc::clone(&state), cached_detail: None, revalidate: false };
    ctx_nu.spawn_next_up();
    let ctx_si = SeriesCtx { id: id.clone(), client: client.clone(), ww: ww.clone(), rt: rt_handle.clone(), state: Arc::clone(&state), cached_detail: None, revalidate: false };
    ctx_si.spawn_similar();
    spawn_recommended(id.clone(), Arc::clone(&state), ww.clone(), rt_handle.clone());
    spawn_missing_seasons(id, client, state, ww, rt_handle);
}

/// "Recommended" row (2026-07-29, Deep Seerr integration) — see
/// `detail.rs::DetailCtx::spawn_recommended`'s own doc comment for the full
/// shape this mirrors (TMDB recommendations via Seerr, filtered to titles
/// not already in the local library, shown below the existing local-only
/// "More Like This" row). A standalone fn rather than a `SeriesCtx` method
/// since it doesn't need the Jellyfin client, only Seerr + state.
fn spawn_recommended(
    id:    String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    let Some(seerr) = state.lock().unwrap().seerr_client.clone() else { return };
    rt.spawn(async move {
        let resolved = {
            let s = state.lock().unwrap();
            crate::context_menu::resolve_tmdb_for_jellyfin_item(&s, &id, "Series")
        };
        let Some((tmdb_id_str, _)) = resolved else {
            debug!("series spawn_recommended({id}): no tmdb id resolved — row will not show");
            return;
        };
        let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else { return };
        let resp = match seerr.get_tv_recommendations(tmdb_id, 1).await {
            Ok(r)  => r,
            Err(e) => { warn!("seerr: get_tv_recommendations({tmdb_id}): {e:#}"); return; }
        };
        let metas = crate::discover::build_filtered_metas(&resp.results);
        let ready = crate::discover::resolve_and_fetch_discovery_row(&state, metas, 20).await;
        debug!("series spawn_recommended({id}): tmdb={tmdb_id} -> {} recommendation(s) after owned-filter", ready.len());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_series_id().as_str() != id { return; }
            let cards = crate::discover::discover_cards_from(ready);
            g.set_series_recommended(crate::apply_cards_preserving_identity(&g.get_series_recommended(), cards));
        });
    });
}

/// "Missing Seasons" row (2026-07-29, Deep Seerr integration) — for a
/// partially-owned series (any season number TMDB knows about but that
/// isn't a local Jellyfin season, excluding season 0/"Specials" — confirmed
/// decision, most libraries deliberately don't own specials, and counting
/// it would flag nearly every partial show as "missing" something nobody
/// wants), shows a card per missing season with a request-status pill
/// (`season_request_status`, discover.rs) so an already-in-flight season
/// isn't re-requested. Scope: any partially-owned series regardless of
/// `Status` — confirmed via `AskUserQuestion`, not restricted to `Continuing`
/// shows the way the Coming Up calendar's own new candidate source is.
/// A standalone fn (not a `SeriesCtx` method) since it needs both the
/// Jellyfin client (for the local season list) and Seerr.
fn spawn_missing_seasons(
    id:     String,
    client: Arc<JellyfinClient>,
    state:  Arc<Mutex<FjordState>>,
    ww:     slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
) {
    let Some(seerr) = state.lock().unwrap().seerr_client.clone() else { return };
    rt.spawn(async move {
        let resolved = {
            let s = state.lock().unwrap();
            crate::context_menu::resolve_tmdb_for_jellyfin_item(&s, &id, "Series")
        };
        let Some((tmdb_id_str, _)) = resolved else {
            debug!("spawn_missing_seasons({id}): no tmdb id resolved — row will not show");
            return;
        };
        let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else { return };

        let (local_seasons_res, tv_res) = tokio::join!(client.get_seasons(&id), seerr.get_tv(tmdb_id));
        let local_seasons = match local_seasons_res {
            Ok(v)  => v,
            Err(e) => { warn!("spawn_missing_seasons get_seasons({id}): {:#}", e); return; }
        };
        let tv = match tv_res {
            Ok(v)  => v,
            Err(e) => { warn!("spawn_missing_seasons get_tv({tmdb_id}): {:#}", e); return; }
        };

        let local_numbers: std::collections::HashSet<u32> =
            local_seasons.iter().filter_map(|s| s.index_number).collect();
        let my_user_id = state.lock().unwrap().seerr_user_id;
        let requests: &[fjord_seerr::MediaRequest] =
            tv.media_info.as_ref().map(|mi| mi.requests.as_slice()).unwrap_or(&[]);

        // (availability_label, request_id, pending, mine) — matches
        // discover::season_request_status's own return shape.
        type SeasonStatus = (String, String, bool, bool);
        let missing: Vec<(fjord_seerr::Season, Option<SeasonStatus>)> = tv
            .seasons
            .into_iter()
            .filter(|s| s.season_number != 0 && !local_numbers.contains(&s.season_number))
            .map(|s| {
                let status = crate::discover::season_request_status(requests, s.season_number, my_user_id);
                (s, status)
            })
            .collect();
        debug!("spawn_missing_seasons({id}): tmdb={tmdb_id} -> {} missing season(s)", missing.len());
        if missing.is_empty() { return; }

        // Bounded-concurrency TMDB poster fetch, same shape as every other
        // Discover-sourced row in this app.
        let Ok(http) = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build() else { return };
        let sem = Arc::new(tokio::sync::Semaphore::new(8));
        let mut set = JoinSet::new();
        for (idx, (season, _)) in missing.iter().enumerate() {
            let Some(path) = season.poster_path.clone() else { continue };
            let http = http.clone();
            let sem = Arc::clone(&sem);
            let cache_key = format!("season-missing-{tmdb_id}-{}", season.season_number);
            set.spawn(async move {
                let _permit = sem.acquire_owned().await.ok();
                let bytes = crate::discover::fetch_tmdb_image(&http, crate::discover::TMDB_POSTER_BASE, &path, &cache_key).await?;
                let buf = decode_poster_buffer(&bytes)?;
                Some((idx, buf))
            });
        }
        let mut bufs: Vec<Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>> = vec![None; missing.len()];
        while let Some(res) = set.join_next().await {
            if let Ok(Some((idx, buf))) = res { bufs[idx] = Some(buf); }
        }

        // Plain Send-safe data only — CardItem (carries a slint::Image field,
        // !Send regardless of whether it's populated) is built only inside
        // the invoke_from_event_loop closure below, same two-phase
        // discipline as every other row added this pass.
        // (season_number, name, episode_count, availability, request_id, pending, mine, poster_buf)
        type MissingSeasonRow = (u32, String, u32, String, String, bool, bool, Option<slint::SharedPixelBuffer<slint::Rgba8Pixel>>);
        let rows: Vec<MissingSeasonRow> = missing
            .into_iter()
            .zip(bufs)
            .map(|((season, status), buf)| {
                let name = if season.name.is_empty() { format!("Season {}", season.season_number) } else { season.name };
                let (availability, request_id, request_pending, request_mine) = match status {
                    Some((a, rid, pending, mine)) => (a, rid, pending, mine),
                    None => (String::new(), String::new(), false, false),
                };
                (season.season_number, name, season.episode_count, availability, request_id, request_pending, request_mine, buf)
            })
            .collect();

        let id_c = id.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_series_id().as_str() != id_c { return; }
            let cards: Vec<CardItem> = rows
                .into_iter()
                .map(|(season_number, name, episode_count, availability, request_id, request_pending, request_mine, buf)| {
                    let mut card = CardItem {
                        id:           season_number.to_string().as_str().into(),
                        item_type:    "MissingSeason".into(),
                        title:        name.as_str().into(),
                        subtitle:     format!("{episode_count} episodes").as_str().into(),
                        availability: availability.as_str().into(),
                        request_id:   request_id.as_str().into(),
                        request_pending,
                        request_mine,
                        ..Default::default()
                    };
                    if let Some(b) = buf {
                        card.poster = slint::Image::from_rgba8(b);
                        card.has_poster = true;
                    }
                    card
                })
                .collect();
            g.set_series_missing_seasons(crate::apply_cards_preserving_identity(&g.get_series_missing_seasons(), cards));
        });
    });
}

/// Confirm on a focused "Missing Seasons" card — a season with no covering
/// request opens the Request Options modal pre-checked to every currently-
/// missing-and-unrequested season (not just the one activated), so several
/// can be requested in one submission; a season that already has one opens
/// RequestDetailScreen normally instead (its own ⋮ More button is the
/// correct place to Edit/Cancel it — see `discover::open_series_request_detail`'s
/// own doc comment for why this doesn't try to build a season-scoped
/// context menu). Routed through an AppState callback (wired in main.rs,
/// where `state`/`rt` are available) rather than handled inline in
/// `handle_key`, matching this codebase's established pattern for any
/// keyboard-triggered action that needs an async network call.
pub(crate) fn activate_missing_season(
    g:     &AppState,
    idx:   usize,
    state: &Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    let Some(card) = g.get_series_missing_seasons().row_data(idx) else { return };
    let series_id = g.get_series_id().to_string();
    let resolved = {
        let s = state.lock().unwrap();
        crate::context_menu::resolve_tmdb_for_jellyfin_item(&s, &series_id, "Series")
    };
    let Some((tmdb_id_str, _)) = resolved else { return };
    if card.request_id.is_empty() {
        let all_missing: Vec<u32> = (0..g.get_series_missing_seasons().row_count())
            .filter_map(|i| g.get_series_missing_seasons().row_data(i))
            .filter(|c| c.request_id.is_empty())
            .filter_map(|c| c.id.parse::<u32>().ok())
            .collect();
        crate::discover::open_series_request_detail(tmdb_id_str, Some(all_missing), Arc::clone(state), ww, rt);
    } else {
        crate::discover::open_series_request_detail(tmdb_id_str, None, Arc::clone(state), ww, rt);
    }
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &crate::AppState) -> bool {
    use crate::keys::Action;
    if *action == Action::Back {
        g.set_series_cast_focused(-1);
        g.set_series_similar_focused(-1);
        g.set_series_recommended_focused(-1);
        g.set_series_missing_seasons_focused(-1);
        g.set_series_next_up_focused(false);
        g.set_series_focused_btn(-1);
        g.set_series_overview_expanded(false);
        g.invoke_close_series();
        return true;
    }

    // ── Header buttons (Back / ♥ / ✓ Watched) ────────────────────────────────
    if g.get_series_focused_btn() == 0 {
        // Back button: Down → ♥ fav; Right → ♥; Enter → close
        return match action {
            Action::Down  => { g.set_series_focused_btn(1); true }
            Action::Right => { g.set_series_focused_btn(1); true }
            Action::Confirm => {
                g.set_series_focused_btn(-1);
                g.invoke_close_series();
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }
    if g.get_series_focused_btn() >= 1 {
        // ♥ (1) and ✓ Watched (2): Left/Right cycle; Up → Back button; Down → content
        return match action {
            Action::Left => {
                let b = g.get_series_focused_btn();
                if (1..=2).contains(&b) { g.set_series_focused_btn(b - 1); }
                true
            }
            Action::Right => {
                let b = g.get_series_focused_btn();
                if b < 2 { g.set_series_focused_btn(b + 1); }
                true
            }
            Action::Up => {
                let b = g.get_series_focused_btn();
                if b == 3 { g.set_series_focused_btn(1); } // Overview → ♥ fav
                else      { g.set_series_focused_btn(0); } // ♥/✓ → Back
                true
            }
            Action::Down => {
                let b = g.get_series_focused_btn();
                if b == 3 {
                    // Overview → content
                    g.set_series_focused_btn(-1);
                    if g.get_series_has_next_up() { g.set_series_next_up_focused(true); }
                    else                          { g.set_series_in_season_row(true); }
                } else if !g.get_series_overview().is_empty() {
                    // ♥/✓ → Overview first
                    g.set_series_focused_btn(3);
                } else {
                    // No overview → straight to content
                    g.set_series_focused_btn(-1);
                    if g.get_series_has_next_up() { g.set_series_next_up_focused(true); }
                    else                          { g.set_series_in_season_row(true); }
                }
                true
            }
            Action::Confirm => {
                match g.get_series_focused_btn() {
                    1 => g.invoke_toggle_series_fav(),
                    2 => g.invoke_toggle_series_played(),
                    3 => g.set_series_overview_expanded(!g.get_series_overview_expanded()),
                    _ => {}
                }
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }

    // ── Next Up row ───────────────────────────────────────────────────────────
    if g.get_series_next_up_focused() {
        return match action {
            Action::Down => {
                g.set_series_next_up_focused(false);
                g.set_series_in_season_row(true);
                true
            }
            Action::Up => {
                g.set_series_next_up_focused(false);
                if !g.get_series_overview().is_empty() {
                    g.set_series_focused_btn(3); // → Overview
                } else {
                    g.set_series_focused_btn(1); // → ♥ fav
                }
                true
            }
            Action::Confirm => {
                g.invoke_play_series_episode(g.get_series_next_up_id());
                true
            }
            Action::OpenContextMenu => {
                if let Some(card) = g.get_series_next_up_cards().row_data(0) {
                    g.set_context_menu_title(card.title.clone());
                }
                g.invoke_open_context_menu(
                    g.get_series_next_up_id(),
                    g.get_series_next_up_has_played(),
                    false,
                    g.get_series_next_up_resume_pct(),
                    "Episode".into(),
                    g.get_series_id(),
                );
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
                g.set_series_in_season_row(false);
                if g.get_series_has_next_up() {
                    g.set_series_next_up_focused(true);
                } else if !g.get_series_overview().is_empty() {
                    g.set_series_focused_btn(3); // → Overview
                } else {
                    g.set_series_focused_btn(1); // → ♥ fav
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
    let in_missing_seasons = g.get_series_missing_seasons_focused() >= 0;
    let in_cast            = g.get_series_cast_focused()            >= 0;
    let in_similar         = g.get_series_similar_focused()         >= 0;
    let in_recommended     = g.get_series_recommended_focused()     >= 0;

    // ── Missing Seasons row (Discover-sourced, 2026-07-29) ────────────────────
    if in_missing_seasons {
        return match action {
            Action::Left => {
                let idx = g.get_series_missing_seasons_focused();
                if idx > 0 { g.set_series_missing_seasons_focused(idx - 1); }
                true
            }
            Action::Right => {
                let idx = g.get_series_missing_seasons_focused();
                if idx < g.get_series_missing_seasons().row_count() as i32 - 1 {
                    g.set_series_missing_seasons_focused(idx + 1);
                }
                true
            }
            Action::Up => {
                g.set_series_missing_seasons_focused(-1); // back to episode row
                true
            }
            Action::Down => {
                if g.get_series_cast().row_count() > 0 {
                    g.set_series_missing_seasons_focused(-1);
                    g.set_series_cast_focused(0);
                    true
                } else if g.get_series_similar().row_count() > 0 {
                    g.set_series_missing_seasons_focused(-1);
                    g.set_series_similar_focused(0);
                    true
                } else if g.get_series_recommended().row_count() > 0 {
                    g.set_series_missing_seasons_focused(-1);
                    g.set_series_recommended_focused(0);
                    true
                } else {
                    false // nothing below — let focus_bar_on_down reach the bars
                }
            }
            Action::Confirm => {
                let idx = g.get_series_missing_seasons_focused();
                g.invoke_series_missing_season_activate(idx);
                true
            }
            Action::Fullscreen => { g.invoke_toggle_fullscreen(); true }
            Action::Quit       => { g.invoke_quit(); true }
            _ => false
        };
    }

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
                g.set_series_cast_focused(-1);
                if g.get_series_missing_seasons().row_count() > 0 {
                    g.set_series_missing_seasons_focused(0);
                } // else: back to episode row
                true
            }
            Action::Down => {
                if g.get_series_similar().row_count() > 0 {
                    g.set_series_cast_focused(-1);
                    g.set_series_similar_focused(0);
                    true
                } else if g.get_series_recommended().row_count() > 0 {
                    g.set_series_cast_focused(-1);
                    g.set_series_recommended_focused(0);
                    true
                } else {
                    false // nothing below — let focus_bar_on_down reach the bars (CR10-19)
                }
            }
            Action::Confirm => {
                let idx = g.get_series_cast_focused();
                if idx >= 0 {
                    if let Some(c) = g.get_series_cast().row_data(idx as usize) {
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
                } else if g.get_series_missing_seasons().row_count() > 0 {
                    g.set_series_missing_seasons_focused(0);
                } // else: back to episode row
                true
            }
            Action::Down => {
                if g.get_series_recommended().row_count() > 0 {
                    g.set_series_similar_focused(-1);
                    g.set_series_recommended_focused(0);
                    true
                } else {
                    false // nothing below — let focus_bar_on_down reach the bars
                }
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

    // ── Recommended row (Discover-sourced, 2026-07-29) ────────────────────────
    if in_recommended {
        return match action {
            Action::Left => {
                let idx = g.get_series_recommended_focused();
                if idx > 0 { g.set_series_recommended_focused(idx - 1); }
                true
            }
            Action::Right => {
                let idx = g.get_series_recommended_focused();
                if idx < g.get_series_recommended().row_count() as i32 - 1 {
                    g.set_series_recommended_focused(idx + 1);
                }
                true
            }
            Action::Up => {
                g.set_series_recommended_focused(-1);
                if g.get_series_similar().row_count() > 0 {
                    g.set_series_similar_focused(0);
                } else if g.get_series_cast().row_count() > 0 {
                    g.set_series_cast_focused(0);
                } else if g.get_series_missing_seasons().row_count() > 0 {
                    g.set_series_missing_seasons_focused(0);
                } // else: back to episode row
                true
            }
            Action::Confirm => {
                let idx = g.get_series_recommended_focused() as usize;
                if let Some(card) = g.get_series_recommended().row_data(idx) {
                    let media_type = if card.item_type == "DiscoverMovie" { "movie" } else { "tv" };
                    g.invoke_open_discover_item(media_type.into(), card.id);
                }
                true
            }
            Action::OpenContextMenu => {
                let idx = g.get_series_recommended_focused() as usize;
                if let Some(card) = g.get_series_recommended().row_data(idx) {
                    g.invoke_open_context_menu_discover(card);
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
            if g.get_series_missing_seasons().row_count() > 0 {
                g.set_series_missing_seasons_focused(0);
                true
            } else if g.get_series_cast().row_count() > 0 {
                g.set_series_cast_focused(0);
                true
            } else if g.get_series_similar().row_count() > 0 {
                g.set_series_similar_focused(0);
                true
            } else if g.get_series_recommended().row_count() > 0 {
                g.set_series_recommended_focused(0);
                true
            } else {
                false // nothing below — let focus_bar_on_down reach the bars (CR10-19)
            }
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
