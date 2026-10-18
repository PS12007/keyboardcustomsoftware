//! Settings and profiles, stored as JSON in %APPDATA%\KeyboardCustomSoftware\config.json.
//! The settings window writes this file; the background service watches it.

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
        Config { profiles, active: 0, paused: false, exit_mode: default_exit() }
    }
}

impl Config {
    pub fn profile(&self) -> &Profile {
        &self.profiles[self.active.min(self.profiles.len() - 1)]
    }
    pub fn profile_mut(&mut self) -> &mut Profile {
        let i = self.active.min(self.profiles.len() - 1);
        &mut self.profiles[i]
    }
    fn fix(&mut self) {
        if self.profiles.is_empty() {
            *self = Config::default();
        }
        self.active = self.active.min(self.profiles.len() - 1);
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
    /// colors for the theme card
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
        pr("Police", "police", json!({"speed": 0.5}), ["#ff0000", "#000000", "#0028ff"]),
        pr("Lavender Breath", "breathing", json!({"color": "#b388ff", "speed": 0.4}), ["#1a0f2a", "#7a50c0", "#b388ff"]),
        pr("Focus", "solid", json!({"color": "#ffcf9e"}), ["#ffcf9e", "#ffe2c4", "#fff4e8"]),
        pr("Stealth", "solid", json!({"color": "#300000"}), ["#000000", "#180000", "#300000"]),
        Preset {
            name: "WASD Gamer",
            effect: "custom",
            params: json!({"color": "#0a1030"}),
            swatch: ["#0a1030", "#ff1030", "#ff8800"],
            keycolors: wasd,
        },
    ]
}
