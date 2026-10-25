//! Lighting effects.
//!
//! Software effects compute a color for each of the 82 keys every frame.
//! Onboard effects are the keyboard's own built-in animations (no CPU use at all).

use crate::color::{self, Rgb, Rng, BLACK, WHITE};
use crate::layout::{self, CENTER, KEYS};
use serde_json::{Map, Value};
use std::f32::consts::TAU;

// ---------------------------------------------------------------- parameters

#[derive(Clone, Copy)]
pub enum Kind {
    Color([u8; 3]),
    /// 0..1 slider
    Float(f32),
    Choice(&'static [&'static str], usize),
    Bool(bool),
}

pub struct ParamDef {
    pub key: &'static str,
    pub label: &'static str,
    pub kind: Kind,
}

const fn p(key: &'static str, label: &'static str, kind: Kind) -> ParamDef {
    ParamDef { key, label, kind }
}

const SPEED: ParamDef = p("speed", "Speed", Kind::Float(0.5));

/// Read effect settings stored as JSON, falling back to the defaults.
pub struct Params<'a> {
    pub defs: &'static [ParamDef],
    pub map: &'a Map<String, Value>,
}

impl<'a> Params<'a> {
    fn def(&self, key: &str) -> Option<&'static ParamDef> {
        self.defs.iter().find(|d| d.key == key)
    }
    pub fn rgb8(&self, key: &str) -> [u8; 3] {
        if let Some(c) = self.map.get(key).and_then(|v| v.as_str()).and_then(color::parse_hex) {
            return c;
        }
        match self.def(key).map(|d| d.kind) {
            Some(Kind::Color(c)) => c,
            _ => [255, 255, 255],
        }
    }
    pub fn color(&self, key: &str) -> Rgb {
        color::from_u8(self.rgb8(key))
    }
    pub fn f(&self, key: &str) -> f32 {
        if let Some(v) = self.map.get(key).and_then(|v| v.as_f64()) {
            return (v as f32).clamp(0.0, 1.0);
        }
        match self.def(key).map(|d| d.kind) {
            Some(Kind::Float(v)) => v,
            _ => 0.5,
        }
    }
    pub fn choice(&self, key: &str) -> usize {
        let Some(def) = self.def(key) else { return 0 };
        let Kind::Choice(opts, dflt) = def.kind else { return 0 };
        self.map
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(|s| opts.iter().position(|o| *o == s))
            .unwrap_or(dflt)
    }
    pub fn b(&self, key: &str) -> bool {
        if let Some(v) = self.map.get(key).and_then(|v| v.as_bool()) {
            return v;
        }
        matches!(self.def(key).map(|d| d.kind), Some(Kind::Bool(true)))
    }
    /// speed slider (0..1) -> multiplier (0.15x .. 3x)
    pub fn speed(&self) -> f32 {
        0.15 * 20f32.powf(self.f("speed"))
    }
}

// ---------------------------------------------------------------- effect api

#[derive(Clone, Copy, Default)]
pub struct AudioFrame {
    pub bands: [f32; 16],
    pub level: f32,
    pub bass: f32,
}

pub struct Ctx<'a> {
    /// seconds
    pub t: f32,
    pub dt: f32,
    pub audio: &'a AudioFrame,
    /// per key (same order as KEYS), used by "Custom keys"
    pub keycolors: &'a [Option<[u8; 3]>],
    /// tiny copy of the screen (screen::GW x screen::GH), empty unless a screen effect is on
    pub screen: &'a [Rgb],
}

pub trait Effect: Send {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]);
    /// A key was pressed (index into KEYS).
    fn key(&mut self, _i: usize, _t: f32) {}
}

#[derive(Clone, Copy, PartialEq)]
pub enum Mode {
    Software,
    /// Runs on the keyboard's chip; value is the firmware effect ID.
    Onboard(u8),
}

pub struct EffectInfo {
    pub id: &'static str,
    pub name: &'static str,
    pub category: &'static str,
    pub desc: &'static str,
    pub params: &'static [ParamDef],
    pub mode: Mode,
    pub uses_keys: bool,
    pub uses_audio: bool,
    pub uses_screen: bool,
    /// true when the picture doesn't change over time -> stored on the keyboard, no streaming
    pub is_static: fn(&Params) -> bool,
    pub make: fn() -> Box<dyn Effect>,
    /// Onboard effects can't be previewed exactly; show this software effect instead.
    pub preview_as: Option<&'static str>,
}

fn mk<T: Effect + Default + 'static>() -> Box<dyn Effect> {
    Box::new(T::default())
}
fn never(_: &Params) -> bool {
    false
}
fn always(_: &Params) -> bool {
    true
}
fn gradient_static(p: &Params) -> bool {
    !p.b("move")
}

pub const CATEGORIES: &[&str] = &["Ambient", "Reactive", "Audio", "Games", "Screen", "Static", "On keyboard"];

pub fn all() -> &'static [EffectInfo] {
    EFFECTS
}

pub fn find(id: &str) -> &'static EffectInfo {
    EFFECTS.iter().find(|e| e.id == id).unwrap_or(&EFFECTS[0])
}

fn dist(k: &layout::Key, x: f32, y: f32) -> f32 {
    let dx = k.cx() - x;
    let dy = k.cy() - y;
    (dx * dx + dy * dy).sqrt()
}

fn fill(out: &mut [Rgb], c: Rgb) {
    out.iter_mut().for_each(|o| *o = c);
}

fn ensure(v: &mut Vec<f32>, n: usize) {
    if v.len() != n {
        *v = vec![0.0; n];
    }
}

// ---------------------------------------------------------------- static

#[derive(Default)]
struct Solid;
impl Effect for Solid {
    fn render(&mut self, _: &Ctx, p: &Params, out: &mut [Rgb]) {
        fill(out, p.color("color"));
    }
}

#[derive(Default)]
struct Gradient;
const DIRS4: &[&str] = &["Horizontal", "Vertical", "Diagonal", "From center"];
impl Effect for Gradient {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let (a, b) = (p.color("color"), p.color("color2"));
        let dir = p.choice("direction");
        for (o, k) in out.iter_mut().zip(KEYS) {
            let mut s = match dir {
                0 => k.u(),
                1 => k.v(),
                2 => (k.u() + k.v()) / 2.0,
                _ => (dist(k, CENTER.0, CENTER.1) / 8.5).min(1.0),
            };
            if p.b("move") {
                s = 0.5 + 0.5 * (TAU * (s - c.t * p.speed() * 0.25)).sin();
            }
            *o = color::lerp(a, b, s);
        }
    }
}

#[derive(Default)]
struct CustomKeys;
impl Effect for CustomKeys {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let bg = p.color("color");
        for (i, o) in out.iter_mut().enumerate() {
            *o = c.keycolors.get(i).copied().flatten().map(color::from_u8).unwrap_or(bg);
        }
    }
}

// ---------------------------------------------------------------- ambient

const WAVE_DIRS: &[&str] = &[
    "Left to right", "Right to left", "Top to bottom", "Bottom to top", "Diagonal", "Out from center", "Into center",
];

#[derive(Default)]
struct Rainbow;
impl Effect for Rainbow {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let scale = 0.25 + p.f("width") * 1.75;
        let sat = p.f("saturation");
        let shift = c.t * p.speed() * 0.4;
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (u, vy) = (k.u(), k.cy() / layout::WIDTH);
            let d = dist(k, CENTER.0, CENTER.1) / layout::WIDTH;
            let coord = match p.choice("direction") {
                0 => u,
                1 => -u,
                2 => vy,
                3 => -vy,
                4 => (u + vy) * 0.8,
                5 => d,
                _ => -d,
            };
            *o = color::hsv(coord * scale - shift, sat, 1.0);
        }
    }
}

#[derive(Default)]
struct Spectrum;
impl Effect for Spectrum {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        fill(out, color::hsv(c.t * p.speed() * 0.08, p.f("saturation"), 1.0));
    }
}

#[derive(Default)]
struct Breathing;
impl Effect for Breathing {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let phase = c.t * p.speed() * 0.35;
        let b = (0.5 - 0.5 * (TAU * phase).cos()).powf(1.6);
        let col = if p.b("rainbow") {
            color::hsv(phase.floor() * 0.17, 1.0, 1.0)
        } else {
            p.color("color")
        };
        fill(out, color::scale(col, 0.02 + 0.98 * b));
    }
}

#[derive(Default)]
struct Plasma;
impl Effect for Plasma {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed();
        let sc = 0.4 + p.f("scale") * 1.2;
        let pal = p.choice("palette");
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (x, y) = (k.cx() * sc * 0.45, k.cy() * sc * 0.45);
            let dx = x - 3.5 * sc;
            let dy = y - 1.4 * sc;
            let v = (x + t).sin()
                + (y * 1.3 - t * 1.2).sin()
                + ((x + y) * 0.7 + t * 0.7).sin()
                + ((dx * dx + dy * dy).sqrt() * 1.5 - t).sin();
            *o = color::palette(pal, v / 8.0 + 0.5 + t * 0.03);
        }
    }
}

#[derive(Default)]
struct Aurora;
impl Effect for Aurora {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed();
        let (a, b) = (p.color("color"), p.color("color2"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (u, v) = (k.u(), k.v());
            let w = (u * 6.0 + (t * 0.4 + v * 3.0).sin() * 2.0 + t * 0.5).sin();
            let intensity = (0.5 + 0.5 * w).powi(2) * (1.15 - v * 0.6);
            let mix = 0.5 + 0.5 * (u * 3.0 - t * 0.3 + v).sin();
            *o = color::add(color::scale(color::lerp(a, b, mix), intensity), [0.0, 0.01, 0.03]);
        }
    }
}

#[derive(Default)]
struct Fire {
    noise: Vec<f32>,
    rng: Rng,
}
const LAVA: &[[u8; 3]] = &[[0, 0, 0], [90, 0, 0], [255, 20, 0], [255, 120, 0], [255, 210, 40], [255, 255, 200]];
impl Effect for Fire {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.noise, out.len());
        let s = p.speed();
        let a = (-c.dt * 10.0 * s).exp();
        let height = 0.6 + p.f("height") * 0.9;
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            self.noise[i] = self.noise[i] * a + self.rng.f() * (1.0 - a);
            let base = ((k.row as f32 + 1.0) / 6.0).powf(1.6 / height);
            let wave = 0.5 + 0.5 * (k.cx() * 1.7 + c.t * s * 4.0 + k.row as f32).sin();
            let heat = base * (0.45 + 0.9 * self.noise[i]) * (0.75 + 0.25 * wave);
            *o = color::gradient(LAVA, heat, false);
        }
    }
}

#[derive(Default)]
struct Ocean;
const OCEAN: &[[u8; 3]] = &[[0, 5, 40], [0, 40, 140], [0, 120, 255], [0, 220, 255]];
impl Effect for Ocean {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed();
        for (o, k) in out.iter_mut().zip(KEYS) {
            let v = 0.5 + 0.3 * (k.cx() * 0.9 - t * 1.6).sin() + 0.2 * (k.cy() * 1.3 + k.cx() * 0.4 + t).sin();
            let mut col = color::gradient(OCEAN, v, false);
            if v > 0.9 {
                col = color::lerp(col, WHITE, (v - 0.9) * 6.0);
            }
            *o = col;
        }
    }
}

#[derive(Default)]
struct Twinkle {
    lvl: Vec<f32>,
    hue: Vec<f32>,
    rng: Rng,
}
impl Effect for Twinkle {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.lvl, out.len());
        ensure(&mut self.hue, out.len());
        let decay = (-c.dt * p.speed() * 2.5).exp();
        let rate = (0.05 + p.f("density") * 0.9) * p.speed();
        let (bg, star, rainbow) = (p.color("color2"), p.color("color"), p.b("rainbow"));
        for i in 0..out.len() {
            self.lvl[i] *= decay;
            if self.rng.f() < rate * c.dt {
                self.lvl[i] = 1.0;
                self.hue[i] = self.rng.f();
            }
            let sc = if rainbow { color::hsv(self.hue[i], 0.8, 1.0) } else { star };
            out[i] = color::lerp(bg, sc, self.lvl[i]);
        }
    }
}

#[derive(Default)]
struct Matrix {
    drops: Vec<(f32, f32)>,
    rng: Rng,
}
impl Effect for Matrix {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let cols = layout::WIDTH.ceil() as usize;
        if self.drops.len() != cols {
            self.drops = (0..cols).map(|_| (-self.rng.f() * 6.0, 0.6 + self.rng.f())).collect();
        }
        let trail = 1.5 + p.f("trail") * 4.0;
        for d in self.drops.iter_mut() {
            d.0 += c.dt * d.1 * p.speed() * 4.0;
            if d.0 > 6.0 + trail {
                d.0 = -self.rng.f() * 4.0;
                d.1 = 0.6 + self.rng.f();
            }
        }
        let col = p.color("color");
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (y, _) = self.drops[(k.cx() as usize).min(cols - 1)];
            let d = y - k.row as f32;
            *o = if (0.0..trail).contains(&d) {
                let b = (1.0 - d / trail).powi(2);
                let c2 = color::scale(col, b);
                if d < 0.7 { color::lerp(c2, WHITE, 0.6) } else { c2 }
            } else {
                color::scale(col, 0.03)
            };
        }
    }
}

#[derive(Default)]
struct Vortex;
impl Effect for Vortex {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let twist = p.f("twist") * 2.0;
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (dx, dy) = (k.cx() - CENTER.0, (k.cy() - CENTER.1) * 1.4);
            let ang = dy.atan2(dx) / TAU;
            let d = (dx * dx + dy * dy).sqrt() / 8.0;
            *o = color::hsv(ang + d * twist - c.t * p.speed() * 0.3, 1.0, 1.0);
        }
    }
}

#[derive(Default)]
struct Police;
impl Effect for Police {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let k = (c.t * p.speed() * 10.0) as i64 % 8;
        let (left_on, right_on) = (k == 0 || k == 2, k == 4 || k == 6);
        let (a, b) = (p.color("color"), p.color("color2"));
        for (o, key) in out.iter_mut().zip(KEYS) {
            *o = if key.u() < 0.5 {
                if left_on { a } else { color::scale(a, 0.04) }
            } else if right_on {
                b
            } else {
                color::scale(b, 0.04)
            };
        }
    }
}

fn smoothstep(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Rockets fly up from the bottom row and burst into rings of sparks.
#[derive(Default)]
struct Fireworks {
    rockets: Vec<(f32, f32, f32, f32)>, // x, y, burst height, hue
    bursts: Vec<(f32, f32, f32, f32)>,  // x, y, start time, hue
    rng: Rng,
}
impl Effect for Fireworks {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let s = p.speed();
        let rate = 0.3 + p.f("density") * 2.5;
        let (bg, col, rainbow) = (p.color("color2"), p.color("color"), p.b("rainbow"));
        if self.rng.f() < rate * c.dt && self.rockets.len() < 6 {
            let x = 1.0 + self.rng.f() * (layout::WIDTH - 2.0);
            let top = 0.5 + self.rng.f() * 2.2;
            self.rockets.push((x, layout::HEIGHT, top, self.rng.f()));
        }
        for r in self.rockets.iter_mut() {
            r.1 -= c.dt * s * 9.0;
        }
        let now = c.t;
        for r in self.rockets.iter().filter(|r| r.1 <= r.2) {
            self.bursts.push((r.0, r.2, now, r.3));
        }
        self.rockets.retain(|r| r.1 > r.2);
        let life = 1.4 / s.max(0.3);
        self.bursts.retain(|b| (0.0..life).contains(&(now - b.2)));

        fill(out, bg);
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let (kx, ky) = (k.cx(), k.cy());
            for r in &self.rockets {
                // bright head with a short tail below it
                let dx = (kx - r.0).abs();
                let below = ky - r.1;
                if dx < 0.7 && (-0.5..1.8).contains(&below) {
                    let a = if below < 0.5 { 1.0 } else { 0.5 * (1.0 - below / 1.8) };
                    *o = color::lerp(*o, [1.0, 0.85, 0.6], a);
                }
            }
            for b in &self.bursts {
                let age = (now - b.2) / life; // 0..1
                let d = dist(k, b.0, b.1);
                let r = age.sqrt() * 4.5;
                let spark = 0.65 + 0.35 * (i as f32 * 12.9898 + now * 30.0).sin();
                let a = (ring(d, r, 0.9) + if d < r { 0.25 } else { 0.0 }) * (1.0 - age).powi(2) * spark;
                if a > 0.01 {
                    let cc = if rainbow { color::hsv(b.3, 0.9, 1.0) } else { col };
                    // starts white-hot, then shows its color
                    let cc = color::lerp(WHITE, cc, (age * 5.0).min(1.0));
                    *o = color::lerp(*o, cc, a);
                }
            }
        }
    }
}

/// Soft blobs that drift around and melt into each other, like a lava lamp.
#[derive(Default)]
struct LavaLamp;
impl Effect for LavaLamp {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        const N: usize = 5;
        let t = c.t * p.speed() * 0.35;
        let r = 1.1 + p.f("size") * 1.6;
        let (bg, col, rainbow) = (p.color("color2"), p.color("color"), p.b("rainbow"));
        let blobs: [(f32, f32, Rgb); N] = std::array::from_fn(|i| {
            let f = i as f32;
            let x = CENTER.0 + 7.0 * (t * (0.31 + f * 0.07) + f * 1.7).sin();
            let y = CENTER.1 + 2.8 * (t * (0.53 + f * 0.11) + f * 2.3).sin();
            let cc = if rainbow { color::hsv(f / N as f32 + t * 0.05, 1.0, 1.0) } else { col };
            (x, y, cc)
        });
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (mut field, mut mix, mut wsum) = (0.0, [0.0; 3], 0.0);
            for (x, y, cc) in &blobs {
                let (dx, dy) = (k.cx() - x, (k.cy() - y) * 1.3);
                let w = r * r / (dx * dx + dy * dy + 0.05);
                field += w;
                mix = color::add(mix, color::scale(*cc, w));
                wsum += w;
            }
            let blob = color::scale(mix, 1.0 / wsum);
            let inside = smoothstep(0.7, 1.3, field);
            // a bit lighter in the middle of each blob
            let core = color::lerp(blob, WHITE, smoothstep(3.0, 9.0, field) * 0.35);
            *o = color::lerp(bg, core, inside);
        }
    }
}

/// Dark clouds drift by and lightning bolts crack down the keyboard.
#[derive(Default)]
struct Storm {
    bolts: Vec<([f32; 6], f32)>, // x per row, start time
    next: f32,
    rng: Rng,
}
impl Effect for Storm {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let (sky, bolt_col) = (p.color("color2"), p.color("color"));
        let t = c.t * p.speed();
        // wait a random time between strikes
        let gap = 0.4 + 5.0 * (1.0 - p.f("density"));
        if self.next <= 0.0 || self.next > c.t + 30.0 {
            self.next = c.t + self.rng.f() * gap;
        }
        if c.t >= self.next {
            self.next = c.t + gap * (0.3 + self.rng.f() * 1.4);
            let mut x = 1.5 + self.rng.f() * (layout::WIDTH - 3.0);
            let mut path = [0.0; 6];
            for px in path.iter_mut() {
                *px = x;
                x += (self.rng.f() - 0.5) * 2.2;
            }
            self.bolts.push((path, c.t));
        }
        self.bolts.retain(|b| (0.0..0.5).contains(&(c.t - b.1)));
        // a bolt flashes, goes dark for a moment, then flickers back and fades
        let flash = |age: f32| match age {
            a if a < 0.06 => 1.0,
            a if a < 0.11 => 0.1,
            a if a < 0.5 => 0.8 * (1.0 - (a - 0.11) / 0.39),
            _ => 0.0,
        };
        for (o, k) in out.iter_mut().zip(KEYS) {
            let cloud = 0.6 + 0.4 * (k.cx() * 0.5 - t * 0.7).sin() * (k.cx() * 0.23 + k.cy() * 0.6 + t * 0.4).cos();
            let mut col = color::scale(sky, cloud);
            for (path, t0) in &self.bolts {
                let f = flash(c.t - t0);
                let d = (k.cx() - path[(k.row as usize).min(5)]).abs();
                // the whole sky lights up a little, the bolt itself goes white-hot
                col = color::lerp(col, bolt_col, f * 0.35 * (-d / 4.0).exp());
                if d < 0.65 {
                    col = color::lerp(col, color::lerp(bolt_col, WHITE, 0.5), f);
                }
            }
            *o = col;
        }
    }
}

/// Comets streak along the rows, leaving glowing tails.
#[derive(Default)]
struct Comets {
    comets: Vec<(f32, u8, f32, f32)>, // x, row, velocity, hue
    rng: Rng,
}
impl Effect for Comets {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let s = p.speed();
        let rate = 0.4 + p.f("density") * 4.0;
        let tail = 2.0 + p.f("trail") * 6.0;
        let (bg, col, rainbow) = (p.color("color2"), p.color("color"), p.b("rainbow"));
        if self.rng.f() < rate * c.dt && self.comets.len() < 12 {
            let row = ((self.rng.f() * 6.0) as u8).min(5);
            let v = (6.0 + self.rng.f() * 8.0) * if self.rng.f() < 0.5 { 1.0 } else { -1.0 };
            let x = if v > 0.0 { -1.0 } else { layout::WIDTH + 1.0 };
            self.comets.push((x, row, v, self.rng.f()));
        }
        for m in self.comets.iter_mut() {
            m.0 += m.2 * c.dt * s;
        }
        self.comets.retain(|m| m.0 > -tail - 1.0 && m.0 < layout::WIDTH + tail + 1.0);
        fill(out, bg);
        for (o, k) in out.iter_mut().zip(KEYS) {
            for m in self.comets.iter().filter(|m| m.1 == k.row) {
                // distance behind the head (tail is on the side it came from)
                let behind = (m.0 - k.cx()) * m.2.signum();
                if (-0.6..tail).contains(&behind) {
                    let cc = if rainbow { color::hsv(m.3, 1.0, 1.0) } else { col };
                    let (cc, a) = if behind < 0.6 {
                        (color::lerp(cc, WHITE, 0.6), 1.0)
                    } else {
                        (cc, (1.0 - behind / tail).powi(2))
                    };
                    *o = color::lerp(*o, cc, a);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- reactive

struct Wave {
    x: f32,
    y: f32,
    row: u8,
    t0: f32,
    hue: f32,
}

fn ring(d: f32, r: f32, width: f32) -> f32 {
    let z = (d - r) / width;
    (-z * z).exp()
}

#[derive(Default)]
struct Ripple {
    waves: Vec<Wave>,
    rng: Rng,
    now: f32,
}
impl Effect for Ripple {
    fn key(&mut self, i: usize, _t: f32) {
        let k = &KEYS[i];
        let hue = self.rng.f();
        self.waves.push(Wave { x: k.cx(), y: k.cy(), row: k.row, t0: self.now, hue });
        if self.waves.len() > 24 {
            self.waves.remove(0);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        let (bg, col, rainbow) = (p.color("color2"), p.color("color"), p.b("rainbow"));
        let width = 0.5 + p.f("width") * 1.8;
        let s = p.speed();
        fill(out, bg);
        self.waves.retain(|w| (c.t - w.t0) * s * 10.0 < 20.0);
        for w in &self.waves {
            let r = (c.t - w.t0) * s * 10.0;
            let fade = (1.0 - r / 18.0).max(0.0);
            for (o, k) in out.iter_mut().zip(KEYS) {
                let d = dist(k, w.x, w.y);
                let a = ring(d, r, width) * fade;
                if a > 0.01 {
                    let cc = if rainbow { color::hsv(w.hue + d * 0.03, 1.0, 1.0) } else { col };
                    *o = color::lerp(*o, cc, a);
                }
            }
        }
    }
}

#[derive(Default)]
struct Reactive {
    lvl: Vec<f32>,
    hue: Vec<f32>,
    rng: Rng,
}
impl Effect for Reactive {
    fn key(&mut self, i: usize, _t: f32) {
        ensure(&mut self.lvl, KEYS.len());
        ensure(&mut self.hue, KEYS.len());
        self.lvl[i] = 1.0;
        self.hue[i] = self.rng.f();
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.lvl, out.len());
        ensure(&mut self.hue, out.len());
        let decay = (-c.dt * p.speed() * 2.5).exp();
        let (bg, col, rainbow) = (p.color("color2"), p.color("color"), p.b("rainbow"));
        for i in 0..out.len() {
            self.lvl[i] *= decay;
            let cc = if rainbow { color::hsv(self.hue[i], 1.0, 1.0) } else { col };
            out[i] = color::lerp(bg, cc, self.lvl[i]);
        }
    }
}

/// Only the background service saves the heatmap; the settings window just reads it.
static PERSIST: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_persist(on: bool) {
    PERSIST.store(on, std::sync::atomic::Ordering::Relaxed);
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct HeatFile {
    /// unix seconds when it was written
    saved: u64,
    counts: std::collections::BTreeMap<String, f32>,
}

fn heat_path() -> std::path::PathBuf {
    crate::config::dir().join("heatmap.json")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn heat_mtime() -> Option<std::time::SystemTime> {
    std::fs::metadata(heat_path()).and_then(|m| m.modified()).ok()
}

fn heat_write(counts: &[f32]) {
    let file = HeatFile {
        saved: unix_now(),
        counts: KEYS.iter().zip(counts).filter(|(_, n)| **n > 0.001).map(|(k, n)| (k.id.to_string(), *n)).collect(),
    };
    let _ = std::fs::create_dir_all(crate::config::dir());
    let tmp = heat_path().with_extension("tmp");
    if std::fs::write(&tmp, serde_json::to_string(&file).unwrap_or_default()).is_ok() {
        let _ = std::fs::rename(tmp, heat_path());
    }
}

/// Forget all typing counts (the "Reset" button).
pub fn heatmap_reset() {
    heat_write(&vec![0.0; KEYS.len()]);
}

const COOL: &[&str] = &["Never", "Over a week", "Over a day", "Over an hour", "Over a few minutes"];

/// Half-life in seconds for the cool-down setting (None = remember forever).
fn half_life(choice: usize) -> Option<f32> {
    match choice {
        1 => Some(2.0 * 86400.0),
        2 => Some(6.0 * 3600.0),
        3 => Some(15.0 * 60.0),
        4 => Some(60.0),
        _ => None,
    }
}

/// Counts every key press and keeps it, even across restarts (saved next to config.json).
#[derive(Default)]
struct Heatmap {
    count: Vec<f32>,
    loaded: bool,
    dirty: bool,
    last_io: f32,
    file_mtime: Option<std::time::SystemTime>,
}
const HEAT: &[[u8; 3]] = &[[0, 0, 40], [0, 60, 255], [0, 220, 200], [40, 255, 0], [255, 230, 0], [255, 0, 0]];
impl Heatmap {
    fn load(&mut self, hl: Option<f32>) {
        let file: HeatFile = std::fs::read_to_string(heat_path())
            .ok()
            .and_then(|s| serde_json::from_str(&s).ok())
            .unwrap_or_default();
        // cool down for the time the app wasn't running
        let away = unix_now().saturating_sub(file.saved) as f32;
        let k = hl.map(|h| 0.5f32.powf(away / h)).unwrap_or(1.0);
        self.count = KEYS.iter().map(|key| file.counts.get(key.id).copied().unwrap_or(0.0) * k).collect();
        self.file_mtime = heat_mtime();
    }
}
impl Effect for Heatmap {
    fn key(&mut self, i: usize, _t: f32) {
        ensure(&mut self.count, KEYS.len());
        self.count[i] += 1.0;
        self.dirty = true;
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let hl = half_life(p.choice("cool"));
        let persist = PERSIST.load(std::sync::atomic::Ordering::Relaxed);
        if !self.loaded {
            let pending = std::mem::take(&mut self.count);
            self.load(hl);
            for (n, extra) in self.count.iter_mut().zip(pending) {
                *n += extra;
            }
            self.loaded = true;
            self.last_io = c.t;
        }
        ensure(&mut self.count, out.len());
        // the service saves every 15 s; the settings window re-reads every 2 s
        let every = if persist { 15.0 } else { 2.0 };
        if c.t - self.last_io > every || c.t < self.last_io {
            self.last_io = c.t;
            let m = heat_mtime();
            if m.is_some() && m != self.file_mtime {
                // written by the other process (or reset)
                self.load(None);
                self.dirty = false;
            } else if persist && self.dirty {
                heat_write(&self.count);
                self.file_mtime = heat_mtime();
                self.dirty = false;
            }
        }
        if let Some(h) = hl {
            let decay = 0.5f32.powf(c.dt / h);
            self.count.iter_mut().for_each(|n| *n *= decay);
        }
        // log scale, so keys you use a little still show up next to the ones you use a lot
        let max = self.count.iter().cloned().fold(8.0, f32::max);
        let lmax = (1.0 + max).ln();
        for (o, n) in out.iter_mut().zip(&self.count) {
            let t = (1.0 + n).ln() / lmax;
            *o = color::gradient(HEAT, t, false);
        }
    }
}
impl Drop for Heatmap {
    fn drop(&mut self) {
        if self.dirty && self.loaded && PERSIST.load(std::sync::atomic::Ordering::Relaxed) {
            heat_write(&self.count);
        }
    }
}

#[derive(Default)]
struct Crosshair {
    waves: Vec<Wave>,
    rng: Rng,
    now: f32,
}
impl Effect for Crosshair {
    fn key(&mut self, i: usize, _t: f32) {
        let k = &KEYS[i];
        let hue = self.rng.f();
        self.waves.push(Wave { x: k.cx(), y: k.cy(), row: k.row, t0: self.now, hue });
        if self.waves.len() > 16 {
            self.waves.remove(0);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        let (bg, col, rainbow) = (p.color("color2"), p.color("color"), p.b("rainbow"));
        let s = p.speed();
        fill(out, bg);
        self.waves.retain(|w| (c.t - w.t0) * s * 12.0 < 18.0);
        for w in &self.waves {
            let r = (c.t - w.t0) * s * 12.0;
            let fade = (1.0 - r / 17.0).max(0.0);
            let cc = if rainbow { color::hsv(w.hue, 1.0, 1.0) } else { col };
            for (o, k) in out.iter_mut().zip(KEYS) {
                let mut a: f32 = 0.0;
                if k.row == w.row {
                    a = a.max(ring((k.cx() - w.x).abs(), r, 0.8));
                }
                if (k.cx() - w.x).abs() < 0.75 {
                    a = a.max(ring((k.cy() - w.y).abs(), r * 0.6, 0.6));
                }
                if r < 0.8 && dist(k, w.x, w.y) < 0.6 {
                    a = 1.0;
                }
                if a > 0.01 {
                    *o = color::lerp(*o, cc, a * fade);
                }
            }
        }
    }
}

// ---------------------------------------------------------------- audio

const BAR_STYLES: &[&str] = &["Green to red", "Rainbow", "Single color"];
const LEVEL: &[[u8; 3]] = &[[0, 255, 60], [180, 255, 0], [255, 200, 0], [255, 0, 0]];

fn gain(p: &Params) -> f32 {
    0.5 + p.f("sensitivity") * 2.5
}

#[derive(Default)]
struct AudioBars;
impl Effect for AudioBars {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let g = gain(p);
        let style = p.choice("style");
        let col = p.color("color");
        for (o, k) in out.iter_mut().zip(KEYS) {
            let band = ((k.u() * 16.0) as usize).min(15);
            let h = (c.audio.bands[band] * g).min(1.0);
            let from_bottom = 5.0 - k.row as f32; // 0 = bottom row
            let lit = (h * 6.0 - from_bottom).clamp(0.0, 1.0);
            let cc = match style {
                0 => color::gradient(LEVEL, from_bottom / 5.0, false),
                1 => color::hsv(k.u() * 0.85, 1.0, 1.0),
                _ => col,
            };
            // faint base glow only while something is playing
            let base = 0.03 * (c.audio.level * 20.0).min(1.0);
            *o = color::add(color::scale(cc, lit), color::scale(cc, base));
        }
    }
}

#[derive(Default)]
struct BassPulse;
impl Effect for BassPulse {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let b = (c.audio.bass * gain(p)).min(1.0);
        let col = if p.b("rainbow") {
            color::hsv(c.t * 0.05 + b * 0.15, 1.0, 1.0)
        } else {
            p.color("color")
        };
        for (o, k) in out.iter_mut().zip(KEYS) {
            let d = dist(k, CENTER.0, CENTER.1) / 9.0;
            let v = 0.04 + 0.96 * b.powf(1.5) * (1.0 - d * 0.5 * (1.0 - b));
            *o = color::scale(col, v);
        }
    }
}

#[derive(Default)]
struct AudioScroll {
    hist: Vec<f32>,
    acc: f32,
}
impl Effect for AudioScroll {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let cols = layout::WIDTH.ceil() as usize;
        ensure(&mut self.hist, cols);
        self.acc += c.dt * p.speed() * 25.0;
        let level = (c.audio.level * gain(p) * 1.4).min(1.0);
        while self.acc >= 1.0 {
            self.acc -= 1.0;
            self.hist.rotate_right(1);
            self.hist[0] = level;
        }
        // newest sample in the middle, spreading out to both sides
        let rainbow = p.b("rainbow");
        let col = p.color("color");
        for (o, k) in out.iter_mut().zip(KEYS) {
            let off = (k.cx() - CENTER.0).abs() as usize;
            let amp = self.hist[off.min(cols - 1)];
            // fade fully out at silence (no leftover glow on the middle rows)
            let lit = (amp * 3.6 - (k.cy() - CENTER.1).abs() + 0.5).clamp(0.0, 1.0) * (amp * 12.0).min(1.0);
            let cc = if rainbow { color::hsv(0.33 - amp * 0.33, 1.0, 1.0) } else { col };
            *o = color::scale(cc, lit);
        }
    }
}

#[derive(Default)]
struct BeatRipple {
    inner: Ripple,
    avg: f32,
    cool: f32,
    rng: Rng,
}
impl Effect for BeatRipple {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let bass = c.audio.bass * gain(p);
        self.avg += (bass - self.avg) * (c.dt * 3.0).min(1.0);
        self.cool -= c.dt;
        if bass > 0.25 && bass > self.avg * 1.35 && self.cool <= 0.0 {
            self.cool = 0.15;
            self.inner.now = c.t;
            let i = (self.rng.f() * KEYS.len() as f32) as usize;
            self.inner.key(i.min(KEYS.len() - 1), c.t);
        }
        self.inner.render(c, p, out);
    }
}

/// Same rings as Beat Ripple, but they appear by themselves (no sound needed).
#[derive(Default)]
struct AmbientRipple {
    inner: Ripple,
    rng: Rng,
}
impl Effect for AmbientRipple {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let rate = 0.3 + p.f("density") * 3.5; // ripples per second
        if self.rng.f() < rate * c.dt {
            self.inner.now = c.t;
            let i = (self.rng.f() * KEYS.len() as f32) as usize;
            self.inner.key(i.min(KEYS.len() - 1), c.t);
        }
        self.inner.render(c, p, out);
    }
}

// ---------------------------------------------------------------- screen

const SCREEN_AREAS: &[&str] = &["Whole screen", "Without taskbar"];

/// Each key shows the average color of its part of the screen.
#[derive(Default)]
struct ScreenGrid {
    cur: Vec<Rgb>,
    cells: Vec<(usize, usize, usize, usize)>,
}
impl Effect for ScreenGrid {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        use crate::screen::{GH, GW};
        if c.screen.len() != GW * GH {
            fill(out, BLACK);
            return;
        }
        if self.cells.len() != out.len() {
            // which cells of the tiny screen picture belong to each key:
            // columns follow the key's position and width, rows split the screen into 6 bands
            self.cells = KEYS
                .iter()
                .map(|k| {
                    let x0 = ((k.x / layout::WIDTH * GW as f32).round() as usize).min(GW - 1);
                    let x1 = (((k.x + k.w) / layout::WIDTH * GW as f32).round() as usize).clamp(x0 + 1, GW);
                    let y0 = (k.row as usize * GH / 6).min(GH - 1);
                    let y1 = ((k.row as usize + 1) * GH / 6).clamp(y0 + 1, GH);
                    (x0, x1, y0, y1)
                })
                .collect();
        }
        let first = self.cur.len() != out.len();
        if first {
            self.cur = vec![BLACK; out.len()];
        }
        let boost = 1.0 + p.f("saturation") * 1.5;
        let smooth = p.f("smooth");
        let a = if first || smooth < 0.02 { 1.0 } else { 1.0 - (-c.dt * 30.0 * 0.05f32.powf(smooth)).exp() };
        let mirror = p.b("mirror");
        for (i, &(x0, x1, y0, y1)) in self.cells.iter().enumerate() {
            let (x0, x1) = if mirror { (GW - x1, GW - x0) } else { (x0, x1) };
            let mut sum = [0.0; 3];
            for y in y0..y1 {
                for px in &c.screen[y * GW + x0..y * GW + x1] {
                    sum = color::add(sum, *px);
                }
            }
            let avg = color::scale(sum, 1.0 / ((x1 - x0) * (y1 - y0)) as f32);
            // push colors away from grey so they read better on LEDs
            let l = avg[0] * 0.299 + avg[1] * 0.587 + avg[2] * 0.114;
            let vivid = avg.map(|v| (l + (v - l) * boost).clamp(0.0, 1.0));
            self.cur[i] = color::lerp(self.cur[i], vivid, a);
        }
        out.copy_from_slice(&self.cur);
    }
}

// ---------------------------------------------------------------- more ambient

/// Soft round falloff: 1 at d = 0, about 0.37 at d = r.
fn glow(d: f32, r: f32) -> f32 {
    (-(d / r) * (d / r)).exp()
}

/// Horizontal distance from x to the key (0 when x is over the key).
fn hdist(k: &layout::Key, x: f32) -> f32 {
    (k.x - x).max(x - (k.x + k.w)).max(0.0)
}

/// Cheap repeatable "random" number in 0..1.
fn hash(a: f32) -> f32 {
    ((a * 12.9898).sin() * 43758.547).fract().abs()
}

/// A light bar sliding back and forth, like KITT from Knight Rider.
#[derive(Default)]
struct Scanner;
const SCAN_DIRS: &[&str] = &["Horizontal", "Vertical"];
impl Effect for Scanner {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let ph = (c.t * p.speed() * 0.6).rem_euclid(2.0);
        let tri = if ph < 1.0 { ph } else { 2.0 - ph };
        let dir = if ph < 1.0 { 1.0 } else { -1.0 };
        let vertical = p.choice("direction") == 1;
        let span = if vertical { layout::HEIGHT } else { layout::WIDTH };
        let x = (0.5 - 0.5 * (std::f32::consts::PI * tri).cos()) * span;
        let w = 0.5 + p.f("width") * 2.0;
        let trail = 0.5 + p.f("trail") * if vertical { 3.0 } else { 9.0 };
        let (col, bg) = (p.color("color"), p.color("color2"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let kc = if vertical { k.cy() } else { k.cx() };
            let behind = (x - kc) * dir;
            let a = if behind.abs() < w / 2.0 {
                1.0
            } else if behind > 0.0 {
                (1.0 - (behind - w / 2.0) / trail).max(0.0).powi(2)
            } else {
                0.0
            };
            let cc = if behind.abs() < w * 0.25 { color::lerp(col, WHITE, 0.35) } else { col };
            *o = color::lerp(bg, cc, a);
        }
    }
}

/// A spiral galaxy turning slowly, with a bright core and twinkling stars.
#[derive(Default)]
struct Galaxy {
    stars: Vec<f32>,
    rng: Rng,
}
impl Effect for Galaxy {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.stars, out.len());
        let t = c.t * p.speed();
        let twist = 0.4 + p.f("twist") * 1.4;
        let (inner, outer, bg) = (p.color("color"), p.color("color2"), [0.0, 0.0, 0.02]);
        let decay = (-c.dt * 3.0).exp();
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let (dx, dy) = (k.cx() - CENTER.0, (k.cy() - CENTER.1) * 1.8);
            let r = (dx * dx + dy * dy).sqrt();
            let ang = dy.atan2(dx);
            let arm = (0.5 + 0.5 * (2.0 * ang - r * twist + t * 1.2).cos()).powf(2.5);
            let light = arm * (-r / 6.5).exp() * 1.4;
            let mut col = color::lerp(outer, inner, (-r / 3.5).exp());
            col = color::lerp(color::add(bg, color::scale(col, light)), color::lerp(inner, WHITE, 0.6), glow(r, 1.4));
            self.stars[i] *= decay;
            if self.rng.f() < 0.15 * c.dt {
                self.stars[i] = 1.0;
            }
            *o = color::add(col, color::scale(WHITE, self.stars[i] * 0.7));
        }
    }
}

/// A heart monitor: the ECG line sweeps across and the keyboard thumps on each beat.
#[derive(Default)]
struct Heartbeat;
impl Heartbeat {
    /// height of the ECG trace (in rows, up is positive) at position s (0..1 within one beat)
    fn ecg(s: f32) -> f32 {
        let g = |m: f32, w: f32| glow(s - m, w);
        0.35 * g(0.15, 0.04) - 0.5 * g(0.31, 0.015) + 2.4 * g(0.35, 0.02) - 0.9 * g(0.39, 0.02) + 0.5 * g(0.6, 0.06)
    }
}
impl Effect for Heartbeat {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let vx = 7.0 * p.speed();
        let span = layout::WIDTH + 3.0;
        let pos = c.t * vx;
        let sweep = (pos / span).floor();
        let head = pos - sweep * span - 1.0;
        let off = sweep * 0.37;
        let line_y = |x: f32| 3.6 - Self::ecg((x / 8.0 + off).rem_euclid(1.0));
        let (col, bg) = (p.color("color"), p.color("color2"));
        // the board thumps when the head passes the big spike
        let pulse = glow(((head / 8.0 + off).rem_euclid(1.0)) - 0.37, 0.05) * 0.18;
        let trail = 2.0 + p.f("trail") * 10.0;
        for (o, k) in out.iter_mut().zip(KEYS) {
            let behind = head - k.cx();
            let mut col_k = color::add(bg, color::scale(col, pulse));
            if behind > -0.5 {
                // closest the trace gets to this key (sample across its width)
                let d = (0..5)
                    .map(|j| (k.cy() - line_y(k.x + k.w * (j as f32 + 0.5) / 5.0)).abs())
                    .fold(f32::MAX, f32::min);
                let a = 1.0 - smoothstep(0.3, 0.8, d);
                let fade = if behind < 0.6 { 1.0 } else { (-(behind - 0.6) / trail * 2.5).exp() };
                let cc = if behind < 0.6 { color::lerp(col, WHITE, 0.6) } else { col };
                col_k = color::lerp(col_k, cc, a * fade);
            }
            *o = col_k;
        }
    }
}

/// Sky from sunrise to night: the sun crosses the keyboard, then the moon and stars come out.
#[derive(Default)]
struct DayCycle;
const SKY_TOP: &[[u8; 3]] = &[[1, 2, 12], [40, 30, 100], [20, 100, 255], [60, 20, 90]];
const SKY_LOW: &[[u8; 3]] = &[[8, 12, 40], [255, 100, 30], [140, 210, 255], [255, 60, 50]];
impl Effect for DayCycle {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        // 0 = midnight, 0.25 = sunrise, 0.5 = noon, 0.75 = sunset
        let phase = if p.b("realtime") {
            crate::sys::local_day_fraction()
        } else {
            (c.t * p.speed() * 0.012 + 0.2).rem_euclid(1.0)
        };
        let top = color::gradient(SKY_TOP, phase, true);
        let low = color::gradient(SKY_LOW, phase, true);
        let arc = |u: f32| (u * layout::WIDTH, layout::HEIGHT + 0.5 - (std::f32::consts::PI * u).sin() * 5.5);
        let sun_u = (phase - 0.22) / 0.56;
        let moon_u = ((phase + 0.5).rem_euclid(1.0) - 0.22) / 0.56;
        let night = 1.0 - smoothstep(0.18, 0.28, phase) + smoothstep(0.72, 0.82, phase);
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let mut col = color::lerp(top, low, k.v().powf(1.3));
            if (0.0..=1.0).contains(&sun_u) {
                let (sx, sy) = arc(sun_u);
                let sun = color::lerp([1.0, 0.45, 0.1], [1.0, 0.95, 0.8], (std::f32::consts::PI * sun_u).sin());
                col = color::lerp(col, sun, glow(dist(k, sx, sy), 1.4));
            }
            if (0.0..=1.0).contains(&moon_u) {
                let (mx, my) = arc(moon_u);
                col = color::lerp(col, [0.75, 0.8, 1.0], glow(dist(k, mx, my), 0.9) * 0.9);
            }
            // stars twinkle on dark nights
            let tw = (hash(i as f32) * 20.0 + c.t * (1.0 + hash(i as f32 + 3.0) * 2.0)).sin();
            if hash(i as f32 + 7.0) < 0.3 && k.v() < 0.7 {
                col = color::add(col, color::scale(WHITE, night * 0.35 * (0.5 + 0.5 * tw)));
            }
            *o = col;
        }
    }
}

/// Mirrored flowing colors, like looking through a kaleidoscope.
#[derive(Default)]
struct Kaleidoscope;
const MIRRORS: &[&str] = &["4 mirrors", "6 mirrors", "8 mirrors"];
impl Effect for Kaleidoscope {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed();
        let n = [4.0, 6.0, 8.0][p.choice("mirrors").min(2)];
        let wedge = TAU / n;
        let pal = p.choice("palette");
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (dx, dy) = (k.cx() - CENTER.0, (k.cy() - CENTER.1) * 1.5);
            let r = (dx * dx + dy * dy).sqrt();
            let mut a = (dy.atan2(dx) + t * 0.15).rem_euclid(wedge);
            if a > wedge / 2.0 {
                a = wedge - a;
            }
            let (fx, fy) = (r * a.cos(), r * a.sin());
            let v = (fx * 1.1 + t).sin()
                + (fy * 2.3 - t * 1.3).sin()
                + ((fx + fy) * 0.9 + t * 0.6).sin()
                + (r * 1.2 - t * 1.5).sin();
            *o = color::palette(pal, v / 8.0 + 0.5 + t * 0.04);
        }
    }
}

/// Raindrops fall down the keys and splash on the bottom row.
#[derive(Default)]
struct Rainfall {
    drops: Vec<(f32, f32, f32)>, // x, y, speed
    splashes: Vec<(f32, f32)>,   // x, start time
    rng: Rng,
}
impl Effect for Rainfall {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let s = p.speed();
        let rate = (3.0 + p.f("density") * 30.0) * s;
        let wind = (p.f("wind") - 0.5) * 6.0;
        if self.rng.f() < rate * c.dt && self.drops.len() < 60 {
            let x = -2.0 + self.rng.f() * (layout::WIDTH + 4.0);
            self.drops.push((x, -0.8, 7.0 + self.rng.f() * 5.0));
        }
        for d in self.drops.iter_mut() {
            d.1 += d.2 * c.dt * s;
            d.0 += wind * c.dt * s;
        }
        for d in self.drops.iter().filter(|d| d.1 >= layout::HEIGHT - 0.4) {
            self.splashes.push((d.0, c.t));
        }
        self.drops.retain(|d| d.1 < layout::HEIGHT - 0.4);
        let life = 0.45 / s.max(0.3);
        self.splashes.retain(|sp| c.t - sp.1 < life);
        let (col, bg) = (p.color("color"), p.color("color2"));
        fill(out, bg);
        for (o, k) in out.iter_mut().zip(KEYS) {
            for d in &self.drops {
                let below = d.1 - k.cy();
                if hdist(k, d.0) < 0.35 && (-0.5..1.8).contains(&below) {
                    let a = if below < 0.5 { 1.0 } else { 0.6 * (1.0 - (below - 0.5) / 1.3) };
                    let cc = if below < 0.5 { color::lerp(col, WHITE, 0.4) } else { col };
                    *o = color::lerp(*o, cc, a);
                }
            }
            if k.row >= 4 {
                for sp in &self.splashes {
                    let age = (c.t - sp.1) / life;
                    let row_fade = if k.row == 5 { 1.0 } else { 0.35 };
                    let a = (ring((k.cx() - sp.0).abs(), age * 2.5, 0.6) + glow(k.cx() - sp.0, 0.6) * (1.0 - age * 3.0).max(0.0))
                        * (1.0 - age)
                        * row_fade;
                    *o = color::lerp(*o, col, a.min(1.0));
                }
            }
        }
    }
}

/// Snowflakes drift down and pile up on the bottom row, then slowly melt.
#[derive(Default)]
struct Snowfall {
    flakes: Vec<(f32, f32, f32, f32)>, // x, y, fall speed, sway phase
    pile: Vec<f32>,
    rng: Rng,
}
impl Effect for Snowfall {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.pile, out.len());
        let s = p.speed();
        let rate = (1.0 + p.f("density") * 10.0) * s;
        if self.rng.f() < rate * c.dt && self.flakes.len() < 40 {
            let x = self.rng.f() * layout::WIDTH;
            self.flakes.push((x, -0.6, 0.8 + self.rng.f() * 0.9, self.rng.f() * TAU));
        }
        for f in self.flakes.iter_mut() {
            f.1 += f.2 * c.dt * s;
        }
        let t = c.t;
        let sway = |f: &(f32, f32, f32, f32)| f.0 + 0.6 * (t * 1.3 + f.3).sin();
        for f in self.flakes.iter().filter(|f| f.1 >= layout::HEIGHT - 0.6) {
            let x = sway(f);
            if let Some(i) = KEYS.iter().position(|k| k.row == 5 && x >= k.x && x < k.x + k.w) {
                self.pile[i] = (self.pile[i] + 0.3).min(1.0);
            }
        }
        self.flakes.retain(|f| f.1 < layout::HEIGHT - 0.6);
        let melt = (-c.dt * 0.08).exp();
        let (col, bg) = (p.color("color"), p.color("color2"));
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            self.pile[i] *= melt;
            let mut cc = color::lerp(bg, col, self.pile[i] * 0.8);
            for f in &self.flakes {
                let a = glow(hdist(k, sway(f)), 0.45) * glow(k.cy() - f.1, 0.55);
                cc = color::lerp(cc, col, a);
            }
            *o = cc;
        }
    }
}

/// Warm candlelight that flickers and sways.
#[derive(Default)]
struct Candle {
    flick: [f32; 3],
    rng: Rng,
}
const CANDLE: &[[u8; 3]] = &[[0, 0, 0], [50, 6, 0], [190, 45, 0], [255, 130, 20], [255, 205, 110], [255, 245, 220]];
const CANDLE_COUNTS: &[&str] = &["One", "Two", "Three"];
impl Effect for Candle {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let s = p.speed();
        let spots: &[(f32, f32)] = match p.choice("count") {
            0 => &[(8.1, 3.6)],
            1 => &[(4.5, 3.4), (11.6, 3.4)],
            _ => &[(3.0, 3.4), (8.1, 3.2), (13.2, 3.4)],
        };
        let a = (-c.dt * 12.0 * s).exp();
        let size = 1.6 + p.f("size") * 2.2;
        let flames: Vec<(f32, f32, f32)> = spots
            .iter()
            .enumerate()
            .map(|(i, &(x, y))| {
                self.flick[i] = self.flick[i] * a + self.rng.f() * (1.0 - a);
                let f = i as f32;
                let level = 0.7 + 0.35 * self.flick[i] + 0.08 * (c.t * 7.0 * s + f).sin();
                (x + 0.3 * (c.t * 2.1 * s + f * 1.7).sin(), y, level)
            })
            .collect();
        for (o, k) in out.iter_mut().zip(KEYS) {
            let mut heat = 0.0;
            for &(x, y, level) in &flames {
                let dy = k.cy() - y;
                // flames reach further up than down
                let dy = if dy < 0.0 { dy * 0.75 } else { dy * 1.3 };
                let d = ((k.cx() - x).powi(2) + dy * dy).sqrt();
                heat += level * glow(d, size * level);
            }
            *o = color::gradient(CANDLE, heat.min(1.0), false);
        }
    }
}

/// A neon sign: a bright frame around the edge, a softer glow inside, and the odd buzzing flicker.
#[derive(Default)]
struct Neon {
    border: Vec<bool>,
    flicker: [(f32, f32); 2], // per tube: flicker until, seed
    rng: Rng,
}
impl Effect for Neon {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        if self.border.len() != out.len() {
            self.border = KEYS
                .iter()
                .map(|k| {
                    let row = KEYS.iter().filter(|o| o.row == k.row);
                    let first = row.clone().map(|o| o.x).fold(f32::MAX, f32::min);
                    let last = row.map(|o| o.x).fold(f32::MIN, f32::max);
                    k.row == 0 || k.row == 5 || k.x == first || k.x == last
                })
                .collect();
        }
        let chance = p.f("flicker") * 0.5;
        for f in self.flicker.iter_mut() {
            if c.t > f.0 && self.rng.f() < chance * c.dt {
                *f = (c.t + 0.2 + self.rng.f() * 1.0, self.rng.f() * 100.0);
            }
        }
        let tube = |i: usize| {
            let (until, seed) = self.flicker[i];
            let hum = 0.94 + 0.06 * (c.t * 9.0 + i as f32).sin();
            if c.t < until && hash((c.t * 25.0).floor() + seed) < 0.45 {
                0.08
            } else {
                hum
            }
        };
        let (outer, inner) = (tube(0), tube(1));
        let (a, b) = (p.color("color"), p.color("color2"));
        for (o, (k, &edge)) in out.iter_mut().zip(KEYS.iter().zip(&self.border)) {
            let _ = k;
            *o = if edge { color::scale(a, outer) } else { color::scale(b, inner * 0.45) };
        }
    }
}

/// Two twisting strands of a DNA helix scroll across.
#[derive(Default)]
struct Helix;
impl Effect for Helix {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let t = c.t * p.speed() * 2.0;
        let (a, b, bg) = (p.color("color"), p.color("color2"), p.color("color3"));
        let mid = CENTER.1 + 0.1;
        for (o, k) in out.iter_mut().zip(KEYS) {
            let ph = k.cx() * 0.75 - t;
            let (y1, y2) = (mid + 2.1 * ph.sin(), mid - 2.1 * ph.sin());
            let depth = ph.cos(); // > 0: strand 1 in front
            let s1 = (1.0 - smoothstep(0.3, 0.85, (k.cy() - y1).abs())) * (0.45 + 0.55 * (0.5 + 0.5 * depth));
            let s2 = (1.0 - smoothstep(0.3, 0.85, (k.cy() - y2).abs())) * (0.45 + 0.55 * (0.5 - 0.5 * depth));
            let q = (ph / TAU * 4.0).rem_euclid(1.0);
            let between = k.cy() > y1.min(y2) && k.cy() < y1.max(y2);
            let rung = if between { glow(q - 0.5, 0.15) * 0.35 } else { 0.0 };
            let mut col = color::lerp(bg, color::lerp(a, b, 0.5), rung);
            // draw the strand at the back first
            let (back, front) = if depth > 0.0 { ((b, s2), (a, s1)) } else { ((a, s1), (b, s2)) };
            col = color::lerp(col, back.0, back.1);
            col = color::lerp(col, front.0, front.1);
            *o = col;
        }
    }
}

/// Bubbles rise from the bottom, wobble, and pop.
#[derive(Default)]
struct Bubbles {
    bubbles: Vec<[f32; 6]>, // x, y, radius, speed, hue, pop height
    pops: Vec<(f32, f32, f32, f32)>, // x, y, start time, hue
    rng: Rng,
}
impl Effect for Bubbles {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let s = p.speed();
        let rate = (0.4 + p.f("density") * 3.0) * s;
        if self.rng.f() < rate * c.dt && self.bubbles.len() < 10 {
            let r = (0.6 + self.rng.f() * 0.6) * (0.6 + p.f("size"));
            let x = 0.5 + self.rng.f() * (layout::WIDTH - 1.0);
            self.bubbles.push([x, layout::HEIGHT + r, r, 1.2 + self.rng.f() * 1.4, self.rng.f(), self.rng.f() * 2.0]);
        }
        for b in self.bubbles.iter_mut() {
            b[1] -= b[3] * c.dt * s;
        }
        for b in self.bubbles.iter().filter(|b| b[1] < b[5]) {
            self.pops.push((b[0], b[1], c.t, b[4]));
        }
        self.bubbles.retain(|b| b[1] >= b[5]);
        let life = 0.35 / s.max(0.3);
        self.pops.retain(|pp| c.t - pp.2 < life);
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        let tint = |h: f32| if rainbow { color::hsv(h, 0.7, 1.0) } else { col };
        fill(out, bg);
        for (o, k) in out.iter_mut().zip(KEYS) {
            for b in &self.bubbles {
                let x = b[0] + 0.35 * (c.t * 2.0 + b[4] * TAU).sin();
                let (dx, dy) = (k.cx() - x, (k.cy() - b[1]) * 1.2);
                let d = (dx * dx + dy * dy).sqrt();
                let a = ring(d, b[2], 0.4) * 0.9 + if d < b[2] { 0.12 } else { 0.0 };
                *o = color::lerp(*o, tint(b[4]), a.min(1.0));
            }
            for pp in &self.pops {
                let age = (c.t - pp.2) / life;
                let a = ring(dist(k, pp.0, pp.1), age * 2.5, 0.5) * (1.0 - age);
                *o = color::lerp(*o, color::lerp(tint(pp.3), WHITE, 0.5), a);
            }
        }
    }
}

/// A calm color that keeps glitching: rows jump sideways, colors split, blocks flash.
#[derive(Default)]
struct Glitch {
    bands: Vec<(u8, f32, f32, f32, u8)>, // row, start, length, shift, channel
    blocks: Vec<(usize, f32)>,
    flash: f32,
    rng: Rng,
}
impl Effect for Glitch {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let amount = 0.2 + p.f("intensity") * 3.0;
        let s = p.speed();
        if self.rng.f() < amount * 1.5 * c.dt {
            let row = ((self.rng.f() * 6.0) as u8).min(5);
            let len = 0.05 + self.rng.f() * 0.25;
            let shift = (self.rng.f() - 0.5) * 8.0;
            let ch = ((self.rng.f() * 3.0) as u8).min(2);
            self.bands.push((row, c.t, len, shift, ch));
        }
        if self.rng.f() < amount * 4.0 * c.dt {
            let i = ((self.rng.f() * KEYS.len() as f32) as usize).min(KEYS.len() - 1);
            self.blocks.push((i, c.t));
        }
        if self.rng.f() < amount * 0.08 * c.dt {
            self.flash = c.t;
        }
        self.bands.retain(|b| c.t - b.1 < b.2);
        self.blocks.retain(|b| c.t - b.1 < 0.08);
        let (a, b) = (p.color("color"), p.color("color2"));
        let base = |x: f32, y: f32| color::lerp(a, b, 0.5 + 0.5 * (x * 0.35 + y * 0.4 + c.t * s * 0.5).sin());
        let inverted = c.t - self.flash < 0.05;
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            let mut col = base(k.cx(), k.cy());
            for band in self.bands.iter().filter(|band| band.0 == k.row) {
                col = base(k.cx() + band.3, k.cy());
                // keep only one color channel boosted, like a broken video signal
                let mut split = [col[0] * 0.3, col[1] * 0.3, col[2] * 0.3];
                split[band.4 as usize] = 1.0;
                col = split;
            }
            if self.blocks.iter().any(|bl| bl.0 == i) {
                col = WHITE;
            }
            if inverted {
                col = col.map(|v| 1.0 - v.min(1.0));
            }
            *o = col;
        }
    }
}

// ---------------------------------------------------------------- games

/// The keyboard as a 16 x 6 grid (one cell per key, wide keys use their middle).
const GRID_W: usize = 16;
const GRID_H: usize = 6;

fn cell_of(k: &layout::Key) -> (usize, usize) {
    ((k.cx() as usize).min(GRID_W - 1), k.row as usize)
}

fn grid_valid() -> [[bool; GRID_W]; GRID_H] {
    let mut v = [[false; GRID_W]; GRID_H];
    for k in KEYS {
        let (x, y) = cell_of(k);
        v[y][x] = true;
    }
    v
}

/// Snake plays itself: it hunts the food, grows, and starts over when it gets stuck.
#[derive(Default)]
struct Snake {
    body: std::collections::VecDeque<(i32, i32)>,
    food: (i32, i32),
    acc: f32,
    dead: f32,
    rng: Rng,
}
impl Snake {
    fn random_cell(&mut self, valid: &[[bool; GRID_W]; GRID_H]) -> (i32, i32) {
        loop {
            let x = ((self.rng.f() * GRID_W as f32) as usize).min(GRID_W - 1);
            let y = ((self.rng.f() * GRID_H as f32) as usize).min(GRID_H - 1);
            if valid[y][x] && !self.body.contains(&(x as i32, y as i32)) {
                return (x as i32, y as i32);
            }
        }
    }
    fn reset(&mut self, valid: &[[bool; GRID_W]; GRID_H]) {
        self.body.clear();
        self.body.push_back((5, 2));
        self.body.push_back((4, 2));
        self.body.push_back((3, 2));
        self.food = self.random_cell(valid);
    }
    fn free(&self, valid: &[[bool; GRID_W]; GRID_H], (x, y): (i32, i32)) -> bool {
        x >= 0 && y >= 0 && (x as usize) < GRID_W && (y as usize) < GRID_H && valid[y as usize][x as usize] && {
            // the tail moves away this step, so it doesn't block
            let n = self.body.len();
            !self.body.iter().take(n - 1).any(|&b| b == (x, y))
        }
    }
    /// How many cells can be reached from `start` (to avoid walking into dead ends).
    fn room(&self, valid: &[[bool; GRID_W]; GRID_H], start: (i32, i32)) -> usize {
        let mut seen = [[false; GRID_W]; GRID_H];
        let mut stack = vec![start];
        let mut n = 0;
        while let Some((x, y)) = stack.pop() {
            if !self.free(valid, (x, y)) || seen[y as usize][x as usize] {
                continue;
            }
            seen[y as usize][x as usize] = true;
            n += 1;
            stack.extend([(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]);
        }
        n
    }
    fn step(&mut self, valid: &[[bool; GRID_W]; GRID_H]) {
        let head = self.body[0];
        let need = self.body.len();
        let mut best: Option<((i32, i32), f32)> = None;
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let next = (head.0 + dx, head.1 + dy);
            if !self.free(valid, next) {
                continue;
            }
            let room = self.room(valid, next);
            let dist = ((next.0 - self.food.0).abs() + (next.1 - self.food.1).abs()) as f32;
            let trapped = if room < need { 100.0 } else { 0.0 };
            let score = dist + trapped + self.rng.f() * 0.5;
            if best.is_none_or(|b| score < b.1) {
                best = Some((next, score));
            }
        }
        let Some((next, _)) = best else {
            self.dead = 0.8;
            return;
        };
        self.body.push_front(next);
        if next == self.food {
            if self.body.len() > 28 {
                self.dead = 0.8;
                return;
            }
            self.food = self.random_cell(valid);
        } else {
            self.body.pop_back();
        }
    }
}
impl Effect for Snake {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let valid = grid_valid();
        if self.body.is_empty() {
            self.reset(&valid);
        }
        let (col, food, bg, rainbow) = (p.color("color"), p.color("color2"), p.color("color3"), p.b("rainbow"));
        if self.dead > 0.0 {
            // flash, then start over
            self.dead -= c.dt;
            if self.dead <= 0.0 {
                self.reset(&valid);
            }
        } else {
            self.acc += c.dt * p.speed() * 9.0;
            while self.acc >= 1.0 && self.dead <= 0.0 {
                self.acc -= 1.0;
                self.step(&valid);
            }
        }
        let flash = if self.dead > 0.0 { (self.dead * 12.0).sin().abs() } else { 1.0 };
        let n = self.body.len() as f32;
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (x, y) = cell_of(k);
            let cell = (x as i32, y as i32);
            *o = if let Some(j) = self.body.iter().position(|&b| b == cell) {
                let f = j as f32 / n;
                let cc = if rainbow { color::hsv(f * 0.8 + c.t * 0.05, 1.0, 1.0) } else { col };
                let cc = if j == 0 { color::lerp(cc, WHITE, 0.5) } else { color::scale(cc, 1.0 - f * 0.6) };
                color::scale(cc, flash)
            } else if cell == self.food {
                color::scale(food, 0.6 + 0.4 * (c.t * 8.0).sin())
            } else {
                bg
            };
        }
    }
}

/// Two paddles play Pong across the keyboard by themselves.
#[derive(Default)]
struct Pong {
    ball: [f32; 4], // x, y, vx, vy
    paddles: [f32; 2],
    trail: Vec<(f32, f32)>,
    score: [f32; 2], // flash time per side
    edges: Vec<i8>,  // -1 left edge key, 1 right edge key
    rng: Rng,
}
impl Pong {
    fn serve(&mut self, dir: f32) {
        let vy = (self.rng.f() - 0.5) * 4.0;
        self.ball = [CENTER.0, 1.0 + self.rng.f() * 4.0, 7.0 * dir, vy];
        self.trail.clear();
    }
}
impl Effect for Pong {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        if self.edges.len() != out.len() {
            self.edges = KEYS
                .iter()
                .map(|k| {
                    let row = KEYS.iter().filter(|o| o.row == k.row);
                    let first = row.clone().map(|o| o.x).fold(f32::MAX, f32::min);
                    let last = row.map(|o| o.x).fold(f32::MIN, f32::max);
                    if k.x == first { -1 } else if k.x == last { 1 } else { 0 }
                })
                .collect();
            self.paddles = [3.0, 3.0];
            self.serve(1.0);
        }
        let dt = c.dt * p.speed() * 1.4;
        let (lo, hi) = (0.5, layout::HEIGHT - 0.5);
        let [mut x, mut y, mut vx, mut vy] = self.ball;
        x += vx * dt;
        y += vy * dt;
        if y < lo || y > hi {
            vy = -vy;
            y = y.clamp(lo, hi);
        }
        // paddles chase the ball, but not perfectly
        let skill = 2.5 + p.f("skill") * 6.0;
        for (side, pad) in self.paddles.iter_mut().enumerate() {
            let coming = if side == 0 { vx < 0.0 } else { vx > 0.0 };
            let target = if coming { y } else { CENTER.1 };
            *pad += (target - *pad).clamp(-skill * dt, skill * dt);
        }
        for side in 0..2 {
            let wall = if side == 0 { 1.0 } else { layout::WIDTH - 1.0 };
            let hit_zone = if side == 0 { x < wall && vx < 0.0 } else { x > wall && vx > 0.0 };
            if hit_zone {
                if (y - self.paddles[side]).abs() < 1.3 {
                    vx = -vx * 1.03;
                    vy += (y - self.paddles[side]) * 2.0 + (self.rng.f() - 0.5);
                    vx = vx.clamp(-14.0, 14.0);
                    x = wall;
                } else if x < -0.5 || x > layout::WIDTH + 0.5 {
                    // missed: the other side scores
                    self.score[1 - side] = c.t;
                    self.serve(if side == 0 { 1.0 } else { -1.0 });
                    [x, y, vx, vy] = self.ball;
                }
            }
        }
        self.ball = [x, y, vx, vy];
        self.trail.insert(0, (x, y));
        self.trail.truncate(6);

        let (ball, pad, bg) = (p.color("color"), p.color("color2"), p.color("color3"));
        for (o, (k, &edge)) in out.iter_mut().zip(KEYS.iter().zip(&self.edges)) {
            let mut col = bg;
            // dotted net down the middle
            if (k.cx() - layout::WIDTH / 2.0).abs() < 0.6 && k.row % 2 == 1 {
                col = color::add(col, [0.06, 0.06, 0.06]);
            }
            if edge != 0 {
                let side = if edge < 0 { 0 } else { 1 };
                if (k.cy() - self.paddles[side]).abs() < 0.9 {
                    col = pad;
                }
                let since = c.t - self.score[1 - side];
                if (0.0..0.6).contains(&since) {
                    col = color::lerp(col, WHITE, (1.0 - since / 0.6) * 0.8);
                }
            }
            for (j, &(tx, ty)) in self.trail.iter().enumerate() {
                let a = glow(dist(k, tx, ty), 0.55) * (1.0 - j as f32 / 6.0);
                col = color::lerp(col, if j == 0 { color::lerp(ball, WHITE, 0.3) } else { ball }, a);
            }
            *o = col;
        }
    }
}

/// Conway's Game of Life on the keys. Starts over when the pattern dies or gets stuck.
#[derive(Default)]
struct Life {
    cells: Vec<bool>,
    prev: Vec<bool>,
    glow: Vec<f32>,
    acc: f32,
    gens: u32,
    hue: f32,
    rng: Rng,
}
impl Life {
    fn seed(&mut self) {
        self.cells = (0..GRID_W * GRID_H).map(|_| self.rng.f() < 0.38).collect();
        self.prev.clear();
        self.gens = 0;
        self.hue += 0.17;
    }
}
impl Effect for Life {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        if self.cells.len() != GRID_W * GRID_H {
            self.seed();
            self.glow = vec![0.0; GRID_W * GRID_H];
        }
        self.acc += c.dt * p.speed() * 5.0;
        while self.acc >= 1.0 {
            self.acc -= 1.0;
            let mut next = vec![false; GRID_W * GRID_H];
            for y in 0..GRID_H {
                for x in 0..GRID_W {
                    let mut n = 0;
                    for dy in [GRID_H - 1, 0, 1] {
                        for dx in [GRID_W - 1, 0, 1] {
                            if (dx, dy) != (0, 0) && self.cells[(y + dy) % GRID_H * GRID_W + (x + dx) % GRID_W] {
                                n += 1;
                            }
                        }
                    }
                    let alive = self.cells[y * GRID_W + x];
                    next[y * GRID_W + x] = n == 3 || (alive && n == 2);
                }
            }
            let stuck = next == self.cells || next == self.prev || next.iter().filter(|&&a| a).count() < 3;
            self.prev = std::mem::replace(&mut self.cells, next);
            self.gens += 1;
            if stuck || self.gens > 150 {
                self.seed();
            }
        }
        let k = (c.dt * 10.0).min(1.0);
        for (g, &alive) in self.glow.iter_mut().zip(&self.cells) {
            *g += ((alive as u8 as f32) - *g) * k;
        }
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        for (o, key) in out.iter_mut().zip(KEYS) {
            let (x, y) = cell_of(key);
            let cc = if rainbow { color::hsv(self.hue + x as f32 / 40.0 + y as f32 / 30.0, 0.9, 1.0) } else { col };
            *o = color::lerp(bg, cc, self.glow[y * GRID_W + x]);
        }
    }
}

// ---------------------------------------------------------------- more reactive

/// Every key you press throws out a burst of sparks that fall with gravity.
#[derive(Default)]
struct Sparks {
    parts: Vec<[f32; 6]>, // x, y, vx, vy, start time, hue
    rng: Rng,
    now: f32,
}
impl Effect for Sparks {
    fn key(&mut self, i: usize, _t: f32) {
        let k = &KEYS[i];
        let hue = self.rng.f();
        for _ in 0..10 {
            let ang = self.rng.f() * TAU;
            let sp = 3.0 + self.rng.f() * 4.0;
            self.parts.push([k.cx(), k.cy(), ang.cos() * sp, ang.sin() * sp - 3.0, self.now, hue + self.rng.f() * 0.08]);
        }
        let n = self.parts.len();
        if n > 200 {
            self.parts.drain(..n - 200);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        self.now = c.t;
        let s = p.speed();
        let life = 0.9 / s.max(0.3);
        let dt = c.dt * s;
        for q in self.parts.iter_mut() {
            q[3] += 9.0 * dt;
            q[0] += q[2] * dt;
            q[1] += q[3] * dt;
        }
        self.parts.retain(|q| c.t - q[4] < life);
        let (col, bg, rainbow) = (p.color("color"), p.color("color2"), p.b("rainbow"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let mut acc = bg;
            for q in &self.parts {
                let age = (c.t - q[4]) / life;
                let a = glow(dist(k, q[0], q[1]), 0.6) * (1.0 - age);
                if a > 0.01 {
                    let cc = if rainbow { color::hsv(q[5], 1.0, 1.0) } else { col };
                    acc = color::add(acc, color::scale(color::lerp(WHITE, cc, (age * 4.0).min(1.0)), a));
                }
            }
            *o = acc;
        }
    }
}

/// Keys you press catch fire, and embers float up from them.
#[derive(Default)]
struct Embers {
    heat: Vec<f32>,
    parts: Vec<[f32; 4]>, // x, y, rise speed, start time
    rng: Rng,
    now: f32,
}
impl Effect for Embers {
    fn key(&mut self, i: usize, _t: f32) {
        ensure(&mut self.heat, KEYS.len());
        self.heat[i] = 1.0;
        let k = &KEYS[i];
        for _ in 0..3 {
            let x = k.cx() + (self.rng.f() - 0.5) * k.w * 0.8;
            self.parts.push([x, k.cy(), 2.5 + self.rng.f() * 3.0, self.now]);
        }
        let n = self.parts.len();
        if n > 120 {
            self.parts.drain(..n - 120);
        }
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.heat, out.len());
        self.now = c.t;
        let s = p.speed();
        let life = 1.0 / s.max(0.3);
        for q in self.parts.iter_mut() {
            q[1] -= q[2] * c.dt * s;
        }
        self.parts.retain(|q| c.t - q[3] < life);
        let decay = (-c.dt * 1.6 * s).exp();
        let glow_bg = p.f("idle") * 0.25;
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            self.heat[i] *= decay;
            let mut h = self.heat[i] * 0.95 + glow_bg * (0.6 + 0.4 * (k.cx() * 1.3 + c.t * 2.0).sin()) * k.v();
            for q in &self.parts {
                let age = (c.t - q[3]) / life;
                let x = q[0] + 0.25 * (c.t * 6.0 + q[2] * 3.0).sin();
                h += glow(hdist(k, x), 0.4) * glow(k.cy() - q[1], 0.6) * (1.0 - age) * 0.8;
            }
            *o = color::gradient(LAVA, h.min(1.0), false);
        }
    }
}

// ---------------------------------------------------------------- more audio

/// Flames whose height follows the music, one column per frequency.
#[derive(Default)]
struct AudioFire {
    noise: Vec<f32>,
    rng: Rng,
}
impl Effect for AudioFire {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.noise, out.len());
        let g = gain(p);
        let a = (-c.dt * 12.0).exp();
        for (i, (o, k)) in out.iter_mut().zip(KEYS).enumerate() {
            self.noise[i] = self.noise[i] * a + self.rng.f() * (1.0 - a);
            let band = ((k.u() * 16.0) as usize).min(15);
            let h = ((c.audio.bands[band] * 1.2 + c.audio.level * 0.4) * g).min(1.2);
            let top = h * 6.5;
            let from_bottom = 5.0 - k.row as f32;
            let heat = ((top - from_bottom) / top.max(0.6)).clamp(0.0, 1.0) * (0.5 + 0.7 * self.noise[i]);
            *o = color::gradient(LAVA, heat, false);
        }
    }
}

/// Rings around the center: bass in the middle, treble at the edges.
#[derive(Default)]
struct AudioRings;
impl Effect for AudioRings {
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        let g = gain(p);
        let (col, rainbow) = (p.color("color"), p.b("rainbow"));
        for (o, k) in out.iter_mut().zip(KEYS) {
            let (dx, dy) = (k.cx() - CENTER.0, (k.cy() - CENTER.1) * 2.2);
            let d = ((dx * dx + dy * dy).sqrt() / 8.6).min(1.0);
            let band = ((d * 15.0) as usize).min(15);
            let v = (c.audio.bands[band] * g).min(1.0).powf(1.3);
            let cc = if rainbow { color::hsv(d * 0.75 + c.t * 0.04, 1.0, 1.0) } else { col };
            *o = color::scale(cc, v);
        }
    }
}

// ---------------------------------------------------------------- onboard

#[derive(Default)]
struct Noop;
impl Effect for Noop {
    fn render(&mut self, _: &Ctx, _: &Params, out: &mut [Rgb]) {
        fill(out, BLACK);
    }
}

const ONBOARD_PARAMS: &[ParamDef] = &[p("color", "Color", Kind::Color([255, 0, 0])), SPEED];

macro_rules! onboard {
    ($id:expr, $name:expr, $fw:expr, $prev:expr, $desc:expr) => {
        EffectInfo {
            id: $id,
            name: $name,
            category: "On keyboard",
            desc: $desc,
            params: ONBOARD_PARAMS,
            mode: Mode::Onboard($fw),
            uses_keys: false,
            uses_audio: false,
            uses_screen: false,
            is_static: never,
            make: mk::<Noop>,
            preview_as: Some($prev),
        }
    };
}

macro_rules! soft {
    ($id:expr, $name:expr, $cat:expr, $ty:ty, $params:expr, $desc:expr) => {
        soft!($id, $name, $cat, $ty, $params, $desc, false, false, never)
    };
    ($id:expr, $name:expr, $cat:expr, $ty:ty, $params:expr, $desc:expr, $keys:expr, $audio:expr, $static:expr) => {
        soft!($id, $name, $cat, $ty, $params, $desc, $keys, $audio, $static, false)
    };
    ($id:expr, $name:expr, $cat:expr, $ty:ty, $params:expr, $desc:expr, $keys:expr, $audio:expr, $static:expr, $screen:expr) => {
        EffectInfo {
            id: $id,
            name: $name,
            category: $cat,
            desc: $desc,
            params: $params,
            mode: Mode::Software,
            uses_keys: $keys,
            uses_audio: $audio,
            uses_screen: $screen,
            is_static: $static,
            make: mk::<$ty>,
            preview_as: None,
        }
    };
}

const C_RED: Kind = Kind::Color([255, 0, 0]);
const BG_BLACK: ParamDef = p("color2", "Background", Kind::Color([0, 0, 0]));
const SENS: ParamDef = p("sensitivity", "Sensitivity", Kind::Float(0.4));

static EFFECTS: &[EffectInfo] = &[
    // ambient
    soft!("rainbow", "Rainbow Wave", "Ambient", Rainbow, &[
        p("direction", "Direction", Kind::Choice(WAVE_DIRS, 0)),
        p("width", "Wave size", Kind::Float(0.3)),
        p("saturation", "Saturation", Kind::Float(1.0)),
        SPEED,
    ], "A rainbow sweeping across the keyboard."),
    soft!("spectrum", "Spectrum Cycle", "Ambient", Spectrum, &[
        p("saturation", "Saturation", Kind::Float(1.0)), SPEED,
    ], "Whole keyboard slowly fades through every color."),
    soft!("breathing", "Breathing", "Ambient", Breathing, &[
        p("color", "Color", Kind::Color([0, 200, 255])),
        p("rainbow", "New color each breath", Kind::Bool(false)),
        SPEED,
    ], "Fades in and out."),
    soft!("plasma", "Plasma", "Ambient", Plasma, &[
        p("palette", "Palette", Kind::Choice(color::PALETTE_NAMES, 1)),
        p("scale", "Size", Kind::Float(0.5)),
        SPEED,
    ], "Flowing blobs of color."),
    soft!("aurora", "Northern Lights", "Ambient", Aurora, &[
        p("color", "Color 1", Kind::Color([0, 255, 140])),
        p("color2", "Color 2", Kind::Color([130, 0, 255])),
        SPEED,
    ], "Soft drifting curtains of light."),
    soft!("fire", "Fire", "Ambient", Fire, &[
        p("height", "Flame height", Kind::Float(0.5)), SPEED,
    ], "Flickering flames rising from the bottom row."),
    soft!("ocean", "Ocean", "Ambient", Ocean, &[SPEED], "Rolling blue waves with white caps."),
    soft!("twinkle", "Starry Night", "Ambient", Twinkle, &[
        p("color", "Star color", Kind::Color([255, 255, 255])),
        p("color2", "Sky color", Kind::Color([2, 2, 24])),
        p("rainbow", "Colorful stars", Kind::Bool(false)),
        p("density", "Stars", Kind::Float(0.4)),
        SPEED,
    ], "Random keys twinkle like stars."),
    soft!("matrix", "Matrix Rain", "Ambient", Matrix, &[
        p("color", "Color", Kind::Color([0, 255, 70])),
        p("trail", "Trail length", Kind::Float(0.4)),
        SPEED,
    ], "Digital rain falling down each column."),
    soft!("vortex", "Vortex", "Ambient", Vortex, &[
        p("twist", "Twist", Kind::Float(0.5)), SPEED,
    ], "A rainbow spinning around the center."),
    soft!("police", "Police Lights", "Ambient", Police, &[
        p("color", "Left", C_RED),
        p("color2", "Right", Kind::Color([0, 40, 255])),
        SPEED,
    ], "Flashing red and blue halves."),
    soft!("gradient", "Gradient", "Ambient", Gradient, &[
        p("color", "From", Kind::Color([255, 0, 200])),
        p("color2", "To", Kind::Color([0, 230, 255])),
        p("direction", "Direction", Kind::Choice(DIRS4, 0)),
        p("move", "Animate", Kind::Bool(false)),
        SPEED,
    ], "Blend between two colors.", false, false, gradient_static),
    soft!("ambient_ripple", "Ambient Ripples", "Ambient", AmbientRipple, &[
        p("color", "Ripple color", Kind::Color([0, 230, 255])),
        p("rainbow", "Rainbow ripples", Kind::Bool(true)),
        BG_BLACK,
        p("density", "How often", Kind::Float(0.3)),
        p("width", "Ring width", Kind::Float(0.4)),
        SPEED,
    ], "Ripples appear on their own at random keys, like raindrops on water."),
    soft!("fireworks", "Fireworks", "Ambient", Fireworks, &[
        p("color", "Burst color", Kind::Color([255, 200, 60])),
        p("rainbow", "Colorful bursts", Kind::Bool(true)),
        p("color2", "Sky color", Kind::Color([0, 0, 8])),
        p("density", "How often", Kind::Float(0.4)),
        SPEED,
    ], "Rockets shoot up and explode into rings of sparks."),
    soft!("lava_lamp", "Lava Lamp", "Ambient", LavaLamp, &[
        p("color", "Blob color", Kind::Color([255, 40, 0])),
        p("rainbow", "Colorful blobs", Kind::Bool(false)),
        p("color2", "Background", Kind::Color([40, 0, 30])),
        p("size", "Blob size", Kind::Float(0.4)),
        SPEED,
    ], "Soft blobs that drift around and melt into each other."),
    soft!("storm", "Lightning Storm", "Ambient", Storm, &[
        p("color", "Lightning color", Kind::Color([150, 170, 255])),
        p("color2", "Sky color", Kind::Color([6, 8, 30])),
        p("density", "Strikes", Kind::Float(0.5)),
        SPEED,
    ], "Dark clouds drift by while lightning cracks down the keys."),
    soft!("comets", "Comets", "Ambient", Comets, &[
        p("color", "Comet color", Kind::Color([0, 200, 255])),
        p("rainbow", "Colorful comets", Kind::Bool(true)),
        BG_BLACK,
        p("density", "How many", Kind::Float(0.3)),
        p("trail", "Tail length", Kind::Float(0.4)),
        SPEED,
    ], "Comets streak along the rows with glowing tails."),
    soft!("scanner", "Scanner", "Ambient", Scanner, &[
        p("color", "Color", C_RED),
        BG_BLACK,
        p("direction", "Direction", Kind::Choice(SCAN_DIRS, 0)),
        p("width", "Bar width", Kind::Float(0.3)),
        p("trail", "Trail length", Kind::Float(0.4)),
        SPEED,
    ], "A light bar sweeping back and forth, like KITT."),
    soft!("galaxy", "Galaxy", "Ambient", Galaxy, &[
        p("color", "Core", Kind::Color([255, 200, 140])),
        p("color2", "Arms", Kind::Color([60, 80, 255])),
        p("twist", "Twist", Kind::Float(0.5)),
        SPEED,
    ], "A spiral galaxy slowly turning, with twinkling stars."),
    soft!("heartbeat", "Heartbeat", "Ambient", Heartbeat, &[
        p("color", "Line color", Kind::Color([0, 255, 90])),
        p("color2", "Background", Kind::Color([0, 6, 2])),
        p("trail", "Trail length", Kind::Float(0.5)),
        SPEED,
    ], "A heart monitor line sweeps across; the keys thump on each beat."),
    soft!("daycycle", "Day & Night", "Ambient", DayCycle, &[
        p("realtime", "Follow the real time of day", Kind::Bool(false)),
        SPEED,
    ], "Sunrise, a sun crossing the sky, sunset, then moon and stars."),
    soft!("kaleidoscope", "Kaleidoscope", "Ambient", Kaleidoscope, &[
        p("palette", "Palette", Kind::Choice(color::PALETTE_NAMES, 0)),
        p("mirrors", "Mirrors", Kind::Choice(MIRRORS, 1)),
        SPEED,
    ], "Mirrored, flowing colors like a kaleidoscope."),
    soft!("rainfall", "Rainfall", "Ambient", Rainfall, &[
        p("color", "Rain color", Kind::Color([90, 170, 255])),
        p("color2", "Background", Kind::Color([0, 4, 14])),
        p("density", "How heavy", Kind::Float(0.4)),
        p("wind", "Wind", Kind::Float(0.5)),
        SPEED,
    ], "Raindrops fall down the keys and splash on the bottom row."),
    soft!("snowfall", "Snowfall", "Ambient", Snowfall, &[
        p("color", "Snow color", Kind::Color([230, 240, 255])),
        p("color2", "Sky color", Kind::Color([2, 6, 24])),
        p("density", "How much", Kind::Float(0.4)),
        SPEED,
    ], "Snowflakes drift down and pile up on the bottom row."),
    soft!("candle", "Candlelight", "Ambient", Candle, &[
        p("count", "Candles", Kind::Choice(CANDLE_COUNTS, 1)),
        p("size", "Glow size", Kind::Float(0.4)),
        SPEED,
    ], "Warm, flickering candlelight."),
    soft!("neon", "Neon Sign", "Ambient", Neon, &[
        p("color", "Frame", Kind::Color([255, 30, 120])),
        p("color2", "Inside", Kind::Color([0, 200, 255])),
        p("flicker", "Flicker", Kind::Float(0.4)),
    ], "A glowing neon frame that buzzes and flickers now and then."),
    soft!("helix", "DNA Helix", "Ambient", Helix, &[
        p("color", "Strand 1", Kind::Color([0, 230, 255])),
        p("color2", "Strand 2", Kind::Color([255, 60, 160])),
        p("color3", "Background", Kind::Color([0, 0, 0])),
        SPEED,
    ], "Two strands twist around each other as they scroll by."),
    soft!("bubbles", "Bubbles", "Ambient", Bubbles, &[
        p("color", "Bubble color", Kind::Color([80, 230, 255])),
        p("rainbow", "Colorful bubbles", Kind::Bool(false)),
        p("color2", "Water color", Kind::Color([0, 10, 30])),
        p("density", "How many", Kind::Float(0.4)),
        p("size", "Size", Kind::Float(0.4)),
        SPEED,
    ], "Bubbles rise from the bottom, wobble and pop."),
    soft!("glitch", "Glitch", "Ambient", Glitch, &[
        p("color", "Color 1", Kind::Color([0, 40, 120])),
        p("color2", "Color 2", Kind::Color([0, 140, 160])),
        p("intensity", "Glitchiness", Kind::Float(0.4)),
        SPEED,
    ], "Calm colors that keep breaking up like a bad video signal."),
    // games
    soft!("snake", "Snake", "Games", Snake, &[
        p("color", "Snake", Kind::Color([0, 255, 80])),
        p("rainbow", "Rainbow snake", Kind::Bool(false)),
        p("color2", "Food", C_RED),
        p("color3", "Background", Kind::Color([0, 0, 0])),
        SPEED,
    ], "Snake plays itself on your keys: hunts food, grows, starts over."),
    soft!("pong", "Pong", "Games", Pong, &[
        p("color", "Ball", Kind::Color([255, 255, 255])),
        p("color2", "Paddles", Kind::Color([0, 200, 255])),
        p("color3", "Background", Kind::Color([0, 0, 0])),
        p("skill", "Player skill", Kind::Float(0.5)),
        SPEED,
    ], "Two paddles play Pong across the keyboard. The edge flashes on a point."),
    soft!("life", "Game of Life", "Games", Life, &[
        p("color", "Cell color", Kind::Color([0, 255, 160])),
        p("rainbow", "Rainbow cells", Kind::Bool(true)),
        BG_BLACK,
        SPEED,
    ], "Conway's Game of Life: cells are born, live and die on your keys."),
    // reactive
    soft!("ripple", "Ripple", "Reactive", Ripple, &[
        p("color", "Ripple color", Kind::Color([0, 230, 255])),
        p("rainbow", "Rainbow ripples", Kind::Bool(true)),
        BG_BLACK,
        p("width", "Ring width", Kind::Float(0.3)),
        SPEED,
    ], "Every key you press sends out a ring of light.", true, false, never),
    soft!("reactive", "Key Glow", "Reactive", Reactive, &[
        p("color", "Glow color", Kind::Color([255, 255, 255])),
        p("rainbow", "Random colors", Kind::Bool(false)),
        BG_BLACK,
        SPEED,
    ], "Keys light up when pressed and fade out.", true, false, never),
    soft!("crosshair", "Crosshair", "Reactive", Crosshair, &[
        p("color", "Color", Kind::Color([255, 0, 120])),
        p("rainbow", "Random colors", Kind::Bool(true)),
        BG_BLACK,
        SPEED,
    ], "Lines shoot out along the row and column of each key.", true, false, never),
    soft!("heatmap", "Typing Heatmap", "Reactive", Heatmap, &[
        p("cool", "Cool down", Kind::Choice(COOL, 0)),
    ], "Keys you use most glow hottest. Remembers your typing even after a restart.", true, false, never),
    soft!("sparks", "Sparks", "Reactive", Sparks, &[
        p("color", "Spark color", Kind::Color([255, 170, 40])),
        p("rainbow", "Random colors", Kind::Bool(false)),
        BG_BLACK,
        SPEED,
    ], "Every key you press throws out a burst of sparks.", true, false, never),
    soft!("embers", "Burning Keys", "Reactive", Embers, &[
        p("idle", "Glow when idle", Kind::Float(0.3)),
        SPEED,
    ], "Keys catch fire when pressed and embers float up.", true, false, never),
    // audio
    soft!("audio_bars", "Audio Bars", "Audio", AudioBars, &[
        p("style", "Colors", Kind::Choice(BAR_STYLES, 0)),
        p("color", "Single color", Kind::Color([0, 230, 255])),
        SENS,
    ], "Equalizer bars from whatever your PC is playing.", false, true, never),
    soft!("bass_pulse", "Bass Pulse", "Audio", BassPulse, &[
        p("color", "Color", Kind::Color([255, 0, 80])),
        p("rainbow", "Shift colors", Kind::Bool(true)),
        SENS,
    ], "The whole keyboard thumps with the bass.", false, true, never),
    soft!("audio_wave", "Audio Wave", "Audio", AudioScroll, &[
        p("color", "Color", Kind::Color([0, 230, 255])),
        p("rainbow", "Green to red by volume", Kind::Bool(true)),
        SENS,
        SPEED,
    ], "Volume spreads out from the middle like a sound wave.", false, true, never),
    soft!("beat_ripple", "Beat Ripple", "Audio", BeatRipple, &[
        p("color", "Ripple color", Kind::Color([0, 230, 255])),
        p("rainbow", "Rainbow ripples", Kind::Bool(true)),
        BG_BLACK,
        p("width", "Ring width", Kind::Float(0.4)),
        SENS,
        SPEED,
    ], "Ripples burst from random keys on every beat.", false, true, never),
    soft!("audio_fire", "Sound Fire", "Audio", AudioFire, &[SENS], "Flames that jump with the music, one column per pitch.", false, true, never),
    soft!("audio_rings", "Sound Rings", "Audio", AudioRings, &[
        p("color", "Color", Kind::Color([0, 230, 255])),
        p("rainbow", "Rainbow", Kind::Bool(true)),
        SENS,
    ], "Rings around the center: bass in the middle, treble at the edges.", false, true, never),
    // screen
    soft!("screen_grid", "Screen Grid", "Screen", ScreenGrid, &[
        p("area", "Area", Kind::Choice(SCREEN_AREAS, 0)),
        p("saturation", "Color boost", Kind::Float(0.4)),
        p("smooth", "Smoothing", Kind::Float(0.3)),
        p("mirror", "Mirror left/right", Kind::Bool(false)),
    ], "Each key shows the color of its part of your screen, like a tiny copy of it.", false, false, never, true),
    // static
    soft!("solid", "Solid Color", "Static", Solid, &[
        p("color", "Color", Kind::Color([0, 200, 255])),
    ], "One color. Saved on the keyboard, uses no CPU.", false, false, always),
    soft!("custom", "Custom Keys", "Static", CustomKeys, &[
        p("color", "Other keys", Kind::Color([10, 10, 20])),
    ], "Paint keys yourself: click keys on the picture. Saved on the keyboard.", false, false, always),
    // onboard (firmware) effects, IDs from the USB capture
    onboard!("hw_currogated", "Corrugated", 0x01, "rainbow", "Built into the keyboard."),
    onboard!("hw_cloud", "Cloud", 0x02, "plasma", "Built into the keyboard."),
    onboard!("hw_serpentine", "Serpentine", 0x03, "rainbow", "Built into the keyboard."),
    onboard!("hw_spectrum", "Spectrum", 0x04, "spectrum", "Built into the keyboard."),
    onboard!("hw_breath", "Breath", 0x05, "breathing", "Built into the keyboard."),
    onboard!("hw_static", "Light On", 0x06, "solid", "Built into the keyboard."),
    onboard!("hw_keypress", "Keypress Reaction", 0x07, "reactive", "Built into the keyboard."),
    onboard!("hw_ripples", "Ripples", 0x08, "ripple", "Built into the keyboard."),
    onboard!("hw_traverse", "Traverse", 0x09, "crosshair", "Built into the keyboard."),
    onboard!("hw_stars", "Stars", 0x0A, "twinkle", "Built into the keyboard."),
    onboard!("hw_fireworks", "Fireworks", 0x0B, "ripple", "Built into the keyboard."),
    onboard!("hw_roll", "Roll", 0x0C, "rainbow", "Built into the keyboard."),
    onboard!("hw_wavebar", "Wave Bar", 0x0D, "rainbow", "Built into the keyboard."),
    onboard!("hw_cartoon", "Cartoon", 0x0E, "plasma", "Built into the keyboard."),
    onboard!("hw_rain", "Rain", 0x0F, "matrix", "Built into the keyboard."),
    onboard!("hw_scan", "Scan", 0x10, "rainbow", "Built into the keyboard."),
    onboard!("hw_surmount", "Surmount", 0x11, "rainbow", "Built into the keyboard."),
    onboard!("hw_speed", "Speed", 0x12, "rainbow", "Built into the keyboard."),
];

#[cfg(test)]
mod tests {
    use super::*;

    /// Every effect runs for a while without panicking and gives sane colors.
    #[test]
    fn all_effects_render() {
        let audio = AudioFrame { bands: [0.6; 16], level: 0.6, bass: 0.8 };
        let keycolors = vec![None; KEYS.len()];
        let screen: Vec<Rgb> = (0..crate::screen::GW * crate::screen::GH)
            .map(|i| color::hsv(i as f32 / 97.0, 0.7, 0.8))
            .collect();
        let map = Map::new();
        for info in all() {
            let mut fx = (info.make)();
            let p = Params { defs: info.params, map: &map };
            let mut out = vec![[0.0; 3]; KEYS.len()];
            let mut lit = false;
            for f in 0..600 {
                let t = f as f32 / 60.0;
                let c = Ctx { t, dt: 1.0 / 60.0, audio: &audio, keycolors: &keycolors, screen: &screen };
                if f % 20 == 0 {
                    fx.key(f % KEYS.len(), t);
                }
                fx.render(&c, &p, &mut out);
                for o in &out {
                    assert!(o.iter().all(|v| v.is_finite()), "{} gave a bad color", info.id);
                }
                lit |= out.iter().any(|o| o.iter().any(|v| *v > 0.05));
            }
            assert!(lit || matches!(info.mode, Mode::Onboard(_)), "{} never lit up", info.id);
        }
    }
}
