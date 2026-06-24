// ── fjord-app · person.rs ─────────────────────────────────────────────────────
//   open_person_screen  reset AppState person props, set app-content-loading=true,
//                       spawn async fetch (portrait + bio + filmography in parallel),
//                       emit app-loading-progress=0.5, then show person on completion
//   handle_key          keyboard dispatch for the person screen:
//                       !in-film-row: Down→filmography, Back/Enter→close
//                       in-film-row: Up→back, Left/Right navigate, Enter→open-detail, C→ctx-menu
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::Arc;

use slint::{Global, Model, ModelRc, VecModel};
use tracing::warn;

use crate::AppState;
use crate::detail::{fetch_card_posters, items_to_cards};
use crate::poster::{decode_poster_buffer, fetch_poster_cached};
use crate::{CardItem, MainWindow};

// ── open_person_screen ────────────────────────────────────────────────────────

pub(crate) fn open_person_screen(
    id:     String,
    name:   String,
    client: Arc<fjord_api::JellyfinClient>,
    ww:     slint::Weak<MainWindow>,
    rt:     tokio::runtime::Handle,
) {
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_person_id(id.as_str().into());
        g.set_person_name(name.as_str().into());
        g.set_person_bio("".into());
        g.set_person_has_portrait(false);
        g.set_person_filmography(ModelRc::new(VecModel::<CardItem>::default()));
        g.set_person_film_focused(0);
        g.set_person_in_film_row(false);
        g.set_app_content_loading(true);
        g.set_app_loading_progress(0.0);
    }

    let ww2 = ww.clone();

    rt.spawn(async move {
        let (detail_res, poster_bytes, film_res) = tokio::join!(
            client.get_item_detail(&id),
            fetch_poster_cached(&client, &id),
            client.get_person_filmography(&id),
        );

        let bio = detail_res.ok()
            .and_then(|d| d.overview)
            .unwrap_or_default();

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
                g.set_person_filmography(
                    ModelRc::new(VecModel::from(items_to_cards(&film_items, film_bufs)))
                );
            }
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
    match action {
        Action::Back => {
            g.set_person_in_film_row(false);
            g.invoke_close_person();
            true
        }
        Action::Down => {
            if !in_film && g.get_person_filmography().row_count() > 0 {
                g.set_person_in_film_row(true);
            }
            true
        }
        Action::Up => {
            if in_film { g.set_person_in_film_row(false); true }
            else { false }
        }
        Action::Left => {
            if in_film {
                let idx = g.get_person_film_focused();
                if idx > 0 { g.set_person_film_focused(idx - 1); }
                true
            } else { false }
        }
        Action::Right => {
            if in_film {
                let idx = g.get_person_film_focused();
                let max = g.get_person_filmography().row_count() as i32 - 1;
                if idx < max { g.set_person_film_focused(idx + 1); }
                true
            } else { false }
        }
        Action::Confirm => {
            if in_film {
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
            if in_film {
                let idx = g.get_person_film_focused() as usize;
                if let Some(card) = g.get_person_filmography().row_data(idx) {
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
