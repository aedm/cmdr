//! Finder-tag color circles for the context menu's plain tag items (macOS).
//!
//! These bitmaps are the FALLBACK look. Once the menu tracks, `tag_row` folds the seven
//! items into Finder's single row of circles, drawn live; the bitmaps show only where that
//! doesn't happen.
//!
//! The "applied" state composites a check INTO the circle, so the fallback says the same
//! thing the row does without a gutter checkmark beside it. The images are pure CPU work
//! (no `MainThreadMarker` needed), encoded as PNG and cached once in a `LazyLock`: the 14
//! bitmaps (seven colors × {normal, checked}) are tiny and identical every right-click.
//! `context_menu_icons.rs` puts them on the items, like every other context-menu image.
//!
//! Colors are the row's light-mode ones (`tag_row::SWATCHES`, the `--color-tag-*` tokens),
//! with the row's darkened ring baked in so a pale fill (yellow) still reads on a light
//! menu. We render at 36 px square and show it at [`SIDE_POINTS`], so 2× keeps it crisp on
//! Retina.

use std::collections::HashMap;
use std::sync::LazyLock;

use super::tag_row::{SWATCHES, ring_rgb};

/// Rendered side length in pixels (2× [`SIDE_POINTS`]).
const SIZE: u32 = 36;

/// The side a circle is shown at, in points.
pub const SIDE_POINTS: f64 = 18.0;

/// Renders one tag circle into a fresh RGBA buffer. Pure float math, so the bytes are
/// deterministic (the unit test pins dimensions and normal≠checked).
fn render_circle(rgb: [u8; 3], checked: bool) -> Vec<u8> {
    let d = SIZE as f32;
    let center = d / 2.0;
    let outer = center - 1.0; // 1 px transparent margin so the disk isn't clipped
    let border = 2.0; // 1 px logical at 2×
    let inner = outer - border;

    let [fr, fg, fb] = rgb.map(f32::from);
    let [br, bg, bb] = ring_rgb(rgb).map(f32::from);

    let mut buf = vec![0u8; (SIZE * SIZE * 4) as usize];
    for y in 0..SIZE {
        for x in 0..SIZE {
            let dx = x as f32 + 0.5 - center;
            let dy = y as f32 + 0.5 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let disk_cov = (outer - dist + 0.5).clamp(0.0, 1.0);
            if disk_cov <= 0.0 {
                continue;
            }
            // Fraction of this pixel that's fill (1.0) vs border (0.0).
            let fill_cov = (inner - dist + 0.5).clamp(0.0, 1.0);
            let mut r = br + (fr - br) * fill_cov;
            let mut g = bg + (fg - bg) * fill_cov;
            let mut b = bb + (fb - bb) * fill_cov;

            if checked {
                // White check, anti-aliased, clipped to the disk.
                let check_cov = check_coverage(x as f32 + 0.5, y as f32 + 0.5);
                if check_cov > 0.0 {
                    r += (255.0 - r) * check_cov;
                    g += (255.0 - g) * check_cov;
                    b += (255.0 - b) * check_cov;
                }
            }

            let i = ((y * SIZE + x) * 4) as usize;
            buf[i] = r.round() as u8;
            buf[i + 1] = g.round() as u8;
            buf[i + 2] = b.round() as u8;
            buf[i + 3] = (disk_cov * 255.0).round() as u8;
        }
    }
    buf
}

/// Anti-aliased coverage of a centered checkmark at pixel center `(px, py)` (in 36 px
/// space). The check is two strokes: down-right to the low vertex, then up-right.
fn check_coverage(px: f32, py: f32) -> f32 {
    // Vertices tuned for a 36 px circle.
    const A: (f32, f32) = (11.0, 19.0);
    const B: (f32, f32) = (16.0, 24.5);
    const C: (f32, f32) = (26.0, 12.0);
    const HALF_WIDTH: f32 = 1.8;

    let d = dist_to_segment(px, py, A, B).min(dist_to_segment(px, py, B, C));
    (HALF_WIDTH - d + 0.5).clamp(0.0, 1.0)
}

/// Euclidean distance from point `(px, py)` to segment `a`–`b`.
fn dist_to_segment(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (ax, ay) = a;
    let (bx, by) = b;
    let abx = bx - ax;
    let aby = by - ay;
    let len_sq = abx * abx + aby * aby;
    let t = if len_sq <= f32::EPSILON {
        0.0
    } else {
        (((px - ax) * abx + (py - ay) * aby) / len_sq).clamp(0.0, 1.0)
    };
    let cx = ax + abx * t;
    let cy = ay + aby * t;
    let dx = px - cx;
    let dy = py - cy;
    (dx * dx + dy * dy).sqrt()
}

/// Cache of the 14 rendered bitmaps, keyed by `(color, checked)`. Built once.
static TAG_BITMAPS: LazyLock<HashMap<(u8, bool), Vec<u8>>> = LazyLock::new(|| {
    SWATCHES
        .iter()
        .flat_map(|swatch| [false, true].map(|checked| ((swatch.color, checked), render_circle(swatch.light, checked))))
        .collect()
});

/// The same 14 bitmaps as PNG, which is what `NSImage` reads. A circle that somehow fails
/// to encode is left out, which costs that item its circle and nothing else.
static TAG_PNGS: LazyLock<HashMap<(u8, bool), Vec<u8>>> = LazyLock::new(|| {
    TAG_BITMAPS
        .iter()
        .filter_map(|(&key, rgba)| Some((key, crate::icons::rgba_to_png(rgba, SIZE, SIZE)?)))
        .collect()
});

/// The tag circle as PNG bytes, or `None` for an out-of-range color. `checked` composites
/// the applied-state checkmark into the circle.
pub fn tag_circle_png(color: u8, checked: bool) -> Option<&'static [u8]> {
    TAG_PNGS.get(&(color, checked)).map(Vec::as_slice)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_all_seven_colors_in_both_states() {
        for color in 1u8..=7 {
            for checked in [false, true] {
                let rgba = TAG_BITMAPS.get(&(color, checked)).expect("bitmap present");
                assert_eq!(
                    rgba.len(),
                    (SIZE * SIZE * 4) as usize,
                    "color {color} checked={checked} has the wrong byte count"
                );
            }
        }
    }

    #[test]
    fn checked_variant_differs_from_normal() {
        for color in 1u8..=7 {
            let normal = TAG_BITMAPS.get(&(color, false)).unwrap();
            let checked = TAG_BITMAPS.get(&(color, true)).unwrap();
            assert_ne!(normal, checked, "checked variant must differ for color {color}");
        }
    }

    #[test]
    fn out_of_range_colors_have_no_image() {
        assert!(tag_circle_png(0, false).is_none());
        assert!(tag_circle_png(8, false).is_none());
    }

    /// Each PNG decodes back to the bitmap it was made from, so `NSImage` gets the circle.
    #[test]
    fn every_circle_encodes_as_a_png_of_its_bitmap() {
        for color in 1u8..=7 {
            for checked in [false, true] {
                let png = tag_circle_png(color, checked).expect("png present");
                let decoded = image::load_from_memory_with_format(png, image::ImageFormat::Png)
                    .expect("decodes")
                    .to_rgba8();
                assert_eq!(decoded.dimensions(), (SIZE, SIZE));
                assert_eq!(decoded.as_raw(), TAG_BITMAPS.get(&(color, checked)).unwrap());
            }
        }
    }

    #[test]
    fn center_pixel_is_opaque() {
        // The circle covers the center, so its alpha must be fully opaque.
        let rgba = TAG_BITMAPS.get(&(6, false)).unwrap();
        let mid = ((SIZE / 2) * SIZE + SIZE / 2) * 4;
        assert_eq!(rgba[(mid + 3) as usize], 255, "center must be opaque");
    }
}
