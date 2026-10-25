// No console window in release builds.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio;
mod color;
mod config;
mod daemon;
mod effects;
mod engine;
mod gui;
mod icon;
mod layout;
mod layout_data;
mod protocol;
mod screen;
mod sys;

fn main() {
    // One program, two modes:
    //   (no args)     -> start the background service and open the settings window
    //   --background  -> background service only (tray icon + lighting engine)
    //   --gui         -> settings window only (runs only while open)
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--gui") {
        gui::run();
    } else {
        daemon::run(!args.iter().any(|a| a == "--background"));
    }
}
