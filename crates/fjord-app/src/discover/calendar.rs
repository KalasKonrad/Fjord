// ── fjord-app · discover/calendar.rs ─────────────────────────────────────────
//   release_dates_for_region / calendar_kind_for_release_type  MovieDetails releases → deduped
//                              (type, date) pairs for types 3/4/5 (Seerr's own frontend filter)
//                              → CalendarEntryKind (Theatrical/Digital/Physical)
//   build_calendar_entries     Coming Up data: watchlist ∪ known requests (take 20) + ongoing local
//                              series (own cap), detail-fetched (bounded), future dates only, sorted
//                              soonest-first; rerun after every watchlist/request change and right
//                              after ensure_discover_landing fills discover_known_requests
//   push_coming_up_row         → discover-coming-up (preview cap + a "Full Calendar" sentinel card) and
//                              the sentinel-free -mixed/-movies/-tv dashboard models, via
//                              apply_cards_preserving_identity; CardItems built only inside
//                              invoke_from_event_loop (it's called from a Tokio task)
//   calendar_entry_to_card     CalendarEntry → CardItem, shared by those 4 models
//   fetch_coming_up_posters    patches posters into the committed row (by index, same cap/order) and
//                              the dashboard models (by id + item_type)
//   calendar_grid_dims / push_calendar_view / calendar_day_entries  month grid (Sunday-first):
//                              blanks + day count; one CardItem per day (title = day, count, first
//                              entry's title); a day's entries for the popup
//   handle_key_calendar / open_calendar_day_popup / handle_key_calendar_day_popup  CalendarScreen keys:
//                              header row (Back/Prev/Next, Left/Right move, Confirm activates) or the
//                              day grid (Left/Right past Sunday/Saturday change month; Confirm →
//                              calendar-day-selected, the same callback as the mouse); popup: Up/Down,
//                              Confirm opens the entry, Back closes the popup
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

/// The "Coming Up" row's data: every id in `discover_watchlist_ids` ∪
/// `discover_known_requests` (already the requested-not-available set — no second
/// `GET /request`), capped at 20 CANDIDATES (dates are only known after the detail
/// fetch, so the cap bounds fetches; the final list is sorted by date). Bounded
/// concurrency like `fetch_requested_row`. A movie contributes up to 3 entries
/// (Theatrical/Digital/Physical with a future date), a series at most 1
/// (`next_episode_to_air`). Past dates are left out.
pub(crate) async fn build_calendar_entries(state: Arc<Mutex<FjordState>>, ww: Weak<MainWindow>) {
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        return;
    };
    // `on_watchlist` per candidate: `push_coming_up_row` builds its cards with defaults,
    // and every rebuild replaces the model — without this, an item that just got
    // watchlisted (and appears here because of its upcoming date) would show as not on
    // the watchlist.
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

        // Third candidate source: ongoing series in the local library, even without a Seerr
        // watchlist/request. Added AFTER the watchlist ∪ requests take(20) (not folded into
        // that set), so hash-set order can't push watchlist/request candidates out. Its cap is
        // a safety valve; the real cost is bounded by the Semaphore(6) below.
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
                // Blocklisted items stay hidden here too (like search_result_to_meta /
                // watchlist_*_to_meta) — blocklisting doesn't remove a title from the watchlist or the
                // ongoing-series scan.
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

/// Pushes the "Coming Up" landing row from `entries` (soonest first, sorted by
/// `build_calendar_entries`), capped to a preview count, plus the trailing sentinel
/// card `handle_key_landing` special-cases. Text first; `fetch_coming_up_posters`
/// patches posters in afterward (this row is rebuilt on its own schedule, outside the
/// landing-row join). Called from an async task on a Tokio worker: `entries` is
/// cloned into `invoke_from_event_loop` and the `CardItem`s are built inside it —
/// `Weak::upgrade()` returns None off the UI thread, so the row would never be set.
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
        // Home/TV/Movies dashboard rows: the same mixed/movies/tv split as the Watchlist
        // rows, without the sentinel ("Full Calendar" only makes sense on Discover).
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
            // U+1F5D3 (🗓) is in Noto Sans Symbols 2; U+1F4C5 (📅) is in no bundled font, and this
            // title has no font pin, so it rendered as tofu on systems without an emoji font.
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

/// Patches posters onto the committed Coming Up row (bounded concurrency, like
/// `refresh_requested_row`'s poster pass). Truncates `entries` with the SAME
/// `COMING_UP_PREVIEW_CAP` and order as `push_coming_up_row`, since patching is by row
/// index; the id/type check on each patch guards against the two drifting apart.
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
            // Dashboard rows: look up by id + item_type, not index — the same tmdb id can sit at
            // a different index in -mixed vs. the type-specific list (like fetch_watchlist_posters).
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

/// Rebuilds `calendar-days`/`-leading-blanks`/`-total-days` for the current
/// `calendar-year`/`-month` (on open and every month change). One `CardItem` per REAL
/// day: `title` = day number, `unplayed-count` = the day's entry count, `subtitle` = the
/// first entry's title (shown in the cell; 2+ entries also show the count pill — the
/// day popup lists them all), `id` unused.
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

/// Zone -1 = header row (Back / Prev month / Next month: Left/Right move between them,
/// Confirm activates); zone >= 0 = the day grid (7 columns; `calendar-cursor-row`/
/// `-col` are raw grid coordinates incl. blank cells — Enter on a blank is inert).
/// Confirm on a day goes through the same `calendar-day-selected` callback as the
/// mouse (`on_calendar_day_selected` in wire_discover), so both paths match. In the
/// grid, Left on Sunday / Right on Saturday continues into the previous/next month
/// (`invoke_calendar_prev_month`/`_next_month` reset the cursor themselves).
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
