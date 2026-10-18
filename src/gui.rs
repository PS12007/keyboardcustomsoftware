//! Settings window. A separate process that only runs while it's open; it edits
//! config.json and the background service picks the changes up.

use crate::audio::AudioCapture;
use crate::color;
use crate::config::{self, Config, Preset, Profile};
use crate::daemon;
use crate::effects::{self, AudioFrame, Kind, Mode, Params, CATEGORIES};
use crate::engine::Renderer;
use crate::icon;
use crate::layout::{self, KEYS};
use crate::protocol::Keyboard;
use crate::sys::{self, InstanceLock, APP_TITLE, DAEMON_MUTEX, GUI_MUTEX, VENDOR_EXE};
use eframe::egui::{
    self, vec2, Align2, Color32, FontId, Pos2, Rect, RichText, Sense, Stroke, StrokeKind, Vec2,
};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

const BG: Color32 = Color32::from_rgb(13, 15, 21);
const PANEL: Color32 = Color32::from_rgb(19, 22, 30);
const CARD: Color32 = Color32::from_rgb(26, 30, 40);
const ACCENT: Color32 = Color32::from_rgb(0, 229, 255);
const DIM: Color32 = Color32::from_rgb(140, 150, 170);
const GOOD: Color32 = Color32::from_rgb(60, 230, 120);
const WARN: Color32 = Color32::from_rgb(255, 180, 40);

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
            .with_inner_size([1180.0, 780.0])
            .with_min_inner_size([920.0, 620.0])
            .with_icon(Arc::new(icon)),
        ..Default::default()
    };
    let _ = eframe::run_native(APP_TITLE, opts, Box::new(|cc| Ok(Box::new(App::new(cc)))));
    drop(lock);
}

#[derive(PartialEq, Clone, Copy)]
enum Tab {
    Effects,
    Themes,
}

struct Status {
    service: bool,
    keyboard: bool,
    vendor: bool,
    autostart: bool,
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
    brush: [u8; 3],
    renaming: Option<(usize, String)>,
    status: Status,
    status_at: Instant,
    hid: Option<hidapi::HidApi>,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> Self {
        style(&cc.egui_ctx);
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
            brush: [255, 0, 60],
            renaming: None,
            status: Status { service: true, keyboard: true, vendor: false, autostart: false },
            status_at: Instant::now() - Duration::from_secs(10),
            hid: hidapi::HidApi::new().ok(),
        };
        app.refresh_status();
        app
    }

    fn changed(&mut self) {
        self.dirty = Some(Instant::now());
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

    // ------------------------------------------------------------ sidebar

    fn sidebar(&mut self, ui: &mut egui::Ui) {
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::hover());
            draw_logo(ui.painter(), rect);
            ui.vertical(|ui| {
                ui.label(RichText::new("KEYBOARD").strong().size(15.0));
                ui.label(RichText::new("CUSTOM SOFTWARE").color(ACCENT).size(11.0));
            });
        });
        ui.add_space(18.0);
        ui.label(RichText::new("PROFILES").color(DIM).size(11.0));
        ui.add_space(4.0);

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
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui.button("+ New").clicked() {
                action = Some(("new", 0));
            }
            if ui.button("Duplicate").clicked() {
                action = Some(("dup", self.cfg.active));
            }
            if ui.add_enabled(self.cfg.profiles.len() > 1, egui::Button::new("Delete")).clicked() {
                action = Some(("delete", self.cfg.active));
            }
        });
        ui.label(RichText::new("Double-click a profile to rename it, right-click for more.").color(DIM).size(11.0));

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

        ui.add_space(22.0);
        ui.label(RichText::new("SETTINGS").color(DIM).size(11.0));
        ui.add_space(4.0);
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
        ui.add_space(6.0);
        ui.label(RichText::new("When the app is fully closed:").size(12.0));
        let exit_name = exit_label(&self.cfg.exit_mode);
        let mut exit = self.cfg.exit_mode.clone();
        egui::ComboBox::from_id_salt("exit")
            .selected_text(exit_name)
            .width(200.0)
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
        ui.add_space(10.0);
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
            pill(
                ui,
                self.status.service,
                "Running in background",
                "Background service stopped",
            );
            if !self.status.service {
                if ui.button("Start").clicked() {
                    sys::spawn_self(&["--background"]);
                    self.status_at = Instant::now() - Duration::from_secs(10);
                }
            }
            if self.status.vendor {
                pill(ui, false, "", "The Surmen app is open and will fight over the lights");
                if ui.button("Close it").clicked() {
                    sys::kill_process(VENDOR_EXE);
                    self.status_at = Instant::now() - Duration::from_secs(10);
                }
            }
        });
    }

    fn keyboard_view(&mut self, ui: &mut egui::Ui) {
        let colors = self.r.key_colors();
        let pad = 0.35;
        let avail = ui.available_width().min(1100.0);
        let unit = (avail / (layout::WIDTH + pad * 2.0)).clamp(28.0, 64.0);
        let size = vec2((layout::WIDTH + pad * 2.0) * unit, (layout::HEIGHT + pad * 2.0) * unit);
        let (resp, painter) = ui.allocate_painter(size, Sense::click_and_drag());
        let body = resp.rect;
        painter.rect_filled(body.expand(2.0), 18.0, Color32::from_rgb(40, 44, 54));
        painter.rect_filled(body, 16.0, Color32::from_rgb(17, 19, 25));
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
            // glow under the key
            painter.rect_filled(rect.expand(unit * 0.08), unit * 0.18, led.gamma_multiply(0.35 * lum.max(0.25)));
            // keycap
            let cap = mix(Color32::from_rgb(28, 31, 40), led, 0.25 + 0.5 * lum.min(1.0));
            painter.rect_filled(rect, unit * 0.12, cap);
            let top = rect.shrink2(vec2(unit * 0.07, unit * 0.06)).translate(vec2(0.0, -unit * 0.03));
            painter.rect_filled(top, unit * 0.1, mix(cap, led, 0.55));
            let text = if lum > 0.62 { Color32::from_rgb(20, 20, 25) } else { Color32::from_rgb(235, 238, 245) };
            painter.text(top.center(), Align2::CENTER_CENTER, k.label, FontId::proportional(unit * 0.26), text);
            if let Some(dir) = arrow_dir(k.id) {
                draw_arrow(&painter, top.center(), dir, unit * 0.13, text);
            }
            if pointer.is_some_and(|p| rect.contains(p)) {
                hovered = Some(i);
                painter.rect_stroke(rect, unit * 0.12, Stroke::new(2.0, ACCENT), StrokeKind::Outside);
            }
        }
        // knob
        let kc = origin + vec2((layout::KNOB.0 + 0.5) * unit, (layout::KNOB.1 + 0.5) * unit);
        painter.circle_filled(kc, unit * 0.42, Color32::from_rgb(70, 74, 84));
        painter.circle_filled(kc, unit * 0.34, Color32::from_rgb(120, 125, 136));
        painter.circle_stroke(kc, unit * 0.42, Stroke::new(1.5, Color32::from_rgb(30, 32, 38)));

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

    fn effects_tab(&mut self, ui: &mut egui::Ui) {
        let profile_effect = self.cfg.profile().effect.clone();
        let info = effects::find(&profile_effect);

        ui.columns(2, |cols| {
            // left: effect + its settings
            let ui = &mut cols[0];
            ui.label(RichText::new("EFFECT").color(DIM).size(11.0));
            let mut chosen = profile_effect.clone();
            egui::ComboBox::from_id_salt("effect")
                .selected_text(RichText::new(info.name).size(16.0))
                .width(300.0)
                .height(560.0)
                .show_ui(ui, |ui| {
                    for cat in CATEGORIES {
                        ui.add_space(4.0);
                        ui.label(RichText::new(*cat).color(ACCENT).size(12.0).strong());
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
            ui.label(RichText::new(info.desc).color(DIM));
            if matches!(info.mode, Mode::Onboard(_)) {
                ui.label(
                    RichText::new("Runs on the keyboard's own chip: super smooth, zero CPU. The preview above is only an approximation.")
                        .color(DIM)
                        .size(11.0),
                );
            }
            ui.add_space(10.0);

            egui::Grid::new("params").num_columns(2).spacing([18.0, 10.0]).show(ui, |ui| {
                ui.label("Brightness");
                let mut b = self.cfg.profile().brightness;
                if ui.add(egui::Slider::new(&mut b, 0.0..=1.0).show_value(false)).changed() {
                    self.cfg.profile_mut().brightness = b;
                    self.changed();
                }
                ui.end_row();

                let mut map = self.cfg.profile().params_for(&profile_effect);
                let mut any = false;
                for def in info.params {
                    ui.label(def.label);
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
                            egui::ComboBox::from_id_salt(def.key).selected_text(opts[idx]).width(180.0).show_ui(ui, |ui| {
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
                            if ui.checkbox(&mut v, "").changed() {
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

            // right: quick colors / painting
            let ui = &mut cols[1];
            let painting = profile_effect == "custom";
            if painting {
                ui.label(RichText::new("PAINT KEYS").color(DIM).size(11.0));
                ui.label("Pick a color, then click or drag over keys in the picture.");
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
                ui.label(RichText::new("QUICK COLORS").color(DIM).size(11.0));
            }
            ui.add_space(4.0);
            let first_color = info.params.iter().find(|d| matches!(d.kind, Kind::Color(_))).map(|d| d.key);
            let mut picked = None;
            for row in SWATCHES.chunks(7) {
                ui.horizontal(|ui| {
                    for c in row {
                        let (rect, resp) = ui.allocate_exact_size(vec2(34.0, 34.0), Sense::click());
                        let col = Color32::from_rgb(c[0], c[1], c[2]);
                        ui.painter().rect_filled(rect, 6.0, col);
                        if resp.hovered() {
                            ui.painter().rect_stroke(rect, 6.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Outside);
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
            if !painting && first_color.is_none() {
                ui.label(RichText::new("This effect picks its own colors.").color(DIM).size(11.0));
            }
            if info.uses_keys {
                ui.add_space(10.0);
                ui.label(RichText::new("Tip: type here or click keys in the picture to test it.").color(DIM).size(11.0));
            }
            if info.uses_audio {
                ui.add_space(10.0);
                ui.label(RichText::new("Plays along with any sound on your PC (music, videos, games).").color(DIM).size(11.0));
            }
        });
    }

    fn themes_tab(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Click a theme to apply it to the current profile.").color(DIM));
        ui.add_space(6.0);
        let mut apply = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = vec2(10.0, 10.0);
            for (i, p) in self.presets.iter().enumerate() {
                let (rect, resp) = ui.allocate_exact_size(vec2(168.0, 78.0), Sense::click());
                theme_card(ui.painter(), rect, p, resp.hovered());
                if resp.clicked() {
                    apply = Some(i);
                }
                resp.on_hover_text(format!("{} effect", effects::find(p.effect).name));
            }
        });
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

        egui::Panel::left("side")
            .exact_size(250.0)
            .resizable(false)
            .frame(egui::Frame::new().fill(PANEL).inner_margin(egui::Margin::symmetric(16, 8)))
            .show(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| self.sidebar(ui));
            });

        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(BG).inner_margin(egui::Margin::same(18)))
            .show(ui, |ui| {
                self.status_bar(ui);
                ui.add_space(10.0);
                ui.vertical_centered(|ui| self.keyboard_view(ui));
                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    for (t, name) in [(Tab::Effects, "Effects"), (Tab::Themes, "Themes")] {
                        let sel = self.tab == t;
                        let txt = RichText::new(name).size(15.0).color(if sel { ACCENT } else { DIM });
                        if ui.add(egui::Button::new(txt).selected(sel).frame(sel)).clicked() {
                            self.tab = t;
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.label(
                            RichText::new("Closing this window keeps your lighting running in the tray.")
                                .color(DIM)
                                .size(11.0),
                        );
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| match self.tab {
                    Tab::Effects => self.effects_tab(ui),
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

fn style(ctx: &egui::Context) {
    let mut v = egui::Visuals::dark();
    v.panel_fill = BG;
    v.window_fill = PANEL;
    v.extreme_bg_color = Color32::from_rgb(10, 12, 16);
    v.faint_bg_color = CARD;
    v.selection.bg_fill = Color32::from_rgb(0, 110, 130);
    v.selection.stroke = Stroke::new(1.0, ACCENT);
    v.hyperlink_color = ACCENT;
    v.widgets.inactive.bg_fill = CARD;
    v.widgets.inactive.weak_bg_fill = CARD;
    v.widgets.hovered.weak_bg_fill = Color32::from_rgb(36, 42, 56);
    v.widgets.active.weak_bg_fill = Color32::from_rgb(0, 90, 110);
    v.slider_trailing_fill = true;
    ctx.set_visuals(v);
    ctx.global_style_mut(|s| {
        s.spacing.item_spacing = vec2(8.0, 7.0);
        s.spacing.button_padding = vec2(10.0, 5.0);
        s.spacing.slider_width = 220.0;
    });
}

fn mix(a: Color32, b: Color32, t: f32) -> Color32 {
    let t = t.clamp(0.0, 1.0);
    let f = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color32::from_rgb(f(a.r(), b.r()), f(a.g(), b.g()), f(a.b(), b.b()))
}

fn pill(ui: &mut egui::Ui, ok: bool, yes: &str, no: &str) {
    let (c, t) = if ok { (GOOD, yes) } else { (WARN, no) };
    let (rect, _) = ui.allocate_exact_size(vec2(10.0, 18.0), Sense::hover());
    ui.painter().circle_filled(rect.center(), 4.5, c);
    ui.label(RichText::new(t).color(c));
    ui.add_space(8.0);
}

fn profile_row(ui: &mut egui::Ui, name: &str, effect: &str, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 46.0), Sense::click());
    let p = ui.painter();
    let bg = if active {
        Color32::from_rgb(0, 60, 72)
    } else if resp.hovered() {
        CARD
    } else {
        Color32::TRANSPARENT
    };
    p.rect_filled(rect, 8.0, bg);
    if active {
        p.rect_filled(Rect::from_min_size(rect.min, vec2(4.0, rect.height())), 2.0, ACCENT);
    }
    p.text(rect.min + vec2(14.0, 8.0), Align2::LEFT_TOP, name, FontId::proportional(15.0), Color32::WHITE);
    p.text(rect.min + vec2(14.0, 27.0), Align2::LEFT_TOP, effect, FontId::proportional(11.5), DIM);
    resp
}

fn theme_card(p: &egui::Painter, rect: Rect, preset: &Preset, hovered: bool) {
    let cols: Vec<Color32> = preset
        .swatch
        .iter()
        .map(|h| {
            let c = color::parse_hex(h).unwrap_or([0, 0, 0]);
            Color32::from_rgb(c[0], c[1], c[2])
        })
        .collect();
    // horizontal gradient through the three swatch colors
    let steps = 40;
    let w = rect.width() / steps as f32;
    for s in 0..steps {
        let t = s as f32 / (steps - 1) as f32 * 2.0;
        let c = if t < 1.0 { mix(cols[0], cols[1], t) } else { mix(cols[1], cols[2], t - 1.0) };
        let r = Rect::from_min_size(Pos2::new(rect.min.x + s as f32 * w, rect.min.y), vec2(w + 1.0, rect.height()));
        p.rect_filled(r, 0.0, c);
    }
    // round the corners by drawing the frame over it
    p.rect_stroke(rect, 10.0, Stroke::new(4.0, BG), StrokeKind::Outside);
    let shade = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 30.0), rect.max);
    p.rect_filled(shade, egui::CornerRadius { nw: 0, ne: 0, sw: 10, se: 10 }, Color32::from_black_alpha(150));
    p.text(shade.left_center() + vec2(10.0, 0.0), Align2::LEFT_CENTER, preset.name, FontId::proportional(14.0), Color32::WHITE);
    let border = if hovered { Stroke::new(2.0, ACCENT) } else { Stroke::new(1.0, Color32::from_white_alpha(30)) };
    p.rect_stroke(rect, 10.0, border, StrokeKind::Inside);
}

fn draw_logo(p: &egui::Painter, rect: Rect) {
    p.rect_filled(rect, 8.0, CARD);
    let inner = rect.shrink(6.0);
    for y in 0..3 {
        for x in 0..4 {
            let c = color::hsv(x as f32 / 4.0 * 0.8 + y as f32 * 0.05, 0.85, 1.0);
            let c = Color32::from_rgb((c[0] * 255.0) as u8, (c[1] * 255.0) as u8, (c[2] * 255.0) as u8);
            let cell = Vec2::new(inner.width() / 4.0, inner.height() / 3.0);
            let r = Rect::from_min_size(inner.min + vec2(x as f32 * cell.x, y as f32 * cell.y), cell).shrink(1.2);
            p.rect_filled(r, 2.0, c);
        }
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
