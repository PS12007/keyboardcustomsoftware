//! Background service: tray icon + lighting engine thread.
//! Uses a few MB of RAM; the settings window is a separate process that only runs while open.

use crate::config::{self, Config};
use crate::color::Rng;
use crate::effects;
use crate::fnkeys;
use crate::engine::{self, Shared};
use crate::icon;
use crate::sys::{self, InstanceLock, KeyHook, APP_TITLE, DAEMON_MUTEX};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, Mutex};
use tray_icon::menu::{CheckMenuItem, Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem, Submenu};
use tray_icon::{Icon, MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, PostQuitMessage, PostThreadMessageW, SetTimer, TranslateMessage, MSG, WM_APP,
    WM_TIMER,
};

/// File the settings window creates to ask the service to quit.
pub fn quit_flag() -> std::path::PathBuf {
    config::dir().join("quit")
}

struct Ids {
    open: MenuId,
    pause: MenuId,
    next: MenuId,
    surprise: MenuId,
    autostart: MenuId,
    quit: MenuId,
    profiles: Vec<MenuId>,
}

fn build_menu(cfg: &Config) -> (Menu, Ids) {
    let menu = Menu::new();
    let open = MenuItem::new("Open settings", true, None);
    let profiles = Submenu::new("Profile", true);
    let mut pids = Vec::new();
    for (i, p) in cfg.profiles.iter().enumerate() {
        let item = CheckMenuItem::new(&p.name, true, i == cfg.active && cfg.live.is_none(), None);
        pids.push(item.id().clone());
        let _ = profiles.append(&item);
    }
    let pause = CheckMenuItem::new("Pause lighting", true, cfg.paused, None);
    let next = MenuItem::new(format!("Next look{}", shortcut_hint(cfg, "next")), true, None);
    let surprise = MenuItem::new(format!("Surprise me{}", shortcut_hint(cfg, "surprise")), true, None);
    let autostart = CheckMenuItem::new("Start with Windows", true, sys::autostart_enabled(), None);
    let quit = MenuItem::new("Quit", true, None);
    let _ = menu.append_items(&[
        &open,
        &PredefinedMenuItem::separator(),
        &profiles,
        &next,
        &surprise,
        &pause,
        &PredefinedMenuItem::separator(),
        &autostart,
        &quit,
    ]);
    let ids = Ids {
        open: open.id().clone(),
        pause: pause.id().clone(),
        next: next.id().clone(),
        surprise: surprise.id().clone(),
        autostart: autostart.id().clone(),
        quit: quit.id().clone(),
        profiles: pids,
    };
    (menu, ids)
}

/// Menu suffix like "Fn+\" (after a tab) when an action has a shortcut.
fn shortcut_hint(cfg: &Config, action: &str) -> String {
    cfg.hotkeys
        .iter()
        .find(|h| h.action == action)
        .and_then(|h| crate::layout::index_of(&h.key))
        .map(|i| format!("\tFn+{}", crate::layout::KEYS[i].label))
        .unwrap_or_default()
}

fn tooltip(cfg: &Config) -> String {
    let p = cfg.profile();
    format!("{APP_TITLE}\n{} — {}", p.name, effects::find(&p.effect).name)
}

fn open_gui() {
    if !sys::focus_window(APP_TITLE) {
        sys::spawn_self(&["--gui"]);
    }
}

/// The keyboard hook is only installed while a reactive effect needs it.
fn update_hook(cfg: &Config, hook: &mut Option<KeyHook>, tx: &Sender<u16>) {
    let want = effects::find(&cfg.profile().effect).uses_keys && !cfg.paused;
    if want && hook.is_none() {
        *hook = KeyHook::install(tx);
    } else if !want {
        *hook = None;
    }
}

struct Service {
    shared: Arc<Shared>,
    tray: TrayIcon,
    ids: Ids,
    hook: Option<KeyHook>,
    tx: Sender<u16>,
    last_mtime: Option<std::time::SystemTime>,
    rng: Rng,
    /// when the roster last switched (for "switch every N minutes")
    last_switch: std::time::Instant,
}

impl Service {
    fn cfg(&self) -> Config {
        self.shared.cfg.lock().map(|c| c.clone()).unwrap_or_default()
    }

    /// Push a new config to the engine, tray menu and hook.
    fn apply(&mut self, cfg: Config, save: bool) {
        if save {
            let _ = config::save(&cfg);
            self.last_mtime = config::mtime();
        }
        update_hook(&cfg, &mut self.hook, &self.tx);
        let (menu, ids) = build_menu(&cfg);
        self.tray.set_menu(Some(Box::new(menu)));
        let _ = self.tray.set_tooltip(Some(tooltip(&cfg)));
        self.ids = ids;
        if let Ok(mut c) = self.shared.cfg.lock() {
            *c = cfg;
        }
        self.shared.version.fetch_add(1, Ordering::Relaxed);
    }

    fn on_timer(&mut self) -> bool {
        if quit_flag().exists() {
            let _ = std::fs::remove_file(quit_flag());
            return false;
        }
        let m = config::mtime();
        if m != self.last_mtime {
            self.last_mtime = m;
            self.apply(config::load(), false);
        }
        let mins = self.cfg().roster.auto_minutes;
        if mins > 0 && self.last_switch.elapsed().as_secs() >= mins as u64 * 60 {
            self.action("next");
        }
        true
    }

    /// Run a shortcut / menu action.
    fn action(&mut self, what: &str) {
        let mut cfg = self.cfg();
        if cfg.run_action(what, &mut self.rng) {
            if matches!(what, "next" | "prev" | "surprise") {
                self.last_switch = std::time::Instant::now();
            }
            self.apply(cfg, true);
        }
    }

    /// Fn + key pressed.
    fn on_fn_key(&mut self, key: &str) {
        let actions: Vec<String> =
            self.cfg().hotkeys.iter().filter(|h| h.key == key).map(|h| h.action.clone()).collect();
        for a in actions {
            self.action(&a);
        }
    }

    fn on_menu(&mut self, id: &MenuId) -> bool {
        let mut cfg = self.cfg();
        if *id == self.ids.open {
            open_gui();
        } else if *id == self.ids.quit {
            return false;
        } else if *id == self.ids.pause {
            cfg.paused = !cfg.paused;
            self.apply(cfg, true);
        } else if *id == self.ids.next {
            self.action("next");
        } else if *id == self.ids.surprise {
            self.action("surprise");
        } else if *id == self.ids.autostart {
            sys::set_autostart(!sys::autostart_enabled());
            self.apply(cfg, false);
        } else if let Some(i) = self.ids.profiles.iter().position(|p| p == id) {
            cfg.select(i);
            self.apply(cfg, true);
        }
        true
    }
}

pub fn run(open_window: bool) {
    let Some(_lock) = InstanceLock::acquire(DAEMON_MUTEX) else {
        // already running: just show the settings
        if open_window {
            open_gui();
        }
        return;
    };
    let _ = std::fs::remove_file(quit_flag());
    // this process owns the typing heatmap file
    crate::effects::set_persist(true);

    let cfg = config::load();
    if config::mtime().is_none() {
        let _ = config::save(&cfg);
    }
    let shared = Arc::new(Shared {
        cfg: Mutex::new(cfg.clone()),
        version: AtomicU64::new(1),
        stop: AtomicBool::new(false),
    });
    let (tx, rx) = channel();
    let engine = {
        let s = shared.clone();
        std::thread::Builder::new()
            .name("lighting".into())
            .spawn(move || engine::run(s, rx))
            .expect("engine thread")
    };

    let (menu, ids) = build_menu(&cfg);
    let icon = Icon::from_rgba(icon::rgba(32), 32, 32).expect("icon");
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .with_tooltip(tooltip(&cfg))
        .with_icon(icon)
        .build()
        .expect("tray icon");

    let mut svc = Service {
        shared: shared.clone(),
        tray,
        ids,
        hook: None,
        tx,
        last_mtime: config::mtime(),
        rng: Rng::default(),
        last_switch: std::time::Instant::now(),
    };
    // Fn + key shortcuts; the reader thread wakes this loop with WM_APP
    let tid = unsafe { windows_sys::Win32::System::Threading::GetCurrentThreadId() };
    let (_fn_listener, fn_rx) = fnkeys::Listener::start(move || unsafe {
        PostThreadMessageW(tid, WM_APP, 0, 0);
    });
    update_hook(&cfg, &mut svc.hook, &svc.tx);

    if open_window {
        open_gui();
    }

    unsafe {
        SetTimer(std::ptr::null_mut(), 0, 400, None);
        let mut msg: MSG = std::mem::zeroed();
        loop {
            if GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) <= 0 {
                break;
            }
            TranslateMessage(&msg);
            DispatchMessageW(&msg);

            let mut keep = true;
            if msg.message == WM_TIMER {
                keep &= svc.on_timer();
            }
            while let Ok(key) = fn_rx.try_recv() {
                svc.on_fn_key(key);
            }
            while let Ok(ev) = MenuEvent::receiver().try_recv() {
                keep &= svc.on_menu(&ev.id);
            }
            while let Ok(ev) = TrayIconEvent::receiver().try_recv() {
                if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = ev {
                    open_gui();
                }
            }
            if !keep {
                PostQuitMessage(0);
            }
        }
    }

    svc.hook = None;
    shared.stop.store(true, Ordering::Relaxed);
    let _ = engine.join();
}
