// ── fjord-app · detail.rs ────────────────────────────────────────────────────
//   open_detail  fetch item detail + cast + backdrop, populate AppState detail-*
// ─────────────────────────────────────────────────────────────────────────────
use std::sync::{Arc, Mutex};

use slint::{Global, ModelRc, VecModel};
use tracing::{debug, warn};

use crate::config::{FjordState, fmt_resume_label};
use crate::AppState;
use crate::poster::{fetch_poster_cached, fetch_backdrop_cached};
use crate::series::open_series_screen;
use crate::{CastMember, MainWindow};

pub(crate) fn open_detail(
    id:        String,
    state:     Arc<Mutex<FjordState>>,
    ww:        slint::Weak<MainWindow>,
    rt_handle: tokio::runtime::Handle,
) {
    let s = state.lock().unwrap();
    let Some(client) = s.client.as_ref().map(Arc::clone) else { return };

    if s.all_series.iter().any(|i| i.id == id) {
        let state3 = state.clone();
        let ww3    = ww.clone();
        let rth3   = rt_handle.clone();
        drop(s);
        open_series_screen(id, state3, ww3, rth3);
        return;
    }

    drop(s);

    let ww2 = ww.clone();
    if let Some(w) = ww.upgrade() {
        let g = AppState::get(&w);
        g.set_show_detail(true);
        g.set_detail_id(id.as_str().into());
        g.set_detail_loading(true);
        g.set_detail_has_backdrop(false);
        g.set_detail_cast(ModelRc::new(VecModel::<CastMember>::default()));
    }

    let id2 = id.clone();
    let ww3 = ww2.clone();
    rt_handle.spawn(async move {
        let detail = match client.get_item_detail(&id2).await {
            Ok(d)  => d,
            Err(e) => { warn!("get_item_detail {}: {:#}", id2, e); return; }
        };
        debug!("detail fetched: {} | genres={:?} | people={}", detail.name, detail.genres, detail.people.len());

        let poster_bytes = fetch_poster_cached(&client, &id2).await;

        let backdrop_bytes = if detail.backdrop_image_tags.is_empty() {
            None
        } else {
            fetch_backdrop_cached(&client, &id2).await
        };

        let cast: Vec<CastMember> = detail.people.iter()
            .filter(|p| p.person_type == "Actor")
            .take(12)
            .map(|p| CastMember { name: p.name.as_str().into(), role: p.role.as_str().into() })
            .collect();

        let mut meta_parts: Vec<String> = vec![];
        if let Some(y) = detail.production_year { meta_parts.push(y.to_string()); }
        if let Some(ref r) = detail.official_rating { meta_parts.push(r.clone()); }
        if let Some(ref rt_str) = detail.runtime_string() { meta_parts.push(rt_str.clone()); }
        let meta = meta_parts.join(" • ");

        let genres       = detail.genres.join(", ");
        let overview     = detail.overview.clone().unwrap_or_default();
        let rating_label = detail.community_rating
            .map(|r| format!("★ {:.1}", r))
            .unwrap_or_default();
        let series_label = if detail.item_type == "Episode" {
            let s = detail.parent_index_number.unwrap_or(0);
            let e = detail.index_number.unwrap_or(0);
            let series = detail.series_name.as_deref().unwrap_or("");
            format!("{} — S{:02}E{:02}", series, s, e)
        } else { String::new() };
        let resume_secs = detail.resume_position_secs().unwrap_or(0.0);

        slint::invoke_from_event_loop(move || {
            let Some(w) = ww3.upgrade() else { return };
            if AppState::get(&w).get_detail_id().as_str() != id2 { return; }

            let g = AppState::get(&w);
            g.set_detail_title(detail.name.as_str().into());
            g.set_detail_series_label(series_label.as_str().into());
            g.set_detail_meta(meta.as_str().into());
            g.set_detail_genres(genres.as_str().into());
            g.set_detail_overview(overview.as_str().into());
            g.set_detail_rating_label(rating_label.as_str().into());
            g.set_detail_can_resume(resume_secs > 0.0);
            g.set_detail_resume_label(fmt_resume_label(resume_secs).into());
            g.set_detail_cast(ModelRc::new(VecModel::from(cast)));
            g.set_detail_loading(false);

            if let Some(bytes) = poster_bytes {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let rgba = img.to_rgba8();
                    let (pw, ph) = rgba.dimensions();
                    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        rgba.as_raw(), pw, ph);
                    AppState::get(&w).set_detail_poster(slint::Image::from_rgba8(buf));
                    AppState::get(&w).set_detail_has_poster(true);
                }
            }

            if let Some(bytes) = backdrop_bytes {
                if let Ok(img) = image::load_from_memory(&bytes) {
                    let rgba = img.to_rgba8();
                    let (bw, bh) = rgba.dimensions();
                    let buf = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
                        rgba.as_raw(), bw, bh);
                    AppState::get(&w).set_detail_backdrop(slint::Image::from_rgba8(buf));
                    AppState::get(&w).set_detail_has_backdrop(true);
                }
            }
        }).ok();
    });
}
