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

pub const CATEGORIES: &[&str] = &["Ambient", "Reactive", "Audio", "Static", "On keyboard"];

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

#[derive(Default)]
struct Heatmap {
    count: Vec<f32>,
}
const HEAT: &[[u8; 3]] = &[[0, 0, 40], [0, 60, 255], [0, 220, 200], [40, 255, 0], [255, 230, 0], [255, 0, 0]];
impl Effect for Heatmap {
    fn key(&mut self, i: usize, _t: f32) {
        ensure(&mut self.count, KEYS.len());
        self.count[i] += 1.0;
    }
    fn render(&mut self, c: &Ctx, p: &Params, out: &mut [Rgb]) {
        ensure(&mut self.count, out.len());
        // "memory" 0..1 -> keys cool down over ~5 s .. ~5 min
        let half_life = 5.0 * 60f32.powf(p.f("memory"));
        let decay = 0.5f32.powf(c.dt / half_life);
        let max = self.count.iter().cloned().fold(4.0, f32::max);
        for (o, n) in out.iter_mut().zip(self.count.iter_mut()) {
            *n *= decay;
            *o = color::gradient(HEAT, *n / max, false);
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
        EffectInfo {
            id: $id,
            name: $name,
            category: $cat,
            desc: $desc,
            params: $params,
            mode: Mode::Software,
            uses_keys: $keys,
            uses_audio: $audio,
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
        p("memory", "Memory", Kind::Float(0.5)),
    ], "Keys you use most glow hottest, then cool down.", true, false, never),
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
