//! App logo, drawn in code: a white keycap on a black rounded square, with a thin
//! line of light under it. Self-contained so build.rs can reuse it for the .exe icon.

#![allow(dead_code)]

/// Signed distance to a rounded box (negative inside).
fn sd_box(x: f32, y: f32, cx: f32, cy: f32, hw: f32, hh: f32, r: f32) -> f32 {
    let qx = (x - cx).abs() - hw + r;
    let qy = (y - cy).abs() - hh + r;
    let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
    outside + qx.max(qy).min(0.0) - r
}

fn over(dst: [f32; 4], src: [f32; 3], a: f32) -> [f32; 4] {
    let a = a.clamp(0.0, 1.0);
    let out_a = a + dst[3] * (1.0 - a);
    if out_a <= 0.0 {
        return [0.0; 4];
    }
    let mix = |s: f32, d: f32| (s * a + d * dst[3] * (1.0 - a)) / out_a;
    [mix(src[0], dst[0]), mix(src[1], dst[1]), mix(src[2], dst[2]), out_a]
}

/// RGBA pixels, size x size. `background` = false draws just the white mark (for dark UIs).
pub fn draw(size: u32, background: bool) -> Vec<u8> {
    let s = size as f32;
    let px = 1.0 / s;
    // anti-aliased coverage from a signed distance
    let cov = |d: f32| (0.5 - d / px).clamp(0.0, 1.0);
    let stroke = (1.4 / s).max(0.055);
    let mut out = vec![0u8; (size * size * 4) as usize];
    for y in 0..size {
        for x in 0..size {
            let (u, v) = ((x as f32 + 0.5) / s, (y as f32 + 0.5) / s);
            let mut c = [0.0f32; 4];
            if background {
                let d = sd_box(u, v, 0.5, 0.5, 0.47, 0.47, 0.22);
                let shade = 0.075 - v * 0.06;
                c = over(c, [shade, shade, shade + 0.004], cov(d));
                // hairline edge
                c = over(c, [0.24, 0.24, 0.26], cov(d.abs() - px * 0.6) * 0.8);
            }
            let white = [0.96, 0.96, 0.97];
            // keycap outline
            let base = sd_box(u, v, 0.5, 0.45, 0.25, 0.215, 0.085);
            c = over(c, white, cov(base.abs() - stroke / 2.0));
            // keycap top face
            let top = sd_box(u, v, 0.5, 0.41, 0.135, 0.105, 0.05);
            c = over(c, white, cov(top));
            // light line under the key, fading toward the ends
            let lx = (u - 0.5) / 0.24;
            if lx.abs() < 1.0 {
                let line = (v - 0.80).abs() - stroke * 0.45;
                c = over(c, white, cov(line) * (1.0 - lx * lx).powf(0.7));
            }
            let i = ((y * size + x) * 4) as usize;
            for ch in 0..4 {
                out[i + ch] = (c[ch].clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
            }
        }
    }
    out
}

/// The full icon (black tile + mark).
pub fn rgba(size: u32) -> Vec<u8> {
    draw(size, true)
}

/// A Windows .ico file with several sizes (uncompressed 32-bit bitmaps).
pub fn ico() -> Vec<u8> {
    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes
        .iter()
        .map(|&n| {
            let px = rgba(n);
            let mut b = Vec::new();
            // BITMAPINFOHEADER (height doubled: color + mask)
            for v in [40u32, n, n * 2] {
                b.extend(v.to_le_bytes());
            }
            b.extend(1u16.to_le_bytes());
            b.extend(32u16.to_le_bytes());
            b.extend([0u8; 24]);
            // pixels bottom-up, BGRA
            for y in (0..n).rev() {
                for x in 0..n {
                    let i = ((y * n + x) * 4) as usize;
                    b.extend([px[i + 2], px[i + 1], px[i], px[i + 3]]);
                }
            }
            // AND mask, rows padded to 4 bytes (all zero: alpha does the work)
            let row = n.div_ceil(32) * 4;
            b.extend(vec![0u8; (row * n) as usize]);
            b
        })
        .collect();
    let mut f = Vec::new();
    f.extend([0u8, 0, 1, 0]);
    f.extend((sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (&n, img) in sizes.iter().zip(&images) {
        let d = if n >= 256 { 0 } else { n as u8 };
        f.extend([d, d, 0, 0]);
        f.extend(1u16.to_le_bytes());
        f.extend(32u16.to_le_bytes());
        f.extend((img.len() as u32).to_le_bytes());
        f.extend(offset.to_le_bytes());
        offset += img.len() as u32;
    }
    for img in images {
        f.extend(img);
    }
    f
}
