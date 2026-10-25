//! Small color helpers. Colors are f32 RGB in 0..1.

pub type Rgb = [f32; 3];

pub const BLACK: Rgb = [0.0, 0.0, 0.0];
pub const WHITE: Rgb = [1.0, 1.0, 1.0];

pub fn hsv(h: f32, s: f32, v: f32) -> Rgb {
    let h = h.rem_euclid(1.0) * 6.0;
    let i = h.floor();
    let f = h - i;
    let p = v * (1.0 - s);
    let q = v * (1.0 - s * f);
    let t = v * (1.0 - s * (1.0 - f));
    match i as i32 % 6 {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}

pub fn lerp(a: Rgb, b: Rgb, t: f32) -> Rgb {
    let t = t.clamp(0.0, 1.0);
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}

pub fn scale(a: Rgb, k: f32) -> Rgb {
    [a[0] * k, a[1] * k, a[2] * k]
}

pub fn add(a: Rgb, b: Rgb) -> Rgb {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

pub fn from_u8(c: [u8; 3]) -> Rgb {
    [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0]
}

pub fn to_u8(c: Rgb) -> [u8; 3] {
    let f = |x: f32| (x.clamp(0.0, 1.0) * 255.0 + 0.5) as u8;
    [f(c[0]), f(c[1]), f(c[2])]
}

pub fn parse_hex(s: &str) -> Option<[u8; 3]> {
    let s = s.trim().trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let v = u32::from_str_radix(s, 16).ok()?;
    Some([(v >> 16) as u8, (v >> 8) as u8, v as u8])
}

pub fn to_hex(c: [u8; 3]) -> String {
    format!("#{:02x}{:02x}{:02x}", c[0], c[1], c[2])
}

/// Interpolate across color stops. `cyclic` wraps the last stop back to the first.
pub fn gradient(stops: &[[u8; 3]], t: f32, cyclic: bool) -> Rgb {
    let n = stops.len();
    if n == 0 {
        return BLACK;
    }
    if n == 1 {
        return from_u8(stops[0]);
    }
    let (t, segs) = if cyclic {
        (t.rem_euclid(1.0), n)
    } else {
        (t.clamp(0.0, 1.0), n - 1)
    };
    let x = t * segs as f32;
    let i = (x.floor() as usize).min(segs - 1);
    let a = from_u8(stops[i]);
    let b = from_u8(stops[(i + 1) % n]);
    lerp(a, b, x - i as f32)
}

pub const PALETTE_NAMES: &[&str] = &[
    "Rainbow", "Neon", "Sunset", "Ocean", "Forest", "Lava", "Ice", "Vaporwave", "Candy",
];

const PALETTES: &[&[[u8; 3]]] = &[
    &[[255, 0, 200], [0, 240, 255], [60, 0, 255]],
    &[[255, 40, 0], [255, 140, 0], [255, 0, 110], [110, 0, 255]],
    &[[0, 10, 80], [0, 90, 255], [0, 220, 255], [0, 255, 170]],
    &[[0, 60, 10], [20, 200, 40], [170, 255, 0], [0, 140, 90]],
    &[[120, 0, 0], [255, 30, 0], [255, 140, 0], [255, 220, 40]],
    &[[255, 255, 255], [120, 230, 255], [0, 90, 255], [160, 220, 255]],
    &[[255, 60, 200], [150, 0, 255], [0, 200, 255]],
    &[[255, 120, 200], [255, 230, 120], [120, 255, 220], [190, 140, 255]],
];

pub fn palette(idx: usize, t: f32) -> Rgb {
    if idx == 0 || idx > PALETTES.len() {
        hsv(t, 1.0, 1.0)
    } else {
        gradient(PALETTES[idx - 1], t, true)
    }
}

/// Small fast random generator (xorshift).
pub struct Rng(u64);

impl Default for Rng {
    fn default() -> Self {
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9E3779B97F4A7C15);
        Rng(seed | 1)
    }
}

impl Rng {
    /// Same seed, same numbers (used so the window preview and the keyboard agree).
    pub fn seeded(seed: u64) -> Self {
        // splitmix64 so nearby seeds still give very different sequences
        let mut z = seed.wrapping_add(0x9E3779B97F4A7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58476D1CE4E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D049BB133111EB);
        Rng((z ^ (z >> 31)) | 1)
    }
    pub fn u(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn f(&mut self) -> f32 {
        (self.u() >> 40) as f32 / (1u64 << 24) as f32
    }
    /// Random index below n.
    pub fn pick(&mut self, n: usize) -> usize {
        ((self.f() * n as f32) as usize).min(n.saturating_sub(1))
    }
}
