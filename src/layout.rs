//! Physical key layout of the Surmen V400 (75% with knob).
//! Units are key widths. The table itself is generated into layout_data.rs.

pub use crate::layout_data::{EXTRA_VK, KEYS};

pub struct Key {
    pub id: &'static str,
    pub label: &'static str,
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub row: u8,
    pub slot: u8,
    pub vk: u16,
}

pub const WIDTH: f32 = 16.25;
pub const HEIGHT: f32 = 6.25;
/// Knob position (drawn only, no LED).
pub const KNOB: (f32, f32) = (15.25, 0.0);
pub const CENTER: (f32, f32) = (8.1, 3.1);

impl Key {
    pub fn cx(&self) -> f32 {
        self.x + self.w / 2.0
    }
    pub fn cy(&self) -> f32 {
        self.y + 0.5
    }
    /// 0..1 left to right
    pub fn u(&self) -> f32 {
        self.cx() / WIDTH
    }
    /// 0..1 top to bottom
    pub fn v(&self) -> f32 {
        self.cy() / HEIGHT
    }
}

pub fn index_of(id: &str) -> Option<usize> {
    KEYS.iter().position(|k| k.id == id)
}

pub fn index_for_vk(vk: u16) -> Option<usize> {
    if vk == 0 {
        return None;
    }
    KEYS.iter().position(|k| k.vk == vk).or_else(|| {
        EXTRA_VK
            .iter()
            .find(|(v, _)| *v == vk)
            .and_then(|(_, id)| index_of(id))
    })
}
