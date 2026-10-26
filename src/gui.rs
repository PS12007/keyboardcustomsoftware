//! Settings window. A separate process that only runs while it's open; it edits
//! config.json and the background service picks the changes up.

use crate::audio::AudioCapture;
use crate::color::{self, Rgb, Rng};
use crate::config::{self, Config, Preset, Profile};
use crate::daemon;
use crate::effects::{self, AudioFrame, Ctx, Effect, EffectInfo, Kind, Mode, Params, CATEGORIES};
use crate::engine::Renderer;
use crate::fnkeys;
use crate::icon;
use crate::layout::{self, KEYS};
use crate::protocol::Keyboard;
use crate::sys::{self, InstanceLock, APP_TITLE, DAEMON_MUTEX, GUI_MUTEX, VENDOR_EXE};
use eframe::egui::{
    self, vec2, Align, Align2, Color32, CornerRadius, FontFamily, FontId, Layout, Pos2, Rect, RichText, Sense, Stroke,
    StrokeKind, Vec2,
};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

// Monochrome: near-black surfaces, white type, greys for everything secondary.
// The only color in the window comes from the lights themselves.
const BG: Color32 = Color32::from_rgb(8, 8, 9);
const SIDE: Color32 = Color32::from_rgb(12, 12, 13);
const RAISED: Color32 = Color32::from_rgb(22, 22, 24);
const RAISED_HI: Color32 = Color32::from_rgb(31, 31, 34);
const LINE: Color32 = Color32::from_rgb(33, 33, 36);
const TEXT: Color32 = Color32::from_rgb(240, 240, 242);
const SUB: Color32 = Color32::from_rgb(168, 168, 174);
const DIM: Color32 = Color32::from_rgb(112, 112, 118);
const PAPER: Color32 = Color32::from_rgb(238, 238, 241);
const INK: Color32 = Color32::from_rgb(10, 10, 12);
const GOOD: Color32 = Color32::from_rgb(48, 209, 88);
const WARN: Color32 = Color32::from_rgb(255, 159, 10);

/// Glyphs from Windows' own icon font (Segoe Fluent Icons / MDL2 Assets).
mod ic {
    pub const EFFECTS: &str = "\u{E8A9}";
    pub const ADJUST: &str = "\u{E9E9}";
    pub const THEMES: &str = "\u{E790}";
    pub const ROSTER: &str = "\u{E8FD}";
    pub const KEYBOARD: &str = "\u{E765}";
    pub const SETTINGS: &str = "\u{E713}";
    pub const SEARCH: &str = "\u{E721}";
    pub const STAR: &str = "\u{E734}";
    pub const STAR_FILL: &str = "\u{E735}";
    pub const SHUFFLE: &str = "\u{E8B1}";
    pub const PAUSE: &str = "\u{E769}";
    pub const PLAY: &str = "\u{E768}";
    pub const NEXT: &str = "\u{E76C}";
    pub const PREV: &str = "\u{E76B}";
    pub const ADD: &str = "\u{E710}";
    pub const MUSIC: &str = "\u{E8D6}";
    pub const SCREEN: &str = "\u{E7F4}";
    pub const SUN: &str = "\u{E706}";
    pub const SPEED: &str = "\u{EC4A}";
    pub const UP: &str = "\u{E70E}";
    pub const DOWN: &str = "\u{E70D}";
    pub const CLOSE: &str = "\u{E711}";
    pub const REFRESH: &str = "\u{E72C}";
    pub const BRUSH: &str = "\u{E771}";
    pub const CHIP: &str = "\u{E950}";
}

const SWATCHES: [[u8; 3]; 16] = [
    [255, 255, 255], [255, 30, 30], [255, 110, 0], [255, 200, 0], [190, 255, 0], [0, 230, 80], [0, 230, 200], [0, 170, 255],
    [30, 70, 255], [120, 40, 255], [200, 0, 255], [255, 0, 170], [255, 60, 110], [255, 190, 140], [140, 220, 255], [0, 0, 0],
];

/// Two-color looks for the Adjust page: (name, main, second).
const PAIRS: [(&str, [u8; 3], [u8; 3]); 12] = [
    ("Neon", [0, 230, 255], [255, 0, 200]),
    ("Sunset", [255, 110, 0], [130, 0, 255]),
    ("Arctic", [255, 255, 255], [60, 160, 255]),
    ("Ember", [255, 40, 0], [255, 190, 0]),
    ("Aurora", [0, 255, 140], [130, 0, 255]),
    ("Lagoon", [0, 90, 255], [0, 255, 210]),
    ("Rose", [255, 0, 90], [255, 170, 200]),
    ("Acid", [190, 255, 0], [0, 255, 120]),
    ("Royal", [120, 40, 255], [255, 200, 0]),
    ("Mono", [255, 255, 255], [90, 90, 96]),
    ("Toxic", [0, 255, 60], [255, 0, 160]),
    ("Classic", [255, 0, 0], [0, 40, 255]),
];

pub fn run() {
    // KCS_SHOT=<folder>: take screenshots of each page and quit (for development).
    // It never saves settings and doesn't touch a window that's already open.
    let shot = std::env::var_os("KCS_SHOT").map(PathBuf::from);
    let lock = if shot.is_some() {
        None
    } else {
        let Some(lock) = InstanceLock::acquire(GUI_MUTEX) else {
            sys::focus_window(APP_TITLE);
            return;
        };
        if !sys::instance_running(DAEMON_MUTEX) {
            sys::spawn_self(&["--background"]);
        }
        Some(lock)
    };
    let icon = egui::IconData { rgba: icon::rgba(64), width: 64, height: 64 };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_inner_size([1280.0, 860.0])
            .with_min_inner_size([980.0, 660.0])
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    let _ = eframe::run_native(APP_TITLE, opts, Box::new(move |cc| Ok(Box::new(App::new(cc, shot)))));
    drop(lock);
}

#[derive(PartialEq, Clone, Copy, Debug)]
enum Page {
    Effects,
    Adjust,
    Themes,
    Roster,
    Shortcuts,
    Settings,
}

const PAGES: [(Page, &str, &str); 6] = [
    (Page::Effects, "Effects", ic::EFFECTS),
    (Page::Adjust, "Adjust", ic::ADJUST),
    (Page::Themes, "Themes", ic::THEMES),
    (Page::Roster, "Roster", ic::ROSTER),
    (Page::Shortcuts, "Shortcuts", ic::KEYBOARD),
    (Page::Settings, "Settings", ic::SETTINGS),
];

/// Waiting for the user to press Fn + a key (shortcut index to change, or None = new).
struct Recording {
    slot: Option<usize>,
    _listener: fnkeys::Listener,
    rx: std::sync::mpsc::Receiver<&'static str>,
}

struct Status {
    service: bool,
    keyboard: bool,
    vendor: bool,
    autostart: bool,
}

/// Development screenshots, one per page.
struct Shot {
    dir: PathBuf,
    step: usize,
    at: Instant,
    done: Arc<AtomicBool>,
    waiting: bool,
}

/// A small live preview of one effect (gallery, theme cards, profile list).
struct Thumb {
    info: &'static EffectInfo,
    fx: Box<dyn Effect>,
    params: Map<String, Value>,
    keycolors: Vec<Option<[u8; 3]>>,
    out: Vec<Rgb>,
    last: f32,
    next_key: f32,
    presses: usize,
    seen: u64,
}

impl Thumb {
    fn new(effect: &str, params: Map<String, Value>, keycolors: Vec<Option<[u8; 3]>>) -> Self {
        let mut info = effects::find(effect);
        let mut params = params;
        if let Some(alt) = info.preview_as {
            info = effects::find(alt);
            params = Map::new();
        }
        Thumb {
            info,
            fx: (info.make)(),
            params,
            keycolors,
            out: vec![[0.0; 3]; KEYS.len()],
            last: -1.0,
            next_key: 0.0,
            presses: 0,
            seen: 0,
        }
    }

    fn step(&mut self, t: f32, audio: &AudioFrame, screen: &[Rgb]) {
        // ~40 fps is plenty for a thumbnail
        if t >= self.last && t - self.last < 0.025 {
            return;
        }
        let dt = if self.last < 0.0 || t < self.last { 0.016 } else { (t - self.last).min(0.1) };
        self.last = t;
        if self.info.uses_keys && t >= self.next_key {
            // pretend someone is typing, in little bursts
            self.presses += 1;
            self.fx.key((self.presses * 37 + 11) % KEYS.len(), t);
            self.next_key = t + if self.presses % 7 < 4 { 0.16 } else { 0.55 };
        }
        let p = Params { defs: self.info.params, map: &self.params };
        let screen = if self.info.uses_screen { screen } else { &[] };
        let c = Ctx { t, dt, audio, keycolors: &self.keycolors, screen };
        self.fx.render(&c, &p, &mut self.out);
    }
}

struct App {
    cfg: Config,
    saved_mtime: Option<SystemTime>,
    dirty: Option<Instant>,
    checked_file: Instant,
    r: Renderer,
    audio: Option<AudioCapture>,
    audio_retry: Instant,
    presets: Vec<Preset>,
    page: Page,
    cat: usize,
    search: String,
    brush: [u8; 3],
    renaming: Option<(usize, String)>,
    status: Status,
    status_at: Instant,
    hid: Option<hidapi::HidApi>,
    logo: egui::TextureHandle,
    thumbs: HashMap<String, Thumb>,
    frame: u64,
    start: Instant,
    fake_audio: AudioFrame,
    fake_screen: Vec<Rgb>,
    rng: Rng,
    recording: Option<Recording>,
    /// card under the mouse this frame, and the one being previewed on the big keyboard
    hover_card: Option<String>,
    peek: Option<(String, Instant)>,
    /// what the big keyboard showed last frame, and a snapshot to fade from after a switch
    shown: Vec<Rgb>,
    source: String,
    fade: Option<(Vec<Rgb>, Instant)>,
    toast: Option<(String, Instant)>,
    shot: Option<Shot>,
}

impl App {
    fn new(cc: &eframe::CreationContext, shot: Option<PathBuf>) -> Self {
        style(&cc.egui_ctx);
        let n = 96;
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([n, n], &icon::rgba(n as u32)),
            egui::TextureOptions::LINEAR,
        );
        let shot = shot.map(|dir| {
            let _ = std::fs::create_dir_all(&dir);
            Shot { dir, step: 0, at: Instant::now(), done: Arc::new(AtomicBool::new(false)), waiting: false }
        });
        let mut app = App {
            cfg: config::load(),
            saved_mtime: config::mtime(),
            dirty: None,
            checked_file: Instant::now(),
            r: Renderer::new(),
            audio: None,
            audio_retry: Instant::now(),
            presets: config::presets(),
            page: Page::Effects,
            cat: 0,
            search: String::new(),
            brush: [255, 0, 60],
            renaming: None,
            status: Status { service: true, keyboard: true, vendor: false, autostart: false },
            status_at: Instant::now() - Duration::from_secs(10),
            hid: hidapi::HidApi::new().ok(),
            logo,
            thumbs: HashMap::new(),
            frame: 0,
            start: Instant::now(),
            fake_audio: AudioFrame::default(),
            fake_screen: Vec::new(),
            rng: Rng::default(),
            recording: None,
            hover_card: None,
            peek: None,
            shown: Vec::new(),
            source: String::new(),
            fade: None,
            toast: None,
            shot,
        };
        app.refresh_status();
        app
    }

    fn changed(&mut self) {
        if self.shot.is_none() {
            self.dirty = Some(Instant::now());
        }
    }

    fn toast(&mut self, s: impl Into<String>) {
        self.toast = Some((s.into(), Instant::now()));
    }

    fn clock(&self) -> f32 {
        (self.start.elapsed().as_secs_f64() % 3600.0) as f32
    }

    fn refresh_status(&mut self) {
        self.status = Status {
            service: sys::instance_running(DAEMON_MUTEX),
            keyboard: self.hid.as_mut().map(Keyboard::present).unwrap_or(false),
            vendor: sys::process_running(VENDOR_EXE),
            autostart: sys::autostart_enabled(),
        };
        self.status_at = Instant::now();
    }

    /// Save shortly after the last change; reload if the service changed the file (tray menu).
    fn sync_file(&mut self) {
        if let Some(t) = self.dirty {
            if t.elapsed() > Duration::from_millis(250) {
                let _ = config::save(&self.cfg);
                self.saved_mtime = config::mtime();
                self.dirty = None;
            }
        } else if self.checked_file.elapsed() > Duration::from_secs(1) {
            self.checked_file = Instant::now();
            let m = config::mtime();
            if m != self.saved_mtime {
                self.saved_mtime = m;
                self.cfg = config::load();
            }
        }
    }

    fn update_preview(&mut self, ctx: &egui::Context) {
        self.r.configure(self.cfg.profile(), true);
        if self.r.info.uses_audio {
            if self.audio.is_none() && Instant::now() >= self.audio_retry {
                self.audio = AudioCapture::start();
                self.audio_retry = Instant::now() + Duration::from_secs(3);
            }
        } else {
            self.audio = None;
        }
        // keys typed while this window is focused drive the preview (not while typing in a text box)
        if !ctx.egui_wants_keyboard_input() {
            let pressed: Vec<usize> = ctx.input(|i| {
                i.events
                    .iter()
                    .filter_map(|e| match e {
                        egui::Event::Key { key, pressed: true, repeat: false, .. } => egui_key_index(*key),
                        _ => None,
                    })
                    .collect()
            });
            for i in pressed {
                self.r.key(i);
            }
        }
        let af = self.audio.as_ref().map(|a| a.frame()).unwrap_or_default();
        self.r.render(&af);
    }

    /// Made-up music and screen picture so audio/screen thumbnails have something to show.
    fn update_fakes(&mut self) {
        let t = self.clock();
        let beat = (t * 2.0).fract();
        let kick = (1.0 - beat).powi(4);
        let mut bands = [0.0; 16];
        for (i, b) in bands.iter_mut().enumerate() {
            let f = i as f32;
            let base = 0.35 + 0.2 * (t * 1.7 + f * 0.6).sin() + 0.15 * (t * 4.3 - f * 1.1).sin();
            *b = (base * (1.0 - f / 22.0) + kick * (1.0 - f / 6.0).max(0.0) * 0.6).max(0.0) / 1.4;
        }
        self.fake_audio = AudioFrame { bands, level: 0.25 + kick * 0.3, bass: 0.2 + kick * 0.7 };
        let (gw, gh) = (crate::screen::GW, crate::screen::GH);
        self.fake_screen = (0..gw * gh)
            .map(|i| {
                let (x, y) = ((i % gw) as f32 / gw as f32, (i / gw) as f32 / gh as f32);
                color::hsv(x * 0.6 + t * 0.05 + y * 0.2, 0.75, 0.9)
            })
            .collect();
    }

    /// Step a thumbnail (creating it if needed) and return its colors.
    fn thumb(&mut self, key: &str, make: impl FnOnce() -> Thumb) -> &Thumb {
        let t = self.clock();
        let frame = self.frame;
        let th = self.thumbs.entry(key.to_string()).or_insert_with(make);
        th.seen = frame;
        th.step(t, &self.fake_audio, &self.fake_screen);
        th
    }

    // ------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(18.0);
        ui.horizontal(|ui| {
            ui.add_space(4.0);
            let (rect, _) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::hover());
            ui.painter().image(self.logo.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            ui.add_space(6.0);
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.label(RichText::new("V400").family(display()).size(17.0).color(TEXT));
                ui.label(RichText::new("Lighting").size(11.5).color(DIM));
            });
        });
        ui.add_space(22.0);

        // navigation, with a highlight that slides to the current page
        let w = ui.available_width();
        let mut rects = Vec::new();
        for (page, _, _) in PAGES {
            let (rect, resp) = ui.allocate_exact_size(vec2(w, 36.0), Sense::click());
            rects.push((page, rect, resp));
        }
        let cur = rects.iter().find(|r| r.0 == self.page).map(|r| r.1.top()).unwrap_or(0.0);
        let y = ui.ctx().animate_value_with_time(egui::Id::unique("nav_y"), cur, 0.16);
        let p = ui.painter();
        p.rect_filled(Rect::from_min_size(Pos2::new(rects[0].1.left(), y), vec2(w, 36.0)), 9.0, RAISED_HI);
        let mut go = None;
        for ((page, name, glyph), (_, rect, resp)) in PAGES.iter().zip(&rects) {
            let on = *page == self.page;
            if resp.hovered() && !on {
                p.rect_filled(*rect, 9.0, RAISED);
            }
            let col = if on { TEXT } else if resp.hovered() { SUB } else { DIM };
            p.text(rect.left_center() + vec2(14.0, 0.0), Align2::LEFT_CENTER, *glyph, FontId::new(15.0, icons()), col);
            p.text(rect.left_center() + vec2(42.0, 0.0), Align2::LEFT_CENTER, *name, FontId::new(14.0, if on { semi() } else { FontFamily::Proportional }), col);
            if resp.clicked() {
                go = Some(*page);
            }
        }
        if let Some(pg) = go {
            self.page = pg;
        }

        ui.add_space(26.0);
        ui.horizontal(|ui| {
            ui.add_space(6.0);
            ui.label(caption("Profiles"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icon_button(ui, ic::ADD, 26.0).on_hover_text("New profile").clicked() {
                    let n = self.cfg.profiles.len() + 1;
                    self.cfg.profiles.push(Profile::new(&format!("Profile {n}"), "rainbow"));
                    self.cfg.select(self.cfg.profiles.len() - 1);
                    self.changed();
                    self.toast(format!("Created Profile {n}"));
                }
            });
        });
        ui.add_space(4.0);

        if let Some(name) = self.cfg.live.as_ref().map(|l| l.name.clone()) {
            self.profile_row(ui, "live", &name, true, true);
            ui.horizontal(|ui| {
                ui.add_space(6.0);
                if small_button(ui, "Keep").on_hover_text("Save this look as a new profile").clicked() {
                    self.cfg.keep_live();
                    self.changed();
                    self.toast("Saved as a new profile");
                }
                let back = format!("Back to {}", self.cfg.profiles[self.cfg.active].name);
                if small_button(ui, &back).clicked() {
                    let a = self.cfg.active;
                    self.cfg.select(a);
                    self.changed();
                }
            });
            ui.add_space(6.0);
        }

        let mut action: Option<(&str, usize)> = None;
        let list_h = (ui.available_height() - 64.0).max(80.0);
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, true]).show(ui, |ui| {
            for i in 0..self.cfg.profiles.len() {
                if let Some((ri, name)) = &mut self.renaming {
                    if *ri == i {
                        let resp = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY).margin(vec2(10.0, 9.0)));
                        resp.request_focus();
                        if resp.lost_focus() || ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                            action = Some(("rename_done", i));
                        }
                        continue;
                    }
                }
                let active = i == self.cfg.active && self.cfg.live.is_none();
                let name = self.cfg.profiles[i].name.clone();
                let resp = self.profile_row(ui, &format!("p{i}"), &name, active, false);
                if resp.clicked() {
                    action = Some(("select", i));
                }
                if resp.double_clicked() {
                    action = Some(("rename", i));
                }
                resp.context_menu(|ui| {
                    if ui.button("Rename").clicked() {
                        action = Some(("rename", i));
                        ui.close();
                    }
                    if ui.button("Duplicate").clicked() {
                        action = Some(("dup", i));
                        ui.close();
                    }
                    if ui.add_enabled(i > 0, egui::Button::new("Move up")).clicked() {
                        action = Some(("up", i));
                        ui.close();
                    }
                    if ui.button("Add to roster").clicked() {
                        action = Some(("roster", i));
                        ui.close();
                    }
                    ui.separator();
                    if ui.add_enabled(self.cfg.profiles.len() > 1, egui::Button::new("Delete")).clicked() {
                        action = Some(("delete", i));
                        ui.close();
                    }
                });
            }
        });

        if let Some((what, i)) = action {
            match what {
                "select" => self.cfg.select(i),
                "rename" => self.renaming = Some((i, self.cfg.profiles[i].name.clone())),
                "rename_done" => {
                    if let Some((_, name)) = self.renaming.take() {
                        let name = name.trim();
                        if !name.is_empty() {
                            // keep roster entries pointing at this profile
                            let (old, new) = (format!("p:{}", self.cfg.profiles[i].name), format!("p:{name}"));
                            for it in self.cfg.roster.items.iter_mut().filter(|it| **it == old) {
                                *it = new.clone();
                            }
                            self.cfg.profiles[i].name = name.to_string();
                        }
                    }
                }
                "dup" => {
                    let mut p = self.cfg.profiles[i].clone();
                    p.name = format!("{} copy", p.name);
                    self.cfg.profiles.insert(i + 1, p);
                    self.cfg.select(i + 1);
                }
                "up" if i > 0 => {
                    self.cfg.profiles.swap(i, i - 1);
                    if self.cfg.active == i {
                        self.cfg.active = i - 1;
                    } else if self.cfg.active == i - 1 {
                        self.cfg.active = i;
                    }
                }
                "roster" => {
                    let name = self.cfg.profiles[i].name.clone();
                    self.cfg.roster.items.push(format!("p:{name}"));
                    self.toast(format!("Added {name} to the roster"));
                }
                "delete" if self.cfg.profiles.len() > 1 => {
                    let name = self.cfg.profiles.remove(i).name;
                    if self.cfg.active >= i && self.cfg.active > 0 {
                        self.cfg.active -= 1;
                    }
                    self.toast(format!("Deleted {name}"));
                }
                _ => {}
            }
            self.changed();
        }

        // connection status pinned to the bottom
        ui.with_layout(Layout::bottom_up(Align::Min), |ui| {
            ui.add_space(16.0);
            let (ok, text) = if !self.status.keyboard {
                (false, "Keyboard not found")
            } else if !self.status.service {
                (false, "Service stopped")
            } else if self.cfg.paused {
                (true, "Paused")
            } else {
                (true, "Connected")
            };
            ui.horizontal(|ui| {
                ui.add_space(8.0);
                let (rect, _) = ui.allocate_exact_size(vec2(8.0, 16.0), Sense::hover());
                let col = if !ok { WARN } else if self.cfg.paused { DIM } else { GOOD };
                ui.painter().circle_filled(rect.center(), 3.5, col);
                ui.label(RichText::new(text).size(12.5).color(if ok { SUB } else { WARN }));
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(format!("v{}", env!("CARGO_PKG_VERSION"))).size(11.5).color(DIM));
                });
            });
        });
    }

    /// One profile in the sidebar: a tiny live picture of it, its name and effect.
    fn profile_row(&mut self, ui: &mut egui::Ui, id: &str, name: &str, active: bool, live: bool) -> egui::Response {
        let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let prof = if live { self.cfg.live.clone().unwrap() } else { self.cfg.profiles[id[1..].parse::<usize>().unwrap_or(0)].clone() };
        let key = format!("prof:{id}:{}", prof.effect);
        let params = prof.params_for(&prof.effect);
        let kc = profile_keycolors(&prof);
        let colors: Vec<Rgb> = {
            let th = self.thumb(&key, || Thumb::new(&prof.effect, params.clone(), kc.clone()));
            th.out.clone()
        };
        if let Some(th) = self.thumbs.get_mut(&key) {
            if th.info.preview_as.is_none() && th.params != params {
                th.params = params;
            }
            th.keycolors = kc;
        }
        let p = &ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let bg = if active { RAISED_HI } else if resp.hovered() { RAISED } else { Color32::TRANSPARENT };
        p.rect_filled(rect, 9.0, bg);
        let pic = Rect::from_min_size(rect.left_center() + vec2(10.0, -10.0), vec2(48.0, 20.0));
        mini_keyboard(p, pic, &colors, 4.0, false);
        let name_col = if active { TEXT } else { Color32::from_rgb(205, 205, 210) };
        p.text(rect.min + vec2(68.0, 7.0), Align2::LEFT_TOP, name, FontId::new(13.5, semi()), name_col);
        let sub = if live { "Now showing".to_string() } else { effects::find(&prof.effect).name.to_string() };
        p.text(rect.min + vec2(68.0, 25.0), Align2::LEFT_TOP, sub, FontId::proportional(11.5), DIM);
        resp
    }

    // ------------------------------------------------------------ header + hero

    fn top_bar(&mut self, ui: &mut egui::Ui) {
        let info = effects::find(&self.cfg.profile().effect);
        let peek_name = self.peek_target().map(|(_, t)| t);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                let (title, sub) = match &peek_name {
                    Some(t) => (t.clone(), "Preview · click to use it".to_string()),
                    None => (info.name.to_string(), format!("{}  ·  {}", self.cfg.profile().name, info.category)),
                };
                ui.label(RichText::new(title).family(display()).size(28.0).color(TEXT));
                ui.label(RichText::new(sub).size(13.0).color(DIM));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let paused = self.cfg.paused;
                let (g, l) = if paused { (ic::PLAY, "Resume") } else { (ic::PAUSE, "Pause") };
                if pill_button(ui, g, l, paused).clicked() {
                    self.cfg.paused = !paused;
                    self.changed();
                }
                let hint = shortcut_for(&self.cfg, "surprise");
                if pill_button(ui, ic::SHUFFLE, "Surprise me", false)
                    .on_hover_text(format!("A random effect with random colors{hint}"))
                    .clicked()
                {
                    self.cfg.surprise(&mut self.rng);
                    self.changed();
                }
                if self.status.vendor && pill_button(ui, ic::CLOSE, "Close Surmen app", false)
                    .on_hover_text("The Surmen app is open and fights over the lights")
                    .clicked()
                {
                    sys::kill_process(VENDOR_EXE);
                    self.status_at = Instant::now() - Duration::from_secs(10);
                }
                if !self.status.service && pill_button(ui, ic::PLAY, "Start service", false).clicked() {
                    sys::spawn_self(&["--background"]);
                    self.status_at = Instant::now() - Duration::from_secs(10);
                }
            });
        });
    }

    /// The card being previewed on the big keyboard: (thumb key, display name).
    fn peek_target(&self) -> Option<(String, String)> {
        let (key, since) = self.peek.as_ref()?;
        if since.elapsed() < Duration::from_millis(220) || !self.thumbs.contains_key(key) {
            return None;
        }
        let name = if let Some(id) = key.strip_prefix("fx:") {
            effects::find(id).name.to_string()
        } else if let Some(i) = key.strip_prefix("theme:").and_then(|s| s.parse::<usize>().ok()) {
            self.presets.get(i).map(|p| p.name.to_string()).unwrap_or_default()
        } else {
            return None;
        };
        Some((key.clone(), name))
    }

    /// Colors for the big keyboard: the live effect, or a hovered card, cross-faded on switches.
    fn hero_colors(&mut self) -> Vec<Rgb> {
        let bright = self.cfg.profile().brightness;
        let (src, mut cols): (String, Vec<Rgb>) = match self.peek_target() {
            Some((key, _)) => {
                let th = &self.thumbs[&key];
                (key, th.out.iter().map(|c| color::scale(*c, bright.max(0.4))).collect())
            }
            None => (
                format!("live:{}:{}", self.cfg.active, self.cfg.profile().effect),
                self.r.key_colors().iter().map(|c| color::from_u8(*c)).collect(),
            ),
        };
        if src != self.source {
            if !self.shown.is_empty() {
                self.fade = Some((self.shown.clone(), Instant::now()));
            }
            self.source = src;
        }
        if let Some((from, t0)) = &self.fade {
            let k = t0.elapsed().as_secs_f32() / 0.35;
            if k >= 1.0 || from.len() != cols.len() {
                self.fade = None;
            } else {
                let k = k * k * (3.0 - 2.0 * k);
                for (c, f) in cols.iter_mut().zip(from) {
                    *c = color::lerp(*f, *c, k);
                }
            }
        }
        if self.cfg.paused {
            for c in cols.iter_mut() {
                *c = color::scale(*c, 0.12);
            }
        }
        self.shown = cols.clone();
        cols
    }

    fn hero(&mut self, ui: &mut egui::Ui) {
        let colors = self.hero_colors();
        let avail = ui.available_width();
        // fit the width, but leave at least half the window for the page below
        let by_h = (ui.ctx().content_rect().height() * 0.47 - 30.0) / (layout::HEIGHT + 2.4);
        let unit = (avail.min(1040.0) / (layout::WIDTH + 1.4)).min(by_h).clamp(24.0, 58.0);
        let pad = 0.45;
        let case = vec2((layout::WIDTH + pad * 2.0) * unit, (layout::HEIGHT + pad * 2.0) * unit);
        let stage_h = case.y + unit * 1.5;
        let (resp, painter) = ui.allocate_painter(vec2(avail, stage_h), Sense::click_and_drag());
        let stage = resp.rect;
        let body = Rect::from_center_size(Pos2::new(stage.center().x, stage.top() + unit * 0.45 + case.y / 2.0), case);

        // light thrown on the wall behind and on the desk below (allowed to spill past the stage)
        let painter = painter.with_clip_rect(ui.clip_rect().expand2(vec2(0.0, 40.0)));
        let zones = zone_colors(&colors, 6);
        for (i, z) in zones.iter().enumerate() {
            let x = body.left() + body.width() * (i as f32 + 0.5) / zones.len() as f32;
            glow(&painter, Pos2::new(x, body.center().y), vec2(body.width() / 3.4, body.height() * 0.95), *z, 0.15);
            glow(&painter, Pos2::new(x, body.bottom() + unit * 0.42), vec2(body.width() / 5.0, unit * 0.62), *z, 0.4);
        }

        // case: soft shadow, slab, hairline edges
        let rad = unit * 0.42;
        painter.rect_filled(body.translate(vec2(0.0, unit * 0.12)).expand(2.0), rad, Color32::from_black_alpha(150));
        painter.rect_filled(body, rad, Color32::from_rgb(23, 23, 25));
        painter.rect_stroke(body, rad, Stroke::new(1.0, Color32::from_rgb(44, 44, 48)), StrokeKind::Inside);
        painter.line_segment(
            [body.left_top() + vec2(rad, 1.5), body.right_top() + vec2(-rad, 1.5)],
            Stroke::new(1.0, Color32::from_white_alpha(16)),
        );
        let origin = body.min + vec2(pad * unit, pad * unit);
        // the plate the keys sit on, where light leaks out
        let plate = Rect::from_min_size(origin, vec2(layout::WIDTH * unit, layout::HEIGHT * unit)).expand(unit * 0.08);
        painter.rect_filled(plate, unit * 0.14, Color32::from_rgb(9, 9, 10));

        let gap = unit * 0.055;
        let key_rect = |k: &layout::Key| {
            Rect::from_min_size(origin + vec2(k.x * unit + gap, k.y * unit + gap), vec2(k.w * unit - gap * 2.0, unit - gap * 2.0))
        };
        let pointer = resp.hover_pos();
        let mut hovered = None;
        for (i, k) in KEYS.iter().enumerate() {
            let rect = key_rect(k);
            let c = colors[i];
            let led = c32(c);
            let lum = (c[0] * 0.3 + c[1] * 0.59 + c[2] * 0.11).min(1.0);
            // light leaking out from under the cap, brightest right at the edge
            painter.rect_filled(rect.expand(unit * 0.13), unit * 0.24, led.gamma_multiply(0.16).additive());
            painter.rect_filled(rect.expand(unit * 0.06), unit * 0.17, led.gamma_multiply(0.3).additive());
            let cap = mix(Color32::from_rgb(27, 27, 30), led, 0.16 + 0.44 * lum);
            painter.rect_filled(rect, unit * 0.12, mix(cap, Color32::BLACK, 0.25));
            let top = rect.shrink2(vec2(unit * 0.075, unit * 0.065)).translate(vec2(0.0, -unit * 0.035));
            painter.rect_filled(top, unit * 0.1, mix(cap, led, 0.22));
            // shine-through legends
            let legend = if lum > 0.68 {
                Color32::from_rgb(20, 20, 22)
            } else {
                mix(Color32::from_rgb(150, 150, 156), mix(led, Color32::WHITE, 0.55), (lum * 4.0).min(1.0))
            };
            painter.text(top.center(), Align2::CENTER_CENTER, k.label, FontId::proportional(unit * 0.24), legend);
            if let Some(dir) = arrow_dir(k.id) {
                draw_arrow(&painter, top.center(), dir, unit * 0.12, legend);
            }
            if pointer.is_some_and(|p| rect.contains(p)) {
                hovered = Some(i);
                painter.rect_stroke(rect, unit * 0.12, Stroke::new(1.5, Color32::from_white_alpha(170)), StrokeKind::Outside);
            }
        }
        // knob: knurled ring and a pointer line
        let kc = origin + vec2((layout::KNOB.0 + 0.5) * unit, (layout::KNOB.1 + 0.5) * unit);
        let kr = unit * 0.44;
        painter.circle_filled(kc + vec2(0.0, unit * 0.05), kr, Color32::from_black_alpha(160));
        painter.circle_filled(kc, kr, Color32::from_rgb(40, 40, 44));
        for j in 0..36 {
            let a = j as f32 / 36.0 * std::f32::consts::TAU;
            let d = vec2(a.cos(), a.sin());
            painter.line_segment([kc + d * kr * 0.84, kc + d * kr * 0.98], Stroke::new(1.0, Color32::from_rgb(62, 62, 68)));
        }
        painter.circle_filled(kc, kr * 0.76, Color32::from_rgb(58, 58, 63));
        painter.circle_stroke(kc, kr * 0.76, Stroke::new(1.0, Color32::from_white_alpha(20)));
        painter.line_segment([kc - vec2(0.0, kr * 0.25), kc - vec2(0.0, kr * 0.62)], Stroke::new(2.0, Color32::from_rgb(200, 200, 206)));

        if self.cfg.paused {
            pill_label(&painter, body.center(), "Paused", 15.0);
        }

        // painting / testing reactive effects with the mouse
        let painting = self.cfg.profile().effect == "custom" && self.peek_target().is_none();
        if let Some(i) = hovered {
            if painting && (resp.clicked() || resp.dragged()) {
                let id = KEYS[i].id.to_string();
                let hex = color::to_hex(self.brush);
                let p = self.cfg.profile_mut();
                if p.keycolors.get(&id) != Some(&hex) {
                    p.keycolors.insert(id, hex);
                    self.changed();
                }
            } else if resp.clicked() {
                self.r.key(i);
            }
            if painting {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Crosshair);
            }
        }
    }

    /// Brightness, speed and effect stepping right under the keyboard.
    fn control_strip(&mut self, ui: &mut egui::Ui) {
        let id = self.cfg.profile().effect.clone();
        let info = effects::find(&id);
        let has_speed = info.params.iter().any(|d| d.key == "speed");
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            ui.label(RichText::new(ic::SUN).family(icons()).size(15.0).color(SUB)).on_hover_text("Brightness");
            let mut b = self.cfg.profile().brightness;
            if slider(ui, &mut b, 170.0).changed() {
                self.cfg.profile_mut().brightness = b;
                self.changed();
            }
            ui.label(RichText::new(format!("{:>3}%", (b * 100.0).round() as i32)).size(12.5).color(DIM).monospace());
            if has_speed {
                ui.add_space(22.0);
                ui.label(RichText::new(ic::SPEED).family(icons()).size(15.0).color(SUB)).on_hover_text("Speed");
                let mut map = self.cfg.profile().params_for(&id);
                let mut v = Params { defs: info.params, map: &map }.f("speed");
                if slider(ui, &mut v, 170.0).changed() {
                    map.insert("speed".into(), serde_json::json!(v));
                    self.cfg.profile_mut().params.insert(id.clone(), map);
                    self.changed();
                }
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let order: Vec<&'static str> = ordered_effects().iter().filter(|e| e.mode == Mode::Software).map(|e| e.id).collect();
                let pos = order.iter().position(|e| *e == id).unwrap_or(0);
                let mut step = None;
                if icon_button(ui, ic::NEXT, 32.0).on_hover_text("Next effect").clicked() {
                    step = Some((pos + 1) % order.len());
                }
                if icon_button(ui, ic::PREV, 32.0).on_hover_text("Previous effect").clicked() {
                    step = Some((pos + order.len() - 1) % order.len());
                }
                if let Some(j) = step {
                    let p = self.cfg.profile_mut();
                    p.effect = order[j].to_string();
                    p.ensure_seed();
                    self.changed();
                }
                if effects::find(&id).uses_keys {
                    ui.label(RichText::new("Type or click keys to try it").size(12.0).color(DIM));
                }
            });
        });
    }

    // ------------------------------------------------------------ cards

    /// A gallery card: live preview, title, subtitle. Returns (card clicked, star clicked).
    #[allow(clippy::too_many_arguments)]
    fn card(
        &mut self,
        ui: &mut egui::Ui,
        key: String,
        make: impl FnOnce() -> Thumb,
        w: f32,
        title: &str,
        sub: &str,
        selected: bool,
        badges: &[&str],
        fav: Option<bool>,
    ) -> (egui::Response, bool) {
        let preview_h = (w * layout::HEIGHT / layout::WIDTH + 14.0).round();
        let (rect, resp) = ui.allocate_exact_size(vec2(w, preview_h + 50.0), Sense::click());
        if !ui.is_rect_visible(rect) {
            return (resp, false);
        }
        let colors = self.thumb(&key, make).out.clone();
        // the pointer anywhere over the card counts, even over the star button on top of it
        let inside = resp.contains_pointer();
        if inside {
            self.hover_card = Some(key.clone());
        }
        let prev = Rect::from_min_size(rect.min, vec2(w, preview_h));
        let lift = ui.ctx().animate_bool_with_time(resp.id, inside, 0.12);
        let p = ui.painter();
        mini_keyboard(p, prev, &colors, 12.0, true);
        let edge = if selected { Stroke::new(1.5, PAPER) } else { Stroke::new(1.0, mix(Color32::from_rgb(30, 30, 33), Color32::from_rgb(80, 80, 86), lift)) };
        p.rect_stroke(prev, 12.0, edge, StrokeKind::Inside);
        if selected {
            let tag = Rect::from_min_size(prev.min + vec2(9.0, 9.0), vec2(34.0, 18.0));
            p.rect_filled(tag, 9.0, PAPER);
            p.text(tag.center(), Align2::CENTER_CENTER, "ON", FontId::new(10.0, semi()), INK);
        }
        // little icons for effects that need typing / music / the screen
        let mut bx = prev.right() - 10.0;
        for b in badges.iter().rev() {
            let c = Pos2::new(bx - 9.0, prev.bottom() - 17.0);
            p.circle_filled(c, 11.0, Color32::from_black_alpha(170));
            p.text(c, Align2::CENTER_CENTER, *b, FontId::new(11.0, icons()), SUB);
            bx -= 26.0;
        }
        let tcol = mix(Color32::from_rgb(220, 220, 225), TEXT, lift);
        p.text(rect.min + vec2(2.0, preview_h + 10.0), Align2::LEFT_TOP, title, FontId::new(14.0, semi()), tcol);
        p.text(rect.min + vec2(2.0, preview_h + 29.0), Align2::LEFT_TOP, sub, FontId::proportional(12.0), DIM);

        let mut star = false;
        if let Some(on) = fav {
            if on || inside || lift > 0.01 {
                let sr = Rect::from_center_size(Pos2::new(prev.right() - 20.0, prev.top() + 20.0), vec2(28.0, 28.0));
                let sresp = ui.interact(sr, resp.id.with("star"), Sense::click());
                let a = if on { 1.0 } else { lift };
                let p = ui.painter();
                p.circle_filled(sr.center(), 14.0, Color32::from_black_alpha((170.0 * a) as u8));
                let col = if on { PAPER } else if sresp.hovered() { TEXT } else { SUB };
                p.text(sr.center(), Align2::CENTER_CENTER, if on { ic::STAR_FILL } else { ic::STAR }, FontId::new(13.0, icons()), col.gamma_multiply(a));
                if sresp.clicked() {
                    star = true;
                }
            }
        }
        (resp, star)
    }

    // ------------------------------------------------------------ effects

    fn effects_page(&mut self, ui: &mut egui::Ui) {
        let mut tabs: Vec<String> = vec!["All".into(), "Saved".into()];
        tabs.extend(CATEGORIES.iter().map(|s| s.to_string()));
        ui.horizontal(|ui| {
            if let Some(i) = text_tabs(ui, &tabs, self.cat) {
                self.cat = i;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let n = effects::all().len();
                search_box(ui, &mut self.search, &format!("Search {n} effects"));
            });
        });
        ui.add_space(18.0);

        let q = self.search.trim().to_lowercase();
        let matches = |e: &EffectInfo| {
            q.is_empty() || e.name.to_lowercase().contains(&q) || e.desc.to_lowercase().contains(&q) || e.category.to_lowercase().contains(&q)
        };
        let favs = self.cfg.favorites.clone();
        let mut groups: Vec<(Option<&str>, Vec<&'static EffectInfo>)> = Vec::new();
        if !q.is_empty() {
            groups.push((None, effects::all().iter().filter(|e| matches(e)).collect()));
        } else if self.cat == 0 {
            if !favs.is_empty() {
                groups.push((Some("Saved"), favs.iter().map(|id| effects::find(id)).collect()));
            }
            for cat in CATEGORIES {
                groups.push((Some(cat), effects::all().iter().filter(|e| e.category == *cat).collect()));
            }
        } else if self.cat == 1 {
            groups.push((None, favs.iter().map(|id| effects::find(id)).collect()));
        } else {
            let cat = CATEGORIES[self.cat - 2];
            groups.push((None, effects::all().iter().filter(|e| e.category == cat).collect()));
        }

        let current = self.cfg.profile().effect.clone();
        let (cols, w) = grid_dims(ui.available_width(), 228.0, 18.0);
        let mut picked = None;
        let mut to_roster = None;
        let mut star = None;
        for (title, list) in groups {
            if let Some(t) = title {
                section_title(ui, t, list.len());
            }
            if list.is_empty() {
                let msg = if !q.is_empty() { "Nothing matches that search." } else { "Star an effect to keep it here." };
                ui.label(RichText::new(msg).color(DIM));
                ui.add_space(20.0);
            }
            for row in list.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 18.0;
                    for e in row {
                        let params = self.cfg.profile().params_for(e.id);
                        let keycolors = profile_keycolors(self.cfg.profile());
                        let mut badges = Vec::new();
                        if e.uses_keys {
                            badges.push(ic::KEYBOARD);
                        }
                        if e.uses_audio {
                            badges.push(ic::MUSIC);
                        }
                        if e.uses_screen {
                            badges.push(ic::SCREEN);
                        }
                        if matches!(e.mode, Mode::Onboard(_)) {
                            badges.push(ic::CHIP);
                        }
                        let key = format!("fx:{}", e.id);
                        let fav = favs.iter().any(|f| f == e.id);
                        let sub = short_desc(e.desc);
                        let (resp, st) = self.card(
                            ui,
                            key.clone(),
                            || Thumb::new(e.id, params.clone(), keycolors),
                            w,
                            e.name,
                            &sub,
                            e.id == current,
                            &badges,
                            Some(fav),
                        );
                        // keep the thumbnail in sync with the profile's settings for that effect
                        if let Some(th) = self.thumbs.get_mut(&key) {
                            if e.preview_as.is_none() && th.params != params {
                                th.params = params;
                            }
                        }
                        if st {
                            star = Some(e.id);
                        } else if resp.clicked() {
                            picked = Some(e.id);
                        }
                        resp.context_menu(|ui| {
                            if ui.button(if fav { "Remove from Saved" } else { "Save" }).clicked() {
                                star = Some(e.id);
                                ui.close();
                            }
                            if ui.button("Add to roster").clicked() {
                                to_roster = Some(e.id);
                                ui.close();
                            }
                        });
                    }
                });
                ui.add_space(20.0);
            }
            ui.add_space(14.0);
        }
        if let Some(id) = picked {
            if id != current {
                let p = self.cfg.profile_mut();
                p.effect = id.to_string();
                p.ensure_seed();
                self.changed();
            }
            self.peek = None;
        }
        if let Some(id) = star {
            let name = effects::find(id).name;
            if let Some(i) = self.cfg.favorites.iter().position(|f| f == id) {
                self.cfg.favorites.remove(i);
                self.toast(format!("Removed {name} from Saved"));
            } else {
                self.cfg.favorites.push(id.to_string());
                self.toast(format!("Saved {name}"));
            }
            self.changed();
        }
        if let Some(id) = to_roster {
            self.cfg.roster.items.push(format!("e:{id}"));
            self.changed();
            self.toast(format!("Added {} to the roster", effects::find(id).name));
        }
    }

    // ------------------------------------------------------------ adjust

    fn adjust_page(&mut self, ui: &mut egui::Ui) {
        let effect = self.cfg.profile().effect.clone();
        let info = effects::find(&effect);
        let wide = ui.available_width() > 860.0;
        let left_w = if wide { (ui.available_width() * 0.56).min(560.0) } else { ui.available_width() };

        ui.horizontal_top(|ui| {
            ui.allocate_ui_with_layout(vec2(left_w, 0.0), Layout::top_down(Align::Min), |ui| {
                ui.set_width(left_w);
                ui.label(RichText::new(info.name).family(display()).size(22.0).color(TEXT));
                ui.add_space(2.0);
                ui.label(RichText::new(info.desc).color(SUB));
                if matches!(info.mode, Mode::Onboard(_)) {
                    ui.label(RichText::new("Runs on the keyboard's own chip, so it's perfectly smooth and uses no CPU. The picture here is an approximation.").color(DIM).size(12.0));
                }
                ui.add_space(14.0);
                self.param_rows(ui, info, &effect);
                ui.add_space(14.0);
                ui.horizontal(|ui| {
                    if effects::needs_seed(&effect) {
                        let label = if effect == "dream" { "New design" } else { "Reshuffle" };
                        if pill_button(ui, ic::REFRESH, label, true).clicked() {
                            let mut map = self.cfg.profile().params_for(&effect);
                            map.insert("seed".into(), effects::new_seed().into());
                            self.cfg.profile_mut().params.insert(effect.clone(), map);
                            self.changed();
                        }
                    }
                    if info.params.iter().any(|d| matches!(d.kind, Kind::Color(_)))
                        && pill_button(ui, ic::SHUFFLE, "Random colors", false).clicked()
                    {
                        // fresh colors, keep everything else
                        let rnd = effects::random_params(info, &mut self.rng);
                        let mut map = self.cfg.profile().params_for(&effect);
                        for d in info.params.iter().filter(|d| matches!(d.kind, Kind::Color(_))) {
                            if let Some(v) = rnd.get(d.key) {
                                map.insert(d.key.into(), v.clone());
                            }
                        }
                        self.cfg.profile_mut().params.insert(effect.clone(), map);
                        self.changed();
                    }
                    if !info.params.is_empty() && pill_button(ui, ic::REFRESH, "Reset", false).on_hover_text("Back to the default settings").clicked() {
                        let seed = self.cfg.profile().params_for(&effect).get("seed").cloned();
                        let mut map = Map::new();
                        if let Some(s) = seed {
                            map.insert("seed".into(), s);
                        }
                        self.cfg.profile_mut().params.insert(effect.clone(), map);
                        self.changed();
                        self.toast("Settings reset");
                    }
                    if effect == "heatmap" && pill_button(ui, ic::REFRESH, "Reset heatmap", false).clicked() {
                        effects::heatmap_reset();
                        self.toast("Heatmap cleared");
                    }
                });
            });
            if wide {
                ui.add_space(40.0);
                ui.vertical(|ui| self.colors_panel(ui, info, &effect));
            }
        });
        if !wide {
            ui.add_space(24.0);
            self.colors_panel(ui, info, &effect);
        }
    }

    fn param_rows(&mut self, ui: &mut egui::Ui, info: &'static EffectInfo, effect: &str) {
        let mut map = self.cfg.profile().params_for(effect);
        let mut any = false;
        divider(ui);
        for def in info.params {
            let cur = Params { defs: info.params, map: &map };
            match def.kind {
                Kind::Color(_) => {
                    let mut c = cur.rgb8(def.key);
                    setting_row(ui, def.label, |ui| {
                        if color_swatch(ui, &mut c) {
                            map.insert(def.key.into(), color::to_hex(c).into());
                            any = true;
                        }
                    });
                }
                Kind::Float(_) => {
                    let mut v = cur.f(def.key);
                    setting_row(ui, def.label, |ui| {
                        ui.label(RichText::new(format!("{:>3}%", (v * 100.0).round() as i32)).size(12.5).color(DIM).monospace());
                        if slider(ui, &mut v, 210.0).changed() {
                            map.insert(def.key.into(), serde_json::json!(v));
                            any = true;
                        }
                    });
                }
                Kind::Choice(opts, _) => {
                    let mut idx = cur.choice(def.key);
                    let before = idx;
                    setting_row(ui, def.label, |ui| {
                        let short = opts.len() <= 4 && opts.iter().map(|o| o.len()).sum::<usize>() <= 30;
                        if short {
                            if let Some(i) = segmented(ui, opts, idx) {
                                idx = i;
                            }
                        } else {
                            egui::ComboBox::from_id_salt(def.key).selected_text(opts[idx]).width(190.0).height(400.0).show_ui(ui, |ui| {
                                for (j, o) in opts.iter().enumerate() {
                                    ui.selectable_value(&mut idx, j, *o);
                                }
                            });
                        }
                    });
                    if idx != before {
                        map.insert(def.key.into(), opts[idx].into());
                        any = true;
                    }
                }
                Kind::Bool(_) => {
                    let mut v = cur.b(def.key);
                    setting_row(ui, def.label, |ui| {
                        if toggle(ui, &mut v).changed() {
                            map.insert(def.key.into(), v.into());
                            any = true;
                        }
                    });
                }
            }
        }
        if any {
            self.cfg.profile_mut().params.insert(effect.to_string(), map);
            self.changed();
        }
    }

    fn colors_panel(&mut self, ui: &mut egui::Ui, info: &'static EffectInfo, effect: &str) {
        let painting = effect == "custom";
        let color_defs: Vec<&'static str> = info
            .params
            .iter()
            .filter(|d| matches!(d.kind, Kind::Color(_)) && !is_background(d.label))
            .map(|d| d.key)
            .collect();
        if painting {
            ui.label(caption("Paint keys"));
            ui.add_space(8.0);
            ui.label(RichText::new("Pick a color, then click or drag across the keys above.").color(SUB));
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new(ic::BRUSH).family(icons()).size(15.0).color(SUB));
                color_swatch(ui, &mut self.brush);
                ui.add_space(8.0);
                if pill_button(ui, ic::BRUSH, "Fill all", false).clicked() {
                    let hex = color::to_hex(self.brush);
                    let p = self.cfg.profile_mut();
                    for k in KEYS {
                        p.keycolors.insert(k.id.to_string(), hex.clone());
                    }
                    self.changed();
                }
                if pill_button(ui, ic::CLOSE, "Clear", false).clicked() {
                    self.cfg.profile_mut().keycolors.clear();
                    self.changed();
                }
            });
            ui.add_space(16.0);
            let mut pick = None;
            swatch_grid(ui, &mut pick, 8);
            if let Some(c) = pick {
                self.brush = c;
            }
            return;
        }
        if color_defs.is_empty() {
            ui.label(caption("Colors"));
            ui.add_space(8.0);
            ui.label(RichText::new("This effect picks its own colors.").color(DIM));
            return;
        }
        ui.label(caption("Color pairs"));
        ui.add_space(10.0);
        let mut chosen = None;
        for row in PAIRS.chunks(6) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 12.0;
                for (name, a, b) in row {
                    let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
                    let p = ui.painter();
                    let c = rect.center();
                    half_disc(p, c, 16.0, Color32::from_rgb(a[0], a[1], a[2]), true);
                    half_disc(p, c, 16.0, Color32::from_rgb(b[0], b[1], b[2]), false);
                    let ring = if resp.hovered() { Color32::from_white_alpha(200) } else { Color32::from_white_alpha(28) };
                    p.circle_stroke(c, 18.5, Stroke::new(1.2, ring));
                    if resp.on_hover_text(*name).clicked() {
                        chosen = Some((*a, *b));
                    }
                }
            });
            ui.add_space(6.0);
        }
        ui.add_space(16.0);
        ui.label(caption("Main color"));
        ui.add_space(10.0);
        let mut pick = None;
        swatch_grid(ui, &mut pick, 8);
        let mut map = self.cfg.profile().params_for(effect);
        let mut any = false;
        if let Some((a, b)) = chosen {
            map.insert(color_defs[0].into(), color::to_hex(a).into());
            if let Some(k) = color_defs.get(1) {
                map.insert((*k).into(), color::to_hex(b).into());
            }
            any = true;
        }
        if let Some(c) = pick {
            map.insert(color_defs[0].into(), color::to_hex(c).into());
            any = true;
        }
        if any {
            self.cfg.profile_mut().params.insert(effect.to_string(), map);
            self.changed();
        }
        let tip = if info.uses_audio {
            Some("Plays along with any sound on your PC: music, videos, games.")
        } else if info.uses_screen {
            Some("Follows your main screen live. Nothing is saved or sent anywhere.")
        } else {
            None
        };
        if let Some(t) = tip {
            ui.add_space(18.0);
            ui.label(RichText::new(t).color(DIM).size(12.0));
        }
    }

    // ------------------------------------------------------------ themes

    fn themes_page(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(format!("{} themes", self.presets.len())).color(SUB));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                search_box(ui, &mut self.search, "Search themes");
            });
        });
        ui.add_space(18.0);
        let q = self.search.trim().to_lowercase();
        let list: Vec<usize> = (0..self.presets.len())
            .filter(|&i| {
                let p = &self.presets[i];
                q.is_empty() || p.name.to_lowercase().contains(&q) || effects::find(p.effect).name.to_lowercase().contains(&q)
            })
            .collect();
        if list.is_empty() {
            ui.label(RichText::new("Nothing matches that search.").color(DIM));
        }
        let (cols, w) = grid_dims(ui.available_width(), 228.0, 18.0);
        let mut apply = None;
        let mut to_roster = None;
        for row in list.chunks(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 18.0;
                for &i in row {
                    let (name, effect, params, kc) = {
                        let p = &self.presets[i];
                        let params = match &p.params {
                            Value::Object(m) => m.clone(),
                            _ => Map::new(),
                        };
                        let mut kc = vec![None; KEYS.len()];
                        for (id, hex) in &p.keycolors {
                            if let Some(j) = layout::index_of(id) {
                                kc[j] = color::parse_hex(hex);
                            }
                        }
                        (p.name, p.effect, params, kc)
                    };
                    let sub = effects::find(effect).name;
                    let (resp, _) = self.card(ui, format!("theme:{i}"), || Thumb::new(effect, params, kc), w, name, sub, false, &[], None);
                    if resp.clicked() {
                        apply = Some(i);
                    }
                    resp.context_menu(|ui| {
                        if ui.button("Add to roster").clicked() {
                            to_roster = Some(name);
                            ui.close();
                        }
                    });
                }
            });
            ui.add_space(20.0);
        }
        if let Some(i) = apply {
            let preset = &self.presets[i];
            let name = preset.name;
            self.cfg.profile_mut().apply_preset(preset);
            self.changed();
            self.peek = None;
            let prof = self.cfg.profile().name.clone();
            self.toast(format!("{name} applied to {prof}"));
        }
        if let Some(name) = to_roster {
            self.cfg.roster.items.push(format!("t:{name}"));
            self.changed();
            self.toast(format!("Added {name} to the roster"));
        }
    }

    // ------------------------------------------------------------ roster

    fn roster_page(&mut self, ui: &mut egui::Ui) {
        let how = match combo_for(&self.cfg, "next") {
            Some(k) => format!("A playlist of looks. Press {k} to step through it."),
            None => "A playlist of looks. Give \"Next in roster\" a shortcut to step through it.".to_string(),
        };
        ui.label(RichText::new(how).color(SUB));
        ui.add_space(14.0);
        let width = ui.available_width().min(680.0);
        ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(width);
            divider(ui);
            let mut sh = self.cfg.roster.shuffle;
            setting_row(ui, "Shuffle", |ui| {
                if toggle(ui, &mut sh).changed() {
                    self.cfg.roster.shuffle = sh;
                    self.dirty = self.shot.is_none().then(Instant::now);
                }
            });
            let opts = [0u32, 1, 5, 15, 30, 60];
            let name = |m: u32| if m == 0 { "Never".to_string() } else if m == 1 { "Every minute".into() } else { format!("Every {m} min") };
            let mut cur = self.cfg.roster.auto_minutes;
            setting_row(ui, "Switch on its own", |ui| {
                egui::ComboBox::from_id_salt("auto").selected_text(name(cur)).width(150.0).show_ui(ui, |ui| {
                    for m in opts {
                        ui.selectable_value(&mut cur, m, name(m));
                    }
                });
            });
            if cur != self.cfg.roster.auto_minutes {
                self.cfg.roster.auto_minutes = cur;
                self.changed();
            }
        });
        ui.add_space(22.0);

        let items = self.cfg.roster.items.clone();
        let mut act: Option<(&str, usize)> = None;
        if items.is_empty() {
            ui.label(RichText::new("Empty, so the shortcut steps through your profiles instead.").color(DIM));
            ui.add_space(8.0);
        }
        for (i, it) in items.iter().enumerate() {
            let current = i == self.cfg.roster.pos;
            let (rect, resp) = ui.allocate_exact_size(vec2(width, 50.0), Sense::click());
            let p = ui.painter();
            p.rect_filled(rect, 10.0, if current { RAISED_HI } else if resp.hovered() { RAISED } else { Color32::from_rgb(15, 15, 17) });
            p.text(rect.left_center() + vec2(18.0, 0.0), Align2::LEFT_CENTER, format!("{:02}", i + 1), FontId::monospace(12.0), DIM);
            p.text(rect.min + vec2(52.0, 9.0), Align2::LEFT_TOP, config::item_label(it), FontId::new(14.0, semi()), TEXT);
            p.text(rect.min + vec2(52.0, 28.0), Align2::LEFT_TOP, config::item_kind(it), FontId::proportional(11.5), DIM);
            if current {
                pill_label(p, Pos2::new(rect.right() - 160.0, rect.center().y), "Now", 10.5);
            }
            // row buttons
            let mut x = rect.right() - 12.0;
            for (glyph, what, ok) in [(ic::CLOSE, "remove", true), (ic::DOWN, "down", i + 1 < items.len()), (ic::UP, "up", i > 0)] {
                let r = Rect::from_center_size(Pos2::new(x - 14.0, rect.center().y), vec2(28.0, 28.0));
                x -= 32.0;
                if !ok {
                    continue;
                }
                let b = ui.interact(r, resp.id.with(what), Sense::click());
                if b.hovered() {
                    ui.painter().rect_filled(r, 8.0, Color32::from_white_alpha(14));
                }
                ui.painter().text(r.center(), Align2::CENTER_CENTER, glyph, FontId::new(12.0, icons()), if b.hovered() { TEXT } else { DIM });
                if b.clicked() {
                    act = Some((what, i));
                }
            }
            if act.is_none() && resp.on_hover_text("Click to show it now").clicked() {
                act = Some(("show", i));
            }
            ui.add_space(4.0);
        }
        if let Some((what, i)) = act {
            let r = &mut self.cfg.roster;
            match what {
                "show" => {
                    r.pos = i;
                    let it = r.items[i].clone();
                    self.cfg.show_item(&it, &mut self.rng);
                }
                "up" => r.items.swap(i, i - 1),
                "down" => r.items.swap(i, i + 1),
                _ => {
                    r.items.remove(i);
                    if r.pos >= r.items.len() {
                        r.pos = 0;
                    }
                }
            }
            self.changed();
        }

        ui.add_space(22.0);
        ui.label(caption("Add"));
        ui.add_space(8.0);
        let mut add: Option<String> = None;
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("add_p").selected_text("A profile").width(160.0).height(400.0).show_ui(ui, |ui| {
                for p in &self.cfg.profiles {
                    if ui.selectable_label(false, &p.name).clicked() {
                        add = Some(format!("p:{}", p.name));
                    }
                }
            });
            egui::ComboBox::from_id_salt("add_t").selected_text("A theme").width(160.0).height(400.0).show_ui(ui, |ui| {
                for t in &self.presets {
                    if ui.selectable_label(false, t.name).clicked() {
                        add = Some(format!("t:{}", t.name));
                    }
                }
            });
            egui::ComboBox::from_id_salt("add_e").selected_text("An effect").width(160.0).height(500.0).show_ui(ui, |ui| {
                for cat in CATEGORIES {
                    ui.label(caption(cat));
                    for e in effects::all().iter().filter(|e| e.category == *cat) {
                        if ui.selectable_label(false, e.name).clicked() {
                            add = Some(format!("e:{}", e.id));
                        }
                    }
                }
            });
            if pill_button(ui, ic::SHUFFLE, "Surprise", false).on_hover_text("A random look, different every time it comes up").clicked() {
                add = Some("surprise".into());
            }
        });
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if pill_button(ui, ic::ADD, "All profiles", false).clicked() {
                for p in &self.cfg.profiles {
                    self.cfg.roster.items.push(format!("p:{}", p.name));
                }
                self.changed();
            }
            if !items.is_empty() && pill_button(ui, ic::CLOSE, "Clear", false).clicked() {
                self.cfg.roster.items.clear();
                self.cfg.roster.pos = 0;
                self.changed();
            }
        });
        if let Some(it) = add {
            self.toast(format!("Added {} to the roster", config::item_label(&it)));
            self.cfg.roster.items.push(it);
            self.changed();
        }
    }

    // ------------------------------------------------------------ shortcuts

    fn shortcuts_page(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Hold Fn and press a key to control the lights from anywhere, even in games.").color(SUB));
        ui.add_space(16.0);

        // a key arrived while recording?
        if let Some(rec) = &self.recording {
            if let Ok(key) = rec.rx.try_recv() {
                let slot = rec.slot;
                self.recording = None;
                match slot.and_then(|i| self.cfg.hotkeys.get_mut(i)) {
                    Some(h) => h.key = key.to_string(),
                    None => self.cfg.hotkeys.push(config::Hotkey { key: key.to_string(), action: "next".into() }),
                }
                self.changed();
            }
        }

        let width = ui.available_width().min(680.0);
        let mut act: Option<(&str, usize)> = None;
        let mut new_action: Option<(usize, String)> = None;
        ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(width);
            divider(ui);
            for (i, h) in self.cfg.hotkeys.iter().enumerate() {
                let waiting = self.recording.as_ref().is_some_and(|r| r.slot == Some(i));
                ui.allocate_ui_with_layout(vec2(width, 58.0), Layout::left_to_right(Align::Center), |ui| {
                    ui.set_min_size(vec2(width, 58.0));
                    let resp = if waiting {
                        ui.add_sized([150.0, 34.0], egui::Button::new(RichText::new("Press Fn + a key…").color(SUB)))
                    } else {
                        keycap_combo(ui, &["Fn", key_label(&h.key)])
                    };
                    if resp.on_hover_text("Click, then press a new Fn combo").clicked() {
                        act = Some(("record", i));
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if icon_button(ui, ic::CLOSE, 30.0).on_hover_text("Remove").clicked() {
                            act = Some(("remove", i));
                        }
                        let mut a = h.action.clone();
                        egui::ComboBox::from_id_salt(("act", i)).selected_text(config::action_label(&a)).width(230.0).show_ui(ui, |ui| {
                            for (id, name) in config::ACTIONS {
                                ui.selectable_value(&mut a, id.to_string(), *name);
                            }
                        });
                        if a != h.action {
                            new_action = Some((i, a));
                        }
                    });
                });
                divider(ui);
            }
        });
        if let Some((i, a)) = new_action {
            self.cfg.hotkeys[i].action = a;
            self.changed();
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let waiting_new = self.recording.as_ref().is_some_and(|r| r.slot.is_none());
            let add_label = if waiting_new { "Press Fn + a key…" } else { "Add shortcut" };
            if pill_button(ui, ic::ADD, add_label, !waiting_new).clicked() {
                act = Some(("add", 0));
            }
            if self.recording.is_some() && pill_button(ui, ic::CLOSE, "Cancel", false).clicked() {
                self.recording = None;
            }
        });
        match act {
            Some(("record", i)) => self.start_recording(ui.ctx(), Some(i)),
            Some(("add", _)) => self.start_recording(ui.ctx(), None),
            Some(("remove", i)) => {
                self.cfg.hotkeys.remove(i);
                self.changed();
            }
            _ => {}
        }

        ui.add_space(26.0);
        ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(width);
            ui.label(
                RichText::new(
                    "Fn + \\ is the most reliable: the keyboard always reports it and it doesn't type anything. Other Fn combos \
                     work too, but the keyboard sometimes skips them, and some still do their normal job (Fn + ] also types ]). \
                     Shortcuts need the background service running.",
                )
                .color(DIM)
                .size(12.0),
            );
        });
    }

    fn start_recording(&mut self, ctx: &egui::Context, slot: Option<usize>) {
        let ctx = ctx.clone();
        let (listener, rx) = fnkeys::Listener::start(move || ctx.request_repaint());
        self.recording = Some(Recording { slot, _listener: listener, rx });
    }

    // ------------------------------------------------------------ settings

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        let width = ui.available_width().min(680.0);
        ui.allocate_ui_with_layout(vec2(width, 0.0), Layout::top_down(Align::Min), |ui| {
            ui.set_width(width);
            ui.label(caption("Lighting"));
            ui.add_space(6.0);
            divider(ui);
            let mut paused = self.cfg.paused;
            setting_row(ui, "Pause lighting", |ui| {
                toggle(ui, &mut paused);
            });
            if paused != self.cfg.paused {
                self.cfg.paused = paused;
                self.changed();
            }
            let mut exit = self.cfg.exit_mode.clone();
            setting_row(ui, "When the app is fully closed", |ui| {
                egui::ComboBox::from_id_salt("exit").selected_text(exit_label(&exit)).width(220.0).height(400.0).show_ui(ui, |ui| {
                    ui.selectable_value(&mut exit, "keep".to_string(), "Keep current colors");
                    ui.selectable_value(&mut exit, "off".to_string(), "Turn lights off");
                    for e in effects::all().iter().filter(|e| matches!(e.mode, Mode::Onboard(_))) {
                        ui.selectable_value(&mut exit, e.id.to_string(), format!("Built-in: {}", e.name));
                    }
                });
            });
            if exit != self.cfg.exit_mode {
                self.cfg.exit_mode = exit;
                self.changed();
            }

            ui.add_space(26.0);
            ui.label(caption("App"));
            ui.add_space(6.0);
            divider(ui);
            let mut auto = self.status.autostart;
            setting_row(ui, "Start with Windows", |ui| {
                if toggle(ui, &mut auto).changed() && self.shot.is_none() {
                    sys::set_autostart(auto);
                }
            });
            self.status.autostart = auto;
            let service = self.status.service;
            setting_row(ui, "Background service", |ui| {
                if service {
                    if small_button(ui, "Quit").on_hover_text("Stops the lighting until you open the app again").clicked() {
                        let _ = std::fs::create_dir_all(config::dir());
                        let _ = std::fs::write(daemon::quit_flag(), b"");
                        self.status_at = Instant::now() - Duration::from_secs(10);
                    }
                    ui.label(RichText::new("Running").color(SUB));
                } else {
                    if small_button(ui, "Start").clicked() {
                        sys::spawn_self(&["--background"]);
                        self.status_at = Instant::now() - Duration::from_secs(10);
                    }
                    ui.label(RichText::new("Stopped").color(WARN));
                }
            });
            let kb = self.status.keyboard;
            setting_row(ui, "Keyboard", |ui| {
                ui.label(RichText::new(if kb { "Surmen V400 connected" } else { "Not found" }).color(if kb { SUB } else { WARN }));
            });
            ui.add_space(18.0);
            ui.label(RichText::new("Closing this window keeps your lighting running from the tray.").color(DIM).size(12.0));
        });
    }

    // ------------------------------------------------------------ overlays

    fn draw_toast(&mut self, ctx: &egui::Context) {
        let Some((text, at)) = &self.toast else { return };
        let age = at.elapsed().as_secs_f32();
        if age > 2.4 {
            self.toast = None;
            return;
        }
        let a = (age / 0.15).min(1.0).min(((2.4 - age) / 0.3).min(1.0));
        let rise = (1.0 - (age / 0.2).min(1.0)) * 8.0;
        let p = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::unique("toast")));
        let screen = ctx.content_rect();
        let galley = p.layout_no_wrap(text.clone(), FontId::new(13.5, semi()), INK.gamma_multiply(a));
        let size = galley.size() + vec2(36.0, 20.0);
        let rect = Rect::from_center_size(Pos2::new(screen.center().x + 128.0, screen.bottom() - 40.0 + rise), size);
        p.rect_filled(rect.translate(vec2(0.0, 4.0)).expand(2.0), 22.0, Color32::from_black_alpha((90.0 * a) as u8));
        p.rect_filled(rect, size.y / 2.0, PAPER.gamma_multiply(a));
        p.galley(rect.center() - galley.size() / 2.0, galley, INK);
    }

    fn shot_tick(&mut self, ctx: &egui::Context) {
        let Some(s) = &mut self.shot else { return };
        if s.waiting && s.done.load(Ordering::SeqCst) {
            s.waiting = false;
            s.done.store(false, Ordering::SeqCst);
            s.step += 1;
            s.at = Instant::now();
        }
        let only: Vec<String> = std::env::var("KCS_SHOT_PAGES").unwrap_or_default().split(',').filter(|x| !x.is_empty()).map(String::from).collect();
        let pages: Vec<Page> = PAGES.iter().map(|p| p.0).filter(|p| only.is_empty() || only.iter().any(|o| o.eq_ignore_ascii_case(&format!("{p:?}")))).collect();
        if s.step >= pages.len() {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        self.page = pages[s.step];
        if let Ok(q) = std::env::var("KCS_SHOT_SEARCH") {
            self.search = q;
        }
        if let Ok(k) = std::env::var("KCS_SHOT_PEEK") {
            self.peek = Some((k, Instant::now() - Duration::from_secs(1)));
        }
        if !s.waiting && s.at.elapsed() > Duration::from_millis(1800) {
            s.waiting = true;
            let path = s.dir.join(format!("{:?}.ppm", self.page).to_lowercase());
            let done = s.done.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::ScreenshotCallback::new(move |img| {
                let [w, h] = img.size;
                let mut buf = format!("P6\n{w} {h}\n255\n").into_bytes();
                for px in &img.pixels {
                    buf.extend_from_slice(&[px.r(), px.g(), px.b()]);
                }
                let _ = std::fs::write(path, buf);
                done.store(true, Ordering::SeqCst);
            })));
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.frame += 1;
        if self.status_at.elapsed() > Duration::from_secs(2) {
            self.refresh_status();
        }
        if self.shot.is_none() {
            self.sync_file();
        }
        self.shot_tick(&ctx);
        self.update_preview(&ctx);
        if self.page != Page::Shortcuts {
            // stop listening for Fn combos once the page is left
            self.recording = None;
        }
        self.update_fakes();
        // a card hovered for a moment is previewed on the big keyboard
        match (self.hover_card.take(), &self.peek) {
            (Some(k), Some((cur, _))) if *cur == k => {}
            (Some(k), _) => self.peek = Some((k, Instant::now())),
            (None, _) if self.shot.is_none() => self.peek = None,
            _ => {}
        }

        egui::Panel::left("side")
            .exact_size(236.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(SIDE).inner_margin(egui::Margin::symmetric(12, 8)).stroke(Stroke::new(1.0, Color32::from_rgb(22, 22, 24))))
            .show(ui, |ui| self.sidebar(ui));

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin { left: 34, right: 34, top: 22, bottom: 0 }))
            .show(ui, |ui| {
                self.top_bar(ui);
                ui.add_space(6.0);
                self.hero(ui);
                self.control_strip(ui);
                ui.add_space(14.0);
                divider(ui);
                egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                    ui.add_space(20.0);
                    match self.page {
                        Page::Effects => self.effects_page(ui),
                        Page::Adjust => self.adjust_page(ui),
                        Page::Themes => self.themes_page(ui),
                        Page::Roster => self.roster_page(ui),
                        Page::Shortcuts => self.shortcuts_page(ui),
                        Page::Settings => self.settings_page(ui),
                    }
                    ui.add_space(30.0);
                });
            });
        self.draw_toast(&ctx);

        // free previews that haven't been on screen for a moment
        let frame = self.frame;
        self.thumbs.retain(|_, t| frame - t.seen < 30);
        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.dirty.is_some() {
            let _ = config::save(&self.cfg);
        }
    }
}

// ---------------------------------------------------------------- style

fn semi() -> FontFamily {
    FontFamily::Name("semi".into())
}
fn display() -> FontFamily {
    FontFamily::Name("display".into())
}
fn icons() -> FontFamily {
    FontFamily::Name("icons".into())
}

/// Windows' own fonts: Segoe UI for text, Segoe UI Semibold, Bahnschrift for big titles
/// and the system icon font. Falls back to egui's built-in font when one is missing.
fn fonts(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    let dir = PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("Fonts");
    let base = f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let mut load = |name: &str, files: &[&str]| -> Option<String> {
        for file in files {
            if let Ok(b) = std::fs::read(dir.join(file)) {
                f.font_data.insert(name.into(), Arc::new(egui::FontData::from_owned(b)));
                return Some(name.to_string());
            }
        }
        None
    };
    let regular = load("segoe", &["segoeui.ttf"]);
    let semibold = load("segoe_sb", &["seguisb.ttf"]);
    let disp = load("bahn", &["bahnschrift.ttf"]);
    let icon_font = load("icons", &["SegoeIcons.ttf", "segmdl2.ttf"]);
    let chain = |first: &[Option<String>]| {
        let mut v: Vec<String> = first.iter().flatten().cloned().collect();
        v.extend(base.iter().cloned());
        v
    };
    f.families.insert(FontFamily::Proportional, chain(&[regular.clone()]));
    f.families.insert(semi(), chain(&[semibold.clone(), regular.clone()]));
    f.families.insert(display(), chain(&[disp, semibold]));
    f.families.insert(icons(), chain(&[icon_font]));
    ctx.set_fonts(f);
}

fn style(ctx: &egui::Context) {
    fonts(ctx);
    let mut v = egui::Visuals::dark();
    let r = CornerRadius::same(9);
    v.panel_fill = BG;
    v.window_fill = Color32::from_rgb(20, 20, 22);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.window_shadow = egui::epaint::Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(140) };
    v.popup_shadow = v.window_shadow;
    v.extreme_bg_color = Color32::from_rgb(16, 16, 18);
    v.faint_bg_color = RAISED;
    // selected things are white with dark text, like an inverted pill
    v.selection.bg_fill = PAPER;
    v.selection.stroke = Stroke::new(1.0, INK);
    v.hyperlink_color = TEXT;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(226, 226, 230));
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.bg_fill = Color32::from_rgb(52, 52, 56);
    v.widgets.inactive.weak_bg_fill = RAISED;
    v.widgets.inactive.bg_stroke = Stroke::NONE;
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.bg_fill = Color32::from_rgb(70, 70, 75);
    v.widgets.hovered.weak_bg_fill = RAISED_HI;
    v.widgets.hovered.bg_stroke = Stroke::NONE;
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.hovered.corner_radius = r;
    v.widgets.hovered.expansion = 0.0;
    v.widgets.active.bg_fill = Color32::from_rgb(90, 90, 96);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(40, 40, 44);
    v.widgets.active.bg_stroke = Stroke::NONE;
    v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.active.corner_radius = r;
    v.widgets.active.expansion = 0.0;
    v.widgets.open.weak_bg_fill = RAISED_HI;
    v.widgets.open.bg_stroke = Stroke::NONE;
    v.widgets.open.corner_radius = r;
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 7.0);
        s.spacing.button_padding = vec2(12.0, 7.0);
        s.spacing.interact_size.y = 30.0;
        s.spacing.menu_margin = egui::Margin::same(8);
        s.spacing.scroll = egui::style::ScrollStyle::thin();
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Monospace, FontId::monospace(12.5));
    });
}

// ---------------------------------------------------------------- widgets

/// Small grey all-caps label with wide letter spacing.
fn caption(s: &str) -> RichText {
    RichText::new(s.to_uppercase()).size(10.5).color(DIM).extra_letter_spacing(1.5).family(semi())
}

fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(rect.x_range(), rect.center().y, Stroke::new(1.0, LINE));
}

fn section_title(ui: &mut egui::Ui, title: &str, n: usize) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(title).family(display()).size(19.0).color(TEXT));
        ui.label(RichText::new(n.to_string()).size(13.0).color(DIM));
    });
    ui.add_space(10.0);
}

/// A label on the left, a control on the right, a hairline underneath.
fn setting_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    let w = ui.available_width();
    ui.allocate_ui_with_layout(vec2(w, 50.0), Layout::left_to_right(Align::Center), |ui| {
        ui.set_min_size(vec2(w, 50.0));
        ui.label(RichText::new(label).size(14.0).color(Color32::from_rgb(214, 214, 219)));
        ui.with_layout(Layout::right_to_left(Align::Center), add);
    });
    divider(ui);
}

fn c32(c: Rgb) -> Color32 {
    let c = color::to_u8(c);
    Color32::from_rgb(c[0], c[1], c[2])
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

/// Thin track, round knob, no numbers.
fn slider(ui: &mut egui::Ui, v: &mut f32, width: f32) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(width, 24.0), Sense::click_and_drag());
    let (l, r) = (rect.left() + 8.0, rect.right() - 8.0);
    if resp.is_pointer_button_down_on() {
        if let Some(p) = resp.interact_pointer_pos() {
            let nv = ((p.x - l) / (r - l)).clamp(0.0, 1.0);
            if (nv - *v).abs() > f32::EPSILON {
                *v = nv;
                resp.mark_changed();
            }
        }
    }
    let hot = ui.ctx().animate_bool_with_time(resp.id, resp.hovered() || resp.dragged(), 0.1);
    let y = rect.center().y;
    let x = l + (r - l) * v.clamp(0.0, 1.0);
    let p = ui.painter();
    p.rect_filled(Rect::from_min_max(Pos2::new(l, y - 2.0), Pos2::new(r, y + 2.0)), 2.0, Color32::from_rgb(46, 46, 50));
    p.rect_filled(Rect::from_min_max(Pos2::new(l, y - 2.0), Pos2::new(x, y + 2.0)), 2.0, mix(Color32::from_rgb(200, 200, 206), PAPER, hot));
    p.circle_filled(Pos2::new(x, y + 1.0), 8.5, Color32::from_black_alpha(120));
    p.circle_filled(Pos2::new(x, y), 7.0 + hot * 1.5, PAPER);
    resp
}

/// iOS-style on/off switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(42.0, 24.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool_with_time(resp.id, *on, 0.14);
    let p = ui.painter();
    p.rect_filled(rect, 12.0, mix(Color32::from_rgb(52, 52, 56), PAPER, t));
    let x = rect.left() + 12.0 + t * (rect.width() - 24.0);
    p.circle_filled(Pos2::new(x, rect.center().y), 9.0, mix(Color32::from_rgb(214, 214, 219), INK, t));
    resp
}

/// Round color dot that opens a picker.
fn color_swatch(ui: &mut egui::Ui, c: &mut [u8; 3]) -> bool {
    let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
    let p = ui.painter();
    p.circle_filled(rect.center(), 12.0, Color32::from_rgb(c[0], c[1], c[2]));
    let ring = if resp.hovered() { Color32::from_white_alpha(210) } else { Color32::from_white_alpha(36) };
    p.circle_stroke(rect.center(), 14.5, Stroke::new(1.2, ring));
    let mut changed = false;
    egui::Popup::from_toggle_button_response(&resp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| {
            ui.set_width(240.0);
            let mut col = Color32::from_rgb(c[0], c[1], c[2]);
            if egui::color_picker::color_picker_color32(ui, &mut col, egui::color_picker::Alpha::Opaque) {
                *c = [col.r(), col.g(), col.b()];
                changed = true;
            }
            ui.add_space(6.0);
            let mut pick = None;
            swatch_grid(ui, &mut pick, 8);
            if let Some(v) = pick {
                *c = v;
                changed = true;
            }
            ui.label(RichText::new(color::to_hex(*c).to_uppercase()).monospace().color(DIM));
        });
    changed
}

fn swatch_grid(ui: &mut egui::Ui, pick: &mut Option<[u8; 3]>, per_row: usize) {
    for row in SWATCHES.chunks(per_row) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            for c in row {
                let (rect, resp) = ui.allocate_exact_size(vec2(24.0, 24.0), Sense::click());
                let p = ui.painter();
                p.circle_filled(rect.center(), 9.5, Color32::from_rgb(c[0], c[1], c[2]));
                let ring = if resp.hovered() { Color32::from_white_alpha(210) } else { Color32::from_white_alpha(26) };
                p.circle_stroke(rect.center(), 11.0, Stroke::new(1.0, ring));
                if resp.clicked() {
                    *pick = Some(*c);
                }
            }
        });
    }
}

fn half_disc(p: &egui::Painter, c: Pos2, r: f32, col: Color32, left: bool) {
    let mut pts = Vec::with_capacity(26);
    for j in 0..=24 {
        let a = std::f32::consts::FRAC_PI_2 + j as f32 / 24.0 * std::f32::consts::PI;
        let a = if left { a } else { a + std::f32::consts::PI };
        pts.push(c + vec2(a.cos(), a.sin()) * r);
    }
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
}

/// Rounded button with an icon and a label. `primary` = white with dark text.
fn pill_button(ui: &mut egui::Ui, glyph: &str, label: &str, primary: bool) -> egui::Response {
    let font = FontId::new(13.5, semi());
    let tw = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), TEXT).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(tw + 52.0, 36.0), Sense::click());
    let hot = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), 0.1);
    let (bg, fg) = if primary {
        (mix(PAPER, Color32::WHITE, hot), INK)
    } else {
        (mix(RAISED, RAISED_HI, hot), TEXT)
    };
    let p = ui.painter();
    p.rect_filled(rect, 18.0, bg);
    p.text(rect.left_center() + vec2(18.0, 0.0), Align2::LEFT_CENTER, glyph, FontId::new(13.0, icons()), fg);
    p.text(rect.left_center() + vec2(38.0, 0.0), Align2::LEFT_CENTER, label, font, fg);
    resp
}

fn small_button(ui: &mut egui::Ui, label: &str) -> egui::Response {
    let font = FontId::new(12.5, semi());
    let tw = ui.fonts_mut(|f| f.layout_no_wrap(label.to_string(), font.clone(), TEXT).size().x);
    let (rect, resp) = ui.allocate_exact_size(vec2(tw + 24.0, 28.0), Sense::click());
    let p = ui.painter();
    p.rect_filled(rect, 14.0, if resp.hovered() { RAISED_HI } else { RAISED });
    p.text(rect.center(), Align2::CENTER_CENTER, label, font, TEXT);
    resp
}

fn icon_button(ui: &mut egui::Ui, glyph: &str, size: f32) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let p = ui.painter();
    if resp.hovered() {
        p.rect_filled(rect, size / 2.0, RAISED_HI);
    }
    p.text(rect.center(), Align2::CENTER_CENTER, glyph, FontId::new(size * 0.4, icons()), if resp.hovered() { TEXT } else { SUB });
    resp
}

fn pill_label(p: &egui::Painter, center: Pos2, text: &str, size: f32) {
    let galley = p.layout_no_wrap(text.to_string(), FontId::new(size, semi()), INK);
    let rect = Rect::from_center_size(center, galley.size() + vec2(size * 1.4, size * 0.7));
    p.rect_filled(rect, rect.height() / 2.0, PAPER);
    p.galley(rect.center() - galley.size() / 2.0, galley, INK);
}

/// Keys drawn as little keycaps: [Fn] + [\].
fn keycap_combo(ui: &mut egui::Ui, keys: &[&str]) -> egui::Response {
    let font = FontId::new(13.0, semi());
    let widths: Vec<f32> = keys.iter().map(|k| ui.fonts_mut(|f| f.layout_no_wrap(k.to_string(), font.clone(), TEXT).size().x).max(12.0) + 22.0).collect();
    let total = widths.iter().sum::<f32>() + 22.0 * (keys.len() as f32 - 1.0);
    let (rect, resp) = ui.allocate_exact_size(vec2(total, 36.0), Sense::click());
    let p = ui.painter();
    let mut x = rect.left();
    for (i, (k, w)) in keys.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, rect.top() + 2.0), vec2(*w, 32.0));
        p.rect_filled(r.translate(vec2(0.0, 2.0)), 8.0, Color32::from_rgb(10, 10, 11));
        p.rect_filled(r, 8.0, if resp.hovered() { Color32::from_rgb(48, 48, 53) } else { Color32::from_rgb(38, 38, 42) });
        p.rect_stroke(r, 8.0, Stroke::new(1.0, Color32::from_white_alpha(14)), StrokeKind::Inside);
        p.text(r.center(), Align2::CENTER_CENTER, *k, font.clone(), TEXT);
        x += w;
        if i + 1 < keys.len() {
            p.text(Pos2::new(x + 11.0, r.center().y), Align2::CENTER_CENTER, "+", FontId::proportional(13.0), DIM);
            x += 22.0;
        }
    }
    resp
}

/// Segmented control (one option selected). Returns the clicked index.
fn segmented(ui: &mut egui::Ui, items: &[&str], selected: usize) -> Option<usize> {
    let font = FontId::new(12.5, semi());
    let widths: Vec<f32> = items.iter().map(|s| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), TEXT).size().x) + 24.0).collect();
    let total: f32 = widths.iter().sum::<f32>() + 6.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 32.0), Sense::hover());
    ui.painter().rect_filled(rect, 16.0, RAISED);
    let mut x = rect.min.x + 3.0;
    let mut clicked = None;
    for (i, (s, w)) in items.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, rect.min.y + 3.0), vec2(*w, 26.0));
        let resp = ui.interact(r, ui.scope_id().with(("seg", s, i)), Sense::click());
        let sel = i == selected;
        if sel {
            ui.painter().rect_filled(r, 13.0, PAPER);
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 13.0, RAISED_HI);
        }
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *s, font.clone(), if sel { INK } else { SUB });
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
}

/// Plain text tabs with a sliding underline. Returns the clicked index.
fn text_tabs(ui: &mut egui::Ui, items: &[String], selected: usize) -> Option<usize> {
    let font = FontId::new(14.0, semi());
    let mut clicked = None;
    let mut sel_rect = None;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 22.0;
        for (i, s) in items.iter().enumerate() {
            let w = ui.fonts_mut(|f| f.layout_no_wrap(s.clone(), font.clone(), TEXT).size().x);
            let (r, resp) = ui.allocate_exact_size(vec2(w, 32.0), Sense::click());
            let sel = i == selected;
            let col = if sel { TEXT } else if resp.hovered() { SUB } else { DIM };
            ui.painter().text(r.left_center(), Align2::LEFT_CENTER, s, font.clone(), col);
            if sel {
                sel_rect = Some(r);
            }
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    if let Some(r) = sel_rect {
        let ctx = ui.ctx();
        let x0 = ctx.animate_value_with_time(egui::Id::unique("tab_x0"), r.left(), 0.16);
        let x1 = ctx.animate_value_with_time(egui::Id::unique("tab_x1"), r.right(), 0.16);
        ui.painter().rect_filled(Rect::from_min_max(Pos2::new(x0, r.bottom() - 1.0), Pos2::new(x1, r.bottom() + 1.0)), 1.0, PAPER);
    }
    clicked
}

fn search_box(ui: &mut egui::Ui, text: &mut String, hint: &str) {
    let (rect, _) = ui.allocate_exact_size(vec2(250.0, 36.0), Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 18.0, RAISED);
    p.text(rect.left_center() + vec2(16.0, 0.0), Align2::LEFT_CENTER, ic::SEARCH, FontId::new(13.0, icons()), DIM);
    let field = Rect::from_min_max(rect.min + vec2(38.0, 8.0), rect.max - vec2(34.0, 8.0));
    let edit = egui::TextEdit::singleline(text)
        .hint_text(RichText::new(hint).color(DIM))
        .frame(egui::Frame::NONE)
        .margin(vec2(0.0, 0.0))
        .text_color(TEXT)
        .font(FontId::proportional(13.5));
    let resp = ui.put(field, edit);
    if resp.has_focus() {
        ui.painter().rect_stroke(rect, 18.0, Stroke::new(1.0, Color32::from_white_alpha(60)), StrokeKind::Inside);
    }
    if !text.is_empty() {
        let x = Rect::from_center_size(rect.right_center() - vec2(18.0, 0.0), vec2(22.0, 22.0));
        let xr = ui.interact(x, ui.scope_id().with("clear_search"), Sense::click());
        ui.painter().text(x.center(), Align2::CENTER_CENTER, ic::CLOSE, FontId::new(10.0, icons()), if xr.hovered() { TEXT } else { DIM });
        if xr.clicked() {
            text.clear();
        }
    }
}

/// How many cards fit in a row, and how wide each one is.
fn grid_dims(avail: f32, target: f32, gap: f32) -> (usize, f32) {
    let cols = (((avail + gap) / (target + gap)).floor() as usize).max(2);
    (cols, ((avail - gap * (cols as f32 - 1.0)) / cols as f32 - 1.0).floor())
}

/// Soft light: a radial gradient added on top of whatever is below.
fn glow(p: &egui::Painter, c: Pos2, r: Vec2, col: Rgb, strength: f32) {
    let base = c32(col);
    if base.r() as u16 + base.g() as u16 + base.b() as u16 == 0 {
        return;
    }
    let mut m = egui::Mesh::default();
    let steps = 40;
    let rings = [(0.0, 1.0), (0.3, 0.72), (0.6, 0.32), (0.82, 0.1), (1.0, 0.0)];
    m.colored_vertex(c, base.gamma_multiply(strength).additive());
    for (ri, (rr, a)) in rings.iter().enumerate().skip(1) {
        for s in 0..steps {
            let ang = s as f32 / steps as f32 * std::f32::consts::TAU;
            m.colored_vertex(c + vec2(ang.cos() * r.x * rr, ang.sin() * r.y * rr), base.gamma_multiply(strength * a).additive());
        }
        let ring0 = 1 + (ri as u32 - 1) * steps;
        for s in 0..steps {
            let n = (s + 1) % steps;
            if ri == 1 {
                m.add_triangle(0, ring0 + s, ring0 + n);
            } else {
                let prev = ring0 - steps;
                m.add_triangle(prev + s, ring0 + s, ring0 + n);
                m.add_triangle(prev + s, ring0 + n, prev + n);
            }
        }
    }
    p.add(egui::Shape::mesh(m));
}

/// Average light in vertical slices of the keyboard (left to right).
fn zone_colors(colors: &[Rgb], n: usize) -> Vec<Rgb> {
    let mut acc = vec![([0.0f32; 3], 0.0f32); n];
    for (k, c) in KEYS.iter().zip(colors) {
        let z = ((k.u() * n as f32) as usize).min(n - 1);
        for j in 0..3 {
            acc[z].0[j] += c[j];
        }
        acc[z].1 += 1.0;
    }
    acc.into_iter().map(|(s, w)| if w > 0.0 { s.map(|v| v / w) } else { [0.0; 3] }).collect()
}

/// A tiny keyboard drawn from key colors.
fn mini_keyboard(p: &egui::Painter, rect: Rect, colors: &[Rgb], radius: f32, bloom: bool) {
    p.rect_filled(rect, radius, Color32::from_rgb(6, 6, 7));
    let inner = rect.shrink((rect.height() * 0.09).max(2.0));
    let unit = (inner.width() / layout::WIDTH).min(inner.height() / layout::HEIGHT);
    let size = vec2(layout::WIDTH * unit, layout::HEIGHT * unit);
    let origin = inner.center() - size / 2.0;
    for (k, c) in KEYS.iter().zip(colors) {
        let r = Rect::from_min_size(origin + vec2(k.x * unit, k.y * unit), vec2(k.w * unit, unit)).shrink(unit * 0.08);
        let led = c32(*c);
        if bloom {
            p.rect_filled(r.expand(unit * 0.2), unit * 0.4, led.gamma_multiply(0.18).additive());
        }
        let key = Color32::from_rgb(led.r().max(18), led.g().max(18), led.b().max(20));
        p.rect_filled(r, unit * 0.2, key);
    }
}

// ---------------------------------------------------------------- misc helpers

/// Effects in the order the gallery shows them.
fn ordered_effects() -> Vec<&'static EffectInfo> {
    CATEGORIES.iter().flat_map(|c| effects::all().iter().filter(move |e| e.category == *c)).collect()
}

fn is_background(label: &str) -> bool {
    ["Background", "Sky", "Water", "Night", "Road", "Other keys"].iter().any(|w| label.contains(w))
}

fn profile_keycolors(p: &Profile) -> Vec<Option<[u8; 3]>> {
    KEYS.iter().map(|k| p.keycolors.get(k.id).and_then(|h| color::parse_hex(h))).collect()
}

/// First sentence of a description, cut to fit on a card.
fn short_desc(desc: &str) -> String {
    let s = desc.split(". ").next().unwrap_or(desc).trim_end_matches('.');
    if s.chars().count() > 34 {
        format!("{}…", s.chars().take(33).collect::<String>().trim_end())
    } else {
        s.to_string()
    }
}

fn arrow_dir(id: &str) -> Option<Vec2> {
    match id {
        "up" => Some(vec2(0.0, -1.0)),
        "down" => Some(vec2(0.0, 1.0)),
        "left" => Some(vec2(-1.0, 0.0)),
        "right" => Some(vec2(1.0, 0.0)),
        _ => None,
    }
}

/// Small filled triangle pointing in `dir` (the UI font has no arrow glyphs).
fn draw_arrow(p: &egui::Painter, c: Pos2, dir: Vec2, size: f32, col: Color32) {
    let side = vec2(-dir.y, dir.x);
    let pts = vec![c + dir * size, c - dir * size * 0.7 + side * size, c - dir * size * 0.7 - side * size];
    p.add(egui::Shape::convex_polygon(pts, col, Stroke::NONE));
}

/// Label of a key id ("backslash" -> "\").
fn key_label(id: &str) -> &str {
    layout::index_of(id).map(|i| KEYS[i].label).unwrap_or(id)
}

/// "Fn + \" if an action has a shortcut.
fn combo_for(cfg: &Config, action: &str) -> Option<String> {
    cfg.hotkeys.iter().find(|h| h.action == action).map(|h| format!("Fn + {}", key_label(&h.key)))
}

/// " (Fn + \)" for tooltips, or "".
fn shortcut_for(cfg: &Config, action: &str) -> String {
    combo_for(cfg, action).map(|k| format!(" ({k})")).unwrap_or_default()
}

fn exit_label(mode: &str) -> String {
    match mode {
        "keep" => "Keep current colors".into(),
        "off" => "Turn lights off".into(),
        id => format!("Built-in: {}", effects::find(id).name),
    }
}

/// Map egui keys (preview window focused) to our key index.
fn egui_key_index(key: egui::Key) -> Option<usize> {
    let name = format!("{key:?}");
    let id = match name.as_str() {
        "Escape" => "esc",
        "Tab" => "tab",
        "Backspace" => "backspace",
        "Enter" => "enter",
        "Space" => "space",
        "Delete" => "del",
        "Home" | "Insert" => "home",
        "End" => "end",
        "PageUp" => "pgup",
        "PageDown" => "pgdn",
        "ArrowUp" => "up",
        "ArrowDown" => "down",
        "ArrowLeft" => "left",
        "ArrowRight" => "right",
        "Minus" => "minus",
        "Equals" | "Plus" => "equal",
        "OpenBracket" => "lbracket",
        "CloseBracket" => "rbracket",
        "Backslash" => "backslash",
        "Semicolon" | "Colon" => "semicolon",
        "Quote" => "quote",
        "Comma" => "comma",
        "Period" => "period",
        "Slash" | "Questionmark" => "slash",
        "Backtick" => "grave",
        n if n.starts_with("Num") && n.len() == 4 => &n[3..],
        n if n.len() == 1 => return layout::index_of(&n.to_lowercase()),
        n if n.starts_with('F') && n[1..].parse::<u8>().is_ok_and(|x| (1..=12).contains(&x)) => {
            return layout::index_of(&n.to_lowercase())
        }
        _ => return None,
    };
    layout::index_of(id)
}
