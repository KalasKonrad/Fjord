// ── fjord-app · discover/calendar.rs ─────────────────────────────────────────
//   release_dates_for_region/calendar_kind_for_release_type  ReleaseDatesResult + region ->
//                              deduped-by-type (3/4/5) (type, date) pairs, mirrors Seerr's own
//                              frontend filter; type -> CalendarEntryKind (Theatrical/Digital/Physical)
//   build_calendar_entries      unions discover_watchlist_ids ∪ discover_known_requests keys,
//                              capped at 20 (mirrors fetch_requested_row's own cap), detail-
//                              fetches (bounded Semaphore+JoinSet) each and extracts movie
//                              release dates or TV next_episode_to_air; sorted soonest-first;
//                              called after every watchlist/request mutation (toggle, submit,
//                              cancel/approve/decline), not just on session fetch — ALSO now
//                              spawned from ensure_discover_landing itself right after it
//                              populates discover_known_requests (real bug, live-reported
//                              2026-07-19: ensure_discover_watchlist's own post-fetch call
//                              races ensure_discover_landing's tokio::join! and nearly always
//                              wins — the watchlist fetch is comparatively instant, the
//                              landing join is a real network round trip — so on a session
//                              with zero watchlist items, candidates was empty at the ONE
//                              call that ever ran, and nothing re-triggered it afterward; the
//                              Coming Up row stayed sentinel-only for the whole session)
//   push_coming_up_row          discover_calendar_entries -> discover-coming-up CardItem list
//                              (capped PREVIEW_CAP=20) + a trailing sentinel card (id="",
//                              title="📅", subtitle="Full Calendar") whose Enter/click opens
//                              CalendarScreen instead of an item. Real bug, live-reported
//                              2026-07-19 ("highlight disappears, nothing shows anywhere"):
//                              this function's only caller (build_calendar_entries) runs on a
//                              Tokio worker thread, never invoke_from_event_loop-wrapped, but
//                              this function called ww.upgrade()/AppState setters directly —
//                              slint::Weak::upgrade() silently returns None off the UI thread
//                              (confirmed from i-slint-core's real source), so
//                              discover-coming-up was never actually set, on any run, since
//                              this feature shipped; build_calendar_entries's own success log
//                              made the Rust-side computation look like it worked, masking
//                              that the UI-side commit was silently failing every time. Fixed
//                              to match every other UI mutation in this file: clone entries
//                              (plain Send-safe data) before the closure, build CardItems and
//                              call the AppState setter only inside invoke_from_event_loop.
//                              Also splits the same (sentinel-free) card list by item_type into
//                              discover-coming-up-mixed/-movies/-tv (2026-08-02, user request —
//                              same 3-way split as the Watchlist dashboard rows, one row on Home
//                              (mixed) and each of Movies/TV shows only its own type) via the
//                              shared calendar_entry_to_card mapper; all 4 models route through
//                              apply_cards_preserving_identity now instead of a raw ModelRc swap
//                              (this function reruns on every watchlist/request mutation, same
//                              "Phase 96 flash bug" reasoning as push_watchlist_rows)
//   calendar_entry_to_card       CalendarEntry -> CardItem (id/item_type/title/date+kind
//                              subtitle/on_watchlist), no sentinel — shared by push_coming_up_row's
//                              4 models so the mapping logic lives in exactly one place
//   fetch_coming_up_posters     patches posters onto the already-committed Coming Up row
//                              (2026-07-19, user request), bounded-concurrency fetch-then-
//                              patch-by-index, same shape as refresh_requested_row's own
//                              poster pass; must truncate with the same COMING_UP_PREVIEW_CAP
//                              and source order push_coming_up_row used (patches by index).
//                              Also patches the 3 dashboard split models by id+item_type lookup
//                              (2026-08-02) — same reason as fetch_watchlist_posters: the same
//                              tmdb id can sit at a different row index in each of the 3 lists
//   calendar_grid_dims/push_calendar_view/calendar_day_entries  month-grid data: leading-
//                              blank-count + day-count for a year/month (Sunday-first);
//                              calendar-days CardItem list (day number as title, entry count
//                              via unplayed-count, first entry's own title via subtitle —
//                              2026-07-19, user request, CalendarDayCell shows it instead of
//                              just a count pill); one day's matching CalendarEntry rows -> popup CardItems
//   handle_key_calendar/handle_key_calendar_day_popup  CalendarScreen's own AppMode dispatch —
//                              header zone (calendar-cursor-row<0) vs. 7-col day grid; Left/Right
//                              at the header directly invoke calendar-prev-month()/-next-month()
//                              (2026-07-19, user request — previously just cycled a cursor among
//                              Back/Prev/Next, needing a separate Confirm; Back is still reachable
//                              via Escape/Backspace, the universal close-key convention, or Enter
//                              at the initial Back-focused position); Confirm on a real day
//                              invokes calendar-day-selected(day) (the SAME callback the mouse
//                              path calls, so keyboard/mouse can't diverge); day popup: Up/Down
//                              cursor, Confirm -> calendar-day-popup-entry-selected(idx), Back
//                              closes the popup only
//
//   build_calendar_entries     candidate set gained a third source: ongoing (Status=="Continuing")
//                              series already in the local library, even if never watchlisted/
//                              requested — unioned in AFTER the existing watchlist∪requests
//                              .take(20) slice (left unchanged) with its own separate defensive
//                              cap, not folded into the same pre-take HashSet (would
//                              non-deterministically starve out the other two sources)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

/// `MovieDetails.releases` -> deduped-by-type `(type, date)` pairs for
/// `type` in {3=Theatrical, 4=Digital, 5=Physical} — mirrors Seerr's own
/// frontend filter exactly (`src/components/MovieDetails/index.tsx`:
/// `releases?.filter((r) => r.type > 2 && r.type < 6)`, `uniqBy(..., 'type')`).
/// TV has no equivalent (Watchlist + Release Calendar, 2026-07-18).
fn release_dates_for_region(
    releases: &fjord_seerr::ReleaseDatesResult,
    region: &str,
) -> Vec<(i32, String)> {
    let Some(entries) = releases
        .results
        .iter()
        .find(|r| r.iso_3166_1 == region)
        .map(|r| &r.release_dates)
    else {
        return Vec::new();
    };
    let mut seen = std::collections::HashSet::new();
    entries
        .iter()
        .filter(|e| (3..6).contains(&e.release_type))
        .filter(|e| seen.insert(e.release_type))
        .map(|e| (e.release_type, e.release_date.clone()))
        .collect()
}

fn calendar_kind_for_release_type(t: i32) -> CalendarEntryKind {
    match t {
        3 => CalendarEntryKind::Theatrical,
        4 => CalendarEntryKind::Digital,
        _ => CalendarEntryKind::Physical,
    }
}

/// Builds the "Coming Up" row's data — every id in `discover_watchlist_ids`
/// union `discover_known_requests`' own keys (the latter already IS the
/// `requested_not_available` result set, populated from that exact call by
/// `ensure_discover_landing`/`refresh_requested_row` — reusing it here
/// avoids a second, duplicate `GET /request` round trip), capped at 20
/// CANDIDATES (not 20 RESULTING entries — a date isn't known until after
/// the detail fetch below, so the cap bounds the number of detail fetches,
/// not a pre-sorted "soonest 20"; the final list is what gets sorted by
/// date, not the candidate selection). Same bounded-concurrency JoinSet
/// shape as `fetch_requested_row`. Movies contribute up to 3 entries each
/// (Theatrical/Digital/Physical, whichever have a real future date); TV
/// contributes at most 1 (`next_episode_to_air`). Past dates are excluded —
/// a "Coming Up" calendar has nothing to say about something already out.
/// Watchlist + Release Calendar, 2026-07-18.
pub(crate) async fn build_calendar_entries(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        return;
    };
    // Real bug, live-reported 2026-07-19 ("the context menu still shows add
    // to watchlist when its already is in the watch list"): `on_watchlist`
    // needs to be known per-candidate here — `push_coming_up_row` builds its
    // CardItems with `..Default::default()`, which silently means
    // `on_watchlist: false` for every card regardless of the real state, and
    // nothing ever re-patches it afterward since a fresh Coming Up rebuild
    // (this exact function, e.g. via refresh_watchlist right after a toggle)
    // replaces the whole model — the item that was JUST successfully
    // watchlisted, now newly appearing in this row because it has an
    // upcoming date, would flip straight back to "not on watchlist" the
    // instant this function's own rebuild ran.
    let candidates: Vec<(&'static str, String, bool)> = {
        let s = state.lock().unwrap();
        let watchlist_ids = &s.discover_watchlist_ids;
        let mut ids: std::collections::HashSet<(&'static str, String)> = watchlist_ids.clone();
        ids.extend(s.discover_known_requests.keys().cloned());
        let mut candidates: Vec<(&'static str, String, bool)> = ids
            .into_iter()
            .take(20)
            .map(|k| {
                let on_watchlist = watchlist_ids.contains(&k);
                (k.0, k.1, on_watchlist)
            })
            .collect();

        // Third candidate source (2026-07-29, Deep Seerr integration):
        // ongoing series already in the local library, even if never
        // watchlisted/requested via Seerr. Unioned in AFTER the existing
        // watchlist∪requests .take(20) slice (left completely unchanged
        // above) rather than folded into the same pre-take HashSet —
        // folding it in would non-deterministically starve out watchlist/
        // request candidates via hash-set iteration order once a library
        // has more than a handful of ongoing shows. Its own defensive cap
        // (a safety valve, not a precisely-chosen number) since the real
        // fetch cost is already bounded by the Semaphore(6) below, not by
        // candidate count — a bounded-concurrency fetch of even a few
        // hundred shows just takes longer wall-clock time, it doesn't fail.
        const ONGOING_CAP: usize = 50;
        let mut seen: std::collections::HashSet<(&'static str, String)> = candidates
            .iter()
            .map(|(t, id, _)| (*t, id.clone()))
            .collect();
        let mut ongoing_added = 0usize;
        for item in &s.all_series {
            if ongoing_added >= ONGOING_CAP {
                break;
            }
            if item.status.as_deref() != Some("Continuing") {
                continue;
            }
            let Some(tmdb_id) = item.provider_ids.get("Tmdb") else {
                continue;
            };
            let key = ("DiscoverTv", tmdb_id.clone());
            if !seen.insert(key.clone()) {
                continue;
            }
            candidates.push(("DiscoverTv", tmdb_id.clone(), watchlist_ids.contains(&key)));
            ongoing_added += 1;
        }
        debug!(
            "build_calendar_entries: {ongoing_added} ongoing series added ({} candidate(s) total)",
            candidates.len()
        );
        candidates
    };
    if candidates.is_empty() {
        state.lock().unwrap().discover_calendar_entries.clear();
        push_coming_up_row(&ww, &[]);
        return;
    }
    let today = chrono::Local::now().date_naive();
    let region = resolve_discover_region(&client, &state).await;

    let sem = Arc::new(tokio::sync::Semaphore::new(6));
    let mut set: tokio::task::JoinSet<Vec<CalendarEntry>> = tokio::task::JoinSet::new();
    for (item_type, tmdb_id_str, on_watchlist) in candidates {
        let Ok(tmdb_id) = tmdb_id_str.parse::<i64>() else {
            continue;
        };
        let client = client.clone();
        let sem = Arc::clone(&sem);
        let region = region.clone();
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let mut entries = Vec::new();
            if item_type == "DiscoverMovie" {
                let d = match client.get_movie(tmdb_id).await {
                    Ok(d) => d,
                    Err(e) => {
                        warn!("build_calendar_entries get_movie({tmdb_id}): {e:#}");
                        return entries;
                    }
                };
                // Same "don't show this in Discover" rule as
                // search_result_to_meta/watchlist_*_to_meta (2026-08-06) —
                // blocklisting doesn't remove the title from the watchlist
                // or an ongoing-series scan, so without this it would keep
                // resurfacing here on every calendar refresh.
                if availability_tag(d.media_info.as_ref().and_then(|mi| mi.status()))
                    == "blocklisted"
                {
                    return entries;
                }
                let Some(releases) = &d.releases else {
                    return entries;
                };
                for (release_type, date_str) in release_dates_for_region(releases, &region) {
                    let Ok(date) = chrono::NaiveDate::parse_from_str(
                        &date_str[..date_str.len().min(10)],
                        "%Y-%m-%d",
                    ) else {
                        continue;
                    };
                    entries.push(CalendarEntry {
                        date,
                        tmdb_id: tmdb_id.to_string(),
                        item_type: "DiscoverMovie",
                        title: d.title.clone(),
                        poster_path: d.poster_path.clone(),
                        on_watchlist,
                        kind: calendar_kind_for_release_type(release_type),
                        episode_label: None,
                    });
                }
            } else {
                let d = match client.get_tv(tmdb_id).await {
                    Ok(d) => d,
                    Err(e) => {
                        warn!("build_calendar_entries get_tv({tmdb_id}): {e:#}");
                        return entries;
                    }
                };
                if availability_tag(d.media_info.as_ref().and_then(|mi| mi.status()))
                    == "blocklisted"
                {
                    return entries;
                }
                let Some(next) = &d.next_episode_to_air else {
                    return entries;
                };
                let Some(date_str) = &next.air_date else {
                    return entries;
                };
                let Ok(date) = chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d") else {
                    return entries;
                };
                let episode_label = match (next.season_number, next.episode_number, &next.name) {
                    (Some(s), Some(e), Some(name)) => Some(format!("S{s}E{e} — {name}")),
                    (Some(s), Some(e), None) => Some(format!("S{s}E{e}")),
                    _ => None,
                };
                entries.push(CalendarEntry {
                    date,
                    tmdb_id: tmdb_id.to_string(),
                    item_type: "DiscoverTv",
                    title: d.name.clone(),
                    poster_path: d.poster_path.clone(),
                    on_watchlist,
                    kind: CalendarEntryKind::Episode,
                    episode_label,
                });
            }
            entries
        });
    }
    let mut all: Vec<CalendarEntry> = Vec::new();
    while let Some(res) = set.join_next().await {
        if let Ok(entries) = res {
            all.extend(entries.into_iter().filter(|e| e.date >= today));
        }
    }
    all.sort_by_key(|e| e.date);
    debug!(
        "seerr: calendar -> {} entr{}",
        all.len(),
        if all.len() == 1 { "y" } else { "ies" }
    );

    state.lock().unwrap().discover_calendar_entries = all.clone();
    push_coming_up_row(&ww, &all);
    fetch_coming_up_posters(ww, &all).await;
}

/// Pushes the "Coming Up" landing row from `entries` (soonest-first,
/// already sorted by `build_calendar_entries`) — capped to a preview count,
/// plus the trailing sentinel card `handle_key_landing` special-cases.
/// Text-only commit first, same two-phase pattern as every other landing
/// row — `fetch_coming_up_posters` (below) patches posters in afterward;
/// `ensure_discover_landing`'s own poster pass doesn't cover this row
/// since it's rebuilt independently on its own schedule, not as part of
/// the 8-way landing join.
///
/// Real bug, live-reported 2026-07-19 ("highlight disappears, nothing
/// shows anywhere"): this function is only ever called from
/// `build_calendar_entries`, an `async fn` that runs entirely on a Tokio
/// worker thread (spawned via `tokio::spawn`/`rt.spawn`, never routed
/// through `invoke_from_event_loop`) — but it called `ww.upgrade()` and
/// `AppState::get(&w).set_discover_coming_up(...)` directly, off the UI
/// thread. `slint::Weak::upgrade()` silently returns `None` when called
/// from any thread other than the one that owns the window (confirmed
/// from `i-slint-core`'s real source, not assumed: `if
/// std::thread::current().id() != self.thread { return None; }`, no
/// panic) — so `discover-coming-up` was never actually set, on any run,
/// since this feature first shipped; the `debug!("seerr: calendar -> N
/// entries")` log line in `build_calendar_entries` (which runs BEFORE
/// this function) made the Rust-side computation look like it succeeded,
/// masking that the UI-side commit was silently failing every single
/// time. Every other UI mutation in this file follows the two-phase
/// pattern (build plain Send-safe data off-thread, construct `CardItem`
/// only inside `invoke_from_event_loop`) for exactly this reason — this
/// one function was written without it. Fixed by clamping/cloning
/// `entries` (plain `Vec<CalendarEntry>`, genuinely `Send`) before the
/// closure, and moving the `CardItem`/`AppState` mutation inside.
fn calendar_entry_to_card(e: &CalendarEntry) -> CardItem {
    let kind_label = match e.kind {
        CalendarEntryKind::Theatrical => "In Theaters",
        CalendarEntryKind::Digital => "Streaming",
        CalendarEntryKind::Physical => "Physical Release",
        CalendarEntryKind::Episode => "New Episode",
    };
    let subtitle = e
        .episode_label
        .clone()
        .unwrap_or_else(|| kind_label.to_string());
    CardItem {
        id: e.tmdb_id.as_str().into(),
        item_type: e.item_type.into(),
        title: e.title.as_str().into(),
        subtitle: format!("{} · {}", e.date.format("%b %-d"), subtitle).into(),
        on_watchlist: e.on_watchlist,
        ..Default::default()
    }
}

fn push_coming_up_row(ww: &Weak<MainWindow>, entries: &[CalendarEntry]) {
    let entries: Vec<CalendarEntry> = entries
        .iter()
        .take(COMING_UP_PREVIEW_CAP)
        .cloned()
        .collect();
    let ww = ww.clone();
    let _ = slint::invoke_from_event_loop(move || {
        let Some(w) = ww.upgrade() else { return };
        let g = AppState::get(&w);
        // Home/TV/Movies dashboard rows (2026-08-02, user request — "the
        // coming up row shuld also be in home dashbord... coming up in
        // series dashbord that is filtered for series and in movies
        // dashbord that is filtered for movies"), same 3-way mixed/movies/tv
        // split as the Watchlist dashboard rows, sentinel-free (a "Full
        // Calendar" card only makes sense on the Discover screen's own
        // landing row, which has a CalendarScreen to open).
        let mixed: Vec<CardItem> = entries.iter().map(calendar_entry_to_card).collect();
        let movies: Vec<CardItem> = mixed
            .iter()
            .filter(|c| c.item_type.as_str() == "DiscoverMovie")
            .cloned()
            .collect();
        let tv: Vec<CardItem> = mixed
            .iter()
            .filter(|c| c.item_type.as_str() == "DiscoverTv")
            .cloned()
            .collect();
        let mut cards = mixed.clone();
        cards.push(CardItem {
            id: "".into(),
            item_type: "".into(),
            // U+1F4C5 (📅 CALENDAR) isn't in any bundled font's cmap (confirmed
            // via fc-query, 2026-07-22, live-reported "still missing symbols on
            // the htpc") — the card title has no font-family pin, so the global
            // Noto fallback mechanism had nothing to fall back TO here, tofu on
            // any system without its own emoji font. U+1F5D3 (🗓 SPIRAL CALENDAR
            // PAD) genuinely is in Noto Sans Symbols2's cmap.
            title: "🗓".into(),
            subtitle: "Full Calendar".into(),
            ..Default::default()
        });
        debug!(
            "seerr: push_coming_up_row -> {} card(s) (mixed={} movies={} tv={})",
            cards.len(),
            mixed.len(),
            movies.len(),
            tv.len()
        );
        g.set_discover_coming_up(crate::apply_cards_preserving_identity(
            &g.get_discover_coming_up(),
            cards,
        ));
        g.set_discover_coming_up_mixed(crate::apply_cards_preserving_identity(
            &g.get_discover_coming_up_mixed(),
            mixed,
        ));
        g.set_discover_coming_up_movies(crate::apply_cards_preserving_identity(
            &g.get_discover_coming_up_movies(),
            movies,
        ));
        g.set_discover_coming_up_tv(crate::apply_cards_preserving_identity(
            &g.get_discover_coming_up_tv(),
            tv,
        ));
    });
}

/// Patches posters onto the already-committed Coming Up row (2026-07-19,
/// user request — "it hust dosent have posters"), same bounded-concurrency
/// fetch-then-patch-by-index shape as `refresh_requested_row`'s own poster
/// pass. Must truncate `entries` with the SAME `COMING_UP_PREVIEW_CAP` and
/// source order `push_coming_up_row` used, since patching is by row index
/// — the id/type check on each patch is the belt-and-braces guard against
/// the two ever drifting out of sync (same pattern used everywhere else in
/// this file a poster fetch patches a model by index).
async fn fetch_coming_up_posters(ww: Weak<MainWindow>, entries: &[CalendarEntry]) {
    let poster_jobs: Vec<(usize, String, String, String)> = entries
        .iter()
        .take(COMING_UP_PREVIEW_CAP)
        .enumerate()
        .filter_map(|(idx, e)| {
            e.poster_path
                .clone()
                .map(|p| (idx, e.item_type.to_string(), e.tmdb_id.clone(), p))
        })
        .collect();
    if poster_jobs.is_empty() {
        return;
    }
    let Ok(http) = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
    else {
        return;
    };
    let sem = Arc::new(tokio::sync::Semaphore::new(8));
    let mut set = tokio::task::JoinSet::new();
    for (idx, item_type, tmdb_id, poster_path) in poster_jobs {
        let http = http.clone();
        let sem = Arc::clone(&sem);
        set.spawn(async move {
            let _permit = sem.acquire_owned().await.ok();
            let cache_key = format!(
                "{}-{}",
                if item_type == "DiscoverMovie" {
                    "movie"
                } else {
                    "tv"
                },
                tmdb_id
            );
            let bytes = fetch_tmdb_image(&http, TMDB_POSTER_BASE, &poster_path, &cache_key).await?;
            let buf = decode_poster_buffer(&bytes)?;
            Some((idx, item_type, tmdb_id, buf))
        });
    }
    while let Some(res) = set.join_next().await {
        let Ok(Some((idx, item_type, tmdb_id, buf))) = res else {
            continue;
        };
        let ww2 = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww2.upgrade() else { return };
            let g = AppState::get(&w);
            // Discover screen's own landing row: index-based patch (the
            // sentinel card sits past every real entry's index, so this
            // never touches it).
            let model = g.get_discover_coming_up();
            if let Some(mut card) = model.row_data(idx)
                && card.id.as_str() == tmdb_id
                && card.item_type.as_str() == item_type
            {
                card.poster = slint::Image::from_rgba8(buf.clone());
                card.has_poster = true;
                model.set_row_data(idx, card);
            }
            // Home/TV/Movies dashboard rows (2026-08-02): id+item_type
            // lookup, not index — the same tmdb id can sit at a different
            // row index in discover-coming-up-mixed vs. its own type-
            // specific list, same reason fetch_watchlist_posters does this.
            for model in [
                g.get_discover_coming_up_mixed(),
                g.get_discover_coming_up_movies(),
                g.get_discover_coming_up_tv(),
            ] {
                for i in 0..model.row_count() {
                    if let Some(mut card) = model.row_data(i)
                        && card.id.as_str() == tmdb_id
                        && card.item_type.as_str() == item_type
                    {
                        card.poster = slint::Image::from_rgba8(buf.clone());
                        card.has_poster = true;
                        model.set_row_data(i, card);
                    }
                }
            }
        });
    }
}

// ── Calendar screen (Watchlist + Release Calendar, 2026-07-18) ─────────────

/// (leading blank cells before day 1, Sunday-first; total real days in the
/// month) — pure chrono math, computed here rather than replicated in
/// Slint. Sunday-first is an arbitrary but consistent choice (this app has
/// no established locale precedent to follow either way).
fn calendar_grid_dims(year: i32, month: u32) -> (i32, i32) {
    use chrono::Datelike;
    let Some(first) = chrono::NaiveDate::from_ymd_opt(year, month, 1) else {
        return (0, 30);
    };
    let leading = first.weekday().num_days_from_sunday() as i32;
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let total = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .map(|next| (next - first).num_days() as i32)
        .unwrap_or(30);
    (leading, total)
}

/// Rebuilds `calendar-days`/`calendar-leading-blanks`/`calendar-total-days`
/// for whatever `calendar-year`/`calendar-month` currently are — called on
/// open and after every month-nav. One `CardItem` per REAL day (no blank
/// placeholders in the model itself, see `calendar-leading-blanks`' own doc
/// comment): `title` is the day number, `unplayed-count` repurposed as the
/// day's entry count, `subtitle` is the first entry's own title (2026-07-19,
/// user request — "write you the relese in the calander instead of just a
/// small marker": `CalendarDayCell` now shows this text directly rather
/// than only a numeric pill; a day with 2+ entries still gets the count
/// pill too, alongside the title, so a second/third release isn't silently
/// dropped from the cell — the day-popup remains the place to see all of
/// them by name), `id` is unused (day index is positional).
pub(crate) fn push_calendar_view(g: &AppState, s: &FjordState) {
    use chrono::Datelike;
    let year = g.get_calendar_year();
    let month = g.get_calendar_month().clamp(1, 12) as u32;
    let (leading, total) = calendar_grid_dims(year, month);
    let days: Vec<CardItem> = (1..=total)
        .map(|day| {
            let day_entries: Vec<&CalendarEntry> = s
                .discover_calendar_entries
                .iter()
                .filter(|e| {
                    e.date.year() == year && e.date.month() == month && e.date.day() as i32 == day
                })
                .collect();
            let subtitle = day_entries
                .first()
                .map(|e| e.title.clone())
                .unwrap_or_default();
            CardItem {
                title: day.to_string().into(),
                subtitle: subtitle.into(),
                unplayed_count: day_entries.len() as i32,
                ..Default::default()
            }
        })
        .collect();
    g.set_calendar_days(ModelRc::new(VecModel::from(days)));
    g.set_calendar_leading_blanks(leading);
    g.set_calendar_total_days(total);
}

/// Entries for a specific day, in `calendar-day-popup-entries`' own CardItem
/// shape (id/item-type = real tmdb id/type, so a popup selection can open
/// the item directly; subtitle = date-independent label, the day itself is
/// already implied by which cell was opened).
fn calendar_day_entries(s: &FjordState, year: i32, month: u32, day: i32) -> Vec<CardItem> {
    use chrono::Datelike;
    s.discover_calendar_entries
        .iter()
        .filter(|e| e.date.year() == year && e.date.month() == month && e.date.day() as i32 == day)
        .map(|e| {
            let kind_label = match e.kind {
                CalendarEntryKind::Theatrical => "In Theaters",
                CalendarEntryKind::Digital => "Streaming",
                CalendarEntryKind::Physical => "Physical Release",
                CalendarEntryKind::Episode => "New Episode",
            };
            let subtitle = e
                .episode_label
                .clone()
                .unwrap_or_else(|| kind_label.to_string());
            CardItem {
                id: e.tmdb_id.as_str().into(),
                item_type: e.item_type.into(),
                title: e.title.as_str().into(),
                subtitle: subtitle.into(),
                ..Default::default()
            }
        })
        .collect()
}

/// Zone -1 = header row (Back=col 0, Prev month=col 1, Next month=col 2,
/// Left/Right cycle among these 3, Confirm activates whichever is
/// focused); zone >= 0 = the day grid itself (7 columns,
/// `calendar-cursor-row`/`-col` are raw grid coordinates including blank
/// cells — landing on a blank is harmless, Enter there is just inert, same
/// "gaps are fine" tolerance the Coming Up row's own sentinel already
/// established, rather than clamping arrow keys around blanks). Confirm on
/// a real day routes through the SAME `calendar-day-selected` callback the
/// mouse path uses (`on_calendar_day_selected` in `wire_discover`) rather
/// than calling `open_calendar_day_popup` directly — `keys.rs`'s per-mode
/// match arms don't hold `state`/`ww`, and funneling both input paths
/// through one callback is also what guarantees they can't diverge (the
/// mouse/keyboard focus-desync bug class documented throughout this file).
///
/// Left/Right month-changing — corrected design, same day, after a live
/// report ("the left right to change the month works when you are on the
/// back button but not when you are on the end ow a row on the
/// monthgrid... it shuld not change when you press left or right on the
/// back buttun then you shuld just navigate the buttons"). The FIRST
/// attempt made header-zone Left/Right always fire the month change
/// immediately — wrong on two counts: it fired from the Back position too
/// (the user explicitly didn't want that — Left/Right on Back should just
/// navigate, not act), and it did nothing useful in the day grid at all.
/// Reverted the header zone back to its original cursor-cycling behavior
/// (Left/Right just move among Back/Prev/Next, Confirm activates); added
/// the actual requested behavior to the DAY GRID instead — Left at the
/// leftmost column (Sunday) or Right at the rightmost column (Saturday)
/// now continues past the edge into the adjacent month, mirroring the
/// common date-picker convention of browsing days seamlessly across a
/// month boundary. Reuses `invoke_calendar_prev_month`/`_next_month`
/// directly (both already reset the cursor into the new month's grid as
/// part of changing it, so no extra cursor bookkeeping needed here either).
pub(crate) fn handle_key_calendar(action: &Action, g: &AppState) -> bool {
    let row = g.get_calendar_cursor_row();
    let col = g.get_calendar_cursor_col();
    let total_days = g.get_calendar_total_days();
    let leading = g.get_calendar_leading_blanks();
    let total_rows = ((leading + total_days) as f32 / 7.0).ceil() as i32;
    match action {
        Action::Left => {
            if row < 0 {
                g.set_calendar_cursor_col((col - 1).max(0));
            } else if col > 0 {
                g.set_calendar_cursor_col(col - 1);
            } else {
                g.invoke_calendar_prev_month();
            }
            true
        }
        Action::Right => {
            if row < 0 {
                g.set_calendar_cursor_col((col + 1).min(2));
            } else if col < 6 {
                g.set_calendar_cursor_col(col + 1);
            } else {
                g.invoke_calendar_next_month();
            }
            true
        }
        Action::Up => {
            if row < 0 {
                // already at the header — nothing above it
            } else if row == 0 {
                g.set_calendar_cursor_row(-1);
                g.set_calendar_cursor_col(0);
            } else {
                g.set_calendar_cursor_row(row - 1);
            }
            true
        }
        Action::Down => {
            if row < 0 {
                g.set_calendar_cursor_row(0);
                g.set_calendar_cursor_col(0);
            } else if row + 1 < total_rows {
                g.set_calendar_cursor_row(row + 1);
            }
            true
        }
        Action::Confirm => {
            if row < 0 {
                match col {
                    0 => g.set_show_calendar(false),
                    1 => g.invoke_calendar_prev_month(),
                    _ => g.invoke_calendar_next_month(),
                }
            } else {
                let day = row * 7 + col - leading + 1;
                if day >= 1 && day <= total_days {
                    g.invoke_calendar_day_selected(day);
                }
            }
            true
        }
        Action::Back => {
            g.set_show_calendar(false);
            true
        }
        _ => false,
    }
}

pub(crate) fn open_calendar_day_popup(g: &AppState, state: &Arc<Mutex<FjordState>>, day: i32) {
    let year = g.get_calendar_year();
    let month = g.get_calendar_month().clamp(1, 12) as u32;
    let entries = calendar_day_entries(&state.lock().unwrap(), year, month, day);
    if entries.is_empty() {
        return;
    }
    g.set_calendar_day_popup_entries(ModelRc::new(VecModel::from(entries)));
    g.set_calendar_day_popup_cursor(0);
    g.set_show_calendar_day_popup(true);
}

pub(crate) fn handle_key_calendar_day_popup(action: &Action, g: &AppState) -> bool {
    let n = g.get_calendar_day_popup_entries().row_count() as i32;
    let cursor = g.get_calendar_day_popup_cursor();
    match action {
        Action::Up => {
            if cursor > 0 {
                g.set_calendar_day_popup_cursor(cursor - 1);
            }
            true
        }
        Action::Down => {
            if cursor + 1 < n {
                g.set_calendar_day_popup_cursor(cursor + 1);
            }
            true
        }
        Action::Confirm => {
            g.invoke_calendar_day_popup_entry_selected(cursor);
            true
        }
        Action::Back => {
            g.set_show_calendar_day_popup(false);
            true
        }
        _ => false,
    }
}
