//! Fn + key shortcuts.
//!
//! Windows never sees the Fn key, but the V400 reports Fn combos on its media-key
//! channel (usage page 0x0C) as 3-byte reports: `03 <LED slot> 05` when a key goes
//! down while Fn is held, `06` when it goes up. Fn itself is slot 0x55.
//! Media keys and the knob use the same channel with a 0 in the last byte, so they
//! never look like this.

use crate::layout::KEYS;
use crate::protocol::{PID, VID};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

const FN_SLOT: u8 = 0x55;
const BACKSLASH_SLOT: u8 = 0x6A;

/// Whether Fn is held. The Fn reports sometimes go missing, so "held" also runs
/// out after a moment; otherwise a lost release would turn plain typing into shortcuts.
#[derive(Default)]
struct FnState {
    since: Option<std::time::Instant>,
}

impl FnState {
    fn held(&self) -> bool {
        self.since.is_some_and(|t| t.elapsed() < Duration::from_secs(2))
    }
}

/// Turn one report into "key id pressed with Fn".
fn parse(buf: &[u8], st: &mut FnState) -> Option<&'static str> {
    if buf.len() < 3 || buf[0] != 0x03 {
        return None;
    }
    // Fn+\ is a built-in combo: the keyboard always sends 03 01 04 for it, while
    // its key reports only arrive now and then. So use that and ignore the rest.
    if buf[1] == 0x01 && buf[2] == 0x04 {
        return Some("backslash");
    }
    if !matches!(buf[2], 0x05 | 0x06) {
        return None;
    }
    let down = buf[2] == 0x05;
    if buf[1] == FN_SLOT {
        st.since = down.then(std::time::Instant::now);
        return None;
    }
    if down && buf[1] != BACKSLASH_SLOT && st.held() {
        KEYS.iter().find(|k| k.slot == buf[1]).map(|k| k.id)
    } else {
        None
    }
}

/// Reads Fn combos on a background thread until dropped.
pub struct Listener {
    stop: Arc<AtomicBool>,
}

impl Listener {
    /// `wake` runs after each combo is sent (e.g. to wake the service's message loop).
    pub fn start(wake: impl Fn() + Send + 'static) -> (Self, Receiver<&'static str>) {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let s = stop.clone();
        let _ = std::thread::Builder::new().name("fn-keys".into()).spawn(move || run(s, tx, wake));
        (Listener { stop }, rx)
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

fn run(stop: Arc<AtomicBool>, tx: Sender<&'static str>, wake: impl Fn()) {
    let Ok(mut api) = hidapi::HidApi::new() else { return };
    while !stop.load(Ordering::Relaxed) {
        let _ = api.refresh_devices();
        let dev = api
            .device_list()
            .find(|d| d.vendor_id() == VID && d.product_id() == PID && d.usage_page() == 0x0C && d.usage() == 0x01)
            .and_then(|d| d.open_device(&api).ok());
        let Some(dev) = dev else {
            // keyboard not plugged in: look again in a bit
            for _ in 0..20 {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                std::thread::sleep(Duration::from_millis(100));
            }
            continue;
        };
        let mut st = FnState::default();
        let mut buf = [0u8; 64];
        while !stop.load(Ordering::Relaxed) {
            match dev.read_timeout(&mut buf, 300) {
                Ok(n) => {
                    if let Some(id) = parse(&buf[..n], &mut st) {
                        if tx.send(id).is_err() {
                            return;
                        }
                        wake();
                    }
                }
                Err(_) => break, // unplugged
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Report sequences the keyboard really sent.
    #[test]
    fn parses_fn_combos() {
        let mut st = FnState::default();
        // Fn+\ : once with all reports, once with only 03 01 04; both count exactly once
        let full: [&[u8]; 5] = [&[3, 0x55, 5], &[3, 0x6A, 5], &[3, 1, 4], &[3, 0x6A, 6], &[3, 0x55, 6]];
        let bare: [&[u8]; 1] = [&[3, 1, 4]];
        for seq in [&full[..], &bare[..]] {
            let hits: Vec<_> = seq.iter().filter_map(|r| parse(r, &mut st)).collect();
            assert_eq!(hits, vec!["backslash"]);
        }
        // Fn + ] goes through the key reports
        assert_eq!(parse(&[3, 0x55, 5], &mut st), None);
        assert!(st.held());
        assert_eq!(parse(&[3, 0xE9, 0], &mut st), None, "volume key");
        assert_eq!(parse(&[3, 0x62, 5], &mut st), Some("rbracket"));
        assert_eq!(parse(&[3, 0x62, 6], &mut st), None, "key up");
        assert_eq!(parse(&[3, 0x55, 6], &mut st), None);
        // typing without Fn (the keyboard reports keys this way after Fn+\ sometimes)
        assert_eq!(parse(&[3, 0x1B, 5], &mut st), None);
    }
}
