// ── fjord-app · collection.rs ─────────────────────────────────────────────────
//   open_collection_screen  reset AppState collection props; set app-content-loading=true;
//                           spawn async: fetch BoxSet items + poster + item-detail in parallel;
//                           backdrop only when backdrop_image_tags non-empty; stale-request guard
//                           (collection-id check) + early-return-on-error with toast;
//                           single invoke_from_event_loop sets all data then shows page
//   handle_key              keyboard dispatch for the collection screen:
//                           grid nav (Up/Down/Left/Right + Enter → open-detail + C → ctx-menu);
//                           Back button focus (Up from row 0); Back → close
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, Model, ModelRc, VecModel};
use tracing::warn;

use crate::config::FjordState;
use crate::AppState;
use crate::detail::{fetch_card_posters, items_to_cards};
use crate::poster::{decode_poster_buffer, fetch_backdrop_cached, fetch_poster_cached};
use crate::MainWindow;

// ── open_collection_screen ────────────────────────────────────────────────────

pub(crate) fn open_collection_screen(
    id:    String,
    title: String,
    state: Arc<Mutex<FjordState>>,
    ww:    slint::Weak<MainWindow>,
    rt:    tokio::runtime::Handle,
) {
    let client = {
        let s = state.lock().unwrap();
        let Some(c) = s.client.as_ref().map(Arc::clone) else { return };
        c
    };

    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_collection_id(id.as_str().into());
        g.set_collection_title(title.as_str().into());
        g.set_collection_has_poster(false);
        g.set_collection_has_backdrop(false);
        g.set_collection_items(ModelRc::new(VecModel::default()));
        g.set_collection_focused(0);
        g.set_collection_back_focused(false);
        g.set_app_loading_progress(0.0);
        g.set_app_content_loading(true);
        // show-collection is deferred until the async task has all data ready
    }

    let id2    = id.clone();
    let title2 = title.clone();
    let ww_ui  = ww.clone();
    let ww_err = ww.clone();
    rt.spawn(async move {
        // Fetch items + poster in parallel; backdrop only if the BoxSet has backdrop tags.
        let (items_res, poster_bytes, detail_res) = tokio::join!(
            client.get_boxset_items(&id2),
            fetch_poster_cached(&client, &id2),
            client.get_item_detail(&id2),
        );
        let backdrop_bytes = match &detail_res {
            Ok(d) if !d.backdrop_image_tags.is_empty() => fetch_backdrop_cached(&client, &id2).await,
            _ => None,
        };

        let items = match items_res {
            Ok(v) => v,
            Err(e) => {
                warn!("open_collection_screen get_boxset_items({}): {:#}", id2, e);
                let _ = slint::invoke_from_event_loop(move || {
                    if let Some(w) = ww_err.upgrade() {
                        let g = AppState::get(&w);
                        // Only clear loading if we still own this request (not superseded by a newer open).
                        if g.get_collection_id().as_str() == id2.as_str() {
                            g.set_app_content_loading(false);
                        }
                    }
                });
                crate::show_toast(ww_ui, "Couldn't load collection — check your server connection".into());
                return;
            }
        };

        // Fetch all item posters in parallel before showing the screen.
        let bufs = fetch_card_posters(&client, &items).await;

        let _ = slint::invoke_from_event_loop(move || {
            let Some(w) = ww_ui.upgrade() else { return };
            let g = AppState::get(&w);

            // Stale-request guard: abort if the user navigated to a different collection.
            if g.get_collection_id().as_str() != id2.as_str() { return; }

            // Collection poster
            if let Some(bytes) = poster_bytes {
                if let Some(spb) = decode_poster_buffer(&bytes) {
                    g.set_collection_poster(slint::Image::from_rgba8(spb));
                    g.set_collection_has_poster(true);
                }
            }

            // Backdrop
            if let Some(bytes) = backdrop_bytes {
                if let Some(spb) = decode_poster_buffer(&bytes) {
                    g.set_collection_backdrop(slint::Image::from_rgba8(spb));
                    g.set_collection_has_backdrop(true);
                }
            }

            let cards = items_to_cards(&items, bufs);
            g.set_collection_items(ModelRc::new(VecModel::from(cards)));
            g.set_collection_focused(0);
            g.set_collection_back_focused(false);
            g.set_collection_title(title2.as_str().into());
            g.set_app_content_loading(false);
            g.set_show_collection(true);
        });
    });
}

// ── handle_key ────────────────────────────────────────────────────────────────

pub(crate) fn handle_key(action: &crate::keys::Action, g: &AppState) -> bool {
    use crate::keys::Action;

    // ── Back button focused ────────────────────────────────────────────────────
    if g.get_collection_back_focused() {
        return match action {
            Action::Confirm | Action::Back => {
                g.set_show_collection(false);
                true
            }
            Action::Down => {
                g.set_collection_back_focused(false);
                true
            }
            Action::Up => false, // let focus_bar_on_up reach the mini-player
            _ => true,
        };
    }

    // ── Grid navigation ────────────────────────────────────────────────────────
    let f    = g.get_collection_focused();
    let cols = g.get_library_cols();
    let len  = g.get_collection_items().row_count() as i32;

    match action {
        Action::Back => {
            g.set_show_collection(false);
            true
        }
        Action::Up => {
            if f >= cols {
                g.set_collection_focused(f - cols);
            } else {
                g.set_collection_back_focused(true);
            }
            true
        }
        Action::Down => {
            if f + cols < len {
                g.set_collection_focused(f + cols);
            }
            true
        }
        Action::Left => {
            if f > 0 { g.set_collection_focused(f - 1); }
            true
        }
        Action::Right => {
            if f < len - 1 { g.set_collection_focused(f + 1); }
            true
        }
        Action::Confirm => {
            if f < len {
                let card = g.get_collection_items().row_data(f as usize).unwrap();
                g.invoke_open_detail(card.id, card.item_type);
            }
            true
        }
        Action::OpenContextMenu => {
            if f < len {
                let card = g.get_collection_items().row_data(f as usize).unwrap();
                g.invoke_open_context_menu(
                    card.id, card.has_played, card.is_favorite,
                    card.resume_pct, card.item_type, card.series_id,
                );
            }
            true
        }
        _ => false,
    }
}
