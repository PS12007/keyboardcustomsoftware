//! App icon, drawn in code (a small rainbow keyboard).

use crate::color;

/// RGBA pixels, size x size.
pub fn rgba(size: u32) -> Vec<u8> {
    let s = size as f32;
    let mut px = vec![0u8; (size * size * 4) as usize];
    let radius = s * 0.18;
    for y in 0..size {
        for x in 0..size {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            // rounded square body
            let (m, top, bot) = (s * 0.04, s * 0.2, s * 0.8);
            let inside = rounded(fx, fy, m, top, s - m, bot, radius);
            if inside <= 0.0 {
                continue;
            }
            let i = ((y * size + x) * 4) as usize;
            // keys grid: 4 x 3
            let kx = (fx - m) / (s - 2.0 * m) * 4.0;
            let ky = (fy - top) / (bot - top) * 3.0;
            let gx = kx.fract();
            let gy = ky.fract();
            let on_key = gx > 0.14 && gx < 0.86 && gy > 0.16 && gy < 0.84;
            let c = if on_key {
                color::hsv(kx / 4.0 * 0.8 + ky * 0.05, 0.85, 1.0)
            } else {
                [0.09, 0.1, 0.13]
            };
            let a = inside.min(1.0);
            px[i] = (c[0] * 255.0) as u8;
            px[i + 1] = (c[1] * 255.0) as u8;
            px[i + 2] = (c[2] * 255.0) as u8;
            px[i + 3] = (a * 255.0) as u8;
        }
    }
    px
}

/// Coverage (0..1) of a rounded rectangle at a point, with 1px anti-aliasing.
fn rounded(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> f32 {
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
    (r - d + 0.5).clamp(0.0, 1.0)
}
