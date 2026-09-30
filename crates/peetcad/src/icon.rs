//! The window icon, drawn procedurally so the binary needs no image decoder.
//!
//! A bent sheet metal bracket on a rounded tile. `assets/icon.svg` is the same design.

use eframe::egui::IconData;

pub fn app_icon() -> IconData {
    const SIZE: usize = 64;
    let mut rgba = vec![0u8; SIZE * SIZE * 4];
    let tile = [38u8, 44, 56];
    let sheet = [120u8, 170, 240];
    let edge = [200u8, 222, 255];

    for y in 0..SIZE {
        for x in 0..SIZE {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let i = (y * SIZE + x) * 4;
            let tile_alpha = rounded_rect_coverage(fx, fy, 2.0, 62.0, 12.0);
            if tile_alpha <= 0.0 {
                continue;
            }
            let mut color = tile;
            // An "L" bracket: a horizontal base and a vertical flange joined by a bend.
            let inside = |x0: f32, x1: f32, y0: f32, y1: f32| {
                (x0..=x1).contains(&fx) && (y0..=y1).contains(&fy)
            };
            let base = inside(14.0, 50.0, 42.0, 48.0);
            let flange = inside(14.0, 20.0, 14.0, 44.0);
            let highlight = inside(14.0, 50.0, 42.0, 43.5) || inside(18.5, 20.0, 14.0, 42.0);
            if base || flange {
                color = if highlight { edge } else { sheet };
            }
            rgba[i..i + 3].copy_from_slice(&color);
            rgba[i + 3] = (tile_alpha * 255.0) as u8;
        }
    }
    IconData {
        rgba,
        width: SIZE as u32,
        height: SIZE as u32,
    }
}

/// Anti-aliased coverage of a rounded square spanning `min..max` on both axes.
fn rounded_rect_coverage(x: f32, y: f32, min: f32, max: f32, radius: f32) -> f32 {
    let cx = x.clamp(min + radius, max - radius);
    let cy = y.clamp(min + radius, max - radius);
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt() - radius;
    (0.5 - d).clamp(0.0, 1.0)
}
