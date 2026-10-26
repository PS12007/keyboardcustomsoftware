//! Even more effects: reactive ones that respond to how you type, and smooth
//! ambient looks built on soft noise. Uses the helpers from effects.rs.

use super::*;
use std::collections::VecDeque;

// ---------------------------------------------------------------- helpers

fn lattice(x: i32, y: i32, s: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d) ^ (y as u32).wrapping_mul(0x1656_67b1) ^ s.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    (h & 0xffff) as f32 / 32767.5 - 1.0
}

/// Smooth value noise, -1..1.
pub(super) fn noise(x: f32, y: f32, s: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (ux, uy) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let (i, j) = (xi as i32, yi as i32);
    let top = lattice(i, j, s) + (lattice(i + 1, j, s) - lattice(i, j, s)) * ux;
    let bot = lattice(i, j + 1, s) + (lattice(i + 1, j + 1, s) - lattice(i, j + 1, s)) * ux;
    top + (bot - top) * uy
}

/// Layers of noise for natural-looking clouds, roughly -1..1.
fn fbm(x: f32, y: f32, s: u32) -> f32 {
    noise(x, y, s) * 0.55 + noise(x * 2.03 + 5.2, y * 2.03 - 1.3, s + 1) * 0.3 + noise(x * 4.1 - 3.7, y * 4.1 + 2.9, s + 2) * 0.15
}

/// Brightens toward 1 without clipping hard, so piled-up light stays colorful.
pub(super) fn tone(c: Rgb) -> Rgb {
    c.map(|v| 1.0 - (-v.max(0.0) * 1.8).exp())
}

/// Distance from a point to the segment a-b.
pub(super) fn seg_dist(px: f32, py: f32, a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 > 0.0 { (((px - a.0) * dx + (py - a.1) * dy) / len2).clamp(0.0, 1.0) } else { 0.0 };
    let (qx, qy) = (a.0 + dx * t - px, a.1 + dy * t - py);
    (qx * qx + qy * qy).sqrt()
}

/// Keys that touch each other (sides and corners).
pub(super) fn neighbors() -> Vec<Vec<usize>> {
    KEYS.iter()
        .enumerate()
        .map(|(i, a)| {
            KEYS.iter()
                .enumerate()
                .filter(|(j, b)| {
                    let dx = ((a.cx() - b.cx()).abs() - (a.w + b.w) / 2.0).max(0.0);
                    let dy = ((a.cy() - b.cy()).abs() - 1.0).max(0.0);
                    *j != i && dx < 0.3 && dy < 0.3
                })
                .map(|(j, _)| j)
                .collect()
        })
        .collect()
}

/// How fast someone is typing, smoothed: 0 = idle, 1 = about 9 keys a second.
#[derive(Default)]
struct Pace {
    times: VecDeque<f32>,
    level: f32,
}
impl Pace {
    fn press(&mut self, t: f32) {
        self.times.push_back(t);
        if self.times.len() > 64 {
            self.times.pop_front();
        }
    }
    fn update(&mut self, t: f32, dt: f32) -> f32 {
        while self.times.front().is_some_and(|&x| t - x > 2.0 || x > t) {
            self.times.pop_front();
        }
        let target = (self.times.len() as f32 / 2.0 / 9.0).min(1.0);
        let k = if target > self.level { 5.0 } else { 0.7 };
        self.level += (target - self.level) * (1.0 - (-dt * k).exp());
        self.level
    }
}

// ---------------------------------------------------------------- reactive

/// Each key you press drops ink that slowly spreads into the keys around it and mixes.
#[derive(Default)]
pub(super) struct Ink {
    dye: Vec<Rgb>,
    tmp: Vec<Rgb>,
    nb: Vec<Vec<usize>>,
    pending: Vec<usize>,
    drops: usize,
    last: f32,
    rng: Rng,
}
impl Effect for Ink {
    fn key(&mut self, i: usize, t: f32) {
        self.pending.push(i);
        self.last = t;
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let n = out.len();
        if self.nb.len() != n {
            self.nb = neighbors();
        }
        self.dye.resize(n, BLACK);
        self.tmp.resize(n, BLACK);
        let s = p.speed();
        if p.b("idle") && (c.t - self.last > 2.5 || c.t < self.last) && self.rng.f() < c.dt * 0.8 {
            self.pending.push(self.rng.pick(n));
        }
        let pal = p.choice("palette");
        for i in std::mem::take(&mut self.pending) {
            self.drops += 1;
            let col = color::palette(pal, (self.drops as f32 * 0.618).fract());
            self.dye[i] = color::add(self.dye[i], color::scale(col, 2.4));
            for &j in &self.nb[i] {
                self.dye[j] = color::add(self.dye[j], color::scale(col, 0.45));
            }
        }
        let spread = 1.0 - (-c.dt * 1.6 * s).exp();
        let decay = (-c.dt * 0.22 * s).exp();
        for i in 0..n {
            let nb = &self.nb[i];
            let mut avg = self.dye[i];
            if !nb.is_empty() {
                avg = BLACK;
                for &j in nb {
                    avg = color::add(avg, self.dye[j]);
                }
                avg = color::scale(avg, 1.0 / nb.len() as f32);
            }
            // spread slower than it fades out of the middle, so drops keep a soft core
            self.tmp[i] = color::scale(color::lerp(self.dye[i], avg, spread * 0.85), decay);
        }
        std::mem::swap(&mut self.dye, &mut self.tmp);
        let bg = p.color("color2");
        for (o, d) in out.iter_mut().zip(&self.dye) {
            *o = color::add(bg, tone(*d));
        }
    }
}

/// A bright flash and a shockwave ring that splits into a rainbow at its edge.
#[derive(Default)]
pub(super) struct Shockwave {
    waves: Vec<(f32, f32, f32)>, // x, y, start
    flash: Vec<f32>,
    now: f32,
}
impl Effect for Shockwave {
    fn key(&mut self, i: usize, _t: f32) {
        ensure(&mut self.flash, KEYS.len());
        self.flash[i] = 1.0;
        self.waves.push((KEYS[i].cx(), KEYS[i].cy(), self.now));
        if self.waves.len() > 20 {
            self.waves.remove(0);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.flash, out.len());
        self.now = c.t;
        let s = p.speed();
        let v = 11.0 * s;
        let (tint, bg, prism) = (p.color("color"), p.color("color2"), p.b("prism"));
        let w = 0.45 + p.f("width") * 1.2;
        self.waves.retain(|q| c.t >= q.2 && (c.t - q.2) * v < 20.0);
        let fl = (-c.dt * 9.0).exp();
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let mut acc = bg;
            for q in &self.waves {
                let r = (c.t - q.2) * v;
                let fade = (1.0 - r / 18.0).max(0.0).powf(1.4);
                let d = dist(k, q.0, q.1);
                let ringc: Rgb = if prism {
                    let sp = 0.35 + w * 0.3;
                    let rgb = [ring(d, r, w), ring(d, r - sp, w), ring(d, r - sp * 2.0, w)];
                    color::lerp(rgb, color::scale(tint, rgb[1]), 0.35)
                } else {
                    color::scale(tint, ring(d, r, w))
                };
                // a faint afterglow inside the ring
                let inner = if d < r { 0.12 * (1.0 - d / r.max(0.01)) } else { 0.0 };
                acc = color::add(acc, color::scale(color::add(ringc, color::scale(tint, inner)), fade));
            }
            self.flash[i] *= fl;
            *o = color::lerp(tone(acc), WHITE, self.flash[i]);
        }
    }
}

/// The keys you type become stars, joined by lines in the order you typed them.
#[derive(Default)]
pub(super) struct Constellation {
    stars: Vec<(usize, f32)>,
    now: f32,
}
impl Effect for Constellation {
    fn key(&mut self, i: usize, _t: f32) {
        self.stars.push((i, self.now));
        if self.stars.len() > 28 {
            self.stars.remove(0);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        let s = p.speed();
        let life = 8.0 / s.max(0.2);
        self.stars.retain(|q| c.t >= q.1 && c.t - q.1 < life);
        let (star, line, bg) = (p.color("color"), p.color("color2"), p.color("color3"));
        // faint background stars
        for (i, o) in out.iter_mut().enumerate() {
            let h = hash(i as f32 * 3.17 + 0.5);
            let tw = if h > 0.8 { (0.5 + 0.5 * (c.t * (0.6 + h) * 1.7 + h * 40.0).sin()).powi(6) * 0.08 } else { 0.0 };
            *o = color::add(bg, color::scale(star, tw));
        }
        for w in self.stars.windows(2) {
            let (a, b) = (&KEYS[w[0].0], &KEYS[w[1].0]);
            if w[0].0 == w[1].0 {
                continue;
            }
            let age = c.t - w[1].1;
            let fade = (1.0 - age / life).max(0.0).powf(1.3);
            // the line draws itself from the old star to the new one
            let grow = (age / 0.3).min(1.0);
            let (pa, pb) = ((a.cx(), a.cy()), (b.cx(), b.cy()));
            let tip = (pa.0 + (pb.0 - pa.0) * grow, pa.1 + (pb.1 - pa.1) * grow);
            for (o, k) in out.iter_mut().zip(KEYS) {
                let d = seg_dist(k.cx(), k.cy(), pa, tip);
                let mut a = glow(d, 0.38) * 0.55 * fade;
                if age < 0.6 {
                    a += glow(dist(k, tip.0, tip.1), 0.6) * (1.0 - age / 0.6);
                }
                if a > 0.005 {
                    *o = color::add(*o, color::scale(line, a));
                }
            }
        }
        for &(i, t0) in &self.stars {
            let age = c.t - t0;
            let fade = (1.0 - age / life).max(0.0);
            let tw = 0.75 + 0.25 * (c.t * 5.0 + i as f32).sin();
            let pop = (1.0 - age * 4.0).max(0.0);
            out[i] = color::add(out[i], color::add(color::scale(star, fade * tw), color::scale(WHITE, pop)));
        }
        for o in out.iter_mut() {
            *o = tone(*o);
        }
    }
}

/// Calm when you're idle; the faster you type, the hotter and faster everything gets.
#[derive(Default)]
pub(super) struct Overdrive {
    pace: Pace,
    flash: Vec<f32>,
    spark: Vec<f32>,
    phase: f32,
    rng: Rng,
}
impl Effect for Overdrive {
    fn key(&mut self, i: usize, t: f32) {
        ensure(&mut self.flash, KEYS.len());
        self.flash[i] = 1.0;
        self.pace.press(t);
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.flash, out.len());
        ensure(&mut self.spark, out.len());
        let s = p.speed();
        let lv = self.pace.update(c.t, c.dt);
        self.phase += c.dt * (0.25 + lv * 2.2) * s;
        let (calm, hot) = (p.color("color"), p.color("color2"));
        let meter = p.b("meter");
        let row0: Vec<usize> = (0..out.len()).filter(|&i| KEYS[i].row == 0).collect();
        let fl = (-c.dt * 6.0).exp();
        let sp = (-c.dt * 5.0).exp();
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let flow = 0.5 + 0.5 * (k.cx() * 0.55 - self.phase * 3.0 + (k.cy() * 0.9 + self.phase).sin() * 1.3).sin();
            let heat = (lv * 1.15 - 0.1 + (flow - 0.5) * 0.35).clamp(0.0, 1.0);
            let col = color::lerp(calm, hot, heat);
            let bright = 0.22 + 0.4 * lv + 0.38 * flow * (0.35 + 0.65 * lv);
            let mut cc = color::scale(col, bright);
            if meter && k.row == 0 {
                let pos = row0.iter().position(|&j| j == i).unwrap_or(0) as f32 / row0.len().max(1) as f32;
                let on = smoothstep(pos - 0.02, pos + 0.06, lv * 1.05);
                cc = color::lerp(color::scale(calm, 0.08), color::lerp(calm, hot, pos), on);
            }
            // flat out: sparks fly
            if lv > 0.75 && self.rng.f() < c.dt * (lv - 0.75) * 10.0 {
                self.spark[i] = 1.0;
            }
            self.spark[i] *= sp;
            self.flash[i] *= fl;
            cc = color::lerp(cc, WHITE, self.spark[i] * 0.8);
            *o = color::lerp(cc, color::lerp(hot, WHITE, 0.6), self.flash[i]);
        }
    }
}

/// A glowing light that flies to every key you press and leaves a trail behind it.
#[derive(Default)]
pub(super) struct Trail {
    pos: (f32, f32),
    vel: (f32, f32),
    target: (f32, f32),
    heat: Vec<f32>,
    hue: Vec<f32>,
    h: f32,
    last: f32,
    ready: bool,
}
impl Effect for Trail {
    fn key(&mut self, i: usize, t: f32) {
        self.target = (KEYS[i].cx(), KEYS[i].cy());
        self.last = t;
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.heat, out.len());
        ensure(&mut self.hue, out.len());
        let s = p.speed();
        if !self.ready {
            self.ready = true;
            self.pos = CENTER;
            self.target = CENTER;
        }
        if c.t - self.last > 2.5 || c.t < self.last {
            // nobody typing: wander around on its own
            let tt = c.t * s;
            self.target = (CENTER.0 + 6.5 * (tt * 0.55).sin(), CENTER.1 + 2.4 * (tt * 0.9 + 1.0).sin());
        }
        let w = 13.0 * s.sqrt();
        let steps = 4;
        let h = c.dt / steps as f32;
        for _ in 0..steps {
            let ax = w * w * (self.target.0 - self.pos.0) - 2.0 * w * self.vel.0;
            let ay = w * w * (self.target.1 - self.pos.1) - 2.0 * w * self.vel.1;
            self.vel.0 += ax * h;
            self.vel.1 += ay * h;
            self.pos.0 += self.vel.0 * h;
            self.pos.1 += self.vel.1 * h;
        }
        self.h += c.dt * 0.07 * s;
        let decay = (-c.dt * (0.6 + 2.6 * (1.0 - p.f("trail"))) * s.sqrt()).exp();
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let g = glow(dist(k, self.pos.0, self.pos.1), 1.0);
            self.heat[i] *= decay;
            if g > self.heat[i] {
                self.heat[i] = g;
                self.hue[i] = self.h;
            }
            let cc = if rainbow { color::hsv(self.hue[i], 1.0, 1.0) } else { col };
            let lit = color::scale(cc, self.heat[i]);
            *o = color::lerp(color::add(bg, lit), WHITE, glow(dist(k, self.pos.0, self.pos.1), 0.42) * 0.7);
        }
    }
}

/// Every key press sends a wave that flips the keys it passes to a new color.
#[derive(Default)]
pub(super) struct Flip {
    col: Vec<Rgb>,
    stamp: Vec<f32>,
    flash: Vec<f32>,
    waves: Vec<(f32, f32, f32, Rgb)>,
    pending: Vec<usize>,
    flips: usize,
    last: f32,
    auto: f32,
    now: f32,
    rng: Rng,
}
impl Effect for Flip {
    fn key(&mut self, i: usize, t: f32) {
        self.pending.push(i);
        self.last = t;
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let n = out.len();
        let pal = p.choice("palette");
        if self.col.len() != n {
            self.col = vec![color::scale(color::palette(pal, 0.0), 0.5); n];
            self.stamp = vec![f32::MIN; n];
            self.flash = vec![0.0; n];
        }
        if c.t < self.now {
            // clock went backwards (preview restarted)
            self.stamp.iter_mut().for_each(|s| *s = f32::MIN);
            self.waves.clear();
        }
        self.now = c.t;
        let s = p.speed();
        if (c.t - self.last > 3.0 || c.t < self.last) && c.t - self.auto > 2.2 / s.max(0.3) {
            self.auto = c.t;
            self.pending.push(self.rng.pick(n));
        }
        for i in std::mem::take(&mut self.pending) {
            self.flips += 1;
            let cc = color::palette(pal, (self.flips as f32 * 0.382).fract());
            self.waves.push((KEYS[i].cx(), KEYS[i].cy(), c.t, cc));
        }
        let v = 9.0 * s;
        self.waves.retain(|w| (c.t - w.2) * v < 24.0);
        for w in &self.waves {
            for (i, k) in KEYS.iter().enumerate().take(n) {
                let arrive = w.2 + ((k.cx() - w.0).abs() + (k.cy() - w.1).abs() * 1.4) / v;
                if c.t >= arrive && w.2 > self.stamp[i] {
                    self.stamp[i] = w.2;
                    self.col[i] = w.3;
                    self.flash[i] = 1.0;
                }
            }
        }
        let fl = (-c.dt * 8.0).exp();
        for i in 0..n {
            self.flash[i] *= fl;
            out[i] = color::lerp(color::scale(self.col[i], 0.85), WHITE, self.flash[i] * 0.75);
        }
    }
}

// ---------------------------------------------------------------- ambient

/// Slow folds of color, like silk moving in the wind, with a soft sheen.
#[derive(Default)]
pub(super) struct Silk;
impl Effect for Silk {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed() * 0.18;
        let sc = 0.12 + p.f("scale") * 0.22;
        let (a, b) = (p.color("color"), p.color("color2"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (x, y) = (k.cx() * sc, k.cy() * sc * 1.4);
            let wx = fbm(x + t, y - t * 0.6, 1);
            let wy = fbm(x - t * 0.7 + 3.1, y + t * 0.4, 4);
            let v = fbm(x + wx * 1.7, y + wy * 1.7 + t * 0.3, 7);
            let m = (v * 1.3 + 0.5).clamp(0.0, 1.0);
            let ridge = 1.0 - (fbm(x * 1.4 + wy, y * 1.4 + wx, 10) * 3.0).abs();
            let sheen = smoothstep(0.55, 1.0, ridge);
            let base = color::scale(color::lerp(a, b, m), 0.35 + 0.65 * m.max(1.0 - m));
            *o = color::lerp(base, WHITE, sheen * 0.32);
        }
    }
}

/// Glowing clouds of gas drifting through space, with stars peeking through.
#[derive(Default)]
pub(super) struct Nebula;
impl Effect for Nebula {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed() * 0.06;
        let pal = p.choice("palette");
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let (x, y) = (k.cx(), k.cy());
            let dens = fbm(x * 0.2 + t, y * 0.32 - t * 0.5, 31);
            let hue = fbm(x * 0.14 - t * 0.7, y * 0.2 + t, 34);
            let cloud = smoothstep(-0.38, 0.32, dens);
            let mut col = color::scale(color::palette(pal, hue * 0.9 + 0.5 + t * 0.2), cloud * (0.25 + 0.75 * cloud));
            let h = hash(i as f32 * 7.31 + 1.7);
            if h > 0.78 {
                let tw = (0.5 + 0.5 * (c.t * (1.0 + h * 2.0) + h * 60.0).sin()).powi(4);
                col = color::add(col, color::scale(WHITE, tw * 0.75 * (1.0 - cloud)));
            }
            *o = color::add(col, [0.004, 0.0, 0.012]);
        }
    }
}

/// Fireflies drift around in the dark and blink on and off.
#[derive(Default)]
pub(super) struct Fireflies;
impl Effect for Fireflies {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed();
        let n = 4 + (p.f("density") * 14.0) as usize;
        let (col, bg) = (p.color("color"), p.color("color2"));
        let mut flies = Vec::with_capacity(n);
        for j in 0..n {
            let jf = j as f32;
            let x = CENTER.0 + 9.5 * noise(t * 0.11 + jf * 13.7, jf * 3.1, 51);
            let y = CENTER.1 + 3.6 * noise(t * 0.13 + jf * 7.9, jf * 5.3 + 9.0, 52);
            let h = hash(jf + 0.37);
            let b = ((t * (0.5 + h * 0.6) + h * TAU).sin() * 0.7 + 0.3).max(0.0).powi(2);
            let tint = color::lerp(col, color::lerp(col, WHITE, 0.4), hash(jf * 2.1 + 4.0));
            flies.push((x, y, b, tint));
        }
        for (o, k) in out.iter_mut().zip(KEYS) {
            let mut acc = color::scale(bg, 0.5 + 0.5 * k.v());
            for f in &flies {
                let a = glow(dist(k, f.0, f.1), 0.95) * f.2 * 1.8;
                if a > 0.004 {
                    acc = color::add(acc, color::scale(f.3, a));
                }
            }
            *o = tone(acc);
        }
    }
}

/// Rippling light on the bottom of a swimming pool.
#[derive(Default)]
pub(super) struct Caustics;
impl Effect for Caustics {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed() * 0.7;
        let (light, water) = (p.color("color"), p.color("color2"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (x, y) = (k.cx() * 0.55, k.cy() * 0.9);
            let v1 = (x + t + 1.4 * (y * 0.8 - t * 0.7).sin()).sin();
            let v2 = (y * 1.2 - t * 0.9 + 1.4 * (x * 0.7 + t * 0.6).sin()).sin();
            let v3 = ((x + y) * 0.6 + t * 0.5).sin();
            let caust = (1.0 - ((v1 + v2 + v3 * 0.6) / 2.6).abs()).powi(5);
            let depth = 0.7 + 0.3 * (k.cx() * 0.3 + t * 0.2).sin();
            *o = color::lerp(color::scale(water, depth), light, caust);
        }
    }
}

/// An 80s sunset: a striped sun over a neon grid racing toward you.
#[derive(Default)]
pub(super) struct Synthwave;
const SKY: &[[u8; 3]] = &[[16, 0, 44], [90, 0, 110], [255, 30, 120]];
impl Effect for Synthwave {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed();
        let (sun, grid) = (p.color("color"), p.color("color2"));
        let horizon = 3.25;
        let sun_bottom = color::lerp(sun, [1.0, 0.1, 0.5], 0.85);
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (x, y) = (k.cx(), k.cy());
            *o = if y < horizon {
                let sky = color::scale(color::gradient(SKY, y / horizon, false), 0.6);
                let r = 2.9;
                let d = dist(k, CENTER.0, horizon);
                let inside = smoothstep(r + 0.5, r - 0.5, d);
                let up = ((horizon - y) / r).clamp(0.0, 1.0);
                // gaps in the lower half of the sun slide downward
                let gap = up < 0.6 && ((y * 0.8 - t * 0.35).rem_euclid(1.0) < 0.3);
                let disc = color::lerp(sun_bottom, sun, up);
                let a = if gap { inside * 0.15 } else { inside };
                color::lerp(sky, disc, a)
            } else {
                let z = 1.0 / (y - horizon + 0.2);
                let rows = ((z * 1.6 + t * 1.2).rem_euclid(1.0) - 0.5).abs();
                let hl = smoothstep(0.32, 0.5, rows);
                let xw = (x - CENTER.0) * z * 0.9;
                let cols = ((xw + 0.5).rem_euclid(1.0) - 0.5).abs();
                let vl = smoothstep(0.3, 0.5, 0.5 - cols);
                let haze = (1.0 - (y - horizon) / 1.6).max(0.0) * 0.35;
                let g = (hl.max(vl * 0.8) + haze).min(1.0);
                color::add([0.03, 0.0, 0.07], color::scale(grid, g))
            };
        }
    }
}

/// A row of pendulums of slightly different lengths drifting in and out of step.
#[derive(Default)]
pub(super) struct Pendulum;
impl Effect for Pendulum {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed() * 0.6;
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let w = TAU * (14.0 + k.cx()) / 36.0;
            let bob = CENTER.1 + 2.7 * (w * t).cos();
            let a = glow((k.cy() - bob).abs(), 0.6);
            let cc = if rainbow { color::hsv(k.u() * 0.85 + c.t * 0.02, 0.9, 1.0) } else { col };
            *o = color::add(bg, color::scale(cc, a));
        }
    }
}

/// Liquid metal: soft reflections roll across the keys with a bright glint now and then.
#[derive(Default)]
pub(super) struct Chrome;
impl Effect for Chrome {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed() * 0.15;
        let tint = p.color("color");
        let sweep = (t * 1.3).rem_euclid(2.2) * 14.0 - 6.0;
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (x, y) = (k.cx(), k.cy());
            let warp = fbm(x * 0.18 + t, y * 0.3 - t * 0.4, 61);
            let band = 0.5 + 0.5 * ((x * 0.35 + y * 0.8 + warp * 4.0 + t * 2.0).sin());
            let g = 0.05 + 0.6 * band.powi(3);
            let glint = glow((x + y * 0.7 - sweep).abs(), 0.9);
            *o = color::add(color::scale(tint, g), color::scale(WHITE, glint * 0.55));
        }
    }
}

// ---------------------------------------------------------------- audio

/// Curtains of light that rise and fall with each pitch of the music.
#[derive(Default)]
pub(super) struct AudioAurora {
    sm: [f32; 16],
}
impl Effect for AudioAurora {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let g = gain(p);
        for (s, b) in self.sm.iter_mut().zip(c.audio.bands) {
            let v = (b * g).min(1.0);
            let k = if v > *s { 12.0 } else { 3.0 };
            *s += (v - *s) * (1.0 - (-c.dt * k).exp());
        }
        let (a, b) = (p.color("color"), p.color("color2"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let f = k.u() * 15.0;
            let i = (f as usize).min(14);
            let e = self.sm[i] + (self.sm[i + 1] - self.sm[i]) * (f - i as f32);
            let up = 1.0 - k.v(); // 0 = bottom
            let shimmer = 0.75 + 0.25 * (c.t * 2.5 + k.cx() * 1.3).sin();
            let lit = smoothstep(e * 1.1 + 0.12, e * 1.1 - 0.2, up) * shimmer;
            let edge = glow(up - e * 1.1, 0.12) * e;
            let cc = color::lerp(a, b, (up * 1.3 + c.audio.level * 0.5).min(1.0));
            *o = color::add(color::scale(cc, lit * (0.35 + 0.65 * e)), color::scale(WHITE, edge * 0.5));
        }
    }
}

/// Rings fly out of the middle like a tunnel; the bass fires big bright ones.
#[derive(Default)]
pub(super) struct Tunnel {
    rings: Vec<(f32, f32, f32)>, // start, strength, hue
    avg: f32,
    cool: f32,
    next: f32,
}
impl Effect for Tunnel {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let g = gain(p);
        let bass = c.audio.bass * g;
        let level = (c.audio.level * g * 1.5).min(1.0);
        self.avg += (bass - self.avg) * (c.dt * 3.0).min(1.0);
        self.cool -= c.dt;
        if self.next > c.t + 5.0 {
            self.next = c.t;
        }
        if bass > 0.25 && bass > self.avg * 1.3 && self.cool <= 0.0 {
            self.cool = 0.16;
            self.rings.push((c.t, 1.0, c.t * 0.05));
        } else if c.t >= self.next {
            self.next = c.t + 0.45;
            self.rings.push((c.t, 0.15 + level * 0.6, c.t * 0.05 + 0.3));
        }
        let v = 7.0 + level * 6.0;
        self.rings.retain(|r| c.t >= r.0 && (c.t - r.0) * v < 12.0);
        let (col, rainbow) = (p.color("color"), p.b("rainbow"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let d = dist(k, CENTER.0, CENTER.1) * 1.15;
            let mut acc = color::scale(col, glow(d, 1.6) * level * 0.4);
            for r in &self.rings {
                let rad = (c.t - r.0) * v;
                let a = ring(d, rad, 0.6 + rad * 0.08) * r.1 * (1.0 - rad / 12.0).max(0.0);
                if a > 0.004 {
                    let cc = if rainbow { color::hsv(r.2 + rad * 0.02, 1.0, 1.0) } else { col };
                    acc = color::add(acc, color::scale(cc, a));
                }
            }
            *o = tone(acc);
        }
    }
}
