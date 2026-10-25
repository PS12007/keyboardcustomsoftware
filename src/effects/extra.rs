//! More effects: random designs (Dream, Shuffle), extras and two more games.
//! Lives in its own file to keep effects.rs readable; it uses the helpers from there.

use super::*;

// ---------------------------------------------------------------- random looks

/// Seconds since 1970. Random designs are chosen from the wall clock, so the
/// settings window preview and the keyboard show the same design at the same time.
fn wall() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

fn mix64(a: u64, b: u64) -> u64 {
    Rng::seeded(a ^ b.wrapping_mul(0x9E37_79B9_7F4A_7C15)).u()
}

/// A fresh random seed (kept below 2^53 so it survives JSON round trips exactly).
pub fn new_seed() -> u64 {
    Rng::default().u() >> 11
}

/// Effects whose picture comes from a seed stored in their settings.
pub fn needs_seed(id: &str) -> bool {
    id == "dream" || id == "shuffle"
}

/// The seed in the settings, or this instance's own one.
fn seed_of(p: &Params, own: &mut u64) -> u64 {
    if *own == 0 {
        *own = new_seed() | 1;
    }
    p.map.get("seed").and_then(|v| v.as_u64()).unwrap_or(*own)
}

/// Which "epoch" of a repeating interval we're in, and seconds into it.
fn epoch(every: Option<f64>) -> (u64, f32) {
    match every {
        Some(iv) => {
            let now = wall();
            ((now / iv).floor() as u64, (now % iv) as f32)
        }
        None => (0, f32::MAX),
    }
}

fn is_background(label: &str) -> bool {
    label.contains("Background") || label.starts_with("Sky") || label.starts_with("Water") || label == "Other keys"
}

/// Random but good-looking settings for an effect: vivid colors that go together,
/// dark backgrounds, moderate speeds.
pub fn random_params(info: &EffectInfo, rng: &mut Rng) -> Map<String, Value> {
    let mut m = Map::new();
    let hue = rng.f();
    let step = [0.08, 0.5, 0.333, 0.42][rng.pick(4)];
    let mut n = 0.0;
    for d in info.params {
        let v: Value = match d.kind {
            Kind::Float(_) if d.key == "speed" => (0.3 + rng.f() * 0.35).into(),
            Kind::Float(_) => (0.2 + rng.f() * 0.6).into(),
            Kind::Bool(_) => (d.key != "realtime" && rng.f() < 0.5).into(),
            Kind::Choice(opts, _) => opts[rng.pick(opts.len())].into(),
            Kind::Color(_) => {
                let c = if is_background(d.label) {
                    color::hsv(hue + 0.5, 0.8, 0.05)
                } else {
                    n += 1.0;
                    color::hsv(hue + (n - 1.0) * step, 0.75 + rng.f() * 0.25, 1.0)
                };
                color::to_hex(color::to_u8(c)).into()
            }
        };
        m.insert(d.key.into(), v);
    }
    if needs_seed(info.id) {
        m.insert("seed".into(), (rng.u() >> 11).into());
    }
    m
}

/// Effects that look good on their own (no typing, music or screen needed).
fn candidates() -> impl Iterator<Item = &'static EffectInfo> {
    all().iter().filter(|e| {
        e.mode == Mode::Software
            && matches!(e.category, "Ambient" | "Games" | "Random")
            && !matches!(e.id, "shuffle" | "clock")
    })
}

/// A random effect with random settings ("Surprise me").
pub fn random_look(rng: &mut Rng) -> (&'static str, Map<String, Value>) {
    let list: Vec<_> = candidates().collect();
    // Dream makes a brand-new design every time, so give it extra chances
    let info = if rng.f() < 0.25 { find("dream") } else { list[rng.pick(list.len())] };
    (info.id, random_params(info, rng))
}

// ---------------------------------------------------------------- Dream

pub(super) const CHANGE_EVERY: &[&str] = &["Never", "Every 30 s", "Every 2 min", "Every 10 min"];

fn change_interval(choice: usize) -> Option<f64> {
    [None, Some(30.0), Some(120.0), Some(600.0)][choice.min(3)]
}

/// Triangle wave, -1..1.
fn tri(x: f32) -> f32 {
    2.0 * (2.0 * x.rem_euclid(1.0) - 1.0).abs() - 1.0
}

#[derive(Clone)]
struct Layer {
    kind: u8,
    ang: f32,
    freq: f32,
    w: f32,
    cx: f32,
    cy: f32,
    ox: f32,
    oy: f32,
    arms: f32,
}

impl Layer {
    fn new(r: &mut Rng) -> Self {
        Layer {
            kind: r.pick(7) as u8,
            ang: r.f() * TAU,
            freq: 0.18 + r.f() * 0.42,
            w: (0.4 + r.f() * 1.4) * if r.f() < 0.5 { -1.0 } else { 1.0 },
            cx: 2.0 + r.f() * 12.0,
            cy: 0.5 + r.f() * 5.0,
            ox: r.f() * TAU,
            oy: r.f() * TAU,
            arms: (1 + r.pick(4)) as f32,
        }
    }

    /// Pattern value at a point, -1..1.
    fn eval(&self, x: f32, y: f32, t: f32, cells: &[(f32, f32)]) -> f32 {
        let tt = t * self.w;
        match self.kind {
            // straight waves at an angle
            0 => (self.freq * (x * self.ang.cos() + y * self.ang.sin()) - tt).sin(),
            // rings around a wandering point
            1 => {
                let cx = self.cx + 2.5 * (t * 0.13 + self.ox).sin();
                let cy = self.cy + 1.5 * (t * 0.17 + self.oy).sin();
                let d = ((x - cx).powi(2) + (y - cy).powi(2)).sqrt();
                (self.freq * 1.3 * d - tt).sin()
            }
            // spiral arms
            2 => {
                let (dx, dy) = (x - CENTER.0, (y - CENTER.1) * 1.6);
                (self.arms * dy.atan2(dx) + self.freq * (dx * dx + dy * dy).sqrt() - tt).sin()
            }
            // plasma
            3 => {
                ((self.freq * x + tt).sin()
                    + (self.freq * 1.3 * y - tt * 0.7 + self.ox).sin()
                    + (self.freq * 0.7 * (x + y) + tt * 1.1 + self.oy).sin())
                    / 3.0
            }
            // cells: each key takes the color of the closest moving point
            4 => {
                let mut best = (f32::MAX, 0usize);
                for (i, c) in cells.iter().enumerate() {
                    let d = (x - c.0).powi(2) + (y - c.1).powi(2);
                    if d < best.0 {
                        best = (d, i);
                    }
                }
                ((best.1 as f32 * 0.618).fract() * 2.0 - 1.0) * 0.85 + 0.15 * (best.0.sqrt() * 1.5 - tt).sin()
            }
            // checkerboard interference
            5 => (self.freq * x + tt).sin() * (self.freq * 1.4 * y - tt * 0.8 + self.ox).sin(),
            // zigzag stripes
            _ => {
                let (c, s) = (self.ang.cos(), self.ang.sin());
                let z = x * c + y * s + 1.2 * (self.freq * 1.5 * (y * c - x * s) + self.ox).sin();
                tri(self.freq * z * 0.5 - tt * 0.3)
            }
        }
    }
}

/// One randomly made design: two patterns, a color scheme and a few extras.
#[derive(Clone)]
struct Design {
    a: Layer,
    b: Layer,
    /// how the second pattern is used: 0 = shifts colors, 1 = light/dark mask, 2 = multiplies
    b_mode: u8,
    b_amt: f32,
    stops: Vec<[u8; 3]>,
    span: f32,
    drift: f32,
    bands: f32,
    sparkle: f32,
    cells: Vec<[f32; 4]>,
    salt: f32,
}

fn random_stops(r: &mut Rng) -> Vec<[u8; 3]> {
    let h = r.f();
    let offs: &[f32] = match r.pick(5) {
        0 => &[0.0, 0.07, 0.14],
        1 => &[0.0, 0.5],
        2 => &[0.0, 0.333, 0.666],
        3 => &[0.0, 0.42, 0.58],
        _ => &[0.0, 0.25, 0.5, 0.75],
    };
    let sat = 0.7 + r.f() * 0.3;
    let mut s: Vec<[u8; 3]> = offs.iter().map(|o| color::to_u8(color::hsv(h + o, sat, 1.0))).collect();
    if r.f() < 0.45 {
        let at = r.pick(s.len() + 1);
        s.insert(at, color::to_u8(color::hsv(h + 0.5, 0.9, 0.05)));
    }
    if r.f() < 0.2 {
        s.push([255, 255, 255]);
    }
    s
}

impl Design {
    fn new(seed: u64) -> Self {
        let mut r = Rng::seeded(seed);
        let a = Layer::new(&mut r);
        let mut b = Layer::new(&mut r);
        if b.kind == a.kind {
            b.kind = (b.kind + 1 + r.pick(5) as u8) % 7;
        }
        Design {
            a,
            b,
            b_mode: r.pick(3) as u8,
            b_amt: 0.3 + r.f() * 0.5,
            stops: random_stops(&mut r),
            span: 0.5 + r.f() * 0.6,
            drift: (r.f() - 0.5) * 0.12,
            bands: if r.f() < 0.25 { (3 + r.pick(4)) as f32 } else { 0.0 },
            sparkle: if r.f() < 0.3 { 0.3 + r.f() * 0.7 } else { 0.0 },
            cells: (0..5).map(|_| [r.f() * 16.0, r.f() * 6.0, 0.2 + r.f() * 0.5, r.f() * TAU]).collect(),
            salt: r.f() * 100.0,
        }
    }

    fn render(&self, t: f32, out: &mut [Rgb]) {
        let cells: Vec<(f32, f32)> = self
            .cells
            .iter()
            .map(|c| (c[0] + 3.0 * (t * c[2] + c[3]).sin(), c[1] + 1.5 * (t * c[2] * 1.3 + c[3] * 2.0).cos()))
            .collect();
        let tick = (t * 3.0).floor();
        let flash = 1.0 - (t * 3.0).fract();
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let (x, y) = (k.cx(), k.cy());
            let va = self.a.eval(x, y, t, &cells);
            let vb = self.b.eval(x, y, t, &cells);
            let mut u = 0.5 + 0.5 * va;
            let mut bright = 1.0;
            match self.b_mode {
                0 => u += self.b_amt * 0.5 * vb,
                1 => bright = 0.3 + 0.7 * (0.5 + 0.5 * vb),
                _ => u = u * 0.5 + 0.5 * (0.5 + 0.5 * va * vb),
            }
            let mut pt = u * self.span + t * self.drift;
            if self.bands > 0.0 {
                pt = (pt * self.bands).floor() / self.bands;
            }
            let mut col = color::scale(color::gradient(&self.stops, pt, true), bright);
            if self.sparkle > 0.0 && hash(i as f32 * 7.31 + tick * 1.7 + self.salt) < self.sparkle * 0.05 {
                col = color::lerp(col, WHITE, flash);
            }
            *o = col;
        }
    }
}

/// Makes up a brand-new design from random patterns and colors, and can keep
/// changing into new ones on its own.
#[derive(Default)]
pub(super) struct Dream {
    own: u64,
    cur: Option<(u64, Design)>,
    old: Option<(u64, Design)>,
    scratch: Vec<Rgb>,
}

impl Effect for Dream {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let base = seed_of(p, &mut self.own);
        let (ep, into) = epoch(change_interval(p.choice("change")));
        let key = mix64(base, ep);
        if self.cur.as_ref().map(|d| d.0) != Some(key) {
            self.cur = Some((key, Design::new(key)));
        }
        let t = c.t * p.speed() * 0.6;
        if let Some((_, d)) = &self.cur {
            d.render(t, out);
        }
        // melt from the previous design into the new one
        const FADE: f32 = 3.0;
        if ep > 0 && into < FADE {
            let pk = mix64(base, ep - 1);
            if self.old.as_ref().map(|d| d.0) != Some(pk) {
                self.old = Some((pk, Design::new(pk)));
            }
            self.scratch.resize(out.len(), BLACK);
            if let Some((_, d)) = &self.old {
                d.render(t, &mut self.scratch);
            }
            let k = smoothstep(0.0, FADE, into);
            for (o, s) in out.iter_mut().zip(&self.scratch) {
                *o = color::lerp(*s, *o, k);
            }
        } else {
            self.old = None;
        }
    }
}

// ---------------------------------------------------------------- Shuffle

pub(super) const SHUFFLE_EVERY: &[&str] = &["Every 20 s", "Every minute", "Every 5 min", "Every 15 min"];

struct Inner {
    key: u64,
    info: &'static EffectInfo,
    fx: Box<dyn Effect>,
    map: Map<String, Value>,
}

impl Inner {
    fn new(key: u64) -> Self {
        let mut r = Rng::seeded(key);
        let (id, map) = random_look(&mut r);
        let info = find(id);
        Inner { key, info, fx: (info.make)(), map }
    }
    fn render(&mut self, c: &Ctx, out: &mut [Rgb]) {
        let p = Params { defs: self.info.params, map: &self.map };
        self.fx.render(c, &p, out);
    }
}

/// Plays a different random effect with random colors every so often.
#[derive(Default)]
pub(super) struct Shuffle {
    own: u64,
    cur: Option<Inner>,
    old: Option<Inner>,
    scratch: Vec<Rgb>,
}

impl Effect for Shuffle {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let base = seed_of(p, &mut self.own);
        let every = [20.0, 60.0, 300.0, 900.0][p.choice("every").min(3)];
        let (ep, into) = epoch(Some(every));
        let key = mix64(base, ep);
        if self.cur.as_ref().map(|i| i.key) != Some(key) {
            let had = self.cur.is_some();
            self.old = self.cur.take().filter(|_| had && into < 2.0);
            self.cur = Some(Inner::new(key));
        }
        if let Some(i) = self.cur.as_mut() {
            i.render(c, out);
        }
        const FADE: f32 = 2.0;
        match self.old.as_mut() {
            Some(old) if into < FADE => {
                self.scratch.resize(out.len(), BLACK);
                old.render(c, &mut self.scratch);
                let k = smoothstep(0.0, FADE, into);
                for (o, s) in out.iter_mut().zip(&self.scratch) {
                    *o = color::lerp(*s, *o, k);
                }
            }
            _ => self.old = None,
        }
    }
}

// ---------------------------------------------------------------- extras

/// Random keys pop with bright colors and fade.
#[derive(Default)]
pub(super) struct Confetti {
    lv: Vec<f32>,
    hue: Vec<f32>,
    rng: Rng,
}

impl Effect for Confetti {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let n = KEYS.len();
        ensure(&mut self.lv, n);
        ensure(&mut self.hue, n);
        let s = p.speed();
        let mut pops = (4.0 + p.f("density") * 40.0) * s * c.dt;
        while pops > 0.0 {
            if self.rng.f() < pops.min(1.0) {
                let i = self.rng.pick(n);
                self.lv[i] = 1.0;
                self.hue[i] = self.rng.f();
            }
            pops -= 1.0;
        }
        let decay = (-c.dt * (0.5 + 1.0 * s)).exp();
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        for (i, o) in out.iter_mut().enumerate() {
            self.lv[i] *= decay;
            let tint = if rainbow { color::hsv(self.hue[i], 0.85, 1.0) } else { col };
            *o = color::lerp(bg, tint, self.lv[i]);
        }
    }
}

/// Stars stream out from the middle, like jumping to hyperspace.
#[derive(Default)]
pub(super) struct Warp {
    stars: Vec<[f32; 4]>, // angle, distance, speed, hue
    rng: Rng,
}

impl Effect for Warp {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let s = p.speed();
        let want = 6 + (p.f("density") * 24.0) as usize;
        while self.stars.len() < want {
            self.stars.push([self.rng.f() * TAU, self.rng.f() * 1.5, 0.6 + self.rng.f() * 0.8, self.rng.f()]);
        }
        for st in self.stars.iter_mut() {
            st[1] += c.dt * s * st[2] * (1.5 + st[1] * 1.2);
        }
        let rng = &mut self.rng;
        for st in self.stars.iter_mut().filter(|st| st[1] > 11.0) {
            *st = [rng.f() * TAU, rng.f() * 0.5, 0.6 + rng.f() * 0.8, rng.f()];
        }
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        let tail = 0.4 + p.f("trail") * 0.5;
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (dx, dy) = (k.cx() - CENTER.0, (k.cy() - CENTER.1) * 1.5);
            let mut acc = bg;
            for st in &self.stars {
                let (ca, sa) = (st[0].cos(), st[0].sin());
                let along = dx * ca + dy * sa;
                let perp = (-dx * sa + dy * ca).abs();
                let start = st[1] * (1.0 - tail);
                let d = if along > st[1] {
                    ((along - st[1]).powi(2) + perp * perp).sqrt()
                } else if along < start {
                    ((start - along).powi(2) + perp * perp).sqrt()
                } else {
                    perp
                };
                // brighter towards the head, and as the star gets closer
                let head = if along >= start && along <= st[1] { 0.35 + 0.65 * (along - start) / (st[1] - start).max(0.01) } else { 1.0 };
                let a = glow(d, 0.5) * head * (st[1] / 2.5).min(1.0);
                let tint = if rainbow { color::hsv(st[3], 0.6, 1.0) } else { col };
                acc = color::add(acc, color::scale(tint, a));
            }
            *o = acc;
        }
    }
}

/// A radar sweep turning around the middle, lighting up blips as it passes.
#[derive(Default)]
pub(super) struct Radar {
    blips: Vec<[f32; 4]>, // x, y, last delta, time hit
    rng: Rng,
}

impl Effect for Radar {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let angle = c.t * p.speed() * 1.6;
        let want = 2 + (p.f("density") * 6.0) as usize;
        while self.blips.len() < want {
            self.blips.push([1.0 + self.rng.f() * 14.0, 0.5 + self.rng.f() * 5.0, TAU, -10.0]);
        }
        self.blips.truncate(want);
        let delta = |x: f32, y: f32| (angle - ((y - CENTER.1) * 1.6).atan2(x - CENTER.0)).rem_euclid(TAU);
        for b in self.blips.iter_mut() {
            let d = delta(b[0], b[1]);
            if d < b[2] {
                // the beam just passed: light it, and sometimes move it for next time
                b[3] = c.t;
                if self.rng.f() < 0.35 {
                    b[0] = 1.0 + self.rng.f() * 14.0;
                    b[1] = 0.5 + self.rng.f() * 5.0;
                }
            }
            b[2] = delta(b[0], b[1]);
        }
        let (beam, blip, bg) = (p.color("color"), p.color("color2"), p.color("color3"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let d = delta(k.cx(), k.cy());
            let r = dist(k, CENTER.0, CENTER.1);
            let sweep = (-d * 2.4).exp() * 0.85 + if d < 0.18 { 0.15 } else { 0.0 };
            let rings = ring(r, 3.0, 0.3) * 0.12 + ring(r, 6.2, 0.35) * 0.1;
            let mut col = color::lerp(bg, beam, (sweep + rings).min(1.0));
            for b in &self.blips {
                let age = c.t - b[3];
                let a = glow(dist(k, b[0], b[1]), 0.6) * (-age * 1.1).exp();
                col = color::lerp(col, blip, a.min(1.0));
            }
            *o = col;
        }
    }
}

/// Ripples from three moving points overlap into shifting patterns.
#[derive(Default)]
pub(super) struct Interference;

impl Effect for Interference {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed();
        let f = 0.45 + (1.0 - p.f("scale")) * 1.0;
        let pal = p.choice("palette");
        let src: Vec<(f32, f32)> = (0..3)
            .map(|i| {
                let i = i as f32;
                (CENTER.0 + 6.0 * (t * (0.21 + i * 0.07) + i * 2.1).sin(), CENTER.1 + 2.5 * (t * (0.17 + i * 0.05) + i * 1.3).cos())
            })
            .collect();
        for (o, k) in out.iter_mut().zip(KEYS) {
            let v: f32 = src.iter().map(|s| (dist(k, s.0, s.1) * f - t * 3.0).sin()).sum::<f32>() / 3.0;
            let u = 0.5 + 0.5 * v;
            *o = color::scale(color::palette(pal, u * 0.8 + t * 0.03), 0.35 + 0.65 * u);
        }
    }
}

/// Cars drive along the rows at night: white headlights in front, red taillights behind.
#[derive(Default)]
pub(super) struct Traffic {
    cars: Vec<[f32; 4]>, // row, front x, speed, police?
    rng: Rng,
}

impl Effect for Traffic {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let s = p.speed();
        const LEN: f32 = 1.7;
        let dir = |row: f32| if row < 2.5 { 1.0 } else { -1.0 };
        if self.rng.f() < (0.6 + p.f("density") * 3.0) * s * c.dt {
            let row = 1.0 + self.rng.pick(4) as f32;
            let d = dir(row);
            let x = if d > 0.0 { -0.5 } else { layout::WIDTH + 0.5 };
            let speed = 2.5 + self.rng.f() * 2.5;
            let free = self.cars.iter().all(|car| car[0] != row || (car[1] - x).abs() > LEN * 2.5);
            if free {
                let police = if self.rng.f() < 0.06 { 1.0 } else { 0.0 };
                self.cars.push([row, x, speed, police]);
            }
        }
        // cars don't drive into the one in front
        let snapshot = self.cars.clone();
        for car in self.cars.iter_mut() {
            let d = dir(car[0]);
            let gap = snapshot
                .iter()
                .filter(|o| o[0] == car[0] && (o[1] - car[1]) * d > 0.0)
                .map(|o| (o[1] - car[1]) * d)
                .fold(f32::MAX, f32::min);
            let v = if gap < LEN * 1.6 { car[2] * 0.4 } else { car[2] };
            car[1] += d * v * s * c.dt;
        }
        self.cars.retain(|car| car[1] > -3.0 && car[1] < layout::WIDTH + 3.0);

        let (head, tail, bg) = (p.color("color"), p.color("color2"), p.color("color3"));
        let lamp = [0.16, 0.09, 0.02];
        for (o, k) in out.iter_mut().zip(KEYS) {
            let mut col = if k.row == 0 { color::scale(lamp, 0.8 + 0.2 * (k.cx() * 0.9).sin()) } else { bg };
            for car in self.cars.iter().filter(|car| car[0] as u8 == k.row) {
                let d = dir(car[0]);
                let (front, back) = (car[1], car[1] - d * LEN);
                let mut hc = head;
                let mut tc = tail;
                if car[3] > 0.0 {
                    let on = (c.t * 6.0).fract() < 0.5;
                    hc = if on { [1.0, 0.0, 0.0] } else { [0.0, 0.2, 1.0] };
                    tc = if on { [0.0, 0.2, 1.0] } else { [1.0, 0.0, 0.0] };
                }
                col = color::add(col, color::scale(hc, glow(hdist(k, front), 0.45)));
                col = color::add(col, color::scale(tc, glow(hdist(k, back), 0.45) * 0.9));
            }
            *o = col;
        }
    }
}

/// A clock: F-keys show the hour, the number row the minutes, the top letter row the seconds.
#[derive(Default)]
pub(super) struct Clock {
    rows: Option<[Vec<usize>; 3]>,
}

impl Effect for Clock {
    fn render(&mut self, _: &Ctx, p: &Params, out: &mut [Rgb]) {
        let rows = self.rows.get_or_insert_with(|| {
            let ids = |list: &[&str]| list.iter().filter_map(|id| layout::index_of(id)).collect::<Vec<_>>();
            [
                ids(&["f1", "f2", "f3", "f4", "f5", "f6", "f7", "f8", "f9", "f10", "f11", "f12"]),
                ids(&["1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "minus", "equal"]),
                ids(&["q", "w", "e", "r", "t", "y", "u", "i", "o", "p", "lbracket", "rbracket"]),
            ]
        });
        let secs = crate::sys::local_day_fraction() * 86400.0;
        let hour = (secs / 3600.0).floor();
        let min = (secs % 3600.0) / 60.0;
        let sec = secs % 60.0;
        let (hc, mc, sc, bg) = (p.color("color"), p.color("color2"), p.color("color3"), p.color("color4"));
        fill(out, bg);
        // hours: F1..F12, current hour brightest (12 and 0 both light F12)
        let h12 = ((hour as usize + 11) % 12) + 1;
        for (j, &i) in rows[0].iter().enumerate() {
            if j + 1 < h12 {
                out[i] = color::lerp(bg, hc, 0.3);
            } else if j + 1 == h12 {
                out[i] = hc;
            }
        }
        // minutes: one key per 5 minutes, the last one fills up
        for (j, &i) in rows[1].iter().enumerate() {
            let fillv = (min / 5.0 - j as f32).clamp(0.0, 1.0);
            out[i] = color::lerp(bg, mc, 0.15 + 0.85 * fillv);
        }
        // seconds: a dot gliding along the row
        let pos = sec / 5.0;
        for (j, &i) in rows[2].iter().enumerate() {
            let d = (pos - j as f32 - 0.5).abs();
            out[i] = color::lerp(bg, sc, glow(d, 0.6));
        }
        // Esc shows morning/afternoon
        if let Some(i) = layout::index_of("esc") {
            out[i] = if hour >= 12.0 { color::lerp(bg, hc, 0.6) } else { color::lerp(bg, mc, 0.6) };
        }
    }
}

// ---------------------------------------------------------------- Tetris (sideways)

const TW: usize = GRID_W;
const TH: usize = GRID_H;
type Cells = [(i8, i8); 4];

const SHAPES: [Cells; 7] = [
    [(0, 0), (1, 0), (2, 0), (3, 0)],
    [(0, 0), (1, 0), (0, 1), (1, 1)],
    [(0, 0), (1, 0), (2, 0), (1, 1)],
    [(1, 0), (2, 0), (0, 1), (1, 1)],
    [(0, 0), (1, 0), (1, 1), (2, 1)],
    [(0, 0), (0, 1), (1, 1), (2, 1)],
    [(2, 0), (0, 1), (1, 1), (2, 1)],
];

const PIECE_COLORS: [Rgb; 7] = [
    [0.0, 0.9, 1.0],
    [1.0, 0.85, 0.0],
    [0.65, 0.0, 1.0],
    [0.0, 1.0, 0.2],
    [1.0, 0.05, 0.1],
    [0.1, 0.25, 1.0],
    [1.0, 0.45, 0.0],
];

fn rotate(c: &Cells) -> Cells {
    let mut r = c.map(|(x, y)| (y, -x));
    let (mx, my) = (r.iter().map(|p| p.0).min().unwrap_or(0), r.iter().map(|p| p.1).min().unwrap_or(0));
    for p in r.iter_mut() {
        *p = (p.0 - mx, p.1 - my);
    }
    r.sort();
    r
}

struct Piece {
    cells: Cells,
    kind: u8,
    x: i32,
    y: i32,
}

type Board = [[u8; TW]; TH];

fn fits(g: &Board, cells: &Cells, x: i32, y: i32) -> bool {
    cells.iter().all(|&(cx, cy)| {
        let (gx, gy) = (x + cx as i32, y + cy as i32);
        (0..TW as i32).contains(&gx) && (0..TH as i32).contains(&gy) && g[gy as usize][gx as usize] == 0
    })
}

fn full_columns(g: &Board) -> Vec<usize> {
    (0..TW).filter(|&x| (0..TH).all(|y| g[y][x] != 0)).collect()
}

/// How good a board is for the computer player (higher is better).
fn board_score(g: &Board, cleared: usize) -> f32 {
    let ext: Vec<f32> = (0..TH).map(|y| (0..TW).rev().find(|&x| g[y][x] != 0).map(|x| x as f32 + 1.0).unwrap_or(0.0)).collect();
    let holes: usize = (0..TH).map(|y| (0..ext[y] as usize).filter(|&x| g[y][x] == 0).count()).sum();
    let bump: f32 = ext.windows(2).map(|w| (w[0] - w[1]).abs()).sum();
    -ext.iter().sum::<f32>() - holes as f32 * 4.0 - bump * 0.5 + cleared as f32 * 8.0
}

/// Tetris turned sideways to fit the keyboard: pieces slide in from the right
/// and stack against the left edge; full columns clear. It plays itself.
#[derive(Default)]
pub(super) struct Tetris {
    grid: Board,
    piece: Option<Piece>,
    rng: Rng,
    next: f32,
    flash: Vec<(usize, f32)>,
    over: Option<f32>,
}

impl Tetris {
    fn spawn(&mut self) {
        let kind = self.rng.pick(7);
        let mut rots = vec![SHAPES[kind]];
        for _ in 0..3 {
            let r = rotate(rots.last().unwrap());
            if !rots.contains(&r) {
                rots.push(r);
            }
        }
        let mut best: Option<(f32, Cells, i32)> = None;
        for cells in &rots {
            let w = cells.iter().map(|p| p.0).max().unwrap_or(0) as i32 + 1;
            for y in 0..TH as i32 {
                let start = TW as i32 - w;
                if !fits(&self.grid, cells, start, y) {
                    continue;
                }
                let mut x = start;
                while fits(&self.grid, cells, x - 1, y) {
                    x -= 1;
                }
                let mut g = self.grid;
                for &(cx, cy) in cells {
                    g[(y + cy as i32) as usize][(x + cx as i32) as usize] = 1;
                }
                let score = board_score(&g, full_columns(&g).len()) + self.rng.f() * 0.3;
                if best.as_ref().is_none_or(|b| score > b.0) {
                    best = Some((score, *cells, y));
                }
            }
        }
        match best {
            Some((_, cells, y)) => {
                let w = cells.iter().map(|p| p.0).max().unwrap_or(0) as i32 + 1;
                self.piece = Some(Piece { cells, kind: kind as u8, x: TW as i32 - w, y });
            }
            None => self.piece = None,
        }
    }

    fn step(&mut self, t: f32) {
        if let Some(when) = self.over {
            if t - when > 1.2 {
                self.grid = [[0; TW]; TH];
                self.over = None;
            }
            return;
        }
        let Some(pc) = self.piece.as_mut() else {
            self.spawn();
            if self.piece.is_none() {
                self.over = Some(t);
            }
            return;
        };
        if fits(&self.grid, &pc.cells, pc.x - 1, pc.y) {
            pc.x -= 1;
            return;
        }
        // lock it in place and clear full columns
        for &(cx, cy) in &pc.cells {
            self.grid[(pc.y + cy as i32) as usize][(pc.x + cx as i32) as usize] = pc.kind + 1;
        }
        self.piece = None;
        let mut full = full_columns(&self.grid);
        full.sort_unstable_by(|a, b| b.cmp(a));
        for col in full {
            self.flash.push((col, t));
            for row in self.grid.iter_mut() {
                for x in col..TW - 1 {
                    row[x] = row[x + 1];
                }
                row[TW - 1] = 0;
            }
        }
    }
}

impl Effect for Tetris {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let interval = 0.12 / p.speed();
        if c.t >= self.next || c.t + 1.0 < self.next {
            self.next = c.t + interval;
            self.step(c.t);
        }
        self.flash.retain(|f| c.t - f.1 < 0.3);
        let bg = p.color("color3");
        let over = self.over.map(|w| c.t - w);
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (x, y) = cell_of(k);
            let mut col = match self.grid[y][x] {
                0 => bg,
                n => PIECE_COLORS[n as usize - 1],
            };
            if let Some(pc) = &self.piece {
                if pc.cells.iter().any(|&(cx, cy)| pc.x + cx as i32 == x as i32 && pc.y + cy as i32 == y as i32) {
                    col = PIECE_COLORS[pc.kind as usize];
                }
            }
            for f in self.flash.iter().filter(|f| f.0 == x) {
                col = color::lerp(col, WHITE, 1.0 - (c.t - f.1) / 0.3);
            }
            if let Some(age) = over {
                // game over: the stack blinks red and clears
                if self.grid[y][x] != 0 && (age * 5.0).fract() < 0.5 {
                    col = [1.0, 0.0, 0.0];
                }
            }
            *o = col;
        }
    }
}

// ---------------------------------------------------------------- Pac-Man

/// Pac-Man eats his way along every row with four ghosts behind him.
/// Power pellets in the corners turn the ghosts blue for a few seconds.
#[derive(Default)]
pub(super) struct PacMan {
    path: Vec<usize>,
    pellets: Vec<usize>,
    s: f32,
    power: f32,
}

impl Effect for PacMan {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        if self.path.is_empty() {
            for row in 0..6u8 {
                let mut keys: Vec<usize> = (0..KEYS.len()).filter(|&i| KEYS[i].row == row).collect();
                keys.sort_by(|a, b| KEYS[*a].x.total_cmp(&KEYS[*b].x));
                if row % 2 == 1 {
                    keys.reverse();
                }
                self.path.extend(keys);
            }
            let n = self.path.len();
            self.pellets = vec![0, n / 3, 2 * n / 3, n - 1];
            self.power = -10.0;
        }
        let n = self.path.len() as f32;
        let before = self.s;
        self.s += c.dt * p.speed() * 5.0;
        if self.s >= n {
            self.s -= n;
        }
        if self.pellets.iter().any(|&pi| (pi as f32) > before && (pi as f32) <= self.s) {
            self.power = c.t;
        }
        let (pac, bg) = (p.color("color"), p.color("color2"));
        let dot = [0.2, 0.2, 0.26];
        let ghosts: [Rgb; 4] = [[1.0, 0.0, 0.0], [1.0, 0.45, 0.75], [0.0, 0.9, 1.0], [1.0, 0.55, 0.1]];
        let scared = c.t - self.power < 5.0;
        let blink = c.t - self.power > 4.0 && (c.t * 6.0).fract() < 0.5;
        let here = self.s as usize;
        for (pos, &i) in self.path.iter().enumerate() {
            let mut col = if pos > here {
                if self.pellets.contains(&pos) {
                    color::scale([1.0, 0.8, 0.6], 0.5 + 0.5 * (c.t * 5.0).sin().abs())
                } else {
                    dot
                }
            } else {
                bg
            };
            for (g, gc) in ghosts.iter().enumerate() {
                let gp = (self.s - 4.0 - g as f32 * 2.5).rem_euclid(n) as usize;
                if gp == pos {
                    col = if scared { if blink { WHITE } else { [0.1, 0.2, 1.0] } } else { *gc };
                }
            }
            if pos == here {
                // chomp
                col = color::scale(pac, 0.55 + 0.45 * (c.t * 12.0).sin().abs());
            }
            out[i] = col;
        }
    }
}

