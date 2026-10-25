# Keyboard Custom Software

Lightweight custom RGB lighting for the **Surmen V400** keyboard (75% layout with a knob, USB ID `320F:505B`). It replaces the vendor app.

- **55+ effects:** rainbow wave, plasma, northern lights, fire, ocean, matrix rain, starry night, galaxy, kaleidoscope, day & night sky, rainfall, snowfall, candlelight, neon sign, DNA helix, bubbles, glitch, heartbeat, scanner, fireworks, lava lamp, lightning storm, comets, warp speed, radar, night traffic, interference, a clock, and more
- **Random designs:** *Dream* makes up a brand-new design from random patterns and colors (never the same twice), *Shuffle* plays a different random effect every so often, and **Surprise me** picks a random look in one click
- **Fn shortcuts and a roster:** build a playlist of profiles, themes and effects, then press **`Fn + \`** to step through it (shuffle and auto-switch optional). Record your own Fn combos for next, previous, surprise, brightness or lights on/off
- **Games that play themselves:** Snake, Pong, Tetris (sideways), Pac-Man and Conway's Game of Life on your keys
- **Reactive effects:** ripple, key glow, crosshair, sparks, burning keys, and a typing heatmap that remembers your typing across restarts
- **Audio effects:** equalizer bars, bass pulse, sound wave, beat ripples, sound fire and sound rings driven by whatever your PC is playing
- **Screen Grid:** every key shows the color of its part of your screen, like a tiny live copy of it
- **Custom keys:** paint each key yourself by clicking it in the preview
- **All 18 built-in keyboard effects**, with brightness, speed and color controls
- **65+ themes** with live previews, applied with one click, plus profiles you can switch from the tray
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
- **The key listener, audio capture and screen capture** only run while an effect needs them. Screen capture lets the graphics card shrink the picture and only reads it when the screen changed.

## Install

1. Download `KeyboardCustomSoftware.exe` from [Releases](../../releases), or build it yourself (below).
2. Run it. The settings window opens and a tray icon appears.
3. Close the vendor "SURMENV400" app (the window offers to do it), because both apps fight over the lights.
4. Optional: tick **Start with Windows**.

Closing the window keeps the lighting running. Right-click the tray icon to switch profiles, go to the next look, get a surprise, pause or quit.

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

Windows never sees the Fn key, but the keyboard reports Fn combos on its media-key interface (usage page `0x0C`): `03 <LED slot> 05/06` for key down/up while Fn is held, and always `03 01 04` for `Fn + \` (the key reports for that combo are unreliable). `src/fnkeys.rs` listens there.

## Project layout

```
src/
  main.rs        picks service or settings mode
  daemon.rs      tray icon, settings file watcher, keyboard hook
  engine.rs      renders effects and decides what to send
  effects.rs     all effects and their settings
  effects/extra.rs  random designs (Dream, Shuffle), more effects, Tetris, Pac-Man
  fnkeys.rs      Fn + key shortcuts
  screen.rs      tiny live copy of the screen for Screen Grid
  protocol.rs    USB/HID protocol
  audio.rs       system audio capture + frequency bands
  gui.rs         settings window (egui)
  icon.rs        app logo, drawn in code (also used by build.rs for the .exe icon)
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

Reactive effects use a Windows keyboard hook, which is only installed while a reactive effect is selected. It only tells the lighting engine which key went down, so it can light that key up. Nothing is logged or sent anywhere. The only thing kept is the Typing Heatmap's count of how often each key was pressed (no order, no text), saved in `%APPDATA%\KeyboardCustomSoftware\heatmap.json`; the Reset button in the window clears it. Audio is analysed in memory and never recorded. Screen Grid shrinks the screen to a 65 x 24 picture in memory to pick key colors; nothing is saved or sent anywhere.

## Notes

- 36 LED slots aren't under any visible key (probably edge or under-case LEDs). They're left off for now.
- Tested on Windows 11 only.
- Not affiliated with Surmen.

## License

MIT
