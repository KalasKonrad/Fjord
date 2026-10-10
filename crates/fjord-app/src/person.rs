// ── fjord-app · person.rs ─────────────────────────────────────────────────────
//   open_person_screen  reset the person props; portrait + bio + filmography in parallel (cached
//                       parts from item_detail_cache/person_filmography_cache skip the network;
//                       app-content-loading only on a miss), then show; spawns spawn_other_work;
//                       a cache hit revalidates (spawn_person_revalidate). Session-guarded.
//   resolve_person_tmdb_id  Jellyfin person → TMDB person id (ProviderIds, else a Seerr name
//                       search); cached incl. misses in person_tmdb_id_cache
//   spawn_other_work    TMDB combined credits minus what's in the library
//                       (resolve_and_fetch_discovery_row) → person-other-work, the Discover-style
//                       row under the local filmography; doesn't hold up the page
//   open_person_from_discover  a Discover cast member: local Person match (resolve_local_person) →
//                       the native screen, else open_person_screen_tmdb (TMDB bio + filmography,
//                       no local row); one resolve at a time per person
//   resolve_local_person  TMDB person id → local Person (name search + ProviderIds, or a single
//                       unambiguous candidate); cached in local_person_by_tmdb_cache
//   handle_key          header: Down → filmography (or Other Work when it's empty), Back/Enter
//                       close; filmography: Up/Down, Left/Right, Enter open, C menu; Other Work:
//                       Up, Left/Right, Enter open-discover-item, C Discover menu
//   wire_person         callbacks moved from main() (0.5.0 step 3): person screen
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::{debug, warn};

use crate::AppState;
use crate::config::FjordState;
use crate::detail::{fetch_card_posters, items_to_cards};
use crate::discover;
use crate::poster::{decode_poster_buffer, fetch_poster_cached};
use crate::{CardItem, MainWindow};

// ── open_person_screen ────────────────────────────────────────────────────────

pub(crate) fn open_person_screen(
    id: String,
    name: String,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    // Screen-open cache (Part 2): skip the loading spinner when both the bio
    // (via detail) and filmography are cached — the remaining work (portrait +
    // film-poster fetch) is disk-cached and fast enough to feel instant.
    let (client, cached_detail, cached_film) = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else {
            return;
        };
        (
            c,
            s.item_detail_cache.get(&id),
            s.person_filmography_cache.get(&id),
        )
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

    spawn_other_work(
        id.clone(),
        name.clone(),
        Arc::clone(&state),
        ww.clone(),
        rt.clone(),
        cached_detail.clone(),
        Arc::clone(&client),
    );

    let id_revalidate = id.clone();
    let state_revalidate = Arc::clone(&state);
    let ww_revalidate = ww.clone();
    let rt_revalidate = rt.clone();

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
            // Logged on every path, including both guards below — a silent bail here once made a
            // live bug undiagnosable.
            let Some(w) = ww.upgrade() else {
                debug!("open_person_screen({id_guard}): commit aborted — window gone");
                return;
            };
            let current_id = AppState::get(&w).get_person_id();
            if current_id.as_str() != id_guard {
                debug!("open_person_screen({id_guard}): commit skipped — person-id changed to {current_id:?} meanwhile");
                return;
            }
            // Session guard: the id check above catches most stale results (reset_session_state
            // clears person-id), but not the same id reopened under a new profile before this fetch
            // resolves.
            if !crate::session_current(&state, &client) {
                debug!("open_person_screen({id_guard}): commit skipped — session changed meanwhile");
                return;
            }
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
            debug!("open_person_screen({id_guard}): committed — show-person=true, bio_len={} filmography={}", bio.len(), film_items.len());
        });
    });

    // Cache hit: the screen already showed from cache. Revalidate what's on screen — Jellyfin
    // only sends LibraryChanged to the most recently connected client of a shared session
    // (JELLYFIN.md), so the caches can otherwise stay stale.
    if is_cache_hit {
        spawn_person_revalidate(
            id_revalidate,
            state_revalidate,
            ww_revalidate,
            rt_revalidate,
        );
    }
}

fn spawn_person_revalidate(
    id: String,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    if !crate::should_revalidate(&state, &id) {
        return;
    }
    let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else {
        return;
    };
    rt.spawn(async move {
        let (detail_res, film_res) = tokio::join!(
            client.get_item_detail(&id),
            client.get_person_filmography(&id)
        );
        let (Ok(detail), Ok(film_items)) = (detail_res, film_res) else {
            return;
        };
        // Per-user data must not land in a new session's cache after a mid-fetch sign-out or
        // switch.
        if !crate::session_current(&state, &client) {
            return;
        }
        {
            let mut s = state.lock().unwrap();
            s.item_detail_cache.insert(id.clone(), detail.clone());
            s.person_filmography_cache
                .insert(id.clone(), film_items.clone());
        }
        let bio = crate::strip_html_to_text(detail.overview.clone().unwrap_or_default().trim());
        let film_bufs = fetch_card_posters(&client, &film_items).await;
        let id_guard = id.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            if AppState::get(&w).get_person_id().as_str() != id_guard {
                return;
            }
            let g = AppState::get(&w);
            if !bio.is_empty() {
                g.set_person_bio(bio.as_str().into());
            }
            let fresh = items_to_cards(&film_items, film_bufs);
            g.set_person_filmography(crate::apply_cards_preserving_identity(
                &g.get_person_filmography(),
                fresh,
            ));
        });
    });
}

// ── Other Work row (2026-07-29, Deep Seerr integration) ───────────────────────

/// Best-effort Jellyfin person → TMDB person id: ProviderIds on the Person item first (often
/// already in `cached_detail`, no network), else a fuzzy SeerrClient::search by name (the one
/// caller that keeps person results). Cached either way, including a None miss.
async fn resolve_person_tmdb_id(
    client: &Arc<fjord_api::JellyfinClient>,
    seerr: &Arc<fjord_seerr::SeerrClient>,
    state: &Arc<Mutex<FjordState>>,
    id: &str,
    name: &str,
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
    if let Some(tmdb_id) = detail
        .as_ref()
        .and_then(|d| d.provider_ids.get("Tmdb"))
        .and_then(|s| s.parse::<i64>().ok())
    {
        debug!("resolve_person_tmdb_id({id}): resolved via ProviderIds -> {tmdb_id}");
        state
            .lock()
            .unwrap()
            .person_tmdb_id_cache
            .insert(id.to_string(), Some(tmdb_id));
        return Some(tmdb_id);
    }
    let resolved = match seerr.search(name, 1).await {
        Ok(resp) => {
            let persons: Vec<_> = resp
                .results
                .iter()
                .filter(|r| r.media_type == "person")
                .collect();
            persons
                .iter()
                .find(|r| {
                    r.name
                        .as_deref()
                        .is_some_and(|n| n.eq_ignore_ascii_case(name))
                })
                .or_else(|| persons.first())
                .map(|r| r.id)
        }
        Err(e) => {
            warn!("seerr: person search for {name:?} failed: {e:#}");
            None
        }
    };
    debug!("resolve_person_tmdb_id({id}): fuzzy search for {name:?} -> {resolved:?}");
    state
        .lock()
        .unwrap()
        .person_tmdb_id_cache
        .insert(id.to_string(), resolved);
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
    id: String,
    name: String,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
    cached_detail: Option<fjord_api::models::MediaItem>,
    client: Arc<fjord_api::JellyfinClient>,
) {
    let Some(seerr) = state.lock().unwrap().seerr_client.clone() else {
        return;
    };
    rt.spawn(async move {
        let Some(tmdb_id) =
            resolve_person_tmdb_id(&client, &seerr, &state, &id, &name, cached_detail).await
        else {
            debug!("spawn_other_work({id}): no tmdb id resolved for {name:?} — row will not show");
            return;
        };
        let cache_key = tmdb_id.to_string();
        let cached = state
            .lock()
            .unwrap()
            .person_other_work_cache
            .get(&cache_key);
        let items = match cached {
            Some(v) => v,
            None => match seerr.get_person_combined_credits(tmdb_id).await {
                Ok(credits) => {
                    let built = discover::build_person_credit_metas(&credits);
                    state
                        .lock()
                        .unwrap()
                        .person_other_work_cache
                        .insert(cache_key, built.clone());
                    built
                }
                Err(e) => {
                    warn!("seerr: get_person_combined_credits({tmdb_id}): {e:#}");
                    return;
                }
            },
        };
        let ready = discover::resolve_and_fetch_discovery_row(&state, items, 20).await;
        debug!(
            "spawn_other_work({id}): tmdb={tmdb_id} -> {} card(s) after owned-filter",
            ready.len()
        );
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            if g.get_person_id().as_str() != id {
                return;
            }
            let cards = discover::discover_cards_from(ready);
            g.set_person_other_work(crate::apply_cards_preserving_identity(
                &g.get_person_other_work(),
                cards,
            ));
        });
    });
}

// ── Person detail from a Discover-context cast member (2026-08-13) ────────────

/// Opens person detail from a Discover cast member (RequestDetailScreen's CastRow): a local
/// Jellyfin Person match (resolve_local_person) opens the native screen (open_person_screen —
/// bio, filmography, watch state); otherwise the TMDB fallback (open_person_screen_tmdb — bio +
/// full filmography, no local row). `tmdb_id` is the decimal TMDB id from CastMember.id; a
/// parse failure is a silent no-op.
pub(crate) fn open_person_from_discover(
    tmdb_id: String,
    name: String,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else {
        return;
    };
    let Ok(tmdb_num) = tmdb_id.parse::<i64>() else {
        warn!("open_person_from_discover: {tmdb_id:?} doesn't parse as a TMDB id");
        return;
    };
    // One resolve per person at a time (see person_discover_resolving in config.rs): repeat
    // presses used to start redundant chains. Both downstream paths pass through here.
    {
        let mut s = state.lock().unwrap();
        if s.person_discover_resolving == Some(tmdb_num) {
            debug!(
                "open_person_from_discover({tmdb_num}): already resolving, ignoring repeat press"
            );
            return;
        }
        s.person_discover_resolving = Some(tmdb_num);
    }
    let state2 = Arc::clone(&state);
    let ww2 = ww.clone();
    let rt2 = rt.clone();
    rt.spawn(async move {
        let resolved = resolve_local_person(&client, &state2, tmdb_num, &name).await;
        // Cleared here, not in either downstream function's own commit
        // closure — open_person_screen is shared by 6+ unrelated call
        // sites that never touch this field, and open_person_screen_tmdb
        // already has its own narrower re-entry guard once this point is
        // reached, so the remaining race window (a repeat press landing in
        // the instant between this clear and that guard's own synchronous
        // check) is microseconds, not the hundreds-of-ms cache-hit window
        // this fix actually closes.
        state2.lock().unwrap().person_discover_resolving = None;
        // Back on the UI thread before calling either opener: they set AppState (person-id …) at
        // the top of their bodies, and from this Tokio worker Weak::upgrade() silently returns None
        // — so the commit later saw an empty person-id and skipped. Their own rt.spawn calls work
        // from any thread.
        let _ = slint::invoke_from_event_loop(move || match resolved {
            Some(local_id) => open_person_screen(local_id, name, state2, ww2, rt2),
            None => open_person_screen_tmdb(tmdb_num, name, state2, ww2, rt2),
        });
    });
}

/// Best-effort TMDB person id → local Jellyfin Person id: a name-search candidate whose
/// ProviderIds.Tmdb matches exactly; otherwise, only if the search returns EXACTLY ONE
/// candidate, that one (like resolve_person_tmdb_id's single-candidate fallback). Several
/// same-named candidates without ProviderIds = no confident match. Cached (hit or miss) in
/// local_person_by_tmdb_cache.
async fn resolve_local_person(
    client: &Arc<fjord_api::JellyfinClient>,
    state: &Arc<Mutex<FjordState>>,
    tmdb_id: i64,
    name: &str,
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
            state
                .lock()
                .unwrap()
                .local_person_by_tmdb_cache
                .insert(key, None);
            return None;
        }
    };
    let resolved = candidates
        .iter()
        .find(|c| c.provider_ids.get("Tmdb").is_some_and(|t| t == &key))
        .map(|c| c.id.clone())
        .or_else(|| {
            if candidates.len() == 1 {
                Some(candidates[0].id.clone())
            } else {
                None
            }
        });
    debug!(
        "resolve_local_person({tmdb_id}, {name:?}): {} candidate(s) -> {resolved:?}",
        candidates.len()
    );
    state
        .lock()
        .unwrap()
        .local_person_by_tmdb_cache
        .insert(key, resolved.clone());
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
    name: String,
    state: Arc<Mutex<FjordState>>,
    ww: slint::Weak<MainWindow>,
    rt: tokio::runtime::Handle,
) {
    let Some(seerr) = state.lock().unwrap().seerr_client.clone() else {
        return;
    };
    let synthetic_id = format!("tmdb:{tmdb_id}");
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        // A repeat press for the same target while one is resolving is a no-op: overlapping fetches
        // could leave app-content-loading stuck on over a finished screen. A different target still
        // starts fresh (person-id differs).
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
    let ww2 = ww.clone();
    let seerr2 = Arc::clone(&seerr);
    let state2 = Arc::clone(&state);
    rt.spawn(async move {
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .ok();
        let (person_res, credits_res) = tokio::join!(
            seerr.get_person(tmdb_id),
            seerr.get_person_combined_credits(tmdb_id)
        );
        if let Err(e) = &person_res {
            warn!("open_person_screen_tmdb({tmdb_id}): get_person: {e:#}");
        }
        let bio = person_res
            .as_ref()
            .ok()
            .and_then(|p| p.biography.clone())
            .map(|b| crate::strip_html_to_text(b.trim()))
            .unwrap_or_default();
        let profile_path = person_res.ok().and_then(|p| p.profile_path);
        let portrait_buf = match (&http, profile_path) {
            (Some(h), Some(path)) => discover::fetch_tmdb_image(
                h,
                discover::TMDB_PROFILE_BASE,
                &path,
                &format!("person-{tmdb_id}"),
            )
            .await
            .and_then(|b| decode_poster_buffer(&b)),
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
        debug!(
            "open_person_screen_tmdb({tmdb_id}): {} filmography card(s)",
            ready.len()
        );
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            // Session guard: a sign-out/profile switch/Seerr disconnect mid-fetch must not land
            // here.
            if !crate::seerr_session_current(&state2, &seerr2) {
                return;
            }
            let g = AppState::get(&w);
            if g.get_person_id().as_str() != synthetic_id {
                return;
            }
            if !bio.is_empty() {
                g.set_person_bio(bio.as_str().into());
            }
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
    let in_film = g.get_person_in_film_row();
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
                    // Down from the header goes to Other Work when the filmography row is empty
                    // (always on the TMDB-only screen) — otherwise that row was unreachable.
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
            } else {
                false
            }
        }
        Action::Left => {
            if in_other_work {
                let idx = g.get_person_other_work_focused();
                if idx > 0 {
                    g.set_person_other_work_focused(idx - 1);
                }
                true
            } else if in_film {
                let idx = g.get_person_film_focused();
                if idx > 0 {
                    g.set_person_film_focused(idx - 1);
                }
                true
            } else {
                false
            }
        }
        Action::Right => {
            if in_other_work {
                let idx = g.get_person_other_work_focused();
                let max = g.get_person_other_work().row_count() as i32 - 1;
                if idx < max {
                    g.set_person_other_work_focused(idx + 1);
                }
                true
            } else if in_film {
                let idx = g.get_person_film_focused();
                let max = g.get_person_filmography().row_count() as i32 - 1;
                if idx < max {
                    g.set_person_film_focused(idx + 1);
                }
                true
            } else {
                false
            }
        }
        Action::Confirm => {
            if in_other_work {
                let idx = g.get_person_other_work_focused() as usize;
                if let Some(card) = g.get_person_other_work().row_data(idx) {
                    let media_type = if card.item_type == "DiscoverMovie" {
                        "movie"
                    } else {
                        "tv"
                    };
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
                        card.id,
                        card.has_played,
                        card.is_favorite,
                        card.resume_pct,
                        card.item_type,
                        card.series_id,
                    );
                }
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

// ── wire_person (moved from main(), 0.5.0 step 3) ────────────────────────
/// Wires person screen: open_person, open_discover_person, close_person.
pub(crate) fn wire_person(
    window: &crate::MainWindow,
    state: &std::sync::Arc<std::sync::Mutex<crate::config::FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    // Moved verbatim from main(): names resolve as they did there.
    use crate::*;
    let window = slint::ComponentHandle::clone_strong(window);
    let state = std::sync::Arc::clone(state);
    // ── person screen ─────────────────────────────────────────────────────────
    {
        let state2 = Arc::clone(&state);
        let ww2 = window.as_weak();
        let rt2 = rt.handle().clone();
        AppState::get(&window).on_open_person(move |id, name| {
            person::open_person_screen(
                id.to_string(),
                name.to_string(),
                Arc::clone(&state2),
                ww2.clone(),
                rt2.clone(),
            );
        });
    }
    {
        let state2 = Arc::clone(&state);
        let ww2 = window.as_weak();
        let rt2 = rt.handle().clone();
        AppState::get(&window).on_open_discover_person(move |tmdb_id, name| {
            person::open_person_from_discover(
                tmdb_id.to_string(),
                name.to_string(),
                Arc::clone(&state2),
                ww2.clone(),
                rt2.clone(),
            );
        });
    }
    {
        let ww2 = window.as_weak();
        AppState::get(&window).on_close_person(move || {
            if let Some(w) = ww2.upgrade() {
                AppState::get(&w).set_show_person(false);
            }
        });
    }
}
