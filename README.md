# Keyboard Custom Software

Lightweight custom RGB lighting for the **Surmen V400** keyboard (75% layout with a knob, USB ID `320F:505B`). It replaces the vendor app.

- **20+ effects:** rainbow wave, plasma, northern lights, fire, ocean, matrix rain, starry night, vortex, and more
- **Reactive effects:** ripple, key glow, crosshair and a typing heatmap that respond to what you type
- **Audio effects:** equalizer bars, bass pulse, sound wave and beat ripples driven by whatever your PC is playing
- **Custom keys:** paint each key yourself by clicking it in the preview
- **All 18 built-in keyboard effects**, with brightness, speed and color controls
- **25 themes** you can apply with one click, plus profiles you can switch from the tray
- Runs quietly in the **system tray** and can start with Windows

## Light on resources

The app is a single native Rust program that runs in two modes:

| Part | When it runs | Typical use |
|---|---|---|
| Background service (tray) | always | **~2 MB RAM, ~1% of one CPU core** while streaming a 60 fps animation |
| Settings window | only while open | closes completely when you close it |

The service also avoids work it doesn't need to do:
- **Built-in effects** are sent once, then the keyboard animates them by itself.
- **Static colors** (Solid, Custom Keys, still gradients) are saved to the keyboard once, then the service sits idle.
- **Animations** only send a frame when it differs from the last one, so an idle ripple effect sends nothing.
- **The key listener and audio capture** only run while an effect needs them.

## Install

1. Download `KeyboardCustomSoftware.exe` from [Releases](../../releases), or build it yourself (below).
2. Run it. The settings window opens and a tray icon appears.
3. Close the vendor "SURMENV400" app (the window offers to do it), because both apps fight over the lights.
4. Optional: tick **Start with Windows**.

Closing the window keeps the lighting running. Right-click the tray icon to switch profiles, pause or quit.

## Build

Needs [Rust](https://rustup.rs) and the Visual Studio C++ build tools.

```
cargo build --release
```

The program is written to `target/release/KeyboardCustomSoftware.exe`.

Command-line modes:
- no arguments: start the service (if needed) and open the settings window
- `--background`: service only (used by "Start with Windows")
- `--gui`: settings window only

## How it works

The lighting protocol was reverse engineered from USB captures of the vendor app (Wireshark + USBPcap). See [docs/PROTOCOL.md](docs/PROTOCOL.md).

In short: 64-byte HID output reports (ID 4) go to the keyboard's vendor interface (usage page `0xFF1C`). Each has a simple checksum, and the keyboard echoes every packet back. Command `0x06` picks an effect, `0x0B` stores per-key colors, and `0x12` streams live per-key frames.

## Project layout

```
src/
  main.rs        picks service or settings mode
  daemon.rs      tray icon, settings file watcher, keyboard hook
  engine.rs      renders effects and decides what to send
  effects.rs     all effects and their settings
  protocol.rs    USB/HID protocol
  audio.rs       system audio capture + frequency bands
  gui.rs         settings window (egui)
  config.rs      profiles, settings, themes
  layout*.rs     key positions (layout_data.rs is generated)
tools/
  led.py         original Python experiments (protocol tests)
  map_keys.py    lights each LED to map it to a key
  gen_layout.py  generates src/layout_data.rs from tools/keymap.json
docs/
  PROTOCOL.md    protocol notes
```

## Privacy

Reactive effects use a Windows keyboard hook, which is only installed while a reactive effect is selected. It only tells the lighting engine which key went down, so it can light that key up. Nothing is stored, logged or sent anywhere. Audio is analysed in memory and never recorded.

## Notes

- 36 LED slots aren't under any visible key (probably edge or under-case LEDs). They're left off for now.
- Tested on Windows 11 only.
- Not affiliated with Surmen.

## License

MIT
