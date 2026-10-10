// ── fjord-app · discover/keys.rs ─────────────────────────────────────────────
//   discover_popup_options / open_discover_popup / handle_key_discover_popup  filter popups: single-
//                              select lists (Type/Sort/Rating/Year) and multi-select chips (Genre/
//                              Provider, Confirm toggles without closing); mouse picks go through the
//                              same function (discover-popup-confirm)
//   handle_key_discover_filter_bar  Left/Right over the 7 pills (… Clear), Up → search field, Down →
//                              grid/landing, Confirm opens the pill's popup; mouse via discover-filter-bar-confirm
//   handle_key                 Discover grid: fs<0 sidebar (Up/Down tabs, Right enters), fs>=0 grid with
//                              LibraryGrid's 2D math; Left at col 0 / Back → sidebar; C → context menu
//   handle_key_landing         landing rows; the Coming Up sentinel opens the calendar
//   existing_zones / zone_focus_reset / handle_key_request_detail  RequestDetailScreen: back → button row
//                              → storyline → cast → tags → seasons (existing zones only)
//   existing_detail_btn_slots  button-row slots: 0=Request 1=Trailer 2=⋮ More 3=Watchlist 4=Blocklist;
//                              Left/Right within existing slots
//   existing_option_zones / option_zone_focus_reset / handle_key_request_options  Request Options modal:
//                              Quality → profile → tags → seasons → Cancel/Request
//   set_quality                swap in the other tier's pre-fetched tags/profiles (keyboard and the 2K/4K
//                              buttons both use it)
// ─────────────────────────────────────────────────────────────────────────────
use super::*;

// ── Keyboard: Discover filter bar + popups ──────────────────────────────────
// Discover's 4th keyboard mechanism, next to the search field's raw pre-dispatch,
// fs<0 sidebar and fs>=0 grid/landing. Two states: `discover-filter-bar-active` (no
// popup — Left/Right move the pill cursor, Enter applies, like the library sort bar)
// and `discover-popup-open` (captures ALL input, like Settings' `settings-dropdown-open`,
// built here because `SettingsDropdown` has no keyboard path of its own).

fn discover_popup_options(kind: &str) -> Vec<&'static str> {
    match kind {
        "type" => vec!["All", "Movies", "TV"],
        "sort" => SORT_KEYS.iter().map(|(_, d)| *d).collect(),
        "rating" => RATING_BUCKETS.iter().map(|(d, _)| *d).collect(),
        "year" => YEAR_BUCKETS.iter().map(|(d, _)| *d).collect(),
        _ => Vec::new(),
    }
}

/// Opens a popup with the cursor pre-set to the currently active value's
/// index — same "cursor starts on the current selection" convention
/// `SettingsDropdown`'s own popup already uses. Genre/Provider (multi-
/// select chip strips, not a single current value) just start at 0.
fn open_discover_popup(g: &AppState, kind: &'static str) {
    let cursor = match kind {
        "genre" | "provider" => 0,
        _ => {
            let current = match kind {
                "type" => g.get_discover_filter_type_desc(),
                "sort" => g.get_discover_filter_sort_desc(),
                "rating" => g.get_discover_filter_rating_desc(),
                "year" => g.get_discover_filter_year_desc(),
                _ => Default::default(),
            };
            discover_popup_options(kind)
                .iter()
                .position(|&d| d == current.as_str())
                .unwrap_or(0)
        }
    };
    g.set_discover_popup_cursor(cursor as i32);
    g.set_discover_popup_open(kind.into());
}

/// `discover-popup-open != ""` — captures all input. Type/Sort/Rating/Year
/// are single-value lists (Up/Down move the cursor, Confirm applies +
/// closes); Genre/Provider are multi-select chip strips (Left/Right move
/// the cursor, Confirm toggles the chip at the cursor WITHOUT closing —
/// same "stays open for further toggling" shape `RequestOptionsOverlay`'s
/// own tag chips already use).
pub(crate) fn handle_key_discover_popup(action: &Action, g: &AppState) -> bool {
    let kind = g.get_discover_popup_open().to_string();
    match kind.as_str() {
        "type" | "sort" | "rating" | "year" => {
            let options = discover_popup_options(&kind);
            match action {
                Action::Up => {
                    let c = g.get_discover_popup_cursor();
                    if c > 0 {
                        g.set_discover_popup_cursor(c - 1);
                    }
                    true
                }
                Action::Down => {
                    let c = g.get_discover_popup_cursor();
                    if (c as usize) + 1 < options.len() {
                        g.set_discover_popup_cursor(c + 1);
                    }
                    true
                }
                Action::Confirm => {
                    let desc: slint::SharedString = options
                        .get(g.get_discover_popup_cursor() as usize)
                        .copied()
                        .unwrap_or("")
                        .into();
                    match kind.as_str() {
                        "type" => g.invoke_discover_filter_type_selected(desc),
                        "sort" => g.invoke_discover_filter_sort_selected(desc),
                        "rating" => g.invoke_discover_filter_rating_selected(desc),
                        "year" => g.invoke_discover_filter_year_selected(desc),
                        _ => {}
                    }
                    g.set_discover_popup_open("".into());
                    true
                }
                Action::Back => {
                    g.set_discover_popup_open("".into());
                    true
                }
                _ => true,
            }
        }
        "genre" | "provider" => {
            let count = if kind == "genre" {
                g.get_discover_filter_genres().row_count()
            } else {
                g.get_discover_filter_providers().row_count()
            } as i32;
            match action {
                Action::Left => {
                    let c = g.get_discover_popup_cursor();
                    if c > 0 {
                        g.set_discover_popup_cursor(c - 1);
                    }
                    true
                }
                Action::Right => {
                    let c = g.get_discover_popup_cursor();
                    if c + 1 < count {
                        g.set_discover_popup_cursor(c + 1);
                    }
                    true
                }
                Action::Confirm => {
                    let c = g.get_discover_popup_cursor();
                    if kind == "genre" {
                        g.invoke_discover_filter_genre_toggle(c);
                    } else {
                        g.invoke_discover_filter_provider_toggle(c);
                    }
                    true // stays open — multi-select
                }
                Action::Back => {
                    g.set_discover_popup_open("".into());
                    true
                }
                _ => true,
            }
        }
        _ => false, // popup-open had an unrecognized value — shouldn't happen
    }
}

/// `discover-filter-bar-active` (no popup open) — the pill row itself.
/// Pill order: 0=Type 1=Genre 2=Sort 3=Rating 4=Year 5=Provider 6=Clear
/// (matches `discover-filter-bar-focused`'s own doc comment in
/// app_state.slint and `discover.slint`'s left-to-right rendering order).
pub(crate) fn handle_key_discover_filter_bar(action: &Action, g: &AppState) -> bool {
    match action {
        Action::Left => {
            let f = g.get_discover_filter_bar_focused();
            if f > 0 {
                g.set_discover_filter_bar_focused(f - 1);
            }
            true
        }
        Action::Right => {
            let f = g.get_discover_filter_bar_focused();
            if f < 6 {
                g.set_discover_filter_bar_focused(f + 1);
            }
            true
        }
        Action::Up => {
            g.set_discover_filter_bar_active(false);
            g.set_discover_header_focused(true);
            true
        }
        Action::Down => {
            g.set_discover_filter_bar_active(false);
            // Filtered-browse (query empty, >=1 filter active) uses the flat
            // discover-results grid, not the landing-row models — same
            // routing fix as handle_key's own `landing` check above.
            if g.get_discover_query().as_str().is_empty() && !g.get_discover_filters_active() {
                if let Some(first) = landing_row_lens(g).iter().position(|&n| n > 0) {
                    g.set_focused_section(first as i32);
                    g.set_discover_landing_card(0);
                }
            } else if g.get_discover_results().row_count() > 0 {
                g.set_focused_section(0);
                g.set_discover_focused(0);
                g.set_discover_focused_row(0);
            }
            true
        }
        Action::Back => {
            g.set_discover_filter_bar_active(false);
            true
        }
        Action::Confirm => {
            match g.get_discover_filter_bar_focused() {
                0 => open_discover_popup(g, "type"),
                1 => open_discover_popup(g, "genre"),
                2 => open_discover_popup(g, "sort"),
                3 => open_discover_popup(g, "rating"),
                4 => open_discover_popup(g, "year"),
                5 => open_discover_popup(g, "provider"),
                6 => g.invoke_discover_filter_clear(),
                _ => {}
            }
            true
        }
        _ => true,
    }
}

// ── Keyboard: Discover grid (search typing is a raw pre-dispatch in keys.rs) ──
// `focused_section` (fs) is the sidebar/content toggle shared by the dashboard tabs:
// < 0 = sidebar, >= 0 = content. Discover has its own AppMode (a flat grid, not
// dispatch_dashboard's SectionRows), so it keeps that contract itself — including
// with zero results (every first visit), where it's the only way out.
pub(crate) fn handle_key(action: &Action, g: &AppState) -> bool {
    if !g.get_discover_popup_open().as_str().is_empty() {
        return handle_key_discover_popup(action, g);
    }
    if g.get_discover_filter_bar_active() {
        return handle_key_discover_filter_bar(action, g);
    }

    let fs = g.get_focused_section();
    // Filtered-browse (query empty, >=1 filter active) renders into the same
    // discover-results grid search uses, not the landing-row models — must
    // route through the flat-grid dispatch below, not handle_key_landing.
    let landing = g.get_discover_query().as_str().is_empty() && !g.get_discover_filters_active();

    if fs < 0 {
        // Sidebar-focused: Up/Down cycle sidebar tabs (matches
        // dispatch_dashboard's fs<0 branch exactly — Down does NOT enter the
        // grid, it moves to the next tab); Right enters Discover's own
        // content — the first non-empty landing row when there's no query,
        // the results grid when there is one, the search field if neither
        // has anything to focus yet.
        return match action {
            Action::Up => {
                crate::browse::sidebar_nav(g, -1);
                true
            }
            Action::Down => {
                crate::browse::sidebar_nav(g, 1);
                true
            }
            Action::Right => {
                if landing {
                    if let Some(first) = landing_row_lens(g).iter().position(|&n| n > 0) {
                        g.set_focused_section(first as i32);
                        g.set_discover_landing_card(0);
                    } else {
                        g.set_discover_header_focused(true);
                    }
                } else if g.get_discover_results().row_count() > 0 {
                    g.set_focused_section(0);
                    g.set_discover_focused(0);
                    g.set_discover_focused_row(0);
                } else {
                    g.set_discover_header_focused(true);
                }
                true
            }
            _ => false,
        };
    }

    if landing {
        return handle_key_landing(action, g, fs);
    }

    let count = g.get_discover_results().row_count() as i32;
    if count == 0 {
        return match action {
            Action::Up => {
                g.set_discover_filter_bar_active(true);
                true
            }
            Action::Back | Action::Left => {
                g.set_focused_section(-1);
                true
            }
            _ => false,
        };
    }
    let cols = g.get_library_cols().max(1);
    match action {
        Action::Left => {
            let f = g.get_discover_focused();
            if f % cols > 0 {
                g.set_discover_focused(f - 1);
            } else if f > 0 {
                let nf = f - 1;
                g.set_discover_focused(nf);
                g.set_discover_focused_row(nf / cols);
            } else {
                g.set_focused_section(-1); // leftmost card, top row — back to sidebar
            }
            true
        }
        Action::Right => {
            let f = g.get_discover_focused();
            if f % cols < cols - 1 && f + 1 < count {
                g.set_discover_focused(f + 1);
            } else if f + 1 < count {
                let nf = f + 1;
                g.set_discover_focused(nf);
                g.set_discover_focused_row(nf / cols);
            }
            true
        }
        Action::Up => {
            let f = g.get_discover_focused();
            if f >= cols {
                let nf = f - cols;
                g.set_discover_focused(nf);
                g.set_discover_focused_row(nf / cols);
            } else {
                g.set_discover_filter_bar_active(true);
            }
            true
        }
        Action::Down => {
            let f = g.get_discover_focused();
            if f + cols < count {
                let nf = f + cols;
                g.set_discover_focused(nf);
                g.set_discover_focused_row(nf / cols);
                true
            } else {
                // At the last row of the currently-loaded results: kick off
                // a fetch of the next search-results page, if one exists
                // (see spawn_discover_search_more — quietly no-ops when
                // there isn't one, one's already in flight, or this isn't
                // a search grid at all). Still returns false either way —
                // this doesn't move focus, so focus_bar_on_down should
                // still get a chance to run for an active player bar.
                g.invoke_discover_load_more();
                false
            }
        }
        Action::Confirm => {
            let f = g.get_discover_focused();
            if f < count
                && let Some(card) = g.get_discover_results().row_data(f as usize)
            {
                let media_type = if card.item_type.as_str() == "DiscoverMovie" {
                    "movie"
                } else {
                    "tv"
                };
                g.invoke_open_discover_item(media_type.into(), card.id);
            }
            true
        }
        Action::OpenContextMenu => {
            let f = g.get_discover_focused();
            if f < count
                && let Some(card) = g.get_discover_results().row_data(f as usize)
            {
                g.invoke_open_context_menu_discover(card);
            }
            true
        }
        Action::Back => {
            g.set_focused_section(-1);
            true
        }
        _ => false,
    }
}

/// Landing-row nav (fs 0-4 selects the row; discover-landing-card is the
/// column within it). No column-tracking-per-row-for-scroll needed the way
/// the flat grid needs discover-focused-row — each row is its own
/// independently-scrolling SectionRow (kb-x, same as Home's), so only the
/// row index (fs) and the column within whichever row is focused matter.
fn handle_key_landing(action: &Action, g: &AppState, fs: i32) -> bool {
    let lens = landing_row_lens(g);
    let count = lens[fs as usize];
    match action {
        Action::Left => {
            let c = g.get_discover_landing_card();
            if c > 0 {
                g.set_discover_landing_card(c - 1);
            } else {
                g.set_focused_section(-1);
            }
            true
        }
        Action::Right => {
            let c = g.get_discover_landing_card();
            if c + 1 < count {
                g.set_discover_landing_card(c + 1);
            }
            true
        }
        Action::Up => {
            if fs > 0 {
                let nf = fs - 1;
                g.set_focused_section(nf);
                g.set_discover_landing_card(
                    g.get_discover_landing_card()
                        .min((lens[nf as usize] - 1).max(0)),
                );
            } else {
                g.set_discover_filter_bar_active(true);
            }
            true
        }
        Action::Down => {
            if (fs as usize) + 1 < lens.len() {
                let nf = fs + 1;
                g.set_focused_section(nf);
                g.set_discover_landing_card(
                    g.get_discover_landing_card()
                        .min((lens[nf as usize] - 1).max(0)),
                );
                debug!(
                    "seerr: landing down fs={fs}->{nf} lens={lens:?} card={}",
                    g.get_discover_landing_card()
                );
                true
            } else {
                false // last row — let focus_bar_on_down handle it
            }
        }
        Action::Confirm => {
            let c = g.get_discover_landing_card().max(0);
            // The Coming Up row's trailing sentinel (no tmdb id) opens the calendar —
            // handle_key_landing is generic over all rows and would otherwise open a Discover
            // item from garbage data.
            if fs as usize == LANDING_ROW_COMING_UP && c == count - 1 && count > 0 {
                g.invoke_open_calendar();
            } else if c < count
                && let Some(card) = landing_row_get(g, fs as usize).row_data(c as usize)
            {
                let media_type = if card.item_type.as_str() == "DiscoverMovie" {
                    "movie"
                } else {
                    "tv"
                };
                g.invoke_open_discover_item(media_type.into(), card.id);
            }
            true
        }
        Action::OpenContextMenu => {
            let c = g.get_discover_landing_card().max(0);
            // Same sentinel special-case as Confirm above — right-click/`C`
            // on the fake last card must be inert, not open a context menu
            // for nothing.
            if fs as usize == LANDING_ROW_COMING_UP && c == count - 1 && count > 0 {
                return true;
            }
            if c < count
                && let Some(card) = landing_row_get(g, fs as usize).row_data(c as usize)
            {
                g.invoke_open_context_menu_discover(card);
            }
            true
        }
        Action::Back => {
            g.set_focused_section(-1);
            true
        }
        _ => false,
    }
}

// ── Keyboard: Request detail screen ─────────────────────────────────────────

/// Zones below the button row, in vertical order, that exist for the current
/// item — storyline/cast each drop out individually when there's nothing to
/// show. Up/Down between zones (and the button row itself, always zone 0)
/// only ever step to the nearest zone that actually exists. Tags/seasons/4K
/// used to be zones here too, but moved into the Request Options modal
/// (`existing_option_zones`/`handle_key_request_options` below) — the
/// Request button opens that modal instead of exposing every picker inline.
fn existing_zones(g: &AppState) -> Vec<i32> {
    let mut zones = vec![0]; // button row always exists
    if !g.get_request_detail_overview().as_str().is_empty() {
        zones.push(1);
    }
    if g.get_request_detail_cast().row_count() > 0 {
        zones.push(2);
    }
    zones
}

/// Which of the button row's slots exist for the current item ("gaps are fine",
/// like `existing_zones`/`existing_discover_menu_rows`): 0=Request (a tier still
/// requestable), 1=Trailer (one known to play + yt-dlp), 2=⋮ More (a request
/// exists), 3=Watchlist (always), 4=Blocklist (untouched or blocklisted item +
/// MANAGE_BLOCKLIST). Values are `request-detail-btn-focused` ids, rendered in this
/// order by request_detail.slint.
pub(crate) fn existing_detail_btn_slots(g: &AppState) -> Vec<i32> {
    let mut slots = Vec::new();
    if g.get_request_detail_status().as_str() == ""
        || g.get_request_detail_status_4k().as_str() == ""
    {
        slots.push(0);
    }
    // Only once a trailer is known to play — while checking (or with none
    // playable) the button is shown greyed out and isn't a D-pad stop.
    if g.get_request_detail_trailer_state().as_str() == "ok" && g.get_yt_dlp_available() {
        slots.push(1);
    }
    if !g.get_request_detail_request_id().as_str().is_empty() {
        slots.push(2);
    }
    slots.push(3); // Watchlist (2026-07-18) — always visible, unlike Request
    // Blocklist (2026-08-06) — mirrors the Discover context menu's row 7
    // eligibility exactly: never-touched or already-blocklisted, and the
    // connected account actually has MANAGE_BLOCKLIST.
    let availability = g.get_request_detail_availability();
    if g.get_seerr_can_manage_blocklist()
        && (availability.as_str() == "" || availability.as_str() == "blocklisted")
    {
        slots.push(4);
    }
    slots
}

fn zone_focus_reset(g: &AppState, zone: i32) {
    if zone == 2 {
        g.set_request_detail_focused_cast(0);
    }
}

/// Vertical flow: Back -> button row (Request) -> storyline (collapsible
/// overview) -> cast row. Each zone's Up-at-top/Down-at-bottom hands off to
/// the nearest neighboring zone that exists for the current item.
pub(crate) fn handle_key_request_detail(action: &Action, g: &AppState) -> bool {
    if g.get_request_detail_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_request_detail(false);
                true
            }
            Action::Down => {
                g.set_request_detail_back_focused(false);
                g.set_request_detail_zone(0);
                true
            }
            Action::Up => false, // let focus_bar_on_up handle the mini-player bar
            _ => true,
        };
    }

    let zones = existing_zones(g);
    let zone = g.get_request_detail_zone();
    let zone_pos = zones.iter().position(|&z| z == zone).unwrap_or(0);
    let prev_zone = || zone_pos.checked_sub(1).and_then(|i| zones.get(i)).copied();
    let next_zone = || zones.get(zone_pos + 1).copied();

    match zone {
        // Storyline — Enter toggles expand/collapse, no L/R (single item).
        1 => {
            return match action {
                Action::Confirm => {
                    g.set_request_detail_overview_expanded(
                        !g.get_request_detail_overview_expanded(),
                    );
                    true
                }
                Action::Up => {
                    g.set_request_detail_back_focused(true);
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_detail_zone(next);
                        zone_focus_reset(g, next);
                    }
                    true
                }
                Action::Back => {
                    g.set_show_request_detail(false);
                    true
                }
                _ => true,
            };
        }
        // Cast & Crew row — L/R scroll, Enter opens the person (the same callback as the
        // mouse click in request_detail.slint).
        2 => {
            let count = g.get_request_detail_cast().row_count() as i32;
            return match action {
                Action::Left => {
                    let f = g.get_request_detail_focused_cast();
                    if f > 0 {
                        g.set_request_detail_focused_cast(f - 1);
                    }
                    true
                }
                Action::Right => {
                    let f = g.get_request_detail_focused_cast();
                    if f + 1 < count {
                        g.set_request_detail_focused_cast(f + 1);
                    }
                    true
                }
                Action::Confirm => {
                    let f = g.get_request_detail_focused_cast() as usize;
                    if let Some(member) = g.get_request_detail_cast().row_data(f) {
                        g.invoke_open_discover_person(member.id, member.name);
                    }
                    true
                }
                Action::Up => {
                    g.set_request_detail_focused_cast(-1);
                    match prev_zone() {
                        Some(prev) => g.set_request_detail_zone(prev),
                        None => g.set_request_detail_back_focused(true),
                    }
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_detail_focused_cast(-1);
                        g.set_request_detail_zone(next);
                        zone_focus_reset(g, next);
                    }
                    true
                }
                Action::Back => {
                    g.set_show_request_detail(false);
                    true
                }
                _ => true,
            };
        }
        _ => {} // zone 0 falls through to the button row below
    }

    // Zone 0: Request button (visible while at least one tier is still
    // requestable) + up to two tier-status pills (non-interactive) + an
    // independent Trailer button (Watch Trailer is unrelated to request
    // status — see request-detail-trailer-url's own doc comment) + ⋮ More
    // (only once a request exists). Confirm on Request opens the Request
    // Options modal rather than submitting directly; 4K/tags/seasons are
    // configured there, not on this page.
    let slots = existing_detail_btn_slots(g);
    // Clamp request-detail-btn-focused to an existing slot on every zone-0 key press
    // (not only on entry), so any transition self-corrects.
    if !slots.contains(&g.get_request_detail_btn_focused()) {
        g.set_request_detail_btn_focused(slots.first().copied().unwrap_or(0));
    }
    let btn_focused = g.get_request_detail_btn_focused();
    let slot_pos = slots.iter().position(|&s| s == btn_focused).unwrap_or(0);
    match action {
        Action::Up => {
            g.set_request_detail_back_focused(true);
            true
        }
        Action::Down => {
            if let Some(next) = next_zone() {
                g.set_request_detail_zone(next);
                zone_focus_reset(g, next);
            }
            true
        }
        Action::Left => match slot_pos.checked_sub(1).and_then(|i| slots.get(i)) {
            Some(&prev) => {
                g.set_request_detail_btn_focused(prev);
                true
            }
            None => false,
        },
        Action::Right => match slots.get(slot_pos + 1) {
            Some(&next) => {
                g.set_request_detail_btn_focused(next);
                true
            }
            None => false,
        },
        Action::Confirm => {
            match btn_focused {
                0 => g.invoke_open_request_options(),
                1 => g.invoke_play_trailer(),
                2 => g.invoke_open_discover_menu_from_detail(),
                // Slot 3 (Watchlist) on Enter, like its mouse `clicked` in request_detail.slint.
                3 => g.invoke_request_detail_toggle_watchlist(),
                4 => g.invoke_request_detail_toggle_blocklist(),
                _ => {}
            }
            true
        }
        Action::Back => {
            g.set_show_request_detail(false);
            true
        }
        _ => false,
    }
}

// ── Keyboard: Request Options modal ─────────────────────────────────────────

/// Zones inside the modal, in vertical order, that exist for the current
/// item: 0=Quality (2K/4K, always — every requestable item can ask for 4K),
/// 1=profile row (if any configured), 2=tags row (if any configured),
/// 3=seasons row (if any, TV only), 4=confirm row (always, Cancel/Request).
/// Same skip-absent-zones array-position navigation idiom as `existing_zones`
/// above — a distinct numbering scheme, not a continuation of it, since this
/// is an independent vertical flow inside its own modal.
pub(crate) fn existing_option_zones(g: &AppState) -> Vec<i32> {
    // Zone 0 (Quality) is hidden entirely while editing — see
    // request_detail.slint's own comment on the Quality section for why.
    let mut zones = if g.get_request_options_editing() {
        Vec::new()
    } else {
        vec![0]
    };
    if g.get_request_detail_profiles().row_count() > 0 {
        zones.push(1);
    }
    if g.get_request_detail_tags().row_count() > 0 {
        zones.push(2);
    }
    if g.get_request_detail_media_type().as_str() == "tv"
        && g.get_request_detail_seasons().row_count() > 0
    {
        zones.push(3);
    }
    zones.push(4);
    zones
}

fn option_zone_focus_reset(g: &AppState, zone: i32) {
    match zone {
        1 => g.set_request_detail_focused_profile(0),
        2 => g.set_request_detail_focused_tag(0),
        3 => g.set_request_detail_focused_season(0),
        _ => {}
    }
}

/// Sets the Quality toggle, swapping in the other tier's pre-fetched
/// tags/profiles/selected-profile-id when the value actually changes —
/// both tiers were fetched up front by `open_discover_item`
/// (`available_request_options_both_tiers`), so this is instant with no
/// network call and no race on rapid toggling. Selections travel with
/// whichever set they belong to (swapped, not reset), so toggling away and
/// back preserves what was picked on each tier. Shared by both the keyboard
/// handler below and the Slint `request-detail-set-quality` callback the
/// 2K/4K buttons' mouse clicks go through.
pub(crate) fn set_quality(g: &AppState, want_4k: bool) {
    if g.get_request_detail_want_4k() == want_4k {
        return;
    }
    let tags = g.get_request_detail_tags();
    g.set_request_detail_tags(g.get_request_detail_tags_alt());
    g.set_request_detail_tags_alt(tags);

    let profiles = g.get_request_detail_profiles();
    g.set_request_detail_profiles(g.get_request_detail_profiles_alt());
    g.set_request_detail_profiles_alt(profiles);

    let selected_profile_id = g.get_request_detail_selected_profile_id();
    g.set_request_detail_selected_profile_id(g.get_request_detail_selected_profile_id_alt());
    g.set_request_detail_selected_profile_id_alt(selected_profile_id);

    g.set_request_detail_focused_tag(0);
    g.set_request_detail_focused_profile(0);
    g.set_request_detail_want_4k(want_4k);
}

/// Back/Escape closes the modal (Cancel) from any zone. Otherwise: Quality
/// row -> profile row -> tags row -> seasons row -> confirm row
/// (Cancel/Request), Up/Down stepping to the nearest zone that exists for
/// this item.
pub(crate) fn handle_key_request_options(action: &Action, g: &AppState) -> bool {
    if matches!(action, Action::Back) {
        g.set_show_request_options(false);
        return true;
    }

    let zones = existing_option_zones(g);
    let zone = g.get_request_options_zone();
    let zone_pos = zones.iter().position(|&z| z == zone).unwrap_or(0);
    let prev_zone = || zone_pos.checked_sub(1).and_then(|i| zones.get(i)).copied();
    let next_zone = || zones.get(zone_pos + 1).copied();

    match zone {
        // Quality (2K/4K) pair, not a single on/off toggle — Left/Right pick
        // the value directly rather than moving a separate cursor, since the
        // selected button already IS the keyboard position (no Confirm step
        // needed on top of it).
        0 => match action {
            Action::Left => {
                set_quality(g, false);
                true
            }
            Action::Right => {
                set_quality(g, true);
                true
            }
            Action::Down => {
                if let Some(next) = next_zone() {
                    g.set_request_options_zone(next);
                    option_zone_focus_reset(g, next);
                }
                true
            }
            _ => true, // Up absorbed — already the top zone
        },
        // Quality profile row (radio-select) — L/R scroll the cursor, Enter
        // selects the profile under it (replacing, not toggling like tags).
        1 => {
            let count = g.get_request_detail_profiles().row_count() as i32;
            match action {
                Action::Left => {
                    let f = g.get_request_detail_focused_profile();
                    if f > 0 {
                        g.set_request_detail_focused_profile(f - 1);
                    }
                    true
                }
                Action::Right => {
                    let f = g.get_request_detail_focused_profile();
                    if f + 1 < count {
                        g.set_request_detail_focused_profile(f + 1);
                    }
                    true
                }
                Action::Confirm => {
                    let idx = g.get_request_detail_focused_profile();
                    if let Some(p) = g.get_request_detail_profiles().row_data(idx as usize) {
                        g.set_request_detail_selected_profile_id(p.id);
                    }
                    true
                }
                Action::Up => {
                    if let Some(prev) = prev_zone() {
                        g.set_request_options_zone(prev);
                    }
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_options_zone(next);
                        option_zone_focus_reset(g, next);
                    }
                    true
                }
                _ => true,
            }
        }
        2 => {
            let count = g.get_request_detail_tags().row_count() as i32;
            match action {
                Action::Left => {
                    let f = g.get_request_detail_focused_tag();
                    if f > 0 {
                        g.set_request_detail_focused_tag(f - 1);
                    }
                    true
                }
                Action::Right => {
                    let f = g.get_request_detail_focused_tag();
                    if f + 1 < count {
                        g.set_request_detail_focused_tag(f + 1);
                    }
                    true
                }
                Action::Confirm => {
                    g.invoke_request_detail_toggle_tag(g.get_request_detail_focused_tag());
                    true
                }
                Action::Up => {
                    if let Some(prev) = prev_zone() {
                        g.set_request_options_zone(prev);
                    }
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_options_zone(next);
                        option_zone_focus_reset(g, next);
                    }
                    true
                }
                _ => true,
            }
        }
        3 => {
            let count = g.get_request_detail_seasons().row_count() as i32;
            match action {
                Action::Left => {
                    let f = g.get_request_detail_focused_season();
                    if f > 0 {
                        g.set_request_detail_focused_season(f - 1);
                    }
                    true
                }
                Action::Right => {
                    let f = g.get_request_detail_focused_season();
                    if f + 1 < count {
                        g.set_request_detail_focused_season(f + 1);
                    }
                    true
                }
                Action::Confirm => {
                    g.invoke_request_detail_toggle_season(g.get_request_detail_focused_season());
                    true
                }
                Action::Up => {
                    if let Some(prev) = prev_zone() {
                        g.set_request_options_zone(prev);
                    }
                    true
                }
                Action::Down => {
                    if let Some(next) = next_zone() {
                        g.set_request_options_zone(next);
                    }
                    true
                }
                _ => true,
            }
        }
        _ => match action {
            // Confirm row: Left/Right toggle Cancel/Request, Enter activates
            // whichever is focused. Request submits and closes; Cancel just closes.
            Action::Left => {
                g.set_request_options_confirm_focused(0);
                true
            }
            Action::Right => {
                g.set_request_options_confirm_focused(1);
                true
            }
            Action::Up => {
                if let Some(prev) = prev_zone() {
                    g.set_request_options_zone(prev);
                }
                true
            }
            Action::Confirm => {
                let submit = g.get_request_options_confirm_focused() == 1;
                g.set_show_request_options(false);
                if submit {
                    g.invoke_request_detail_request();
                }
                true
            }
            _ => true,
        },
    }
}
