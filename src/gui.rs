//! Settings window. A separate process that only runs while it's open; it edits
//! config.json and the background service picks the changes up.

use crate::audio::AudioCapture;
use crate::color::{self, Rgb};
use crate::config::{self, Config, Preset, Profile};
use crate::daemon;
use crate::effects::{self, AudioFrame, Ctx, Effect, EffectInfo, Kind, Mode, Params, CATEGORIES};
use crate::engine::Renderer;
use crate::icon;
use crate::layout::{self, KEYS};
use crate::protocol::Keyboard;
use crate::sys::{self, InstanceLock, APP_TITLE, DAEMON_MUTEX, GUI_MUTEX, VENDOR_EXE};
use eframe::egui::{
    self, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind,
    Vec2,
};
use serde_json::{Map, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

// Monochrome look: near-black surfaces, white text, grey secondary text.
// The only color in the window comes from the lights themselves.
const BG: Color32 = Color32::from_rgb(9, 9, 10);
const PANEL: Color32 = Color32::from_rgb(15, 15, 16);
const CARD: Color32 = Color32::from_rgb(24, 24, 26);
const CARD_HOVER: Color32 = Color32::from_rgb(32, 32, 35);
const LINE: Color32 = Color32::from_rgb(40, 40, 43);
const TEXT: Color32 = Color32::from_rgb(242, 242, 245);
const DIM: Color32 = Color32::from_rgb(142, 142, 147);
const GOOD: Color32 = Color32::from_rgb(48, 209, 88);
const WARN: Color32 = Color32::from_rgb(255, 159, 10);

const SWATCHES: [[u8; 3]; 14] = [
    [255, 0, 0], [255, 110, 0], [255, 230, 0], [0, 220, 40], [0, 160, 255], [40, 80, 255], [120, 0, 255],
    [255, 0, 255], [255, 0, 130], [255, 170, 0], [140, 255, 140], [60, 240, 255], [0, 50, 160], [255, 255, 255],
];

pub fn run() {
    let Some(lock) = InstanceLock::acquire(GUI_MUTEX) else {
        sys::focus_window(APP_TITLE);
        return;
    };
    if !sys::instance_running(DAEMON_MUTEX) {
        sys::spawn_self(&["--background"]);
    }
    let icon = egui::IconData { rgba: icon::rgba(64), width: 64, height: 64 };
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(APP_TITLE)
            .with_inner_size([1220.0, 820.0])
            .with_min_inner_size([940.0, 640.0])
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    let _ = eframe::run_native(APP_TITLE, opts, Box::new(|cc| Ok(Box::new(App::new(cc)))));
    drop(lock);
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Effects,
    Customize,
    Themes,
}

struct Status {
    service: bool,
    keyboard: bool,
    vendor: bool,
    autostart: bool,
}

/// A small live preview of one effect (gallery and theme cards).
struct Thumb {
    info: &'static EffectInfo,
    fx: Box<dyn Effect>,
    params: Map<String, Value>,
    keycolors: Vec<Option<[u8; 3]>>,
    out: Vec<Rgb>,
    last: f32,
    next_key: f32,
    presses: usize,
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
            // pretend someone is typing
            self.presses += 1;
            self.fx.key((self.presses * 37 + 11) % KEYS.len(), t);
            self.next_key = t + 0.35;
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
    tab: Tab,
    gallery_cat: usize,
    brush: [u8; 3],
    renaming: Option<(usize, String)>,
    status: Status,
    status_at: Instant,
    hid: Option<hidapi::HidApi>,
    logo: egui::TextureHandle,
    thumbs: HashMap<String, Thumb>,
    start: Instant,
    fake_audio: AudioFrame,
    fake_screen: Vec<Rgb>,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        style(&cc.egui_ctx);
        let n = 96;
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([n, n], &icon::rgba(n as u32)),
            egui::TextureOptions::LINEAR,
        );
        let mut app = App {
            cfg: config::load(),
            saved_mtime: config::mtime(),
            dirty: None,
            checked_file: Instant::now(),
            r: Renderer::new(),
            audio: None,
            audio_retry: Instant::now(),
            presets: config::presets(),
            tab: Tab::Effects,
            gallery_cat: 0,
            brush: [255, 0, 60],
            renaming: None,
            status: Status { service: true, keyboard: true, vendor: false, autostart: false },
            status_at: Instant::now() - Duration::from_secs(10),
            hid: hidapi::HidApi::new().ok(),
            logo,
            thumbs: HashMap::new(),
            start: Instant::now(),
            fake_audio: AudioFrame::default(),
            fake_screen: Vec::new(),
        };
        app.refresh_status();
        app
    }

    fn changed(&mut self) {
        self.dirty = Some(Instant::now());
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
        // keys typed while this window is focused drive the preview
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
        let af = self.audio.as_ref().map(|a| a.frame()).unwrap_or_else(AudioFrame::default);
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

    // ------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(36.0, 36.0), Sense::hover());
            ui.painter().image(self.logo.id(), rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
            ui.add_space(4.0);
            ui.vertical(|ui| {
                ui.add_space(1.0);
                ui.label(RichText::new("Keyboard").family(semi()).size(17.0).color(TEXT));
                ui.label(caption_text("Custom Software"));
            });
        });
        ui.add_space(26.0);
        ui.label(caption_text("Profiles"));
        ui.add_space(6.0);

        let mut action: Option<(&str, usize)> = None;
        for i in 0..self.cfg.profiles.len() {
            if let Some((ri, name)) = &mut self.renaming {
                if *ri == i {
                    let resp = ui.add(egui::TextEdit::singleline(name).desired_width(f32::INFINITY));
                    resp.request_focus();
                    if resp.lost_focus() || ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                        action = Some(("rename_done", i));
                    }
                    continue;
                }
            }
            let active = i == self.cfg.active;
            let name = self.cfg.profiles[i].name.clone();
            let effect = effects::find(&self.cfg.profiles[i].effect).name;
            let resp = profile_row(ui, &name, effect, active);
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
                if ui.button("Move up").clicked() {
                    action = Some(("up", i));
                    ui.close();
                }
                if ui.add_enabled(self.cfg.profiles.len() > 1, egui::Button::new("Delete")).clicked() {
                    action = Some(("delete", i));
                    ui.close();
                }
            });
        }
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            if ui.button("New").clicked() {
                action = Some(("new", 0));
            }
            if ui.button("Duplicate").clicked() {
                action = Some(("dup", self.cfg.active));
            }
            if ui.add_enabled(self.cfg.profiles.len() > 1, egui::Button::new("Delete")).clicked() {
                action = Some(("delete", self.cfg.active));
            }
        });
        ui.add_space(2.0);
        ui.label(RichText::new("Double-click to rename. Right-click for more.").color(DIM).size(11.5));

        if let Some((what, i)) = action {
            match what {
                "select" => self.cfg.active = i,
                "rename" => self.renaming = Some((i, self.cfg.profiles[i].name.clone())),
                "rename_done" => {
                    if let Some((_, name)) = self.renaming.take() {
                        let name = name.trim();
                        if !name.is_empty() {
                            self.cfg.profiles[i].name = name.to_string();
                        }
                    }
                }
                "new" => {
                    let n = self.cfg.profiles.len() + 1;
                    self.cfg.profiles.push(Profile::new(&format!("Profile {n}"), "rainbow"));
                    self.cfg.active = self.cfg.profiles.len() - 1;
                }
                "dup" => {
                    let mut p = self.cfg.profiles[i].clone();
                    p.name = format!("{} copy", p.name);
                    self.cfg.profiles.insert(i + 1, p);
                    self.cfg.active = i + 1;
                }
                "up" if i > 0 => {
                    self.cfg.profiles.swap(i, i - 1);
                    if self.cfg.active == i {
                        self.cfg.active = i - 1;
                    } else if self.cfg.active == i - 1 {
                        self.cfg.active = i;
                    }
                }
                "delete" if self.cfg.profiles.len() > 1 => {
                    self.cfg.profiles.remove(i);
                    if self.cfg.active >= i && self.cfg.active > 0 {
                        self.cfg.active -= 1;
                    }
                }
                _ => {}
            }
            self.changed();
        }

        ui.add_space(28.0);
        ui.label(caption_text("Settings"));
        ui.add_space(6.0);
        let mut paused = self.cfg.paused;
        if ui.checkbox(&mut paused, "Pause lighting").changed() {
            self.cfg.paused = paused;
            self.changed();
        }
        let mut auto = self.status.autostart;
        if ui.checkbox(&mut auto, "Start with Windows").changed() {
            sys::set_autostart(auto);
            self.status.autostart = auto;
        }
        ui.add_space(10.0);
        ui.label(RichText::new("When the app is fully closed").color(DIM).size(12.0));
        let exit_name = exit_label(&self.cfg.exit_mode);
        let mut exit = self.cfg.exit_mode.clone();
        egui::ComboBox::from_id_salt("exit")
            .selected_text(exit_name)
            .width(ui.available_width())
            .height(400.0)
            .show_ui(ui, |ui| {
                ui.selectable_value(&mut exit, "keep".to_string(), "Keep current colors");
                ui.selectable_value(&mut exit, "off".to_string(), "Turn lights off");
                for e in effects::all().iter().filter(|e| matches!(e.mode, Mode::Onboard(_))) {
                    ui.selectable_value(&mut exit, e.id.to_string(), format!("Built-in: {}", e.name));
                }
            });
        if exit != self.cfg.exit_mode {
            self.cfg.exit_mode = exit;
            self.changed();
        }
        ui.add_space(14.0);
        if self.status.service && ui.button("Quit background service").clicked() {
            let _ = std::fs::create_dir_all(config::dir());
            let _ = std::fs::write(daemon::quit_flag(), b"");
            self.status_at = Instant::now() - Duration::from_secs(10);
        }
    }

    // ------------------------------------------------------------ main area

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            pill(ui, self.status.keyboard, "Keyboard connected", "Keyboard not found");
            pill(ui, self.status.service, "Running in background", "Background service stopped");
            if !self.status.service && ui.button("Start").clicked() {
                sys::spawn_self(&["--background"]);
                self.status_at = Instant::now() - Duration::from_secs(10);
            }
            if self.status.vendor {
                pill(ui, false, "", "The Surmen app is open and will fight over the lights");
                if ui.button("Close it").clicked() {
                    sys::kill_process(VENDOR_EXE);
                    self.status_at = Instant::now() - Duration::from_secs(10);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let info = effects::find(&self.cfg.profile().effect);
                ui.label(RichText::new(info.name).family(semi()).size(14.0).color(TEXT));
                ui.label(RichText::new(format!("{}  ·", self.cfg.profile().name)).size(14.0).color(DIM));
            });
        });
    }

    fn keyboard_view(&mut self, ui: &mut egui::Ui) {
        let colors = self.r.key_colors();
        let pad = 0.4;
        let avail = ui.available_width().min(1100.0);
        let unit = (avail / (layout::WIDTH + pad * 2.0)).clamp(28.0, 62.0);
        let size = vec2((layout::WIDTH + pad * 2.0) * unit, (layout::HEIGHT + pad * 2.0) * unit);
        let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let body = resp.rect;
        // aluminium-like case: flat dark slab with a hairline edge
        painter.rect_filled(body, 20.0, Color32::from_rgb(20, 20, 22));
        painter.rect_stroke(body, 20.0, Stroke::new(1.0, LINE), StrokeKind::Inside);
        let origin = body.min + vec2(pad * unit, pad * unit);
        let gap = unit * 0.06;
        let key_rect = |k: &layout::Key| {
            Rect::from_min_size(
                origin + vec2(k.x * unit + gap, k.y * unit + gap),
                vec2(k.w * unit - gap * 2.0, unit - gap * 2.0),
            )
        };

        let pointer = resp.hover_pos();
        let mut hovered = None;
        for (i, k) in KEYS.iter().enumerate() {
            let rect = key_rect(k);
            let c = colors[i];
            let led = Color32::from_rgb(c[0], c[1], c[2]);
            let lum = (c[0] as f32 * 0.3 + c[1] as f32 * 0.59 + c[2] as f32 * 0.11) / 255.0;
            // light spilling around the key
            painter.rect_filled(rect.expand(unit * 0.07), unit * 0.18, led.gamma_multiply(0.3 * lum.max(0.2)));
            let cap = mix(Color32::from_rgb(30, 30, 33), led, 0.22 + 0.5 * lum.min(1.0));
            painter.rect_filled(rect, unit * 0.12, cap);
            let top = rect.shrink2(vec2(unit * 0.07, unit * 0.06)).translate(vec2(0.0, -unit * 0.03));
            painter.rect_filled(top, unit * 0.1, mix(cap, led, 0.55));
            let text = if lum > 0.62 { Color32::from_rgb(18, 18, 20) } else { Color32::from_rgb(236, 236, 240) };
            painter.text(top.center(), Align2::CENTER_CENTER, k.label, FontId::proportional(unit * 0.25), text);
            if let Some(dir) = arrow_dir(k.id) {
                draw_arrow(&painter, top.center(), dir, unit * 0.13, text);
            }
            if pointer.is_some_and(|p| rect.contains(p)) {
                hovered = Some(i);
                painter.rect_stroke(rect, unit * 0.12, Stroke::new(1.5, TEXT), StrokeKind::Outside);
            }
        }
        // knob
        let kc = origin + vec2((layout::KNOB.0 + 0.5) * unit, (layout::KNOB.1 + 0.5) * unit);
        painter.circle_filled(kc, unit * 0.42, Color32::from_rgb(52, 52, 56));
        painter.circle_filled(kc, unit * 0.34, Color32::from_rgb(92, 92, 98));
        painter.circle_stroke(kc, unit * 0.42, Stroke::new(1.0, Color32::from_rgb(24, 24, 26)));

        // painting / testing reactive effects with the mouse
        let painting = self.cfg.profile().effect == "custom";
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
        }
    }

    /// Step (if on screen) and draw one live thumbnail card. Returns the card's response.
    fn thumb_card(&mut self, ui: &mut egui::Ui, key: String, make: impl FnOnce() -> Thumb, w: f32, title: &str, sub: &str, selected: bool) -> egui::Response {
        let preview_h = (w - 20.0) * layout::HEIGHT / layout::WIDTH + 12.0;
        let (rect, resp) = ui.allocate_exact_size(vec2(w, preview_h + 62.0), Sense::click());
        if !ui.is_rect_visible(rect) {
            return resp;
        }
        let t = self.clock();
        let thumb = self.thumbs.entry(key).or_insert_with(make);
        thumb.step(t, &self.fake_audio, &self.fake_screen);
        let p = ui.painter();
        let fill = if resp.hovered() { CARD_HOVER } else { CARD };
        p.rect_filled(rect, 16.0, fill);
        let prev = Rect::from_min_size(rect.min + vec2(10.0, 10.0), vec2(w - 20.0, preview_h));
        mini_keyboard(p, prev, &thumb.out);
        p.text(rect.min + vec2(14.0, preview_h + 22.0), Align2::LEFT_TOP, title, FontId::new(14.5, semi()), TEXT);
        p.text(rect.min + vec2(14.0, preview_h + 42.0), Align2::LEFT_TOP, sub, FontId::proportional(12.0), DIM);
        if selected {
            p.rect_stroke(rect, 16.0, Stroke::new(1.5, TEXT), StrokeKind::Inside);
            p.circle_filled(Pos2::new(rect.max.x - 18.0, rect.min.y + preview_h + 33.0), 4.0, TEXT);
        }
        resp
    }

    fn effects_tab(&mut self, ui: &mut egui::Ui) {
        let mut cats = vec!["All"];
        cats.extend(CATEGORIES.iter().copied());
        if let Some(i) = chips(ui, &cats, self.gallery_cat) {
            self.gallery_cat = i;
        }
        ui.add_space(14.0);
        let current = self.cfg.profile().effect.clone();
        let shown: Vec<&str> = if self.gallery_cat == 0 { CATEGORIES.to_vec() } else { vec![cats[self.gallery_cat]] };
        let (cols, w) = grid_dims(ui.available_width(), 230.0);
        let mut picked = None;
        for cat in shown {
            if self.gallery_cat == 0 {
                ui.label(caption_text(cat));
                ui.add_space(6.0);
            }
            let list: Vec<&'static EffectInfo> = effects::all().iter().filter(|e| e.category == cat).collect();
            for row in list.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 14.0;
                    for e in row {
                        let params = self.cfg.profile().params_for(e.id);
                        let keycolors = profile_keycolors(self.cfg.profile());
                        let sub = short_desc(e.desc);
                        let resp = self.thumb_card(
                            ui,
                            format!("fx:{}", e.id),
                            || Thumb::new(e.id, params.clone(), keycolors),
                            w,
                            e.name,
                            &sub,
                            e.id == current,
                        );
                        // keep the thumbnail in sync with the profile's settings for that effect
                        if let Some(th) = self.thumbs.get_mut(&format!("fx:{}", e.id)) {
                            if e.preview_as.is_none() && th.params != params {
                                th.params = params;
                            }
                        }
                        if resp.clicked() {
                            picked = Some(e.id);
                        }
                        resp.on_hover_text(e.desc);
                    }
                });
                ui.add_space(14.0);
            }
            ui.add_space(8.0);
        }
        if let Some(id) = picked {
            if id != current {
                self.cfg.profile_mut().effect = id.to_string();
                self.changed();
            }
        }
    }

    fn customize_tab(&mut self, ui: &mut egui::Ui) {
        let profile_effect = self.cfg.profile().effect.clone();
        let info = effects::find(&profile_effect);

        ui.columns(2, |cols| {
            // left: effect + its settings
            let ui = &mut cols[0];
            ui.label(caption_text("Effect"));
            ui.add_space(2.0);
            let mut chosen = profile_effect.clone();
            egui::ComboBox::from_id_salt("effect")
                .selected_text(RichText::new(info.name).family(semi()).size(16.0))
                .width(300.0)
                .height(560.0)
                .show_ui(ui, |ui| {
                    for cat in CATEGORIES {
                        ui.add_space(6.0);
                        ui.label(caption_text(cat));
                        for e in effects::all().iter().filter(|e| e.category == *cat) {
                            ui.selectable_value(&mut chosen, e.id.to_string(), e.name);
                        }
                    }
                });
            if chosen != profile_effect {
                self.cfg.profile_mut().effect = chosen;
                self.changed();
                return;
            }
            ui.add_space(2.0);
            ui.label(RichText::new(info.desc).color(DIM));
            if matches!(info.mode, Mode::Onboard(_)) {
                ui.label(
                    RichText::new("Runs on the keyboard's own chip: super smooth, zero CPU. The preview is only an approximation.")
                        .color(DIM)
                        .size(11.5),
                );
            }
            ui.add_space(14.0);

            egui::Grid::new("params").num_columns(2).spacing([22.0, 12.0]).show(ui, |ui| {
                ui.label(RichText::new("Brightness").color(DIM));
                let mut b = self.cfg.profile().brightness;
                if ui.add(egui::Slider::new(&mut b, 0.0..=1.0).show_value(false)).changed() {
                    self.cfg.profile_mut().brightness = b;
                    self.changed();
                }
                ui.end_row();

                let mut map = self.cfg.profile().params_for(&profile_effect);
                let mut any = false;
                for def in info.params {
                    ui.label(RichText::new(def.label).color(DIM));
                    let cur = Params { defs: info.params, map: &map };
                    match def.kind {
                        Kind::Color(_) => {
                            let mut c = cur.rgb8(def.key);
                            if egui::color_picker::color_edit_button_srgb(ui, &mut c).changed() {
                                map.insert(def.key.into(), color::to_hex(c).into());
                                any = true;
                            }
                        }
                        Kind::Float(_) => {
                            let mut v = cur.f(def.key);
                            if ui.add(egui::Slider::new(&mut v, 0.0..=1.0).show_value(false)).changed() {
                                map.insert(def.key.into(), serde_json::json!(v));
                                any = true;
                            }
                        }
                        Kind::Choice(opts, _) => {
                            let mut idx = cur.choice(def.key);
                            let before = idx;
                            egui::ComboBox::from_id_salt(def.key).selected_text(opts[idx]).width(190.0).show_ui(ui, |ui| {
                                for (j, o) in opts.iter().enumerate() {
                                    ui.selectable_value(&mut idx, j, *o);
                                }
                            });
                            if idx != before {
                                map.insert(def.key.into(), opts[idx].into());
                                any = true;
                            }
                        }
                        Kind::Bool(_) => {
                            let mut v = cur.b(def.key);
                            if toggle(ui, &mut v).changed() {
                                map.insert(def.key.into(), v.into());
                                any = true;
                            }
                        }
                    }
                    ui.end_row();
                }
                if any {
                    self.cfg.profile_mut().params.insert(profile_effect.clone(), map);
                    self.changed();
                }
            });
            if profile_effect == "heatmap" {
                ui.add_space(12.0);
                if ui.button("Reset heatmap").on_hover_text("Forget all typing counts and start fresh").clicked() {
                    effects::heatmap_reset();
                }
            }

            // right: quick colors / painting
            let ui = &mut cols[1];
            let painting = profile_effect == "custom";
            if painting {
                ui.label(caption_text("Paint keys"));
                ui.label(RichText::new("Pick a color, then click or drag over keys in the picture.").color(DIM));
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("Brush");
                    egui::color_picker::color_edit_button_srgb(ui, &mut self.brush);
                    if ui.button("Fill all").clicked() {
                        let hex = color::to_hex(self.brush);
                        let p = self.cfg.profile_mut();
                        for k in KEYS {
                            p.keycolors.insert(k.id.to_string(), hex.clone());
                        }
                        self.changed();
                    }
                    if ui.button("Clear").clicked() {
                        self.cfg.profile_mut().keycolors.clear();
                        self.changed();
                    }
                });
            } else {
                ui.label(caption_text("Quick colors"));
            }
            ui.add_space(6.0);
            let first_color = info.params.iter().find(|d| matches!(d.kind, Kind::Color(_))).map(|d| d.key);
            let mut picked = None;
            for row in SWATCHES.chunks(7) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 10.0;
                    for c in row {
                        let (rect, resp) = ui.allocate_exact_size(vec2(30.0, 30.0), Sense::click());
                        let col = Color32::from_rgb(c[0], c[1], c[2]);
                        ui.painter().circle_filled(rect.center(), 13.0, col);
                        if resp.hovered() {
                            ui.painter().circle_stroke(rect.center(), 15.5, Stroke::new(1.5, TEXT));
                        }
                        if resp.clicked() {
                            picked = Some(*c);
                        }
                    }
                });
            }
            if let Some(c) = picked {
                if painting {
                    self.brush = c;
                } else if let Some(key) = first_color {
                    let mut map = self.cfg.profile().params_for(&profile_effect);
                    map.insert(key.into(), color::to_hex(c).into());
                    self.cfg.profile_mut().params.insert(profile_effect.clone(), map);
                    self.changed();
                }
            }
            let hint = |ui: &mut egui::Ui, s: &str| {
                ui.add_space(12.0);
                ui.label(RichText::new(s).color(DIM).size(12.0));
            };
            if !painting && first_color.is_none() {
                hint(ui, "This effect picks its own colors.");
            }
            if info.uses_keys {
                hint(ui, "Tip: type here or click keys in the picture to test it.");
            }
            if info.uses_audio {
                hint(ui, "Plays along with any sound on your PC (music, videos, games).");
            }
            if info.uses_screen {
                hint(ui, "Follows your main screen live. Nothing is saved or sent anywhere. Some protected videos show up black.");
            }
        });
    }

    fn themes_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Click a theme to apply it to the current profile.").color(DIM));
        ui.add_space(12.0);
        let (cols, w) = grid_dims(ui.available_width(), 230.0);
        let mut apply = None;
        let n = self.presets.len();
        for start in (0..n).step_by(cols) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 14.0;
                for i in start..(start + cols).min(n) {
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
                    let resp = self.thumb_card(ui, format!("theme:{i}"), || Thumb::new(effect, params, kc), w, name, sub, false);
                    if resp.clicked() {
                        apply = Some(i);
                    }
                }
            });
            ui.add_space(14.0);
        }
        if let Some(i) = apply {
            let preset = &self.presets[i];
            self.cfg.profile_mut().apply_preset(preset);
            self.changed();
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.status_at.elapsed() > Duration::from_secs(2) {
            self.refresh_status();
        }
        self.sync_file();
        self.update_preview(&ctx);
        if self.tab != Tab::Customize {
            self.update_fakes();
        } else if !self.thumbs.is_empty() {
            // free the thumbnails while they aren't shown
            self.thumbs.clear();
        }

        egui::Panel::left("side")
            .exact_size(256.0)
            .resizable(false)
            .frame(
                egui::Frame::new()
                    .fill(PANEL)
                    .inner_margin(egui::Margin::symmetric(18, 8))
                    .stroke(Stroke::new(1.0, Color32::from_rgb(26, 26, 28))),
            )
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.sidebar(ui));
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::symmetric(26, 20)))
            .show(ui, |ui| {
                self.status_bar(ui);
                ui.add_space(14.0);
                ui.vertical_centered(|ui| self.keyboard_view(ui));
                ui.add_space(18.0);
                ui.horizontal(|ui| {
                    let tabs = [(Tab::Effects, "Effects"), (Tab::Customize, "Customize"), (Tab::Themes, "Themes")];
                    let names: Vec<&str> = tabs.iter().map(|t| t.1).collect();
                    let cur = tabs.iter().position(|t| t.0 == self.tab).unwrap_or(0);
                    if let Some(i) = segmented(ui, &names, cur) {
                        self.tab = tabs[i].0;
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(RichText::new("Closing this window keeps your lighting running in the tray.").color(DIM).size(11.5));
                    });
                });
                ui.add_space(14.0);
                egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                    Tab::Effects => self.effects_tab(ui),
                    Tab::Customize => self.customize_tab(ui),
                    Tab::Themes => self.themes_tab(ui),
                });
            });

        ctx.request_repaint_after(Duration::from_millis(16));
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.dirty.is_some() {
            let _ = config::save(&self.cfg);
        }
    }
}

// ---------------------------------------------------------------- helpers

fn semi() -> FontFamily {
    FontFamily::Name("semi".into())
}

/// Use Windows' Segoe UI (regular + semibold) when it's there; egui's built-in font otherwise.
fn fonts(ctx: &egui::Context) {
    let mut f = egui::FontDefinitions::default();
    let dir = std::path::PathBuf::from(std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into())).join("Fonts");
    let mut semi_list = Vec::new();
    if let Ok(b) = std::fs::read(dir.join("segoeui.ttf")) {
        f.font_data.insert("segoe".into(), Arc::new(egui::FontData::from_owned(b)));
        if let Some(list) = f.families.get_mut(&FontFamily::Proportional) {
            list.insert(0, "segoe".into());
        }
    }
    if let Ok(b) = std::fs::read(dir.join("seguisb.ttf")) {
        f.font_data.insert("segoe_sb".into(), Arc::new(egui::FontData::from_owned(b)));
        semi_list.push("segoe_sb".to_string());
    }
    semi_list.extend(f.families.get(&FontFamily::Proportional).cloned().unwrap_or_default());
    f.families.insert(semi(), semi_list);
    ctx.set_fonts(f);
}

fn style(ctx: &egui::Context) {
    fonts(ctx);
    let mut v = egui::Visuals::dark();
    let r = CornerRadius::same(8);
    v.panel_fill = BG;
    v.window_fill = Color32::from_rgb(22, 22, 24);
    v.window_stroke = Stroke::new(1.0, LINE);
    v.window_corner_radius = CornerRadius::same(12);
    v.menu_corner_radius = CornerRadius::same(10);
    v.extreme_bg_color = Color32::from_rgb(6, 6, 7);
    v.faint_bg_color = CARD;
    // selected things are white with dark text, like an inverted pill
    v.selection.bg_fill = Color32::from_rgb(236, 236, 240);
    v.selection.stroke = Stroke::new(1.0, Color32::from_rgb(12, 12, 14));
    v.hyperlink_color = TEXT;
    v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(226, 226, 230));
    v.widgets.noninteractive.corner_radius = r;
    v.widgets.inactive.bg_fill = Color32::from_rgb(52, 52, 56);
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.inactive.bg_stroke = Stroke::new(1.0, LINE);
    v.widgets.inactive.fg_stroke = Stroke::new(1.0, TEXT);
    v.widgets.inactive.corner_radius = r;
    v.widgets.hovered.bg_fill = Color32::from_rgb(70, 70, 75);
    v.widgets.hovered.weak_bg_fill = CARD_HOVER;
    v.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(70, 70, 75));
    v.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.hovered.corner_radius = r;
    v.widgets.active.bg_fill = Color32::from_rgb(90, 90, 96);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(44, 44, 48);
    v.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(110, 110, 116));
    v.widgets.active.fg_stroke = Stroke::new(1.0, Color32::WHITE);
    v.widgets.active.corner_radius = r;
    v.widgets.open.weak_bg_fill = CARD_HOVER;
    v.widgets.open.corner_radius = r;
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 7.0);
        s.spacing.button_padding = vec2(12.0, 6.0);
        s.spacing.slider_width = 230.0;
        s.spacing.interact_size.y = 26.0;
        s.text_styles.insert(egui::TextStyle::Body, FontId::proportional(14.0));
        s.text_styles.insert(egui::TextStyle::Button, FontId::proportional(14.0));
    });
}

/// Small grey all-caps label with wide letter spacing.
fn caption_text(s: &str) -> RichText {
    RichText::new(s.to_uppercase()).size(10.5).color(DIM).extra_letter_spacing(1.4)
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

fn pill(ui: &mut egui::Ui, ok: bool, yes: &str, no: &str) {
    let (c, t) = if ok { (GOOD, yes) } else { (WARN, no) };
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 18.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 3.5, c);
    ui.label(RichText::new(t).color(if ok { DIM } else { c }).size(13.0));
    ui.add_space(12.0);
}

/// iOS-style on/off switch.
fn toggle(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut resp) = ui.allocate_exact_size(vec2(40.0, 22.0), Sense::click());
    if resp.clicked() {
        *on = !*on;
        resp.mark_changed();
    }
    let t = ui.ctx().animate_bool(resp.id, *on);
    let p = ui.painter();
    let track = mix(Color32::from_rgb(58, 58, 62), Color32::from_rgb(236, 236, 240), t);
    p.rect_filled(rect, 11.0, track);
    let x = rect.left() + 11.0 + t * (rect.width() - 22.0);
    let knob = mix(Color32::from_rgb(220, 220, 224), Color32::from_rgb(14, 14, 16), t);
    p.circle_filled(Pos2::new(x, rect.center().y), 8.0, knob);
    resp
}

/// Segmented control (one option selected). Returns the clicked index.
fn segmented(ui: &mut egui::Ui, items: &[&str], selected: usize) -> Option<usize> {
    let font = FontId::new(14.0, semi());
    let widths: Vec<f32> = items
        .iter()
        .map(|s| ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), TEXT).size().x) + 34.0)
        .collect();
    let total: f32 = widths.iter().sum::<f32>() + 8.0;
    let (rect, _) = ui.allocate_exact_size(vec2(total, 36.0), Sense::hover());
    ui.painter().rect_filled(rect, 18.0, CARD);
    let mut x = rect.min.x + 4.0;
    let mut clicked = None;
    for (i, (s, w)) in items.iter().zip(&widths).enumerate() {
        let r = Rect::from_min_size(Pos2::new(x, rect.min.y + 4.0), vec2(*w, 28.0));
        let resp = ui.interact(r, ui.scope_id().with(("seg", i)), Sense::click());
        let sel = i == selected;
        if sel {
            ui.painter().rect_filled(r, 14.0, Color32::from_rgb(236, 236, 240));
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 14.0, CARD_HOVER);
        }
        let col = if sel { Color32::from_rgb(12, 12, 14) } else { DIM };
        ui.painter().text(r.center(), Align2::CENTER_CENTER, *s, font.clone(), col);
        if resp.clicked() {
            clicked = Some(i);
        }
        x += w;
    }
    clicked
}

/// Row of filter chips. Returns the clicked index.
fn chips(ui: &mut egui::Ui, items: &[&str], selected: usize) -> Option<usize> {
    let mut clicked = None;
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        for (i, s) in items.iter().enumerate() {
            let font = FontId::proportional(13.0);
            let w = ui.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font.clone(), TEXT).size().x) + 26.0;
            let (r, resp) = ui.allocate_exact_size(vec2(w, 30.0), Sense::click());
            let sel = i == selected;
            let (fill, col) = if sel {
                (Color32::from_rgb(236, 236, 240), Color32::from_rgb(12, 12, 14))
            } else if resp.hovered() {
                (CARD_HOVER, TEXT)
            } else {
                (CARD, DIM)
            };
            ui.painter().rect_filled(r, 15.0, fill);
            ui.painter().text(r.center(), Align2::CENTER_CENTER, *s, font, col);
            if resp.clicked() {
                clicked = Some(i);
            }
        }
    });
    clicked
}

/// How many cards fit in a row, and how wide each one is.
fn grid_dims(avail: f32, target: f32) -> (usize, f32) {
    let gap = 14.0;
    let cols = (((avail + gap) / (target + gap)).floor() as usize).max(2);
    (cols, ((avail - gap * (cols as f32 - 1.0)) / cols as f32 - 1.0).floor())
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

/// A tiny keyboard drawn from key colors.
fn mini_keyboard(p: &egui::Painter, rect: Rect, colors: &[Rgb]) {
    p.rect_filled(rect, 10.0, Color32::from_rgb(6, 6, 7));
    let inner = rect.shrink(6.0);
    let unit = (inner.width() / layout::WIDTH).min(inner.height() / layout::HEIGHT);
    let size = vec2(layout::WIDTH * unit, layout::HEIGHT * unit);
    let origin = inner.center() - size / 2.0;
    for (k, c) in KEYS.iter().zip(colors) {
        let r = Rect::from_min_size(origin + vec2(k.x * unit, k.y * unit), vec2(k.w * unit, unit)).shrink(unit * 0.09);
        let c = color::to_u8(*c);
        let led = Color32::from_rgb(c[0].max(22), c[1].max(22), c[2].max(24));
        p.rect_filled(r, unit * 0.2, led);
    }
}

fn profile_row(ui: &mut egui::Ui, name: &str, effect: &str, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 48.0), Sense::click());
    let p = ui.painter();
    let bg = if active {
        Color32::from_rgb(30, 30, 33)
    } else if resp.hovered() {
        CARD
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 10.0, bg);
    let name_col = if active { TEXT } else { Color32::from_rgb(200, 200, 205) };
    p.text(rect.min + vec2(14.0, 8.0), Align2::LEFT_TOP, name, FontId::new(14.5, semi()), name_col);
    p.text(rect.min + vec2(14.0, 27.0), Align2::LEFT_TOP, effect, FontId::proportional(12.0), DIM);
    if active {
        p.circle_filled(Pos2::new(rect.max.x - 16.0, rect.center().y), 3.5, TEXT);
    }
    resp
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
