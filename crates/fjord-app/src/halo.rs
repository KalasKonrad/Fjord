// ── fjord-app · halo.rs ──────────────────────────────────────────────────────
//   wire               AppState.focus-halo(radius, glow-1, glow-2, glow-3) → the hollow glow image
//                      widgets.slint's FocusHalo draws around a focused row, field or menu entry
//   halo_image         cached per (radius, layers): white, alpha = the glow, a nine-slice image
//   halo_pixels        the pixels: up to three gaussian edge profiles (σ = blur / 2, like
//                      femtovg's drop shadow) by distance outside a rounded rectangle, combined
//                      like stacked shadows; nothing inside the shape, so it never covers the row
//   erfc               complementary error function (Abramowitz–Stegun 7.1.26)
// ─────────────────────────────────────────────────────────────────────────────
// Why an image: a drop shadow is the whole shape blurred, so under a see-through or transparent
// row it floods the row with the ring colour (FocusGlow avoids that with an opaque fill, which
// rows don't have). A nine-slice image keeps its corners and edges at their own size and only
// stretches the middle, so one small image fits a row of any size: the reach (the widest
// layer, rounded up) plus the corner radius on every side, 2px of straight edge in the middle.
use std::cell::RefCell;
use std::collections::HashMap;

use slint::{Global, Image, Rgba8Pixel, SharedPixelBuffer};

use crate::{AppState, MainWindow};

/// Enough for every radius × strength × thickness combination a session sees; cleared when full.
const CACHE_MAX: usize = 64;

thread_local! {
    static CACHE: RefCell<HashMap<(u32, [u32; 3]), Image>> = RefCell::new(HashMap::new());
}

pub(crate) fn wire(window: &MainWindow) {
    AppState::get(window).on_focus_halo(|radius, g1, g2, g3| halo_image(radius, [g1, g2, g3]));
}

/// Hundredths of a pixel — the cache key (lengths arrive as f32 logical px).
fn q(v: f32) -> u32 {
    (v.max(0.0) * 100.0).round() as u32
}

fn halo_image(radius: f32, layers: [f32; 3]) -> Image {
    let key = (q(radius), layers.map(q));
    CACHE.with(|c| {
        let mut c = c.borrow_mut();
        if let Some(img) = c.get(&key) {
            return img.clone();
        }
        if c.len() >= CACHE_MAX {
            c.clear();
        }
        let (buf, edge) = halo_pixels(radius, layers);
        let mut img = Image::from_rgba8(buf);
        img.set_nine_slice_edges(edge, edge, edge, edge);
        c.insert(key, img.clone());
        img
    })
}

/// The halo around a rounded rectangle of corner `radius`, as (pixels, nine-slice edge). The shape
/// starts `reach` px in from every side (reach = the widest layer, rounded up — FocusHalo places
/// the image at the same rounded reach); edge = reach + radius, with 2px of straight edge between.
fn halo_pixels(radius: f32, layers: [f32; 3]) -> (SharedPixelBuffer<Rgba8Pixel>, u16) {
    let reach = layers.iter().fold(0.0f32, |a, &b| a.max(b)).ceil().max(1.0) as u32;
    let r = radius.max(0.0).ceil() as u32;
    let edge = reach + r;
    let size = 2 * edge + 2;
    // The shape: from `reach` to `size - reach` on both axes, corner radius r.
    let half = (size - 2 * reach) as f32 / 2.0;
    let centre = size as f32 / 2.0;
    let mut buf = SharedPixelBuffer::<Rgba8Pixel>::new(size, size);
    let px = buf.make_mut_slice();
    for y in 0..size {
        for x in 0..size {
            // Signed distance from the pixel centre to the rounded rectangle (> 0 outside).
            let qx = ((x as f32 + 0.5) - centre).abs() - (half - r as f32);
            let qy = ((y as f32 + 0.5) - centre).abs() - (half - r as f32);
            let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
            let d = outside + qx.max(qy).min(0.0) - r as f32;
            // Antialiased edge: 0 inside, 1 a pixel out.
            let cover = (d + 0.5).clamp(0.0, 1.0);
            let mut clear = 1.0f32;
            for &b in &layers {
                if b > 0.0 {
                    let sigma = b / 2.0;
                    let i = 0.5 * erfc(d.max(0.0) / (sigma * std::f32::consts::SQRT_2));
                    clear *= 1.0 - i;
                }
            }
            let a = ((1.0 - clear) * cover * 255.0).round() as u8;
            px[(y * size + x) as usize] = Rgba8Pixel {
                r: 255,
                g: 255,
                b: 255,
                a,
            };
        }
    }
    (buf, edge as u16)
}

/// Complementary error function, |error| < 1.5e-7 (Abramowitz & Stegun 7.1.26).
fn erfc(x: f32) -> f32 {
    let z = x.abs() as f64;
    let t = 1.0 / (1.0 + 0.327_591_1 * z);
    let poly = t
        * (0.254_829_592
            + t * (-0.284_496_736
                + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let r = poly * (-z * z).exp();
    (if x >= 0.0 { r } else { 2.0 - r }) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erfc_matches_known_values() {
        for (x, want) in [
            (0.0, 1.0),
            (0.5, 0.479_500),
            (1.0, 0.157_299),
            (2.0, 0.004_678),
        ] {
            assert!((erfc(x) - want).abs() < 1e-5, "erfc({x}) = {}", erfc(x));
        }
        assert!((erfc(-1.0) - 1.842_701).abs() < 1e-5);
    }

    #[test]
    fn halo_is_hollow_and_fades_outward() {
        // Strong at a 3px ring around a 6px-radius row: reach 32, edge 38, 78×78 image.
        let (buf, edge) = halo_pixels(6.0, [6.0, 16.0, 32.0]);
        assert_eq!(edge, 38);
        assert_eq!((buf.width(), buf.height()), (78, 78));
        let alpha = |x: u32, y: u32| buf.as_slice()[(y * buf.width() + x) as usize].a;
        // The middle (inside the shape) is empty: the row itself is never covered.
        assert_eq!(alpha(39, 39), 0);
        assert_eq!(alpha(39, 33), 0);
        // Along the top edge: brightest right outside the shape, fading to almost nothing.
        let (near, mid, far) = (alpha(39, 31), alpha(39, 16), alpha(39, 0));
        assert!(near > 200, "near {near}");
        assert!(near > mid && mid > far, "{near} {mid} {far}");
        assert!(far < 10, "far {far}");
        // Symmetric.
        assert_eq!(alpha(39, 31), alpha(39, 78 - 1 - 31));
        assert_eq!(alpha(31, 39), alpha(39, 31));
    }

    #[test]
    fn one_layer_and_no_radius() {
        let (buf, edge) = halo_pixels(0.0, [17.0, 0.0, 0.0]);
        assert_eq!(edge, 17);
        assert_eq!(buf.width(), 36);
    }
}
