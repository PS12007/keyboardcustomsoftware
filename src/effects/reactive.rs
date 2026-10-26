//! More typing-reactive effects: echoes, bouncing balls, lightning, paint, fire and more.

use super::more::{neighbors, noise, seg_dist, tone};
use super::*;

/// Every key press repeats a few times after a short delay, each echo wider and fainter.
#[derive(Default)]
pub(super) struct Echo {
    hits: Vec<(usize, f32, f32)>, // key, time, hue
    now: f32,
    rng: Rng,
}
impl Effect for Echo {
    fn key(&mut self, i: usize, _t: f32) {
        self.hits.push((i, self.now, self.rng.f()));
        if self.hits.len() > 40 {
            self.hits.remove(0);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        let s = p.speed();
        let delay = 0.42 / s.max(0.2);
        let repeats = 2 + (p.f("feedback") * 5.0) as usize;
        let fb = 0.45 + p.f("feedback") * 0.4;
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        self.hits.retain(|h| c.t >= h.1 && c.t - h.1 < delay * (repeats as f32 + 1.0) + 0.7);
        let mut acc = vec![bg; out.len()];
        for &(i, t0, hue) in &self.hits {
            let k0 = &KEYS[i];
            for r in 0..=repeats {
                let age = c.t - (t0 + r as f32 * delay);
                if !(0.0..0.7).contains(&age) {
                    continue;
                }
                let a = (1.0 - age / 0.7).powi(2) * fb.powi(r as i32) * 1.7;
                let rad = 0.45 + r as f32 * 0.45;
                let cc = if rainbow { color::hsv(hue + r as f32 * 0.09, 1.0, 1.0) } else { col };
                for (o, k) in acc.iter_mut().zip(KEYS) {
                    let g = glow(dist(k, k0.cx(), k0.cy()), rad) * a;
                    if g > 0.004 {
                        *o = color::add(*o, color::scale(cc, g));
                    }
                }
            }
        }
        for (o, a) in out.iter_mut().zip(acc) {
            *o = tone(a);
        }
    }
}

/// Each key press launches a glowing ball that bounces around the keyboard.
#[derive(Default)]
pub(super) struct Bounce {
    balls: Vec<[f32; 6]>, // x, y, vx, vy, hue, born
    trail: Vec<Rgb>,
    now: f32,
    rng: Rng,
}
impl Effect for Bounce {
    fn key(&mut self, i: usize, _t: f32) {
        let k = &KEYS[i];
        let dir = if self.rng.f() < 0.5 { -1.0 } else { 1.0 };
        let vx = dir * (1.5 + self.rng.f() * 3.0);
        let vy = -7.0 - self.rng.f() * 4.0;
        self.balls.push([k.cx(), k.cy(), vx, vy, self.rng.f(), self.now]);
        if self.balls.len() > 16 {
            self.balls.remove(0);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        self.trail.resize(out.len(), BLACK);
        let s = p.speed();
        let dt = c.dt * s;
        let life = 7.0;
        let (floor, left, right) = (5.75, 0.5, layout::WIDTH - 0.5);
        for b in self.balls.iter_mut() {
            b[3] += 20.0 * dt;
            b[0] += b[2] * dt;
            b[1] += b[3] * dt;
            if b[1] > floor {
                b[1] = floor;
                b[3] = -b[3].abs() * 0.78;
            }
            if b[1] < 0.3 {
                b[1] = 0.3;
                b[3] = b[3].abs();
            }
            if b[0] < left || b[0] > right {
                b[0] = b[0].clamp(left, right);
                b[2] = -b[2];
            }
        }
        self.balls.retain(|b| c.t >= b[5] && c.t - b[5] < life);
        let decay = (-c.dt * (1.5 + 5.0 * (1.0 - p.f("trail")))).exp();
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        for (i, k) in KEYS.iter().enumerate().take(out.len()) {
            let mut t = color::scale(self.trail[i], decay);
            let mut head = 0.0f32;
            for b in &self.balls {
                let fade = (life - (c.t - b[5])).clamp(0.0, 1.0);
                let g = glow(dist(k, b[0], b[1]), 0.6) * fade;
                if g > 0.01 {
                    let cc = if rainbow { color::hsv(b[4], 1.0, 1.0) } else { col };
                    t = [t[0].max(cc[0] * g), t[1].max(cc[1] * g), t[2].max(cc[2] * g)];
                    head = head.max(glow(dist(k, b[0], b[1]), 0.35) * fade);
                }
            }
            self.trail[i] = t;
            out[i] = color::lerp(color::add(bg, t), WHITE, head * 0.6);
        }
    }
}

/// Lightning arcs from the key you press to the one you pressed before (or somewhere random).
#[derive(Default)]
pub(super) struct Lightning {
    bolts: Vec<(Vec<(f32, f32)>, f32)>, // path, start
    last: Option<(usize, f32)>,
    now: f32,
    rng: Rng,
}
impl Lightning {
    fn path(&mut self, a: (f32, f32), b: (f32, f32)) -> Vec<(f32, f32)> {
        let n = 9;
        let (dx, dy) = (b.0 - a.0, b.1 - a.1);
        let len = (dx * dx + dy * dy).sqrt().max(0.01);
        let (nx, ny) = (-dy / len, dx / len);
        let mut pts = vec![a];
        for j in 1..n {
            let t = j as f32 / n as f32;
            let off = (self.rng.f() - 0.5) * 1.3 * (1.0 - (2.0 * t - 1.0).abs()).sqrt();
            pts.push((a.0 + dx * t + nx * off, a.1 + dy * t + ny * off));
        }
        pts.push(b);
        pts
    }
}
impl Effect for Lightning {
    fn key(&mut self, i: usize, t: f32) {
        let k = &KEYS[i];
        let to = match self.last {
            Some((j, t0)) if j != i && t >= t0 && t - t0 < 3.0 => (KEYS[j].cx(), KEYS[j].cy()),
            _ => {
                let j = self.rng.pick(KEYS.len());
                (KEYS[j].cx(), KEYS[j].cy())
            }
        };
        let main = self.path((k.cx(), k.cy()), to);
        // a small side branch off the main bolt
        let m = main[3 + self.rng.pick(4)];
        let end = (m.0 + (self.rng.f() - 0.5) * 4.0, m.1 + (self.rng.f() - 0.5) * 3.0);
        let branch = self.path(m, end);
        self.bolts.push((main, self.now));
        self.bolts.push((branch, self.now + 0.02));
        self.last = Some((i, t));
        if self.bolts.len() > 24 {
            self.bolts.drain(..2);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        let s = p.speed();
        let life = 0.7 / s.max(0.3);
        self.bolts.retain(|b| c.t + 0.05 >= b.1 && c.t - b.1 < life);
        let (col, bg) = (p.color("color"), p.color("color2"));
        // the sky lights up for a split second on each strike
        let flash = self.bolts.iter().map(|b| (1.0 - (c.t - b.1) / 0.12).clamp(0.0, 1.0)).fold(0.0, f32::max);
        for (o, k) in out.iter_mut().zip(KEYS) {
            let mut a = 0.0f32;
            for (path, t0) in &self.bolts {
                let age = (c.t - t0).max(0.0);
                let flick = if ((age * 30.0) as i32) % 3 == 1 { 0.45 } else { 1.0 };
                let fade = (1.0 - age / life).max(0.0).powi(2) * flick;
                let d = path.windows(2).map(|w| seg_dist(k.cx(), k.cy(), w[0], w[1])).fold(f32::MAX, f32::min);
                a = a.max(glow(d, 0.42) * fade);
            }
            let sky = color::add(bg, color::scale(col, flash * 0.12));
            *o = color::lerp(color::add(sky, color::scale(col, a)), WHITE, (a - 0.6).max(0.0) * 1.5);
        }
    }
}

/// Each key you press sends a note bar floating up to the top, like a piano roll.
#[derive(Default)]
pub(super) struct PianoRoll {
    notes: Vec<(f32, f32, f32, f32)>, // x, width, start y, start time
    flash: Vec<f32>,
    now: f32,
}
impl Effect for PianoRoll {
    fn key(&mut self, i: usize, _t: f32) {
        ensure(&mut self.flash, KEYS.len());
        self.flash[i] = 1.0;
        let k = &KEYS[i];
        self.notes.push((k.x, k.w, k.cy(), self.now));
        if self.notes.len() > 60 {
            self.notes.remove(0);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.flash, out.len());
        self.now = c.t;
        let v = 4.5 * p.speed();
        let len = 0.6 + p.f("trail") * 2.4;
        self.notes.retain(|n| c.t >= n.3 && n.2 - (c.t - n.3) * v + len > -0.5);
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        let fl = (-c.dt * 7.0).exp();
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let mut acc = bg;
            for n in &self.notes {
                let overlap = (k.x + k.w).min(n.0 + n.1) - k.x.max(n.0);
                if overlap < 0.3 {
                    continue;
                }
                let head = n.2 - (c.t - n.3) * v;
                let behind = k.cy() - head; // 0 at the head, grows along the tail
                if !(-0.5..=len).contains(&behind) {
                    continue;
                }
                let a = if behind < 0.0 { 1.0 + behind * 2.0 } else { (1.0 - behind / len).powf(1.5) };
                let cc = if rainbow { color::hsv((n.0 + n.1 / 2.0) / layout::WIDTH * 0.85, 0.9, 1.0) } else { col };
                acc = color::add(acc, color::scale(cc, a.max(0.0)));
            }
            self.flash[i] *= fl;
            *o = color::lerp(tone(acc), WHITE, self.flash[i] * 0.8);
        }
    }
}

/// Paint splats where you type and drips down the keys, then slowly dries away.
#[derive(Default)]
pub(super) struct Splat {
    paint: Vec<Rgb>,
    drips: Vec<(f32, f32, f32, Rgb)>, // x, y, speed, color
    pending: Vec<usize>,
    count: usize,
    rng: Rng,
}
impl Effect for Splat {
    fn key(&mut self, i: usize, _t: f32) {
        self.pending.push(i);
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.paint.resize(out.len(), BLACK);
        let s = p.speed();
        let pal = p.choice("palette");
        for i in std::mem::take(&mut self.pending) {
            self.count += 1;
            let col = color::palette(pal, (self.count as f32 * 0.618).fract());
            let k0 = &KEYS[i];
            let size = 0.7 + self.rng.f() * 0.5;
            for (j, k) in KEYS.iter().enumerate().take(out.len()) {
                let a = glow(dist(k, k0.cx(), k0.cy()), size).min(1.0);
                self.paint[j] = color::lerp(self.paint[j], col, a);
            }
            for _ in 0..1 + self.rng.pick(3) {
                let x = k0.cx() + (self.rng.f() - 0.5) * k0.w.min(2.0);
                self.drips.push((x, k0.cy(), 0.8 + self.rng.f() * 1.6, col));
            }
        }
        for d in self.drips.iter_mut() {
            d.1 += d.2 * c.dt * s;
        }
        self.drips.retain(|d| d.1 < 6.6);
        let dry = (-c.dt * 0.07 * s).exp();
        let bg = p.color("color2");
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            for d in &self.drips {
                let a = glow(dist(k, d.0, d.1), 0.45) * c.dt * 7.0;
                if a > 0.0005 {
                    self.paint[i] = color::lerp(self.paint[i], d.3, a.min(1.0));
                }
            }
            self.paint[i] = color::scale(self.paint[i], dry);
            *o = color::add(bg, self.paint[i]);
        }
    }
}

/// Typing starts a fire that spreads from key to key, burns out to ash, then grows back.
#[derive(Default)]
pub(super) struct Wildfire {
    fuel: Vec<f32>,
    burn: Vec<f32>,
    nb: Vec<Vec<usize>>,
    last: f32,
    rng: Rng,
}
const FLAME: &[[u8; 3]] = &[[40, 0, 0], [200, 20, 0], [255, 110, 0], [255, 200, 40], [255, 250, 200]];
impl Effect for Wildfire {
    fn key(&mut self, i: usize, t: f32) {
        ensure(&mut self.burn, KEYS.len());
        self.burn[i] = 1.0;
        self.last = t;
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let n = out.len();
        if self.nb.len() != n {
            self.nb = neighbors();
            self.fuel = vec![1.0; n];
        }
        ensure(&mut self.burn, n);
        let s = p.speed();
        if (c.t - self.last > 4.0 || c.t < self.last) && self.rng.f() < c.dt * 0.25 {
            // a lightning strike now and then when nobody types
            let i = self.rng.pick(n);
            self.burn[i] = 1.0;
        }
        let spread = 1.5 + p.f("spread") * 6.0;
        let ignite: Vec<usize> = (0..n).filter(|&i| self.burn[i] > 0.35).flat_map(|i| self.nb[i].clone()).collect();
        for j in ignite {
            if self.burn[j] < 0.1 && self.fuel[j] > 0.5 && self.rng.f() < c.dt * spread * s * self.fuel[j] {
                self.burn[j] = 1.0;
            }
        }
        let cool = (-c.dt * 0.85 * s).exp();
        let grass = p.color("color");
        for i in 0..n {
            if self.burn[i] > 0.05 {
                self.fuel[i] = self.fuel[i].min(1.0 - self.burn[i]);
            } else {
                self.fuel[i] = (self.fuel[i] + c.dt * 0.09 * s).min(1.0);
            }
            self.burn[i] *= cool;
            let b = self.burn[i];
            let flicker = 0.85 + 0.15 * (c.t * 23.0 + i as f32 * 1.7).sin();
            let fire = color::scale(color::gradient(FLAME, b, false), (b * 1.6).min(1.0) * flicker);
            let ground = color::scale(grass, 0.05 + 0.25 * self.fuel[i]);
            let base = color::lerp([0.05, 0.02, 0.01], ground, self.fuel[i]);
            out[i] = color::add(base, fire);
        }
    }
}

/// A field of drifting sparks; every key press blasts them away like a force field.
#[derive(Default)]
pub(super) struct ForceField {
    parts: Vec<[f32; 4]>, // x, y, vx, vy
    pushes: Vec<(f32, f32, f32, bool)>, // x, y, time, already applied
    now: f32,
    rng: Rng,
}
impl Effect for ForceField {
    fn key(&mut self, i: usize, _t: f32) {
        self.pushes.push((KEYS[i].cx(), KEYS[i].cy(), self.now, false));
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        let s = p.speed();
        let want = 12 + (p.f("density") * 30.0) as usize;
        while self.parts.len() < want {
            self.parts.push([self.rng.f() * layout::WIDTH, self.rng.f() * layout::HEIGHT, 0.0, 0.0]);
        }
        self.parts.truncate(want);
        for q in self.pushes.iter_mut().filter(|q| !q.3) {
            q.3 = true;
            for pt in self.parts.iter_mut() {
                let (dx, dy) = (pt[0] - q.0, pt[1] - q.1);
                let d = (dx * dx + dy * dy).sqrt().max(0.3);
                let f = 22.0 / (d * d + 1.0);
                pt[2] += dx / d * f;
                pt[3] += dy / d * f;
            }
        }
        self.pushes.retain(|q| c.t >= q.2 && c.t - q.2 < 1.0);
        let dt = c.dt * s;
        let damp = (-dt * 1.6).exp();
        for (j, pt) in self.parts.iter_mut().enumerate() {
            // gentle drift so the field never sits still
            let jf = j as f32;
            pt[2] += noise(c.t * 0.3 + jf * 5.1, jf, 71) * 2.0 * dt;
            pt[3] += noise(c.t * 0.3 + jf * 3.7, jf + 9.0, 72) * 2.0 * dt;
            pt[2] *= damp;
            pt[3] *= damp;
            pt[0] = (pt[0] + pt[2] * dt).rem_euclid(layout::WIDTH);
            pt[1] = (pt[1] + pt[3] * dt).rem_euclid(layout::HEIGHT);
        }
        let (cold, hot, bg) = (p.color("color"), p.color("color2"), p.color("color3"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let mut acc = bg;
            for pt in &self.parts {
                let g = glow(dist(k, pt[0], pt[1]), 0.55);
                if g > 0.01 {
                    let sp = ((pt[2] * pt[2] + pt[3] * pt[3]).sqrt() / 8.0).min(1.0);
                    acc = color::add(acc, color::scale(color::lerp(cold, hot, sp), g * (0.6 + 0.6 * sp)));
                }
            }
            // a faint ring shows the blast
            for q in &self.pushes {
                let age = c.t - q.2;
                let a = ring(dist(k, q.0, q.1), age * 14.0, 0.6) * (1.0 - age).max(0.0) * 0.35;
                acc = color::add(acc, color::scale(hot, a));
            }
            *o = tone(acc);
        }
    }
}

/// Each key press opens a little black hole: color spirals in and it collapses with a flash.
#[derive(Default)]
pub(super) struct BlackHole {
    holes: Vec<(f32, f32, f32, f32)>, // x, y, start, hue
    now: f32,
    rng: Rng,
}
impl Effect for BlackHole {
    fn key(&mut self, i: usize, _t: f32) {
        self.holes.push((KEYS[i].cx(), KEYS[i].cy(), self.now, self.rng.f()));
        if self.holes.len() > 10 {
            self.holes.remove(0);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        let s = p.speed();
        let life = 2.4 / s.max(0.3);
        self.holes.retain(|h| c.t >= h.2 && c.t - h.2 < life + 0.6);
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let mut acc = bg;
            let mut dark = 1.0f32;
            for h in &self.holes {
                let age = (c.t - h.2) / life;
                let (dx, dy) = (k.cx() - h.0, (k.cy() - h.1) * 1.3);
                let d = (dx * dx + dy * dy).sqrt();
                let cc = if rainbow { color::hsv(h.3 + d * 0.06, 0.9, 1.0) } else { col };
                if age < 1.0 {
                    let r = 3.2 * (1.0 - age).powf(0.7) + 0.4;
                    let swirl = 0.5 + 0.5 * (2.0 * dy.atan2(dx) + d * 2.2 - c.t * 9.0 * s).sin();
                    let disc = smoothstep(r, r * 0.35, d) * (0.25 + 0.75 * swirl) * (age * 4.0).min(1.0);
                    let horizon = r * 0.28;
                    acc = color::add(acc, color::scale(cc, disc + ring(d, horizon, 0.25) * 0.8));
                    if d < horizon {
                        dark = dark.min(d / horizon);
                    }
                } else {
                    // collapse: a bright ring flies out
                    let t = (age - 1.0) * life;
                    let a = ring(d, t * 14.0, 0.6) * (1.0 - t / 0.6).max(0.0);
                    acc = color::add(acc, color::scale(color::lerp(cc, WHITE, 0.6), a));
                }
            }
            *o = color::scale(tone(acc), dark);
        }
    }
}
