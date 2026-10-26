//! Settings and profiles, stored as JSON in %APPDATA%\KeyboardCustomSoftware\config.json.
//! The settings window writes this file; the background service watches it.

use crate::color::Rng;
use crate::effects;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::SystemTime;

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub effect: String,
    #[serde(default = "one")]
    pub brightness: f32,
    /// effect id -> that effect's settings (kept per effect so switching back remembers them)
    #[serde(default)]
    pub params: BTreeMap<String, Map<String, Value>>,
    /// key id -> "#rrggbb" for the Custom Keys effect
    #[serde(default)]
    pub keycolors: BTreeMap<String, String>,
}

fn one() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Config {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub active: usize,
    #[serde(default)]
    pub paused: bool,
    /// What the keyboard shows after the service stops: "keep", "off" or an onboard effect id.
    #[serde(default = "default_exit")]
    pub exit_mode: String,
    /// A look picked by the roster or "Surprise me". Shown instead of the active
    /// profile until a profile is picked again. Edits go to it too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub live: Option<Profile>,
    #[serde(default)]
    pub roster: Roster,
    /// Fn + key shortcuts
    #[serde(default = "default_hotkeys")]
    pub hotkeys: Vec<Hotkey>,
    /// effect ids starred in the gallery
    #[serde(default)]
    pub favorites: Vec<String>,
}

/// The list of looks Fn+\ steps through.
#[derive(Serialize, Deserialize, Clone, PartialEq, Default)]
pub struct Roster {
    /// "p:<profile name>", "t:<theme name>", "e:<effect id>" or "surprise".
    /// Empty = step through all profiles.
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub pos: usize,
    #[serde(default)]
    pub shuffle: bool,
    /// switch on its own every this many minutes (0 = never)
    #[serde(default)]
    pub auto_minutes: u32,
}

#[derive(Serialize, Deserialize, Clone, PartialEq)]
pub struct Hotkey {
    /// key id from the layout, pressed together with Fn
    pub key: String,
    pub action: String,
}

/// (id, label) of everything a shortcut can do.
pub const ACTIONS: &[(&str, &str)] = &[
    ("next", "Next in roster"),
    ("prev", "Previous in roster"),
    ("surprise", "Surprise me (random look)"),
    ("next_profile", "Next profile"),
    ("pause", "Lights on / off"),
    ("bright_up", "Brighter"),
    ("bright_down", "Dimmer"),
];

pub fn action_label(id: &str) -> &'static str {
    ACTIONS.iter().find(|a| a.0 == id).map(|a| a.1).unwrap_or("Nothing")
}

fn default_hotkeys() -> Vec<Hotkey> {
    vec![Hotkey { key: "backslash".into(), action: "next".into() }]
}

fn default_exit() -> String {
    "hw_spectrum".into()
}

impl Profile {
    pub fn new(name: &str, effect: &str) -> Self {
        Profile {
            name: name.into(),
            effect: effect.into(),
            brightness: 1.0,
            params: BTreeMap::new(),
            keycolors: BTreeMap::new(),
        }
    }

    pub fn params_for(&self, effect: &str) -> Map<String, Value> {
        self.params.get(effect).cloned().unwrap_or_default()
    }

    /// Apply a theme preset.
    pub fn apply_preset(&mut self, p: &Preset) {
        self.effect = p.effect.into();
        if let Value::Object(m) = &p.params {
            self.params.insert(p.effect.into(), m.clone());
        }
        if !p.keycolors.is_empty() {
            self.keycolors = p.keycolors.iter().map(|(k, c)| (k.to_string(), c.to_string())).collect();
        }
        self.ensure_seed();
    }

    /// Random-design effects need a stored seed so the window preview and the
    /// keyboard show the same design.
    pub fn ensure_seed(&mut self) {
        if effects::needs_seed(&self.effect) {
            let m = self.params.entry(self.effect.clone()).or_default();
            if !m.contains_key("seed") {
                m.insert("seed".into(), effects::new_seed().into());
            }
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        let mut profiles = Vec::new();
        for (name, preset) in [
            ("Rainbow", "Rainbow Rush"),
            ("Typing", "Neon Ripple"),
            ("Music", "Party"),
            ("Chill", "Northern Lights"),
            ("Gaming", "WASD Gamer"),
        ] {
            let mut p = Profile::new(name, "rainbow");
            if let Some(pr) = presets().iter().find(|x| x.name == preset) {
                p.apply_preset(pr);
            }
            profiles.push(p);
        }
        Config {
            profiles,
            active: 0,
            paused: false,
            exit_mode: default_exit(),
            live: None,
            roster: Roster::default(),
            hotkeys: default_hotkeys(),
            favorites: Vec::new(),
        }
    }
}

impl Config {
    /// What's showing right now: the roster/surprise look, or the active profile.
    pub fn profile(&self) -> &Profile {
        match &self.live {
            Some(l) => l,
            None => &self.profiles[self.active.min(self.profiles.len() - 1)],
        }
    }
    pub fn profile_mut(&mut self) -> &mut Profile {
        let i = self.active.min(self.profiles.len() - 1);
        match self.live.as_mut() {
            Some(l) => l,
            None => &mut self.profiles[i],
        }
    }
    /// Switch to a saved profile (drops any roster/surprise look).
    pub fn select(&mut self, i: usize) {
        self.active = i.min(self.profiles.len() - 1);
        self.live = None;
    }
    fn fix(&mut self) {
        if self.profiles.is_empty() {
            *self = Config::default();
        }
        self.active = self.active.min(self.profiles.len() - 1);
    }

    /// Show a random look ("Surprise me").
    pub fn surprise(&mut self, rng: &mut Rng) {
        let brightness = self.profile().brightness;
        let mut p = if rng.f() < 0.2 {
            // sometimes one of the themes
            let all = presets();
            let pr = &all[rng.pick(all.len())];
            let mut p = Profile::new(&format!("Surprise: {}", pr.name), pr.effect);
            p.apply_preset(pr);
            p
        } else {
            let (id, params) = effects::random_look(rng);
            let mut p = Profile::new(&format!("Surprise: {}", effects::find(id).name), id);
            p.params.insert(id.into(), params);
            p
        };
        p.brightness = brightness;
        self.live = Some(p);
    }

    /// Show one roster entry. Returns false if it no longer exists.
    pub fn show_item(&mut self, item: &str, rng: &mut Rng) -> bool {
        let brightness = self.profile().brightness;
        if item == "surprise" {
            self.surprise(rng);
            return true;
        }
        if let Some(name) = item.strip_prefix("p:") {
            if let Some(i) = self.profiles.iter().position(|p| p.name == name) {
                self.select(i);
                return true;
            }
        } else if let Some(name) = item.strip_prefix("t:") {
            if let Some(pr) = presets().iter().find(|p| p.name == name) {
                let mut p = Profile::new(pr.name, pr.effect);
                p.apply_preset(pr);
                p.brightness = brightness;
                self.live = Some(p);
                return true;
            }
        } else if let Some(id) = item.strip_prefix("e:") {
            let info = effects::find(id);
            if info.id == id {
                let mut p = Profile::new(info.name, id);
                // use the settings the active profile has for this effect, if any
                p.params = self.profiles[self.active].params.clone();
                if effects::needs_seed(id) {
                    p.params.entry(id.to_string()).or_default().insert("seed".into(), effects::new_seed().into());
                }
                p.brightness = brightness;
                self.live = Some(p);
                return true;
            }
        }
        false
    }

    /// Step through the roster (dir = +1 / -1). With an empty roster, steps through profiles.
    pub fn step_roster(&mut self, dir: i32, rng: &mut Rng) {
        let n = self.roster.items.len();
        if n == 0 {
            if self.live.is_some() {
                // first press just goes back to the profile
                self.select(self.active);
            } else {
                let m = self.profiles.len() as i32;
                self.select((self.active as i32 + dir).rem_euclid(m) as usize);
            }
            return;
        }
        let step = if dir < 0 { -1 } else { 1 };
        let mut pos = if self.roster.shuffle && n > 1 {
            // anything but the current one
            (self.roster.pos + 1 + rng.pick(n - 1)) % n
        } else {
            (self.roster.pos as i32 + step).rem_euclid(n as i32) as usize
        };
        // skip entries whose profile or theme was deleted
        for _ in 0..n {
            let item = self.roster.items[pos].clone();
            if self.show_item(&item, rng) {
                break;
            }
            pos = (pos as i32 + step).rem_euclid(n as i32) as usize;
        }
        self.roster.pos = pos;
    }

    /// Run a shortcut action. Returns true if something changed.
    pub fn run_action(&mut self, action: &str, rng: &mut Rng) -> bool {
        match action {
            "next" => self.step_roster(1, rng),
            "prev" => self.step_roster(-1, rng),
            "surprise" => self.surprise(rng),
            "next_profile" => {
                let i = if self.live.is_some() { self.active } else { (self.active + 1) % self.profiles.len() };
                self.select(i);
            }
            "pause" => self.paused = !self.paused,
            "bright_up" | "bright_down" => {
                let d = if action == "bright_up" { 0.2 } else { -0.2 };
                let p = self.profile_mut();
                p.brightness = (((p.brightness + d) * 5.0).round() / 5.0).clamp(0.0, 1.0);
            }
            _ => return false,
        }
        true
    }

    /// Turn the current roster/surprise look into a saved profile.
    pub fn keep_live(&mut self) {
        if let Some(mut p) = self.live.take() {
            if let Some(rest) = p.name.strip_prefix("Surprise: ") {
                p.name = rest.to_string();
            }
            self.profiles.push(p);
            self.active = self.profiles.len() - 1;
        }
    }
}

/// Human-readable name for a roster entry.
pub fn item_label(item: &str) -> String {
    if item == "surprise" {
        "Surprise me".into()
    } else if let Some(n) = item.strip_prefix("p:").or_else(|| item.strip_prefix("t:")) {
        n.into()
    } else if let Some(id) = item.strip_prefix("e:") {
        effects::find(id).name.into()
    } else {
        item.into()
    }
}

/// What kind of roster entry this is (small grey label).
pub fn item_kind(item: &str) -> &'static str {
    match item.split(':').next().unwrap_or("") {
        "p" => "Profile",
        "t" => "Theme",
        "e" => "Effect",
        _ => "Random every time",
    }
}

pub fn dir() -> PathBuf {
    let base = std::env::var_os("APPDATA").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    base.join("KeyboardCustomSoftware")
}

pub fn path() -> PathBuf {
    dir().join("config.json")
}

pub fn mtime() -> Option<SystemTime> {
    std::fs::metadata(path()).and_then(|m| m.modified()).ok()
}

pub fn load() -> Config {
    let mut c = std::fs::read_to_string(path())
        .ok()
        .and_then(|s| serde_json::from_str::<Config>(&s).ok())
        .unwrap_or_default();
    c.fix();
    c
}

/// Write atomically (temp file + rename) so the other process never reads half a file.
pub fn save(c: &Config) -> std::io::Result<()> {
    std::fs::create_dir_all(dir())?;
    let tmp = dir().join("config.json.tmp");
    std::fs::write(&tmp, serde_json::to_string_pretty(c).unwrap_or_default())?;
    std::fs::rename(tmp, path())
}

// ---------------------------------------------------------------- themes

pub struct Preset {
    pub name: &'static str,
    pub effect: &'static str,
    pub params: Value,
    /// colors for the theme card (kept for older tools; cards now show a live preview)
    #[allow(dead_code)]
    pub swatch: [&'static str; 3],
    pub keycolors: Vec<(&'static str, &'static str)>,
}

fn pr(name: &'static str, effect: &'static str, params: Value, swatch: [&'static str; 3]) -> Preset {
    Preset { name, effect, params, swatch, keycolors: vec![] }
}

pub fn presets() -> Vec<Preset> {
    let mut wasd: Vec<(&str, &str)> = ["w", "a", "s", "d"].iter().map(|k| (*k, "#ff1030")).collect();
    wasd.extend(["space", "lshift", "lctrl"].iter().map(|k| (*k, "#ff8800")));
    wasd.extend(["1", "2", "3", "4", "q", "e", "r", "f"].iter().map(|k| (*k, "#ffffff")));
    wasd.extend(["up", "down", "left", "right"].iter().map(|k| (*k, "#ff1030")));
    vec![
        pr("Rainbow Rush", "rainbow", json!({"speed": 0.6, "width": 0.3}), ["#ff0000", "#00ff00", "#0000ff"]),
        pr("Cyberpunk", "gradient", json!({"color": "#ff00c8", "color2": "#00f0ff", "direction": "Diagonal", "move": true, "speed": 0.4}), ["#ff00c8", "#8000ff", "#00f0ff"]),
        pr("Synthwave", "plasma", json!({"palette": "Vaporwave", "speed": 0.35, "scale": 0.4}), ["#ff3cc8", "#9600ff", "#00c8ff"]),
        pr("Sunset", "plasma", json!({"palette": "Sunset", "speed": 0.3, "scale": 0.3}), ["#ff2800", "#ff8c00", "#6e00ff"]),
        pr("Deep Ocean", "ocean", json!({"speed": 0.4}), ["#000a50", "#0078ff", "#00dcff"]),
        pr("Northern Lights", "aurora", json!({"speed": 0.4}), ["#00ff8c", "#00a0a0", "#8200ff"]),
        pr("Inferno", "fire", json!({"speed": 0.55, "height": 0.5}), ["#5a0000", "#ff1400", "#ffd228"]),
        pr("The Matrix", "matrix", json!({"color": "#00ff46", "speed": 0.5}), ["#000000", "#008020", "#00ff46"]),
        pr("Ice Cave", "plasma", json!({"palette": "Ice", "speed": 0.25}), ["#ffffff", "#78e6ff", "#005aff"]),
        pr("Forest", "plasma", json!({"palette": "Forest", "speed": 0.3}), ["#003c0a", "#14c828", "#aaff00"]),
        pr("Candy", "plasma", json!({"palette": "Candy", "speed": 0.4}), ["#ff78c8", "#ffe678", "#78ffdc"]),
        pr("Unicorn", "vortex", json!({"speed": 0.4, "twist": 0.6}), ["#ff00ff", "#00ffff", "#ffff00"]),
        pr("Starry Night", "twinkle", json!({"color": "#ffffff", "color2": "#020218", "density": 0.45, "speed": 0.45}), ["#020218", "#1a1a60", "#ffffff"]),
        pr("Neon Ripple", "ripple", json!({"rainbow": true, "color2": "#05001a", "speed": 0.5}), ["#05001a", "#ff00c8", "#00f0ff"]),
        pr("Hacker Typing", "reactive", json!({"color": "#00ff46", "color2": "#001505", "speed": 0.4}), ["#001505", "#00a030", "#00ff46"]),
        pr("Laser Grid", "crosshair", json!({"rainbow": false, "color": "#ff0060", "color2": "#0a0010"}), ["#0a0010", "#ff0060", "#ff80b0"]),
        pr("Heat Map", "heatmap", json!({"memory": 0.5}), ["#0000aa", "#28ff00", "#ff0000"]),
        pr("Party", "audio_bars", json!({"style": "Rainbow", "sensitivity": 0.45}), ["#ff0000", "#ffff00", "#00ff60"]),
        pr("Bass Drop", "bass_pulse", json!({"rainbow": true, "sensitivity": 0.45}), ["#200010", "#ff0050", "#ff80a0"]),
        pr("Sound Wave", "audio_wave", json!({"rainbow": true, "sensitivity": 0.45}), ["#00ff3c", "#ffc800", "#ff0000"]),
        pr("Beat Drop", "beat_ripple", json!({"rainbow": true, "sensitivity": 0.45}), ["#000000", "#00e6ff", "#ff00c8"]),
        pr("Raindrops", "ambient_ripple", json!({"rainbow": false, "color": "#40a0ff", "color2": "#000814", "density": 0.45, "speed": 0.35}), ["#000814", "#1050a0", "#40a0ff"]),
        pr("New Year", "fireworks", json!({"rainbow": true, "density": 0.5, "speed": 0.5}), ["#000008", "#ffc83c", "#ff2080"]),
        pr("Lava Lamp", "lava_lamp", json!({"color": "#ff2800", "color2": "#28001e", "size": 0.4, "speed": 0.4}), ["#28001e", "#ff2800", "#ffa060"]),
        pr("Thunderstorm", "storm", json!({"density": 0.5, "speed": 0.5}), ["#06081e", "#3040a0", "#96aaff"]),
        pr("Shooting Stars", "comets", json!({"rainbow": false, "color": "#a0e0ff", "color2": "#02020c", "density": 0.3}), ["#02020c", "#4080c0", "#a0e0ff"]),
        pr("Screen Mirror", "screen_grid", json!({"saturation": 0.4, "smooth": 0.3}), ["#ff4040", "#40ff80", "#4080ff"]),
        pr("Knight Rider", "scanner", json!({"color": "#ff0000", "width": 0.3, "trail": 0.45, "speed": 0.45}), ["#000000", "#800000", "#ff0000"]),
        pr("Milky Way", "galaxy", json!({"color": "#ffc88c", "color2": "#3c50ff", "twist": 0.5, "speed": 0.35}), ["#000005", "#3c50ff", "#ffc88c"]),
        pr("Vital Signs", "heartbeat", json!({"color": "#00ff5a", "speed": 0.45}), ["#000602", "#008030", "#00ff5a"]),
        pr("Golden Hour", "daycycle", json!({"speed": 0.4}), ["#28186a", "#ff6420", "#8cd2ff"]),
        pr("Real Sky", "daycycle", json!({"realtime": true}), ["#010210", "#1464ff", "#ffaa50"]),
        pr("Prism", "kaleidoscope", json!({"palette": "Rainbow", "mirrors": "6 mirrors", "speed": 0.4}), ["#ff0000", "#00ff00", "#0000ff"]),
        pr("Mandala", "kaleidoscope", json!({"palette": "Sunset", "mirrors": "8 mirrors", "speed": 0.3}), ["#ff2800", "#ff0070", "#6e00ff"]),
        pr("Monsoon", "rainfall", json!({"density": 0.7, "wind": 0.65, "speed": 0.5}), ["#00040e", "#1a4080", "#5aaaff"]),
        pr("First Snow", "snowfall", json!({"density": 0.45, "speed": 0.4}), ["#020618", "#5070a0", "#e6f0ff"]),
        pr("Candlelit", "candle", json!({"count": "Three", "size": 0.4, "speed": 0.5}), ["#320600", "#ff8214", "#fff5dc"]),
        pr("Night Diner", "neon", json!({"color": "#ff1e78", "color2": "#00c8ff", "flicker": 0.4}), ["#ff1e78", "#200818", "#00c8ff"]),
        pr("Tokyo Neon", "neon", json!({"color": "#ffd000", "color2": "#ff0040", "flicker": 0.25}), ["#ffd000", "#300010", "#ff0040"]),
        pr("Genome", "helix", json!({"speed": 0.4}), ["#00e6ff", "#000000", "#ff3ca0"]),
        pr("Aquarium", "bubbles", json!({"density": 0.5, "speed": 0.45}), ["#000a1e", "#0a5a80", "#50e6ff"]),
        pr("Soda Pop", "bubbles", json!({"rainbow": true, "color2": "#0a0010", "density": 0.7, "size": 0.25, "speed": 0.6}), ["#ff78c8", "#ffe678", "#78ffdc"]),
        pr("Corrupted", "glitch", json!({"intensity": 0.5}), ["#002878", "#008ca0", "#ffffff"]),
        pr("Retro Snake", "snake", json!({"speed": 0.45}), ["#000000", "#00ff50", "#ff0000"]),
        pr("Rainbow Snake", "snake", json!({"rainbow": true, "color2": "#ffffff", "speed": 0.5}), ["#ff0000", "#00ff00", "#0000ff"]),
        pr("Arcade Pong", "pong", json!({"skill": 0.5, "speed": 0.5}), ["#000000", "#00c8ff", "#ffffff"]),
        pr("Petri Dish", "life", json!({"rainbow": true, "speed": 0.4}), ["#000000", "#00ffa0", "#ff00c8"]),
        pr("Welder", "sparks", json!({"color": "#ffaa28", "speed": 0.5}), ["#000000", "#ff6000", "#fff0a0"]),
        pr("Hot Keys", "embers", json!({"idle": 0.3, "speed": 0.5}), ["#000000", "#ff1400", "#ffd228"]),
        pr("Concert", "audio_fire", json!({"sensitivity": 0.45}), ["#5a0000", "#ff7800", "#ffffc8"]),
        pr("Subwoofer", "audio_rings", json!({"rainbow": true, "sensitivity": 0.45}), ["#00e6ff", "#ff00c8", "#ffff00"]),
        pr("Daydream", "dream", json!({"change": "Every 2 min", "speed": 0.45}), ["#ff6ad5", "#8c1eff", "#00e5ff"]),
        pr("Channel Surfing", "shuffle", json!({"every": "Every minute"}), ["#ff0000", "#00ff00", "#0000ff"]),
        pr("Confetti Party", "confetti", json!({"rainbow": true, "density": 0.6, "speed": 0.5}), ["#ff2080", "#ffe000", "#00d0ff"]),
        pr("Hyperspace", "warp", json!({"density": 0.5, "trail": 0.6, "speed": 0.5}), ["#000006", "#5070ff", "#dcebff"]),
        pr("Sonar", "radar", json!({"speed": 0.4}), ["#000803", "#00ff5a", "#ffffff"]),
        pr("Moire", "interference", json!({"palette": "Vaporwave", "speed": 0.35}), ["#ff3cc8", "#9600ff", "#00c8ff"]),
        pr("Night Highway", "traffic", json!({"density": 0.5, "speed": 0.5}), ["#020206", "#fff0c8", "#ff0000"]),
        pr("Desk Clock", "clock", json!({}), ["#ffa028", "#00c8ff", "#ffffff"]),
        pr("Falling Blocks", "tetris", json!({"speed": 0.5}), ["#00e6ff", "#ffd900", "#a600ff"]),
        pr("Arcade Night", "pacman", json!({"speed": 0.45}), ["#00000c", "#ffdc00", "#ff0000"]),
        pr("Police", "police", json!({"speed": 0.5}), ["#ff0000", "#000000", "#0028ff"]),
        pr("Lavender Breath", "breathing", json!({"color": "#b388ff", "speed": 0.4}), ["#1a0f2a", "#7a50c0", "#b388ff"]),
        pr("Focus", "solid", json!({"color": "#ffcf9e"}), ["#ffcf9e", "#ffe2c4", "#fff4e8"]),
        pr("Stealth", "solid", json!({"color": "#300000"}), ["#000000", "#180000", "#300000"]),
        pr("Ink Wash", "ink", json!({"palette": "Ocean", "speed": 0.45}), ["#000a50", "#0090ff", "#00ffaa"]),
        pr("Watercolor", "ink", json!({"palette": "Candy", "color2": "#06040a", "speed": 0.5}), ["#ff78c8", "#ffe678", "#78ffdc"]),
        pr("Sonic Boom", "shockwave", json!({"color": "#78c8ff", "prism": true, "width": 0.3}), ["#000000", "#78c8ff", "#ff40a0"]),
        pr("Red Alert", "shockwave", json!({"color": "#ff2000", "prism": false, "color2": "#0a0000", "width": 0.45}), ["#0a0000", "#ff2000", "#ffa080"]),
        pr("Star Map", "constellation", json!({"speed": 0.45}), ["#000006", "#5080ff", "#ffffff"]),
        pr("Golden Thread", "constellation", json!({"color": "#fff0c0", "color2": "#ff9a20", "color3": "#050200"}), ["#050200", "#ff9a20", "#fff0c0"]),
        pr("Overdrive", "overdrive", json!({"speed": 0.5}), ["#005aff", "#8000a0", "#ff2800"]),
        pr("Glacier to Magma", "overdrive", json!({"color": "#00e0ff", "color2": "#ff0060", "speed": 0.5}), ["#00e0ff", "#8040ff", "#ff0060"]),
        pr("Comet Tail", "trail", json!({"rainbow": true, "trail": 0.6, "speed": 0.5}), ["#00ffd0", "#ff00ff", "#ffe000"]),
        pr("Pop Art", "flip", json!({"palette": "Candy", "speed": 0.5}), ["#ff78c8", "#ffe678", "#78ffdc"]),
        pr("Silk Road", "silk", json!({"speed": 0.4}), ["#1428ff", "#a028d0", "#ff2896"]),
        pr("Champagne", "silk", json!({"color": "#3a2000", "color2": "#ffd28a", "scale": 0.5, "speed": 0.3}), ["#3a2000", "#c09050", "#ffd28a"]),
        pr("Deep Space", "nebula", json!({"palette": "Vaporwave", "speed": 0.4}), ["#04000c", "#9600ff", "#00c8ff"]),
        pr("Summer Night", "fireflies", json!({"density": 0.5, "speed": 0.45}), ["#000804", "#203a10", "#c8ff3c"]),
        pr("Pool Party", "caustics", json!({"speed": 0.45}), ["#00326e", "#2080c0", "#b4ffff"]),
        pr("Outrun", "synthwave", json!({"speed": 0.45}), ["#10002c", "#ff00c8", "#ffd200"]),
        pr("Hypnotic", "pendulum", json!({"rainbow": true, "speed": 0.45}), ["#000000", "#ff00ff", "#00ffd0"]),
        pr("Mercury", "chrome", json!({"speed": 0.4}), ["#101216", "#8c96a8", "#ffffff"]),
        pr("Molten Gold", "chrome", json!({"color": "#ffb030", "speed": 0.35}), ["#140800", "#a06010", "#ffd080"]),
        pr("Aurora Sound", "audio_aurora", json!({"sensitivity": 0.45}), ["#00ff8c", "#40a0c0", "#8c00ff"]),
        pr("Delay Pedal", "echo", json!({"rainbow": true, "feedback": 0.6}), ["#000000", "#00c8ff", "#ff00c8"]),
        pr("Pinball", "bounce", json!({"rainbow": true, "trail": 0.55}), ["#000000", "#ffc800", "#ff0080"]),
        pr("Tesla Coil", "lightning", json!({"color": "#a0b4ff", "speed": 0.5}), ["#02020c", "#5060ff", "#e0e8ff"]),
        pr("Red Storm", "lightning", json!({"color": "#ff3020", "color2": "#0c0000"}), ["#0c0000", "#ff3020", "#ffc0a0"]),
        pr("Night Synth", "piano", json!({"rainbow": true, "trail": 0.5}), ["#000000", "#ff00a0", "#00e6ff"]),
        pr("Gallery Wall", "splat", json!({"palette": "Candy", "color2": "#0a0a0a"}), ["#ff78c8", "#ffe678", "#78ffdc"]),
        pr("Forest Fire", "wildfire", json!({"spread": 0.55}), ["#062000", "#ff6e00", "#ffd228"]),
        pr("Repulsor", "forcefield", json!({"density": 0.55}), ["#000000", "#2878ff", "#ff3cc8"]),
        pr("Event Horizon", "blackhole", json!({"color": "#ff7800"}), ["#000000", "#ff7800", "#ffffff"]),
        pr("Subwoofer", "tunnel", json!({"rainbow": true, "sensitivity": 0.45}), ["#000000", "#ff0078", "#00c8ff"]),
        Preset {
            name: "WASD Gamer",
            effect: "custom",
            params: json!({"color": "#0a1030"}),
            swatch: ["#0a1030", "#ff1030", "#ff8800"],
            keycolors: wasd,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roster_steps_and_skips_missing() {
        let mut rng = Rng::seeded(7);
        let mut c = Config::default();
        c.roster.items = vec!["p:Chill".into(), "p:Gone".into(), "t:Sonar".into(), "surprise".into(), "e:dream".into()];
        c.roster.pos = c.roster.items.len() - 1;
        c.step_roster(1, &mut rng);
        assert_eq!(c.roster.pos, 0);
        assert!(c.live.is_none());
        assert_eq!(c.profile().name, "Chill");
        c.step_roster(1, &mut rng); // "p:Gone" is skipped
        assert_eq!(c.roster.pos, 2);
        assert_eq!(c.profile().effect, "radar");
        c.step_roster(1, &mut rng);
        assert!(c.profile().name.starts_with("Surprise: "));
        c.step_roster(1, &mut rng);
        assert_eq!(c.profile().effect, "dream");
        assert!(c.profile().params["dream"].contains_key("seed"));
        c.step_roster(-1, &mut rng);
        assert_eq!(c.roster.pos, 3);
        // editing a roster look doesn't touch the saved profiles
        let before = c.profiles.clone();
        c.run_action("bright_down", &mut rng);
        assert!(c.profiles == before);
        c.keep_live();
        assert_eq!(c.profiles.len(), before.len() + 1);
        assert!(c.live.is_none());
    }

    #[test]
    fn every_theme_effect_exists() {
        for p in presets() {
            assert_eq!(effects::find(p.effect).id, p.effect, "theme {} uses a missing effect", p.name);
        }
    }
}
