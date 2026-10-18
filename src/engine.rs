//! The lighting engine.
//!
//! `Renderer` turns a profile into key colors (used by both the service and the
//! settings window preview). `run` is the service loop that talks to the keyboard.
//!
//! To keep CPU use near zero it only does work when it has to:
//! - onboard effects: sent once, the keyboard animates by itself
//! - static effects (solid, custom keys): saved on the keyboard once, then idle
//! - animated effects: 60 fps, but a frame is only sent if it changed

use crate::audio::AudioCapture;
use crate::color::{self, Rgb};
use crate::config::{Config, Profile};
use crate::effects::{self, AudioFrame, Ctx, Effect, EffectInfo, Mode, Params};
use crate::layout::{self, KEYS};
use crate::protocol::{Frame, Keyboard, SLOTS};
use hidapi::{HidApi, HidResult};
use serde_json::{Map, Value};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Renderer {
    pub info: &'static EffectInfo,
    effect: Box<dyn Effect>,
    params: Map<String, Value>,
    keycolors: Vec<Option<[u8; 3]>>,
    pub brightness: f32,
    start: Instant,
    last: f32,
    pub out: Vec<Rgb>,
}

impl Renderer {
    pub fn new() -> Self {
        let info = effects::find("rainbow");
        Renderer {
            info,
            effect: (info.make)(),
            params: Map::new(),
            keycolors: vec![None; KEYS.len()],
            brightness: 1.0,
            start: Instant::now(),
            last: 0.0,
            out: vec![[0.0; 3]; KEYS.len()],
        }
    }

    /// Load a profile. `preview` swaps onboard effects for a look-alike software effect.
    pub fn configure(&mut self, profile: &Profile, preview: bool) {
        let mut info = effects::find(&profile.effect);
        if preview {
            if let Some(alt) = info.preview_as {
                info = effects::find(alt);
            }
        }
        if !std::ptr::eq(info, self.info) {
            self.info = info;
            self.effect = (info.make)();
        }
        self.params = profile.params_for(&profile.effect);
        self.brightness = profile.brightness.clamp(0.0, 1.0);
        for (i, k) in KEYS.iter().enumerate() {
            self.keycolors[i] = profile.keycolors.get(k.id).and_then(|h| color::parse_hex(h));
        }
    }

    pub fn params(&self) -> Params<'_> {
        Params { defs: self.info.params, map: &self.params }
    }

    fn now(&self) -> f32 {
        // wrap so f32 keeps enough precision when running for days
        (self.start.elapsed().as_secs_f64() % 3600.0) as f32
    }

    pub fn key(&mut self, i: usize) {
        let t = self.now();
        self.effect.key(i, t);
    }

    pub fn render(&mut self, audio: &AudioFrame) {
        let t = self.now();
        let dt = if t >= self.last { (t - self.last).min(0.1) } else { 0.016 };
        self.last = t;
        let ctx = Ctx { t, dt, audio, keycolors: &self.keycolors };
        let p = Params { defs: self.info.params, map: &self.params };
        self.effect.render(&ctx, &p, &mut self.out);
    }

    /// Colors per key, with brightness applied (0..255).
    pub fn key_colors(&self) -> Vec<[u8; 3]> {
        self.out.iter().map(|c| color::to_u8(color::scale(*c, self.brightness))).collect()
    }

    /// Colors per LED slot, ready to send.
    pub fn frame(&self) -> Frame {
        let mut f = [[0u8; 3]; SLOTS];
        for (k, c) in KEYS.iter().zip(self.key_colors()) {
            f[k.slot as usize] = c;
        }
        f
    }
}

pub struct Shared {
    pub cfg: Mutex<Config>,
    pub version: AtomicU64,
    pub stop: AtomicBool,
}

#[derive(PartialEq)]
enum Applied {
    Nothing,
    Off,
    Onboard(u8, u8, u8, [u8; 3]),
    Custom(Box<Frame>),
    Live(Option<Box<Frame>>),
}

fn level(x: f32) -> u8 {
    (x.clamp(0.0, 1.0) * 4.0).round() as u8
}

const FRAME: Duration = Duration::from_micros(16_667);

/// In live mode the keyboard turns all LEDs off if it gets no frame for about a
/// second, so an unchanged frame is still re-sent this often.
const KEEPALIVE: Duration = Duration::from_millis(300);

struct Loop {
    applied: Applied,
    last_custom: Instant,
    last_live: Instant,
}

impl Loop {
    /// One step. Returns how long to wait before the next one.
    fn step(&mut self, k: &Keyboard, cfg: &Config, r: &mut Renderer, af: &AudioFrame) -> HidResult<Duration> {
        if cfg.paused {
            if self.applied != Applied::Off {
                k.save_custom(&[[0; 3]; SLOTS])?;
                self.applied = Applied::Off;
            }
            return Ok(Duration::from_millis(100));
        }
        match r.info.mode {
            Mode::Onboard(id) => {
                let p = r.params();
                let want = Applied::Onboard(id, level(r.brightness), level(p.f("speed")), p.rgb8("color"));
                if self.applied != want {
                    let Applied::Onboard(id, b, s, c) = want else { unreachable!() };
                    k.set_mode(id, b, s, c)?;
                    self.applied = Applied::Onboard(id, b, s, c);
                }
                Ok(Duration::from_millis(100))
            }
            Mode::Software if (r.info.is_static)(&r.params()) => {
                r.render(af);
                let f = Box::new(r.frame());
                if self.applied != Applied::Custom(f.clone()) {
                    // don't rewrite the keyboard's memory too often while a slider is dragged
                    if self.last_custom.elapsed() < Duration::from_millis(600) {
                        return Ok(Duration::from_millis(50));
                    }
                    k.save_custom(&f)?;
                    self.applied = Applied::Custom(f);
                    self.last_custom = Instant::now();
                }
                Ok(Duration::from_millis(100))
            }
            Mode::Software => {
                r.render(af);
                let f = Box::new(r.frame());
                if !matches!(self.applied, Applied::Live(_)) {
                    k.start_live()?;
                    self.applied = Applied::Live(None);
                }
                if self.applied != Applied::Live(Some(f.clone())) || self.last_live.elapsed() >= KEEPALIVE {
                    k.send_live(&f)?;
                    self.applied = Applied::Live(Some(f));
                    self.last_live = Instant::now();
                }
                Ok(FRAME)
            }
        }
    }

    /// What the keyboard should show after the service stops.
    fn exit(&self, k: &Keyboard, cfg: &Config, r: &Renderer) {
        let _ = match cfg.exit_mode.as_str() {
            "keep" => match &self.applied {
                Applied::Live(Some(f)) => k.save_custom(f),
                _ => Ok(()),
            },
            "off" => k.save_custom(&[[0; 3]; SLOTS]),
            id => match effects::find(id).mode {
                Mode::Onboard(fw) => k.set_mode(fw, level(r.brightness), 2, [255, 0, 0]),
                _ => Ok(()),
            },
        };
    }
}

pub fn run(shared: Arc<Shared>, keys: Receiver<u16>) {
    let mut api = HidApi::new().ok();
    let mut kb: Option<Keyboard> = None;
    let mut next_try = Instant::now();
    let mut seen = u64::MAX;
    let mut cfg = Config::default();
    let mut r = Renderer::new();
    let mut lp = Loop {
        applied: Applied::Nothing,
        last_custom: Instant::now() - Duration::from_secs(5),
        last_live: Instant::now(),
    };
    let mut audio: Option<AudioCapture> = None;
    let mut audio_retry = Instant::now();
    let mut next_frame = Instant::now();

    while !shared.stop.load(Ordering::Relaxed) {
        let v = shared.version.load(Ordering::Relaxed);
        if v != seen {
            seen = v;
            cfg = shared.cfg.lock().map(|c| c.clone()).unwrap_or_default();
            r.configure(cfg.profile(), false);
        }

        if kb.is_none() && Instant::now() >= next_try {
            next_try = Instant::now() + Duration::from_secs(2);
            if api.is_none() {
                api = HidApi::new().ok();
            }
            if let Some(a) = api.as_mut() {
                kb = Keyboard::open(a);
                lp.applied = Applied::Nothing;
            }
        }

        // audio capture only while an audio effect is showing
        let want_audio = r.info.uses_audio && !cfg.paused && kb.is_some();
        if !want_audio {
            audio = None;
        } else if audio.is_none() && Instant::now() >= audio_retry {
            audio = AudioCapture::start();
            audio_retry = Instant::now() + Duration::from_secs(3);
        }
        let af = audio.as_ref().map(|a| a.frame()).unwrap_or_default();

        while let Ok(vk) = keys.try_recv() {
            if let Some(i) = layout::index_for_vk(vk) {
                r.key(i);
            }
        }

        let wait = match kb.as_ref() {
            Some(k) => match lp.step(k, &cfg, &mut r, &af) {
                Ok(d) => d,
                Err(_) => {
                    // unplugged or busy: retry later
                    kb = None;
                    lp.applied = Applied::Nothing;
                    Duration::from_millis(200)
                }
            },
            None => Duration::from_millis(200),
        };

        // steady frame pacing for animations
        next_frame += wait;
        let now = Instant::now();
        if next_frame < now {
            next_frame = now;
        }
        std::thread::sleep(next_frame - now);
    }

    if let Some(k) = kb.as_ref() {
        lp.exit(k, &cfg, &r);
    }
}
