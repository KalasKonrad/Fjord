// ── fjord-app · discover/filters.rs ──────────────────────────────────────────
//   ── Discover filters (2026-07-18, planned via /plan, 3 rounds of
//      AskUserQuestion — see CLAUDE.md's Seerr integration section) ──
//   ensure_discover_filter_options  fetches genre + watch-provider lists (both media
//                              types) once per session; also restores discover-filters-active
//                              + the 4 *-desc properties from persisted Config and, if filters
//                              were already active, kicks off spawn_discover_filtered_browse
//                              immediately rather than waiting for a pill touch
//   build_discover_filters      current filter selections -> a real fjord_seerr::DiscoverFilters
//                              for one media type's /discover/* call; genre NAMES re-resolved
//                              to that type's own raw id (movie/TV genre id spaces don't match)
//   build_genre_items/build_provider_items/push_or_merge_genre/refresh_discover_filter_models
//                              raw Seerr genre/provider lists -> Slint GenreItem/ProviderItem
//                              chip models; push_or_merge_genre merges a same-named genre's
//                              movie-side and TV-side ids into one chip (GenreItem carries
//                              both, since the id spaces don't overlap); providers dedupe by
//                              id directly (shared across media types, unlike genre)
//   discover_filters_active/search_filters_active  discover_filters_active: is ANY of the
//                              6 dimensions non-default (landing-rows vs filtered-browse
//                              switch); search_filters_active: the narrower subset that
//                              actually applies to search results (excludes Type — /search
//                              always mixes both types — and Provider, which /search's
//                              response carries no data for at all)
//   apply_search_filters        client-side genre/rating/year/sort pass over the full raw
//                              fetch history (FjordState.discover_search_metas) — the only
//                              way filters can apply to search results, since /search takes
//                              no filter params; preserves posters by id lookup (not index —
//                              filtering reorders/removes rows); must run strictly AFTER
//                              fetch_and_patch_posters finishes, never before
//   build_filtered_metas/merge_filtered_metas/sort_filtered_metas  SearchResult list ->
//                              (meta, poster_path) pairs; merge_filtered_metas interleaves
//                              movie+TV results into one grid for Type=All, sorted (via
//                              sort_filtered_metas, extracted 2026-07-31 so _more can reuse it
//                              on the FULL accumulated set, not just one page — see below) by
//                              the active sort key's real value (concatenate-then-sort, not a
//                              two-pointer merge — the inputs are small enough that this is
//                              simpler for the same result)
//   spawn_discover_filtered_browse/_more  the new filtered-browse view (query empty, >=1
//                              filter active) — mirrors spawn_discover_search/_more's two-
//                              phase commit shape but sources from discover_movies_filtered/
//                              discover_tv_filtered (real server-side filtering); fetches both
//                              types in parallel when Type=All; shares spawn_discover_search's
//                              OWN discover_gen counter (required — a race between the two
//                              view types would otherwise clobber discover-results); load-more
//                              advances both underlying TMDB pages in lockstep, stopping on
//                              max() of the two total_pages so Type=All doesn't cut off early;
//                              real bug fixed 2026-07-31 (code review) — _more used to sort
//                              and commit only each page's own batch, silently breaking global
//                              sort order across the page boundary; now accumulates every
//                              fetched page into FjordState.discover_filtered_metas and
//                              re-sorts the WHOLE set on every page, preserving already-known
//                              posters by id (same idiom as home.rs::refresh_row_preserving_-
//                              posters) and skipping a redundant re-fetch for them
//   build_filtered_metas       bumped pub(crate) — reused verbatim (no changes) by Detail/Series
//                              Recommended and Collection Missing Items, all of which already
//                              have a plain &[SearchResult] to convert
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Discover filters (2026-07-18) ───────────────────────────────────────────
//
// Six pills: Type/Sort/Rating/Year are single-value (desc string shown in
// the pill, internal key/value persisted in Config); Genre/Provider are
// multi-select chip pickers (GenreItem/ProviderItem models, each row's own
// `selected` toggled independently — TMDB's with_genres/with_watch_providers
// both take pipe-separated OR, see DiscoverFilters' own doc comment in
// fjord-seerr for why). Config stores the INTERNAL representation (""/
// "movie"/"tv", ""/"rating"/"newest"/"oldest", a raw f32/u32 bucket floor,
// genre NAMES (stable across the movie/TV id-space mismatch — see
// GenreItem's own doc comment in theme.slint), provider ids (stable across
// media types, unlike genre) — never the display string, which is derived
// fresh by the *_desc functions below every time it's needed.

pub(crate) const SORT_KEYS: &[(&str, &str)] = &[
    ("", "Popularity"),
    ("rating", "Rating"),
    ("newest", "Newest"),
    ("oldest", "Oldest"),
];

pub(crate) const RATING_BUCKETS: &[(&str, f32)] =
    &[("Any", 0.0), ("6+", 6.0), ("7+", 7.0), ("8+", 8.0)];

pub(crate) const YEAR_BUCKETS: &[(&str, u32)] = &[
    ("Any", 0),
    ("2000+", 2000),
    ("2010+", 2010),
    ("2015+", 2015),
    ("2020+", 2020),
];

pub(crate) fn discover_type_desc(key: &str) -> &'static str {
    match key {
        "movie" => "Movies",
        "tv" => "TV",
        _ => "All",
    }
}

pub(crate) fn discover_type_key(desc: &str) -> &'static str {
    match desc {
        "Movies" => "movie",
        "TV" => "tv",
        _ => "",
    }
}

pub(crate) fn discover_sort_desc(key: &str) -> &'static str {
    SORT_KEYS
        .iter()
        .find(|(k, _)| *k == key)
        .map(|(_, d)| *d)
        .unwrap_or("Popularity")
}

pub(crate) fn discover_sort_key(desc: &str) -> &'static str {
    SORT_KEYS
        .iter()
        .find(|(_, d)| *d == desc)
        .map(|(k, _)| *k)
        .unwrap_or("")
}

pub(crate) fn discover_rating_desc(v: f32) -> &'static str {
    RATING_BUCKETS
        .iter()
        .find(|(_, r)| *r == v)
        .map(|(d, _)| *d)
        .unwrap_or("Any")
}

pub(crate) fn discover_rating_value(desc: &str) -> f32 {
    RATING_BUCKETS
        .iter()
        .find(|(d, _)| *d == desc)
        .map(|(_, r)| *r)
        .unwrap_or(0.0)
}

pub(crate) fn discover_year_desc(v: u32) -> &'static str {
    YEAR_BUCKETS
        .iter()
        .find(|(_, y)| *y == v)
        .map(|(d, _)| *d)
        .unwrap_or("Any")
}

pub(crate) fn discover_year_value(desc: &str) -> u32 {
    YEAR_BUCKETS
        .iter()
        .find(|(d, _)| *d == desc)
        .map(|(_, y)| *y)
        .unwrap_or(0)
}

/// Whether ANY of the 6 filter dimensions is set away from its default —
/// the landing-rows / filtered-browse view switch (query empty + this ==
/// false shows the original 6 landing rows unchanged; true replaces them
/// with the filtered-browse grid).
pub(crate) fn discover_filters_active(cfg: &ProfileSettings) -> bool {
    !cfg.discover_filter_type.is_empty()
        || !cfg.discover_filter_genre_names.is_empty()
        || !cfg.discover_filter_sort.is_empty()
        || cfg.discover_filter_min_rating > 0.0
        || cfg.discover_filter_min_year > 0
        || !cfg.discover_filter_provider_ids.is_empty()
}

/// Narrower check for `apply_search_filters` below — Type and Provider
/// deliberately don't apply to search results (Type: `/search` always
/// returns both movies and TV mixed, matching the approved plan's own
/// scope, which lists genre/sort/rating/year for search but not Type;
/// Provider: TMDB's multi-search response carries no per-item provider
/// data at all, so there's nothing to filter by — the Provider pill is
/// shown disabled while a query is active).
fn search_filters_active(cfg: &ProfileSettings) -> bool {
    !cfg.discover_filter_genre_names.is_empty()
        || !cfg.discover_filter_sort.is_empty()
        || cfg.discover_filter_min_rating > 0.0
        || cfg.discover_filter_min_year > 0
}

/// The TMDB sortBy VALUE for an internal sort key, resolved per media type
/// since movies/TV genuinely use different date-sort key names (confirmed
/// from Seerr's real route source — see `DiscoverFilters`' own doc comment
/// in fjord-seerr). `None` (Popularity) means omit `sortBy` entirely —
/// Seerr/TMDB's own default is already popularity-ranked.
fn tmdb_sort_value(key: &str, media_type: &str) -> Option<&'static str> {
    match key {
        "rating" => Some("vote_average.desc"),
        "newest" => Some(if media_type == "movie" {
            "primary_release_date.desc"
        } else {
            "first_air_date.desc"
        }),
        "oldest" => Some(if media_type == "movie" {
            "primary_release_date.asc"
        } else {
            "first_air_date.asc"
        }),
        _ => None,
    }
}

fn tmdb_date_gte_key(media_type: &str) -> &'static str {
    if media_type == "movie" {
        "primaryReleaseDateGte"
    } else {
        "firstAirDateGte"
    }
}

/// Resolves the current filter selections into a real `DiscoverFilters`
/// for one specific media type's `/discover/*` call — genre NAMES are
/// re-resolved to whichever id that name has in THIS type's own raw genre
/// list (a name with no match for this type, e.g. a TV-only genre while
/// querying movies, is silently skipped rather than erroring — the same
/// "gaps are fine" tolerance this codebase uses throughout for optional
/// per-item data).
fn build_discover_filters(
    s: &FjordState,
    media_type: &str,
    region: &str,
) -> fjord_seerr::DiscoverFilters {
    let cfg = s.config.active();
    let raw_genres: &[fjord_seerr::Genre] = if media_type == "movie" {
        &s.seerr_genres_movie
    } else {
        &s.seerr_genres_tv
    };
    let genre_ids: Vec<i64> = cfg
        .discover_filter_genre_names
        .iter()
        .filter_map(|name| raw_genres.iter().find(|g| &g.name == name).map(|g| g.id))
        .collect();
    fjord_seerr::DiscoverFilters {
        genre_ids: if genre_ids.is_empty() {
            None
        } else {
            Some(genre_ids)
        },
        provider_ids: if cfg.discover_filter_provider_ids.is_empty() {
            None
        } else {
            Some(cfg.discover_filter_provider_ids.clone())
        },
        watch_region: Some(region.to_string()),
        sort: tmdb_sort_value(&cfg.discover_filter_sort, media_type),
        vote_average_gte: if cfg.discover_filter_min_rating > 0.0 {
            Some(cfg.discover_filter_min_rating)
        } else {
            None
        },
        date_gte: if cfg.discover_filter_min_year > 0 {
            Some((
                tmdb_date_gte_key(media_type),
                format!("{}-01-01", cfg.discover_filter_min_year),
            ))
        } else {
            None
        },
        date_lte: None,
    }
}

/// Merges a same-name genre from the movie and TV lists into one chip —
/// see `GenreItem`'s own doc comment (theme.slint) for why both ids are
/// tracked separately rather than assuming they match.
fn push_or_merge_genre(
    items: &mut Vec<GenreItem>,
    name: &str,
    movie_id: Option<i64>,
    tv_id: Option<i64>,
    selected_names: &[String],
) {
    if let Some(existing) = items.iter_mut().find(|g| g.name.as_str() == name) {
        if let Some(id) = movie_id {
            existing.movie_id = id as i32;
        }
        if let Some(id) = tv_id {
            existing.tv_id = id as i32;
        }
    } else {
        items.push(GenreItem {
            movie_id: movie_id.unwrap_or(0) as i32,
            tv_id: tv_id.unwrap_or(0) as i32,
            name: name.into(),
            selected: selected_names.iter().any(|n| n == name),
        });
    }
}

fn build_genre_items(
    movie: &[fjord_seerr::Genre],
    tv: &[fjord_seerr::Genre],
    type_key: &str,
    selected_names: &[String],
) -> Vec<GenreItem> {
    let mut items: Vec<GenreItem> = Vec::new();
    match type_key {
        "movie" => {
            for g in movie {
                push_or_merge_genre(&mut items, &g.name, Some(g.id), None, selected_names);
            }
        }
        "tv" => {
            for g in tv {
                push_or_merge_genre(&mut items, &g.name, None, Some(g.id), selected_names);
            }
        }
        _ => {
            for g in movie {
                push_or_merge_genre(&mut items, &g.name, Some(g.id), None, selected_names);
            }
            for g in tv {
                push_or_merge_genre(&mut items, &g.name, None, Some(g.id), selected_names);
            }
        }
    }
    items.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    items
}

/// Provider ids ARE shared across movie/TV in TMDB's real system (unlike
/// genre ids) — this just dedupes by id across whichever list(s) apply to
/// the current Type filter, no dual-id tracking needed.
fn build_provider_items(
    movie: &[fjord_seerr::WatchProviderDetail],
    tv: &[fjord_seerr::WatchProviderDetail],
    type_key: &str,
    selected_ids: &[i64],
) -> Vec<ProviderItem> {
    let mut items: Vec<ProviderItem> = Vec::new();
    let mut seen: std::collections::HashSet<i64> = std::collections::HashSet::new();
    let empty: &[fjord_seerr::WatchProviderDetail] = &[];
    let sources: [&[fjord_seerr::WatchProviderDetail]; 2] = match type_key {
        "movie" => [movie, empty],
        "tv" => [tv, empty],
        _ => [movie, tv],
    };
    for list in sources {
        for p in list {
            if seen.insert(p.id) {
                items.push(ProviderItem {
                    id: p.id as i32,
                    name: p.name.as_str().into(),
                    selected: selected_ids.contains(&p.id),
                });
            }
        }
    }
    items.sort_by(|a, b| a.name.as_str().cmp(b.name.as_str()));
    items
}

/// Rebuilds the Genre/Provider chip models from FjordState's cached raw
/// lists — called once after `ensure_discover_filter_options`'s fetch
/// lands, and again every time the Type pill changes (no re-fetch needed,
/// the raw lists don't depend on Type).
pub(crate) fn refresh_discover_filter_models(g: &AppState, s: &FjordState) {
    let cp = s.config.active();
    let type_key = cp.discover_filter_type.as_str();
    g.set_discover_filter_genres(ModelRc::new(VecModel::from(build_genre_items(
        &s.seerr_genres_movie,
        &s.seerr_genres_tv,
        type_key,
        &cp.discover_filter_genre_names,
    ))));
    g.set_discover_filter_providers(ModelRc::new(VecModel::from(build_provider_items(
        &s.seerr_providers_movie,
        &s.seerr_providers_tv,
        type_key,
        &cp.discover_filter_provider_ids,
    ))));
    g.set_discover_filter_genre_count(cp.discover_filter_genre_names.len() as i32);
    g.set_discover_filter_provider_count(cp.discover_filter_provider_ids.len() as i32);
}

/// Fetches genre + watch-provider lists (both media types) once per
/// session — same guard shape as `ensure_discover_landing`. A fetch
/// failure for any one list just leaves that picker empty (best-effort,
/// matching this codebase's existing tolerance for optional Discover
/// metadata like tags/profiles), not a hard error. Also restores
/// `discover-filters-active`/the 4 `*-desc` properties from whatever was
/// persisted last session, and — if that means filters are already active
/// — kicks off the filtered-browse fetch right here rather than leaving
/// the screen showing landing rows until the user touches a filter pill
/// (hence needing `generation`, unlike a pure fetch-and-cache function).
pub(crate) fn ensure_discover_filter_options(
    state: Arc<Mutex<FjordState>>,
    ww: Weak<MainWindow>,
    generation: Arc<AtomicU64>,
    rt: tokio::runtime::Handle,
) {
    let client = {
        let mut s = state.lock().unwrap();
        if s.discover_filter_options_fetched {
            return;
        }
        let Some(client) = s.seerr_client.clone() else {
            return;
        };
        s.discover_filter_options_fetched = true;
        client
    };
    let rt2 = rt.clone();
    rt.spawn(async move {
        let region = resolve_streaming_region(&client, &state).await;
        let (movie_genres, tv_genres, movie_providers, tv_providers) = tokio::join!(
            client.get_movie_genres(),
            client.get_tv_genres(),
            client.get_movie_watch_providers(&region),
            client.get_tv_watch_providers(&region),
        );
        let movie_genres = movie_genres.unwrap_or_else(|e| {
            warn!("seerr: get_movie_genres: {e:#}");
            Vec::new()
        });
        let tv_genres = tv_genres.unwrap_or_else(|e| {
            warn!("seerr: get_tv_genres: {e:#}");
            Vec::new()
        });
        let movie_providers = movie_providers.unwrap_or_else(|e| {
            warn!("seerr: get_movie_watch_providers: {e:#}");
            Vec::new()
        });
        let tv_providers = tv_providers.unwrap_or_else(|e| {
            warn!("seerr: get_tv_watch_providers: {e:#}");
            Vec::new()
        });
        {
            let mut s = state.lock().unwrap();
            s.seerr_genres_movie = movie_genres;
            s.seerr_genres_tv = tv_genres;
            s.seerr_providers_movie = movie_providers;
            s.seerr_providers_tv = tv_providers;
        }
        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww.upgrade() else { return };
            let g = AppState::get(&w);
            let active = {
                let s = state.lock().unwrap();
                g.set_discover_filter_type_desc(
                    discover_type_desc(&s.config.active().discover_filter_type).into(),
                );
                g.set_discover_filter_sort_desc(
                    discover_sort_desc(&s.config.active().discover_filter_sort).into(),
                );
                g.set_discover_filter_rating_desc(
                    discover_rating_desc(s.config.active().discover_filter_min_rating).into(),
                );
                g.set_discover_filter_year_desc(
                    discover_year_desc(s.config.active().discover_filter_min_year).into(),
                );
                refresh_discover_filter_models(&g, &s);
                discover_filters_active(s.config.active())
            };
            g.set_discover_filters_active(active);
            // Filters were already active last session — show the filtered-
            // browse view immediately rather than landing rows until the
            // user touches a pill. query must still be empty (a saved
            // in-progress search query isn't persisted, but this fires
            // before the user could have typed anything new yet either way).
            if active && g.get_discover_query().as_str().is_empty() {
                spawn_discover_filtered_browse(
                    ww.clone(),
                    Arc::clone(&state),
                    Arc::clone(&generation),
                    &rt2,
                );
            }
        });
    });
}

/// Client-side genre/rating/year/sort pass over the full raw fetch history
/// for the CURRENT search (`FjordState.discover_search_metas`, accumulated
/// across every page `spawn_discover_search`/`_more` have committed) —
/// rebuilds `discover-results` from it. `/search` accepts no filter params
/// at all (see `DiscoverFilters`' own doc comment in fjord-seerr), so this
/// is the only way genre/rating/year/sort can apply to search results at
/// all; `search_filters_active` deliberately excludes Type and Provider —
/// see that function's own doc comment. No-op (leaves `discover-results`
/// alone) when no relevant filter is set, so callers can call this
/// unconditionally after every search commit without a wasted rebuild.
///
/// Preserves already-decoded poster Images for kept rows by looking them
/// up BY ID from the model's CURRENT content, not by index — filtering can
/// reorder/remove rows, so an index-based carry-forward would silently
/// mismatch. This is why this is only ever called strictly AFTER
/// `fetch_and_patch_posters` has finished patching the full unfiltered
/// list (posters are correctly placed by then); calling it any earlier
/// would race that patch's own index-based lookups.
pub(crate) fn apply_search_filters(state: &Arc<Mutex<FjordState>>, ww: &Weak<MainWindow>) {
    let Some(w) = ww.upgrade() else { return };
    let g = AppState::get(&w);
    let s = state.lock().unwrap();
    if !search_filters_active(s.config.active()) {
        return;
    }
    let cfg = s.config.active();
    let genre_names: std::collections::HashSet<&str> = cfg
        .discover_filter_genre_names
        .iter()
        .map(String::as_str)
        .collect();
    // A search result's genre_ids come back in whichever id-space matches
    // its OWN media_type — resolve every selected NAME to every id it
    // could appear as (movie side or TV side) so matching works regardless
    // of which type the name was originally selected under.
    let genre_ids: std::collections::HashSet<i64> = if genre_names.is_empty() {
        std::collections::HashSet::new()
    } else {
        s.seerr_genres_movie
            .iter()
            .chain(s.seerr_genres_tv.iter())
            .filter(|g| genre_names.contains(g.name.as_str()))
            .map(|g| g.id)
            .collect()
    };
    let min_rating = cfg.discover_filter_min_rating;
    let min_year = cfg.discover_filter_min_year;
    let sort_key = cfg.discover_filter_sort.clone();

    let mut kept: Vec<DiscoverCardMeta> = s
        .discover_search_metas
        .iter()
        .filter(|m| genre_ids.is_empty() || m.genre_ids.iter().any(|id| genre_ids.contains(id)))
        .filter(|m| min_rating <= 0.0 || m.vote_average >= min_rating as f64)
        .filter(|m| min_year == 0 || m.year >= min_year as i32)
        .cloned()
        .collect();
    match sort_key.as_str() {
        "rating" => kept.sort_by(|a, b| {
            b.vote_average
                .partial_cmp(&a.vote_average)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "newest" => kept.sort_by_key(|m| std::cmp::Reverse(m.year)),
        "oldest" => kept.sort_by_key(|m| m.year),
        _ => {} // Popularity — keep TMDB's own original relevance order
    }
    drop(s);

    let old = g.get_discover_results();
    let old_posters: std::collections::HashMap<(String, String), (slint::Image, bool)> = (0..old
        .row_count())
        .filter_map(|i| old.row_data(i))
        .map(|c| {
            (
                (c.id.to_string(), c.item_type.to_string()),
                (c.poster.clone(), c.has_poster),
            )
        })
        .collect();
    let cards: Vec<CardItem> = kept
        .into_iter()
        .map(|m| {
            let key = (m.id.clone(), m.item_type.to_string());
            let mut card = m.into_card_item();
            if let Some((poster, has_poster)) = old_posters.get(&key) {
                card.poster = poster.clone();
                card.has_poster = *has_poster;
            }
            card
        })
        .collect();
    let len = cards.len() as i32;
    g.set_discover_results(ModelRc::new(VecModel::from(cards)));
    g.set_discover_focused(g.get_discover_focused().clamp(0, (len - 1).max(0)));
    maybe_autofill_grid(&g);
}

/// One filtered-browse page's raw results, tagged with poster path — mirrors
/// `RequestedRowItem`'s own (meta, poster_path) shape for the identical
/// reason: the poster path has to travel alongside its meta through
/// `merge_filtered_metas`' re-sort, since an index-based zip (the pattern
/// `spawn_discover_search` itself uses) would break the moment merging
/// reorders rows.
pub(crate) type FilteredRowItem = (DiscoverCardMeta, Option<String>);

pub(crate) fn build_filtered_metas(results: &[SearchResult]) -> Vec<FilteredRowItem> {
    results
        .iter()
        .filter_map(|r| search_result_to_meta(r).map(|m| (m, r.poster_path.clone())))
        .collect()
}

/// Merges movie + TV filtered-browse results into one grid for Type=All —
/// confirmed via `AskUserQuestion`: interleaved by the ACTUAL value of
/// whichever sort key is active (both types' `popularity`/`vote_average`
/// are directly comparable; Newest/Oldest compare `year`, already
/// normalized to a plain int regardless of which date field it came from),
/// not the simpler movies-then-TV split. Implemented as concatenate-then-
/// sort rather than a true two-pointer merge — the two inputs are already
/// server-sorted, but re-sorting the small (≤2 pages') combined list
/// outright is simpler code for the identical final order.
/// Extracted from `merge_filtered_metas` (2026-07-31, code review finding)
/// so `spawn_discover_filtered_browse_more` can re-sort the FULL accumulated
/// set across a page boundary, not just each page's own batch — see that
/// function's own doc comment for the bug this fixes.
fn sort_filtered_metas(items: &mut [FilteredRowItem], sort_key: &str) {
    match sort_key {
        "rating" => items.sort_by(|a, b| {
            b.0.vote_average
                .partial_cmp(&a.0.vote_average)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
        "newest" => items.sort_by_key(|m| std::cmp::Reverse(m.0.year)),
        "oldest" => items.sort_by_key(|m| m.0.year),
        _ => items.sort_by(|a, b| {
            b.0.popularity
                .partial_cmp(&a.0.popularity)
                .unwrap_or(std::cmp::Ordering::Equal)
        }),
    }
}

fn merge_filtered_metas(
    movie: Vec<FilteredRowItem>,
    tv: Vec<FilteredRowItem>,
    sort_key: &str,
) -> Vec<FilteredRowItem> {
    let mut merged: Vec<FilteredRowItem> = movie.into_iter().chain(tv).collect();
    sort_filtered_metas(&mut merged, sort_key);
    merged
}

/// Discover filters' filtered-browse view (query empty, ≥1 filter active) —
/// page 1. Mirrors `spawn_discover_search`'s two-phase (text-then-posters)
/// commit shape closely, but sources from `discover_movies_filtered`/
/// `discover_tv_filtered` (real server-side filtering) instead of
/// `client.search`, and fetches both media types in parallel when Type is
/// "All" (`tokio::join!`, same shape `ensure_discover_landing` already
/// uses for its own 6-way parallel fetch), merging via
/// `merge_filtered_metas`. Shares the exact same `discover_gen` counter
/// `spawn_discover_search` uses — required, not optional: without it, a
/// slow debounced search response landing after this commits (or vice
/// versa) would clobber `discover-results` with a stale patch, since both
/// write into the same model.
pub(crate) fn spawn_discover_filtered_browse(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    generation: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = generation.fetch_add(1, Ordering::SeqCst) + 1;
    let Some(client) = state.lock().unwrap().seerr_client.clone() else {
        warn!("seerr: filtered-browse dispatched with no seerr_client set — not connected?");
        return;
    };
    {
        let mut s = state.lock().unwrap();
        s.discover_filtered_page = 0;
        s.discover_filtered_total_pages_movie = 0;
        s.discover_filtered_total_pages_tv = 0;
        s.discover_filtered_loading_more = false;
    }
    let is_session_auth = client.is_session_auth();

    rt.spawn(async move {
        let (type_key, sort_key) = {
            let s = state.lock().unwrap();
            let cp = s.config.active();
            (
                cp.discover_filter_type.clone(),
                cp.discover_filter_sort.clone(),
            )
        };
        let region = resolve_streaming_region(&client, &state).await;
        if generation.load(Ordering::SeqCst) != my_gen {
            return; // superseded before the region lookup even finished
        }
        let want_movie = type_key != "tv";
        let want_tv = type_key != "movie";
        let (movie_filters, tv_filters) = {
            let s = state.lock().unwrap();
            (
                build_discover_filters(&s, "movie", &region),
                build_discover_filters(&s, "tv", &region),
            )
        };
        let (movie_res, tv_res) = tokio::join!(
            async {
                if want_movie {
                    Some(client.discover_movies_filtered(1, &movie_filters).await)
                } else {
                    None
                }
            },
            async {
                if want_tv {
                    Some(client.discover_tv_filtered(1, &tv_filters).await)
                } else {
                    None
                }
            },
        );
        if generation.load(Ordering::SeqCst) != my_gen {
            return; // a newer filter change / search already superseded this
        }
        let movie_resp = match movie_res {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                handle_seerr_error(
                    &state,
                    &ww,
                    is_session_auth,
                    "Discover filter (movies) failed",
                    &e,
                );
                None
            }
            None => None,
        };
        let tv_resp = match tv_res {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                handle_seerr_error(
                    &state,
                    &ww,
                    is_session_auth,
                    "Discover filter (TV) failed",
                    &e,
                );
                None
            }
            None => None,
        };
        if movie_resp.is_none() && tv_resp.is_none() {
            return; // both wanted sides failed (error already surfaced above)
        }

        let movie_metas = movie_resp
            .as_ref()
            .map(|r| build_filtered_metas(&r.results))
            .unwrap_or_default();
        let tv_metas = tv_resp
            .as_ref()
            .map(|r| build_filtered_metas(&r.results))
            .unwrap_or_default();
        {
            let mut s = state.lock().unwrap();
            s.discover_filtered_page = 1;
            s.discover_filtered_total_pages_movie =
                movie_resp.as_ref().map(|r| r.total_pages).unwrap_or(0);
            s.discover_filtered_total_pages_tv =
                tv_resp.as_ref().map(|r| r.total_pages).unwrap_or(0);
            s.discover_filtered_loading_more = false;
        }
        let merged = merge_filtered_metas(movie_metas, tv_metas, &sort_key);
        debug!(
            "seerr: filtered-browse page 1 (type={type_key:?}) -> {} card(s)",
            merged.len()
        );
        state.lock().unwrap().discover_filtered_metas = merged.clone();

        let poster_jobs: Vec<(usize, String, String, String)> = merged
            .iter()
            .enumerate()
            .filter_map(|(i, (m, p))| {
                p.clone()
                    .map(|p| (i, m.item_type.to_string(), m.id.clone(), p))
            })
            .collect();

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_commit.upgrade() {
                let g = AppState::get(&w);
                let cards: Vec<CardItem> = merged
                    .into_iter()
                    .map(|(m, _)| m.into_card_item())
                    .collect();
                g.set_discover_results(ModelRc::new(VecModel::from(cards)));
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
                maybe_autofill_grid(&g);
            }
        });

        fetch_and_patch_posters(ww, generation, my_gen, poster_jobs).await;
    });
}

/// Filtered-browse's own load-more — mirrors `spawn_discover_search_more`
/// exactly, including the same synchronous-offset-read-before-network-call
/// safety (see that function's own comment for why it's race-safe against
/// a fresh fetch landing first). Both underlying TMDB pages (movie + TV)
/// advance together in lockstep rather than tracking two independent
/// cursors (confirmed via `AskUserQuestion` — simpler, and requesting one
/// page past a side that's already exhausted just returns an empty result
/// for that side, which merges in as a no-op); stops once BOTH sides are
/// exhausted (`max` of the two `total_pages`, not `min` — so a Type=All
/// browse doesn't stop early just because the shorter-tailed type ran out
/// first).
pub(crate) fn spawn_discover_filtered_browse_more(
    ww: Weak<MainWindow>,
    state: Arc<Mutex<FjordState>>,
    generation: Arc<AtomicU64>,
    rt: &tokio::runtime::Handle,
) {
    let my_gen = generation.load(Ordering::SeqCst);
    let (client, next_page, type_key, sort_key) = {
        let mut s = state.lock().unwrap();
        if s.discover_filtered_loading_more {
            return;
        }
        let max_total = s
            .discover_filtered_total_pages_movie
            .max(s.discover_filtered_total_pages_tv);
        if s.discover_filtered_page == 0 || s.discover_filtered_page >= max_total {
            return;
        }
        let Some(client) = s.seerr_client.clone() else {
            return;
        };
        s.discover_filtered_loading_more = true;
        (
            client,
            s.discover_filtered_page + 1,
            s.config.active().discover_filter_type.clone(),
            s.config.active().discover_filter_sort.clone(),
        )
    };
    let is_session_auth = client.is_session_auth();
    // Ids that already have a decoded poster in the live model — used below
    // to skip a redundant re-fetch/re-decode for them once the accumulated
    // set is re-sorted (their position in the list can change across a page
    // boundary, but their poster data doesn't need to). Snapshotting a plain
    // HashSet<String> here (not the Image itself, which is !Send) is safe to
    // read from inside the async block below; the model can't be mutated
    // from off the UI thread regardless.
    let known_poster_ids: std::collections::HashSet<String> = ww
        .upgrade()
        .map(|w| {
            let results = AppState::get(&w).get_discover_results();
            (0..results.row_count())
                .filter_map(|i| results.row_data(i))
                .filter(|c| c.has_poster)
                .map(|c| c.id.to_string())
                .collect()
        })
        .unwrap_or_default();

    let state2 = Arc::clone(&state);
    rt.spawn(async move {
        let region = resolve_streaming_region(&client, &state2).await;
        if generation.load(Ordering::SeqCst) != my_gen {
            state2.lock().unwrap().discover_filtered_loading_more = false;
            return;
        }
        let want_movie = type_key != "tv";
        let want_tv = type_key != "movie";
        let (movie_filters, tv_filters) = {
            let s = state2.lock().unwrap();
            (build_discover_filters(&s, "movie", &region), build_discover_filters(&s, "tv", &region))
        };
        let (movie_res, tv_res) = tokio::join!(
            async { if want_movie { Some(client.discover_movies_filtered(next_page, &movie_filters).await) } else { None } },
            async { if want_tv { Some(client.discover_tv_filtered(next_page, &tv_filters).await) } else { None } },
        );
        if generation.load(Ordering::SeqCst) != my_gen {
            state2.lock().unwrap().discover_filtered_loading_more = false;
            return;
        }
        let movie_resp = match movie_res {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                state2.lock().unwrap().discover_filtered_loading_more = false;
                handle_seerr_error(&state2, &ww, is_session_auth, "Discover filter (movies) failed", &e);
                None
            }
            None => None,
        };
        let tv_resp = match tv_res {
            Some(Ok(r)) => Some(r),
            Some(Err(e)) => {
                state2.lock().unwrap().discover_filtered_loading_more = false;
                handle_seerr_error(&state2, &ww, is_session_auth, "Discover filter (TV) failed", &e);
                None
            }
            None => None,
        };
        if movie_resp.is_none() && tv_resp.is_none() {
            state2.lock().unwrap().discover_filtered_loading_more = false;
            return;
        }
        let movie_metas = movie_resp.as_ref().map(|r| build_filtered_metas(&r.results)).unwrap_or_default();
        let tv_metas = tv_resp.as_ref().map(|r| build_filtered_metas(&r.results)).unwrap_or_default();
        let new_page_count = movie_metas.len() + tv_metas.len();
        // Accumulate this page onto the full fetch history, then re-sort the
        // WHOLE set — real bug, code review 2026-07-31: sorting and
        // committing only each page's own batch (the old behavior) left the
        // combined list visibly out of order across the page boundary the
        // moment a later page's top item outranked an earlier page's tail
        // item, since a plain append never re-establishes global order.
        let all_metas = {
            let mut s = state2.lock().unwrap();
            s.discover_filtered_page = next_page;
            if let Some(r) = &movie_resp {
                s.discover_filtered_total_pages_movie = r.total_pages;
            }
            if let Some(r) = &tv_resp {
                s.discover_filtered_total_pages_tv = r.total_pages;
            }
            s.discover_filtered_loading_more = false;
            s.discover_filtered_metas.extend(movie_metas);
            s.discover_filtered_metas.extend(tv_metas);
            let mut all = s.discover_filtered_metas.clone();
            sort_filtered_metas(&mut all, &sort_key);
            all
        };
        debug!("seerr: filtered-browse page {next_page} (type={type_key:?}) -> {new_page_count} more card(s), {} total", all_metas.len());

        // Only fetch/decode posters for ids that didn't already have one
        // before this page landed — re-sorting can move a known-poster item
        // to a new index, but its poster data doesn't need refetching.
        let poster_jobs: Vec<(usize, String, String, String)> = all_metas
            .iter()
            .enumerate()
            .filter(|(_, (m, _))| !known_poster_ids.contains(&m.id))
            .filter_map(|(i, (m, p))| p.clone().map(|p| (i, m.item_type.to_string(), m.id.clone(), p)))
            .collect();

        let ww_commit = ww.clone();
        let _ = slint::invoke_from_event_loop(move || {
            if let Some(w) = ww_commit.upgrade() {
                let g = AppState::get(&w);
                let existing = g.get_discover_results();
                let old_by_id: std::collections::HashMap<String, CardItem> = (0..existing.row_count())
                    .filter_map(|i| existing.row_data(i))
                    .map(|c| (c.id.to_string(), c))
                    .collect();
                let all: Vec<CardItem> = all_metas.into_iter().map(|(m, _)| {
                    let mut card = m.into_card_item();
                    if let Some(old) = old_by_id.get(card.id.as_str()) && old.has_poster {
                        card.poster = old.poster.clone();
                        card.has_poster = true;
                    }
                    card
                }).collect();
                g.set_discover_results(ModelRc::new(VecModel::from(all)));
                maybe_autofill_grid(&g);
            }
        });

        fetch_and_patch_posters(ww, generation, my_gen, poster_jobs).await;
    });
}
