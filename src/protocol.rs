//! Surmen V400 lighting protocol (see docs/PROTOCOL.md).
//!
//! Every message is a 64-byte HID output report (ID 4) on the vendor channel:
//!   04 | checksum(2, LE) | cmd | len | offset(3, LE) | data...
//! The keyboard echoes every packet back; we wait for the echo before sending
//! the next one, otherwise it starts dropping packets.

use hidapi::{HidApi, HidDevice, HidResult};

pub const VID: u16 = 0x320F;
pub const PID: u16 = 0x505B;
pub const USAGE_PAGE: u16 = 0xFF1C;

pub const SLOTS: usize = 128;
pub type Frame = [[u8; 3]; SLOTS];

const CMD_START: u8 = 0x01;
const CMD_END: u8 = 0x02;
const CMD_MODE: u8 = 0x06;
const CMD_SAVED_KEYS: u8 = 0x0B;
const CMD_LIVE_KEYS: u8 = 0x12;
const SAVED_BASE: u32 = 0x0200;
const CHUNK: usize = 56;

pub const MODE_CUSTOM: u8 = 0x14;
pub const MODE_LIVE: u8 = 0xFE;

/// Slots the vendor app always sends as black.
const GAP_SLOTS: [usize; 10] = [47, 55, 63, 71, 79, 87, 95, 103, 104, 111];

/// Mode packet data captured from the vendor app; bytes 1 (mode), 2 (brightness),
/// 3 (speed) and 6..9 (color) are filled in.
const MODE_TEMPLATE: [u8; 34] = [
    0x00, 0x14, 0x04, 0x01, 0x00, 0x00, 0xFF, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00,
    0x00, 0x00, 0x01, 0x04,
];

pub fn packet(cmd: u8, data: &[u8], offset: u32) -> [u8; 64] {
    let mut p = [0u8; 64];
    p[0] = 0x04;
    p[3] = cmd;
    p[4] = data.len() as u8;
    p[5..8].copy_from_slice(&offset.to_le_bytes()[..3]);
    p[8..8 + data.len()].copy_from_slice(data);
    let sum: u32 = p[3..].iter().map(|&b| b as u32).sum();
    p[1] = sum as u8;
    p[2] = (sum >> 8) as u8;
    p
}

pub struct Keyboard {
    dev: HidDevice,
}

impl Keyboard {
    pub fn open(api: &mut HidApi) -> Option<Self> {
        api.refresh_devices().ok()?;
        let info = api.device_list().find(|d| {
            d.vendor_id() == VID && d.product_id() == PID && d.usage_page() == USAGE_PAGE
        })?;
        let dev = info.open_device(api).ok()?;
        Some(Self { dev })
    }

    /// Is the keyboard plugged in? (cheap check, no open)
    pub fn present(api: &mut HidApi) -> bool {
        api.refresh_devices().is_ok()
            && api.device_list().any(|d| {
                d.vendor_id() == VID && d.product_id() == PID && d.usage_page() == USAGE_PAGE
            })
    }

    fn xfer(&self, p: &[u8; 64]) -> HidResult<()> {
        self.dev.write(p)?;
        let mut echo = [0u8; 64];
        self.dev.read_timeout(&mut echo, 100)?;
        Ok(())
    }

    pub fn set_mode(&self, mode: u8, brightness: u8, speed: u8, rgb: [u8; 3]) -> HidResult<()> {
        let mut d = MODE_TEMPLATE;
        d[1] = mode;
        d[2] = brightness.min(4);
        d[3] = speed.min(4);
        d[6..9].copy_from_slice(&rgb);
        self.xfer(&packet(CMD_START, &[], 0))?;
        self.xfer(&packet(CMD_MODE, &d, 0))?;
        self.xfer(&packet(CMD_END, &[], 0))
    }

    fn key_buffer(frame: &Frame) -> [u8; SLOTS * 3] {
        let mut buf = [0u8; SLOTS * 3];
        for (i, c) in frame.iter().enumerate() {
            if !GAP_SLOTS.contains(&i) {
                buf[i * 3..i * 3 + 3].copy_from_slice(c);
            }
        }
        buf
    }

    fn send_keys(&self, cmd: u8, base: u32, frame: &Frame) -> HidResult<()> {
        let buf = Self::key_buffer(frame);
        for off in (0..buf.len()).step_by(CHUNK) {
            let end = (off + CHUNK).min(buf.len());
            self.xfer(&packet(cmd, &buf[off..end], base + off as u32))?;
        }
        Ok(())
    }

    /// Switch to live mode (the PC streams frames).
    pub fn start_live(&self) -> HidResult<()> {
        self.set_mode(MODE_LIVE, 4, 0, [0xFF, 0, 0])
    }

    /// One live frame (~13 ms incl. echoes).
    pub fn send_live(&self, frame: &Frame) -> HidResult<()> {
        self.send_keys(CMD_LIVE_KEYS, 0, frame)
    }

    /// Store per-key colors on the keyboard itself and show them.
    /// Keeps showing after the app quits; no streaming needed.
    pub fn save_custom(&self, frame: &Frame) -> HidResult<()> {
        self.send_keys(CMD_SAVED_KEYS, SAVED_BASE, frame)?;
        self.set_mode(MODE_CUSTOM, 4, 1, [0xFF, 0, 0])
    }
}
