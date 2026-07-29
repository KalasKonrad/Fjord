// ── fjord-app · person.rs ─────────────────────────────────────────────────────
//   open_person_screen  now takes state (not client) so it can check item_detail_cache +
//                       person_filmography_cache (Part 2) — only sets app-content-loading=true
//                       when either is a miss; reset AppState person props, spawn async fetch
//                       (portrait + bio + filmography in parallel, cached ones skip their
//                       network call), emit app-loading-progress=0.5, then show person on
//                       completion; separately spawns spawn_other_work (independent task, doesn't
//                       block the main page from showing — see that fn's own doc comment)
//   resolve_person_tmdb_id  best-effort Jellyfin person -> TMDB person id (ProviderIds on the
//                       Person item, falling back to a fuzzy Seerr name search); cached either
//                       way (including a None miss) in person_tmdb_id_cache (2026-07-29, Deep
//                       Seerr integration)
//   spawn_other_work    independent task: resolves TMDB person id, fetches combined_credits,
//                       filters to not-already-owned (resolve_and_fetch_discovery_row), commits
//                       to person-other-work — a second, Discover-flavored SectionRow below the
//                       local-only filmography row (2026-07-29, Deep Seerr integration)
//   handle_key          keyboard dispatch for the person screen:
//                       !in-film-row && !in-other-work-row: Down→filmography, Back/Enter→close
//                       in-film-row: Up→back, Down→other-work (if non-empty), Left/Right navigate,
//                       Enter→open-detail, C→ctx-menu
//                       in-other-work-row: Up→filmography, Left/Right navigate, Enter→open-
//                       discover-item (in-library redirect handled there), C→discover ctx-menu
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::{debug, warn};

use crate::config::FjordState;
use crate::discover;
use crate::AppState;
use crate::detail::{fetch_card_posters, items_to_cards};
use crate::poster::{decode_poster_buffer, fetch_poster_cached};
use crate::{CardItem, MainWindow};

// ── open_person_screen ────────────────────────────────────────────────────────

pub(crate) fn open_person_screen(
    id:    String,
    name:  String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    // Screen-open cache (Part 2): skip the loading spinner when both the bio
    // (via detail) and filmography are cached — the remaining work (portrait +
    // film-poster fetch) is disk-cached and fast enough to feel instant.
    let (client, cached_detail, cached_film) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        (c, s.item_detail_cache.get(&id), s.person_filmography_cache.get(&id))
    };
    let is_cache_hit = cached_detail.is_some() && cached_film.is_some();
    tracing::debug!("open_person_screen({id}): cache_hit={is_cache_hit}");

    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_person_id(id.as_str().into());
        g.set_person_name(name.as_str().into());
        g.set_person_bio("".into());
        g.set_person_has_portrait(false);
        g.set_person_filmography(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_person_film_focused(0);
        g.set_person_in_film_row(false);
        g.set_person_other_work(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_person_other_work_focused(0);
        g.set_person_in_other_work_row(false);
        if !is_cache_hit {
            g.set_app_content_loading(true);
        }
        g.set_app_loading_progress(0.0);
    }

    let ww2 = ww.clone();

    spawn_other_work(id.clone(), name.clone(), Arc::clone(&state), ww.clone(), rt.clone(), cached_detail.clone(), Arc::clone(&client));

    rt.spawn(async move {
        let detail_fut = async {
            if let Some(d) = cached_detail { return Ok(d); }
            client.get_item_detail(&id).await
        };
        let film_fut = async {
            if let Some(v) = cached_film { return Ok(v); }
            client.get_person_filmography(&id).await
        };
        let (detail_res, poster_bytes, film_res) = tokio::join!(
            detail_fut,
            fetch_poster_cached(&client, &id),
            film_fut,
        );

        if let Ok(d) = &detail_res {
            state.lock().unwrap().item_detail_cache.insert(id.clone(), d.clone());
        }
        let bio = detail_res.ok()
            .and_then(|d| d.overview)
            .unwrap_or_default()
            .trim()
            .to_string();

        if let Ok(v) = &film_res {
            state.lock().unwrap().person_filmography_cache.insert(id.clone(), v.clone());
        }
        let film_items = film_res.unwrap_or_else(|e| {
            warn!("get_person_filmography {}: {:#}", id, e);
            vec![]
        });

        let id_prog = id.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            if AppState::get(&w).get_person_id().as_str() != id_prog { return; }
            AppState::get(&w).set_app_loading_progress(0.5);
        });

        let film_bufs  = fetch_card_posters(&client, &film_items).await;
        let poster_buf = poster_bytes.as_deref().and_then(decode_poster_buffer);
        let has_poster = poster_buf.is_some();
        let id_guard   = id.clone();

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            if AppState::get(&w).get_person_id().as_str() != id_guard { return; }
            let g = AppState::get(&w);
            if !bio.is_empty() { g.set_person_bio(bio.as_str().into()); }
            if let Some(buf) = poster_buf {
                g.set_person_portrait(slint::Image::from_rgba8(buf));
                g.set_person_has_portrait(has_poster);
            }
            if !film_items.is_empty() {
                let fresh = items_to_cards(&film_items, film_bufs);
                g.set_person_filmography(crate::apply_cards_preserving_identity(&g.get_person_filmography(), fresh));
            }
            g.set_show_person(true);
            g.set_app_content_loading(false);
            g.set_app_loading_progress(0.0);
            w.invoke_grab_keyboard_focus();
        });
    });
}

// ── Other Work row (2026-07-29, Deep Seerr integration) ───────────────────────

/// Best-effort Jellyfin person -> TMDB person id resolution. First tries
/// `ProviderIds` on the Person item itself (free once `get_item_detail`'s
/// Fields gained `ProviderIds`, 2026-07-29 — `cached_detail` is usually
/// already in hand from the main fetch, so this is often a zero-network-call
/// check); if absent (the common case — Jellyfin's own Person metadata is
/// usually much thinner than movie/series metadata), falls back to a fuzzy
/// `SeerrClient::search` by name (persons are normally filtered out of
/// search results by callers, not the crate itself — this is the one
/// deliberate exception). Cached either way, including a `None` miss, so a
/// failed resolution isn't retried on every visit to the same person.
async fn resolve_person_tmdb_id(
    client:        &Arc<fjord_api::JellyfinClient>,
    seerr:         &Arc<fjord_seerr::SeerrClient>,
    state:         &Arc<Mutex<FjordState>>,
    id:            &str,
    name:          &str,
    cached_detail: Option<fjord_api::models::MediaItem>,
) -> Option<i64> {
    if let Some(cached) = state.lock().unwrap().person_tmdb_id_cache.get(id) {
        debug!("resolve_person_tmdb_id({id}): cache hit -> {cached:?}");
        return cached;
    }
    let detail = match cached_detail {
        Some(d) => Some(d),
        None => client.get_item_detail(id).await.ok(),
    };
    if let Some(tmdb_id) =
        detail.as_ref().and_then(|d| d.provider_ids.get("Tmdb")).and_then(|s| s.parse::<i64>().ok())
    {
        debug!("resolve_person_tmdb_id({id}): resolved via ProviderIds -> {tmdb_id}");
        state.lock().unwrap().person_tmdb_id_cache.insert(id.to_string(), Some(tmdb_id));
        return Some(tmdb_id);
    }
    let resolved = match seerr.search(name, 1).await {
        Ok(resp) => {
            let persons: Vec<_> = resp.results.iter().filter(|r| r.media_type == "person").collect();
            persons
                .iter()
                .find(|r| r.name.as_deref().is_some_and(|n| n.eq_ignore_ascii_case(name)))
                .or_else(|| persons.first())
                .map(|r| r.id)
        }
        Err(e) => {
            warn!("seerr: person search for {name:?} failed: {e:#}");
            None
        }
    };
    debug!("resolve_person_tmdb_id({id}): fuzzy search for {name:?} -> {resolved:?}");
    state.lock().unwrap().person_tmdb_id_cache.insert(id.to_string(), resolved);
    resolved
}

/// Independent task, deliberately not part of `open_person_screen`'s own
/// `rt.spawn` block — this row's resolution (a fuzzy name search in the
/// common no-ProviderIds case, then a full combined_credits fetch) can
/// genuinely take longer or fail outright, and shouldn't hold up the main
/// page (bio/portrait/filmography) from showing. Silently does nothing when
/// Seerr isn't connected, or when TMDB resolution fails — same `if
/// .length > 0` idiom as every other conditional row in this codebase, no
/// error surfaced to the user for what is an inherently best-effort feature.
fn spawn_other_work(
    id:            String,
    name:          String,
    state:         Arc<Mutex<FjordState>>,
    ww:            slint::Weak<MainWindow>,
    rt:            tokio::runtime::Handle,
    cached_detail: Option<fjord_api::models::MediaItem>,
    client:        Arc<fjord_api::JellyfinClient>,
) {
    let Some(seerr) = state.lock().unwrap().seerr_client.clone() else { return };
    rt.spawn(async move {
        let Some(tmdb_id) = resolve_person_tmdb_id(&client, &seerr, &state, &id, &name, cached_detail).await else {
            debug!("spawn_other_work({id}): no tmdb id resolved for {name:?} — row will not show");
            return;
        };
        let cache_key = tmdb_id.to_string();
        let cached = state.lock().unwrap().person_other_work_cache.get(&cache_key);
        let items = match cached {
            Some(v) => v,
            None => match seerr.get_person_combined_credits(tmdb_id).await {
                Ok(credits) => {
                    let built = discover::build_person_credit_metas(&credits);
                    state.lock().unwrap().person_other_work_cache.insert(cache_key, built.clone());
                    built
                }
                Err(e) => {
                    warn!("seerr: get_person_combined_credits({tmdb_id}): {e:#}");
                    return;
                }
            },
        };
        let ready = discover::resolve_and_fetch_discovery_row(&state, items, 20).await;
        debug!("spawn_other_work({id}): tmdb={tmdb_id} -> {} card(s) after owned-filter", ready.len());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_person_id().as_str() != id { return; }
            let cards = discover::discover_cards_from(ready);
            g.set_person_other_work(crate::apply_cards_preserving_identity(&g.get_person_other_work(), cards));
        });
    });
}

// ── Keyboard dispatch ─────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;
    let in_film       = g.get_person_in_film_row();
    let in_other_work = g.get_person_in_other_work_row();
    match action {
        Action::Back => {
            g.set_person_in_film_row(false);
            g.set_person_in_other_work_row(false);
            g.invoke_close_person();
            true
        }
        Action::Down => {
            if !in_film && !in_other_work && g.get_person_filmography().row_count() > 0 {
                g.set_person_in_film_row(true);
            } else if in_film && g.get_person_other_work().row_count() > 0 {
                g.set_person_in_film_row(false);
                g.set_person_in_other_work_row(true);
            }
            true
        }
        Action::Up => {
            if in_other_work {
                g.set_person_in_other_work_row(false);
                g.set_person_in_film_row(true);
                true
            } else if in_film {
                g.set_person_in_film_row(false);
                true
            } else { false }
        }
        Action::Left => {
            if in_other_work {
                let idx = g.get_person_other_work_focused();
                if idx > 0 { g.set_person_other_work_focused(idx - 1); }
                true
            } else if in_film {
                let idx = g.get_person_film_focused();
                if idx > 0 { g.set_person_film_focused(idx - 1); }
                true
            } else { false }
        }
        Action::Right => {
            if in_other_work {
                let idx = g.get_person_other_work_focused();
                let max = g.get_person_other_work().row_count() as i32 - 1;
                if idx < max { g.set_person_other_work_focused(idx + 1); }
                true
            } else if in_film {
                let idx = g.get_person_film_focused();
                let max = g.get_person_filmography().row_count() as i32 - 1;
                if idx < max { g.set_person_film_focused(idx + 1); }
                true
            } else { false }
        }
        Action::Confirm => {
            if in_other_work {
                let idx = g.get_person_other_work_focused() as usize;
                if let Some(card) = g.get_person_other_work().row_data(idx) {
                    let media_type = if card.item_type == "DiscoverMovie" { "movie" } else { "tv" };
                    g.invoke_open_discover_item(media_type.into(), card.id);
                }
            } else if in_film {
                let idx = g.get_person_film_focused() as usize;
                if let Some(card) = g.get_person_filmography().row_data(idx) {
                    g.invoke_open_detail(card.id, card.item_type);
                }
            } else {
                g.invoke_close_person();
            }
            true
        }
        Action::OpenContextMenu => {
            if in_other_work {
                let idx = g.get_person_other_work_focused() as usize;
                if let Some(card) = g.get_person_other_work().row_data(idx) {
                    g.invoke_open_context_menu_discover(card);
                }
            } else if in_film {
                let idx = g.get_person_film_focused() as usize;
                if let Some(card) = g.get_person_filmography().row_data(idx) {
                    g.set_context_menu_title(card.title.clone());
                    g.invoke_open_context_menu(
                        card.id, card.has_played, card.is_favorite,
                        card.resume_pct, card.item_type, card.series_id,
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
