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
//                       !in-film-row && !in-other-work-row: Down→filmography, or straight to
//                       other-work when filmography is empty (2026-08-13 fix — see its own
//                       comment), Back/Enter→close
//                       in-film-row: Up→back, Down→other-work (if non-empty), Left/Right navigate,
//                       Enter→open-detail, C→ctx-menu
//                       in-other-work-row: Up→filmography, Left/Right navigate, Enter→open-
//                       discover-item (in-library redirect handled there), C→discover ctx-menu
//   open_person_from_discover  entry point for a Discover-context cast member (RequestDetailScreen's
//                       CastRow, previously unclickable — "item-selected left unbound"), 2026-08-13.
//                       Resolves a local Jellyfin Person first (resolve_local_person), opens the
//                       real native screen above when found, else open_person_screen_tmdb (TMDB-
//                       only bio + full filmography, no local filmography row at all)
//   resolve_local_person  best-effort TMDB person id -> local Jellyfin Person id (name search +
//                       ProviderIds cross-check, single-candidate fallback), cached in
//                       local_person_by_tmdb_cache
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

    let id_revalidate    = id.clone();
    let state_revalidate = Arc::clone(&state);
    let ww_revalidate    = ww.clone();
    let rt_revalidate    = rt.clone();

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
            // Session guard (Bonfire Phase 1, step 8 audit, 2026-08-09) —
            // the id check above now catches most of this (reset_session_state
            // clears person-id on a switch/sign-out), but a coincidental
            // same-id reopen under a NEW profile before this stale fetch
            // resolves would still slip through an id check alone. Same
            // guard class as spawn_person_revalidate's own, just also
            // applied to the actual open-screen path, not only its
            // background revalidate sibling.
            if !crate::session_current(&state, &client) { return; }
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

    // Cache-hit only: the screen above already showed instantly from cached
    // data. Real gap, live-reported: Jellyfin's WebSocket only delivers
    // LibraryChanged to the most-recently-connected client when multiple
    // clients share a session (JELLYFIN.md) — this can silently starve Fjord
    // of the event, leaving these caches stale indefinitely with no other
    // fallback. This revalidation is what closes that gap for whatever's
    // actually on screen right now.
    if is_cache_hit {
        spawn_person_revalidate(id_revalidate, state_revalidate, ww_revalidate, rt_revalidate);
    }
}

fn spawn_person_revalidate(
    id:    String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    if !crate::should_revalidate(&state, &id) { return; }
    let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
    rt.spawn(async move {
        let (detail_res, film_res) = tokio::join!(client.get_item_detail(&id), client.get_person_filmography(&id));
        let (Ok(detail), Ok(film_items)) = (detail_res, film_res) else { return };
        // Sign-out (or a different account signing in on a shared HTPC)
        // mid-fetch must not let this per-user data land in the new session's
        // cache — same guard class as main.rs::session_current's own doc
        // comment (CR11-2).
        if !crate::session_current(&state, &client) { return; }
        {
            let mut s = state.lock().unwrap();
            s.item_detail_cache.insert(id.clone(), detail.clone());
            s.person_filmography_cache.insert(id.clone(), film_items.clone());
        }
        let bio = crate::strip_html_to_text(detail.overview.clone().unwrap_or_default().trim());
        let film_bufs = fetch_card_posters(&client, &film_items).await;
        let id_guard  = id.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            if AppState::get(&w).get_person_id().as_str() != id_guard { return; }
            let g = AppState::get(&w);
            if !bio.is_empty() { g.set_person_bio(bio.as_str().into()); }
            let fresh = items_to_cards(&film_items, film_bufs);
            g.set_person_filmography(crate::apply_cards_preserving_identity(&g.get_person_filmography(), fresh));
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

// ── Person detail from a Discover-context cast member (2026-08-13) ────────────

/// Entry point for opening person detail from a Discover-context cast
/// member — `RequestDetailScreen`'s `CastRow` previously left
/// `item-selected` deliberately unbound ("there's no TMDB-person detail
/// screen to open"). Scoped via `AskUserQuestion`: user picked "Try local
/// match first, TMDB fallback" over always-TMDB-only or local-only-no-
/// fallback. Resolves a local Jellyfin `Person` match (`resolve_local_person`)
/// and opens the real native screen (`open_person_screen`, above — real
/// bio/filmography/watch-state) when found; falls back to
/// `open_person_screen_tmdb` (TMDB-sourced bio + full filmography, no local
/// filmography row) otherwise. `tmdb_id` is a plain decimal string (as
/// carried on `CastMember.id` for a Discover-sourced cast row, itself
/// straight from TMDB's own `Credits` response) — a parse failure means the
/// caller passed something that isn't actually a TMDB cast row, so this
/// silently no-ops rather than guessing.
pub(crate) fn open_person_from_discover(
    tmdb_id: String,
    name:    String,
    state:   Arc<Mutex<FjordState>>,
    ww:      slint::Weak<MainWindow>,
    rt:      tokio::runtime::Handle,
) {
    let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else { return };
    let Ok(tmdb_num) = tmdb_id.parse::<i64>() else {
        warn!("open_person_from_discover: {tmdb_id:?} doesn't parse as a TMDB id");
        return;
    };
    let state2 = Arc::clone(&state);
    let ww2    = ww.clone();
    let rt2    = rt.clone();
    rt.spawn(async move {
        match resolve_local_person(&client, &state2, tmdb_num, &name).await {
            Some(local_id) => open_person_screen(local_id, name, state2, ww2, rt2),
            None           => open_person_screen_tmdb(tmdb_num, name, state2, ww2, rt2),
        }
    });
}

/// Best-effort TMDB person id -> local Jellyfin Person id. High-confidence
/// path: any name-search candidate whose own `ProviderIds.Tmdb` matches the
/// known id exactly. Lower-confidence fallback, only when the search
/// returns EXACTLY ONE candidate (name search is Jellyfin's own fuzzy
/// match, not this function's) and none had a `ProviderIds` match either
/// way — accepting a single unambiguous candidate mirrors this codebase's
/// own established tolerance for `resolve_person_tmdb_id`'s identical
/// single-candidate fallback in the opposite direction, above. Two or more
/// same-named candidates with no `ProviderIds` to disambiguate them is
/// deliberately treated as "no confident match," not a coin flip — the
/// exact false-match risk flagged to the user when this design was chosen.
/// Cached (hit or miss) in `local_person_by_tmdb_cache`.
async fn resolve_local_person(
    client:  &Arc<fjord_api::JellyfinClient>,
    state:   &Arc<Mutex<FjordState>>,
    tmdb_id: i64,
    name:    &str,
) -> Option<String> {
    let key = tmdb_id.to_string();
    if let Some(cached) = state.lock().unwrap().local_person_by_tmdb_cache.get(&key) {
        debug!("resolve_local_person({tmdb_id}): cache hit -> {cached:?}");
        return cached;
    }
    let candidates = match client.search_persons_by_name(name).await {
        Ok(v) => v,
        Err(e) => {
            warn!("search_persons_by_name({name:?}): {e:#}");
            state.lock().unwrap().local_person_by_tmdb_cache.insert(key, None);
            return None;
        }
    };
    let resolved = candidates.iter()
        .find(|c| c.provider_ids.get("Tmdb").is_some_and(|t| t == &key))
        .map(|c| c.id.clone())
        .or_else(|| if candidates.len() == 1 { Some(candidates[0].id.clone()) } else { None });
    debug!("resolve_local_person({tmdb_id}, {name:?}): {} candidate(s) -> {resolved:?}", candidates.len());
    state.lock().unwrap().local_person_by_tmdb_cache.insert(key, resolved.clone());
    resolved
}

/// TMDB-only person screen for a Discover-context cast member with no local
/// Jellyfin Person match. Reuses the exact same `AppState.person-*`
/// properties/models as the native screen (`person.slint` has no idea
/// which path populated them) — bio/portrait come from TMDB's
/// `GET /person/{id}` instead of Jellyfin's `get_item_detail`, and
/// `person-filmography` stays empty (there is no local data at all, and
/// `person.slint` already conditionally hides that row when empty, so this
/// isn't a new code path there) while `person-other-work` carries the
/// person's full TMDB filmography via the exact same
/// `build_person_credit_metas`/`resolve_and_fetch_discovery_row` pipeline
/// `spawn_other_work` already uses — still excluding anything that
/// resolves to a locally-owned item, since an individual title can be
/// locally owned even when the Person entity itself wasn't matched.
/// `person-id` is set to a synthetic `"tmdb:<id>"` key (can never collide
/// with a real Jellyfin GUID) purely so this file's existing stale-result
/// guards (`if g.get_person_id().as_str() != ...`) keep working unchanged.
fn open_person_screen_tmdb(
    tmdb_id: i64,
    name:    String,
    state:   Arc<Mutex<FjordState>>,
    ww:      slint::Weak<MainWindow>,
    rt:      tokio::runtime::Handle,
) {
    let Some(seerr) = state.lock().unwrap().seerr_client.clone() else { return };
    let synthetic_id = format!("tmdb:{tmdb_id}");
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        // Real bug, live-reported 2026-08-19 ("Dose still not work") —
        // confirmed from the log: 6 identical fetches for the same
        // tmdb_id fired within ~1s, no re-entrancy guard at all. Each
        // press re-set app-content-loading=true synchronously; with
        // several overlapping async fetches in flight, the LAST "true"
        // write could land after an EARLIER fetch's own "false"
        // completion (this function's own final commit closure below),
        // leaving the loading overlay stuck covering an already-correctly-
        // populated PersonScreen forever — indistinguishable from "nothing
        // happens" at all. A repeat press for the exact same target while
        // one is already resolving is now a no-op; a genuinely different
        // target still interrupts and starts fresh, since person-id would
        // differ.
        if g.get_person_id().as_str() == synthetic_id && g.get_app_content_loading() {
            return;
        }
        g.set_person_id(synthetic_id.as_str().into());
        g.set_person_name(name.as_str().into());
        g.set_person_bio("".into());
        g.set_person_has_portrait(false);
        g.set_person_filmography(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_person_film_focused(0);
        g.set_person_in_film_row(false);
        g.set_person_other_work(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_person_other_work_focused(0);
        g.set_person_in_other_work_row(false);
        g.set_app_content_loading(true);
        g.set_app_loading_progress(0.0);
    }
    let ww2      = ww.clone();
    let seerr2   = Arc::clone(&seerr);
    let state2   = Arc::clone(&state);
    rt.spawn(async move {
        let http = reqwest::Client::builder().timeout(std::time::Duration::from_secs(30)).build().ok();
        let (person_res, credits_res) = tokio::join!(seerr.get_person(tmdb_id), seerr.get_person_combined_credits(tmdb_id));
        if let Err(e) = &person_res {
            warn!("open_person_screen_tmdb({tmdb_id}): get_person: {e:#}");
        }
        let bio = person_res.as_ref().ok()
            .and_then(|p| p.biography.clone())
            .map(|b| crate::strip_html_to_text(b.trim()))
            .unwrap_or_default();
        let profile_path = person_res.ok().and_then(|p| p.profile_path);
        let portrait_buf = match (&http, profile_path) {
            (Some(h), Some(path)) => {
                discover::fetch_tmdb_image(h, discover::TMDB_PROFILE_BASE, &path, &format!("person-{tmdb_id}"))
                    .await
                    .and_then(|b| decode_poster_buffer(&b))
            }
            _ => None,
        };
        let items = match credits_res {
            Ok(c) => discover::build_person_credit_metas(&c),
            Err(e) => {
                warn!("open_person_screen_tmdb({tmdb_id}): get_person_combined_credits: {e:#}");
                Vec::new()
            }
        };
        let ready = discover::resolve_and_fetch_discovery_row(&state, items, 20).await;
        debug!("open_person_screen_tmdb({tmdb_id}): {} filmography card(s)", ready.len());
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            // Session guard, same class as every other Seerr-fetch commit
            // closure in this codebase (Bonfire Phase 1 step 8 audit) — a
            // sign-out/profile-switch/Seerr-disconnect mid-fetch must not
            // let this land in the new session's UI.
            if !crate::seerr_session_current(&state2, &seerr2) { return; }
            let g = AppState::get(&w);
            if g.get_person_id().as_str() != synthetic_id { return; }
            if !bio.is_empty() { g.set_person_bio(bio.as_str().into()); }
            if let Some(buf) = portrait_buf {
                g.set_person_portrait(slint::Image::from_rgba8(buf));
                g.set_person_has_portrait(true);
            }
            let cards = discover::discover_cards_from(ready);
            g.set_person_other_work(ModelRc::new(VecModel::from(cards)));
            g.set_show_person(true);
            g.set_app_content_loading(false);
            g.set_app_loading_progress(0.0);
            w.invoke_grab_keyboard_focus();
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
            if !in_film && !in_other_work {
                if g.get_person_filmography().row_count() > 0 {
                    g.set_person_in_film_row(true);
                } else if g.get_person_other_work().row_count() > 0 {
                    // Real dead-end, found 2026-08-13 while adding the
                    // TMDB-only person screen: that screen always has an
                    // empty filmography row (no local data at all), and
                    // this branch previously only ever transitioned
                    // header→film-row or film-row→other-work-row — with
                    // filmography empty, Down from the header did nothing,
                    // making Other Work keyboard-unreachable even though
                    // it's the only row present.
                    g.set_person_in_other_work_row(true);
                }
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
