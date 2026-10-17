// ── fjord-app · open_item.rs ─────────────────────────────────────────────────
//   Page / page_for   which page a Jellyfin item type opens — the one table (unit-tested)
//   open_item         opens an item's page: Series, Season (its series id from the server when
//                     the caller doesn't know it), BoxSet → collection, MusicAlbum / Playlist →
//                     the album page, MusicArtist, Person, anything else → the detail page
//   wire              AppState.open-detail(id, type) → open_item — every "open this item" goes
//                     through it: card click/Enter in grids and rows, View Details, filmographies,
//                     the artist grid, the dashboards' collection cards
// ─────────────────────────────────────────────────────────────────────────────
// Each page takes its title from the server's answer (CLAUDE.md → Screens), so callers pass only
// the id and type — no name lookups, which fell back to the raw id when a library list wasn't
// loaded yet. Before this, each entry point had its own type → page mapping: View Details sent a
// collection to the movie detail page while a click opened the collection page.
use std::sync::{Arc, Mutex};

use slint::{ComponentHandle, Global};
use tracing::{debug, warn};

use crate::config::FjordState;
use crate::{AppState, MainWindow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Page {
    Detail,
    Series,
    Season,
    Collection,
    Album,
    Playlist,
    Artist,
    Person,
}

/// The page an item of this Jellyfin type opens.
pub(crate) fn page_for(item_type: &str) -> Page {
    match item_type {
        "Series" => Page::Series,
        "Season" => Page::Season,
        "BoxSet" => Page::Collection,
        "MusicAlbum" => Page::Album,
        "Playlist" => Page::Playlist,
        "MusicArtist" => Page::Artist,
        "Person" => Page::Person,
        _ => Page::Detail,
    }
}

/// Opens the item's page. `series_id` is only for a Season (its page shows the series' seasons);
/// empty → asked from the server first.
pub(crate) fn open_item(
    id: String,
    item_type: &str,
    series_id: String,
    state: &Arc<Mutex<FjordState>>,
    ww: &slint::Weak<MainWindow>,
    rt: &tokio::runtime::Handle,
) {
    let page = page_for(item_type);
    debug!("open_item({id}, {item_type}) → {page:?}");
    let (state, ww, rt) = (Arc::clone(state), ww.clone(), rt.clone());
    match page {
        Page::Detail => crate::detail::open_detail(id, item_type.to_string(), state, ww, rt),
        Page::Series => crate::series::open_series_screen(id, state, ww, rt),
        Page::Collection => {
            crate::collection::open_collection_screen(id, String::new(), state, ww, rt)
        }
        Page::Album => crate::album::open_album_screen(id, String::new(), state, ww, rt),
        Page::Playlist => crate::album::open_playlist_screen(id, String::new(), state, ww, rt),
        Page::Artist => crate::artist::open_artist_screen(id, String::new(), state, ww, rt),
        Page::Person => crate::person::open_person_screen(id, String::new(), state, ww, rt),
        Page::Season if !series_id.is_empty() => {
            crate::season::open_season_screen(id, series_id, state, ww, rt)
        }
        Page::Season => {
            let Some(client) = state.lock().unwrap().client.as_ref().map(Arc::clone) else {
                return;
            };
            let rt2 = rt.clone();
            rt.spawn(async move {
                let series_id = match client.get_item_detail(&id).await {
                    Ok(d) => d.series_id.unwrap_or_default(),
                    Err(e) => {
                        warn!("open_item: season {id} detail: {e:#}");
                        crate::show_toast(
                            ww.clone(),
                            format!("Couldn't open the season — {}", crate::short_error(&e)),
                        );
                        return;
                    }
                };
                let _ = slint::invoke_from_event_loop(move || {
                    crate::season::open_season_screen(id, series_id, state, ww, rt2);
                });
            });
        }
    }
}

/// AppState.open-detail(id, type): the one "open this item" callback.
pub(crate) fn wire(
    window: &MainWindow,
    state: &Arc<Mutex<FjordState>>,
    rt: &tokio::runtime::Runtime,
) {
    let state = Arc::clone(state);
    let ww = window.as_weak();
    let rt = rt.handle().clone();
    AppState::get(window).on_open_detail(move |id, item_type| {
        open_item(
            id.to_string(),
            item_type.as_str(),
            String::new(),
            &state,
            &ww,
            &rt,
        );
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_type_opens_its_page() {
        for (t, page) in [
            ("Movie", Page::Detail),
            ("Episode", Page::Detail),
            ("Video", Page::Detail),
            ("Audio", Page::Detail),
            ("Series", Page::Series),
            ("Season", Page::Season),
            ("BoxSet", Page::Collection),
            ("MusicAlbum", Page::Album),
            ("Playlist", Page::Playlist),
            ("MusicArtist", Page::Artist),
            ("Person", Page::Person),
        ] {
            assert_eq!(page_for(t), page, "{t}");
        }
    }
}
