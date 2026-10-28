//! Shared design tokens: sizes, colours, and the stable accent colour that
//! tells the user which cards belong to the same application.
//!
//! Two Chrome windows must look like siblings and two different apps must not.
//! The accent is derived from the executable name, not from the window handle,
//! so it survives restarts and stays consistent for the same app.

/// An 8-bit sRGB colour.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rgb(pub u8, pub u8, pub u8);

impl Rgb {
    pub const fn new(r: u8, g: u8, b: u8) -> Self {
        Rgb(r, g, b)
    }

    /// Blends towards white; `t` of 0 leaves the colour alone.
    pub fn lighten(self, t: f32) -> Rgb {
        let t = t.clamp(0.0, 1.0);
        Rgb(
            (self.0 as f32 + (255.0 - self.0 as f32) * t) as u8,
            (self.1 as f32 + (255.0 - self.1 as f32) * t) as u8,
            (self.2 as f32 + (255.0 - self.2 as f32) * t) as u8,
        )
    }

    /// Blends towards black.
    pub fn darken(self, t: f32) -> Rgb {
        let t = 1.0 - t.clamp(0.0, 1.0);
        Rgb((self.0 as f32 * t) as u8, (self.1 as f32 * t) as u8, (self.2 as f32 * t) as u8)
    }

    pub fn as_tuple(self) -> (u8, u8, u8) {
        (self.0, self.1, self.2)
    }
}

/// Device-independent sizes, in logical pixels.
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    /// The visible card, including its thin frame.
    pub card_w: f32,
    pub card_h: f32,
    pub card_radius: f32,
    /// Space above the card where the clothespin grips the rope.
    pub pin_h: f32,
    pub pin_w: f32,
    /// Application icon in the card's footer.
    pub icon: f32,
    pub footer_h: f32,
    pub title_px: f32,
    pub subtitle_px: f32,
    /// How wide the card is allowed to look while it is still a window.
    pub thumb_radius: f32,
}

impl Default for Theme {
    fn default() -> Self {
        Theme {
            card_w: 168.0,
            card_h: 118.0,
            card_radius: 14.0,
            pin_h: 22.0,
            pin_w: 9.0,
            icon: 16.0,
            footer_h: 26.0,
            title_px: 11.5,
            subtitle_px: 9.5,
            thumb_radius: 10.0,
        }
    }
}

impl Theme {
    /// The area the thumbnail occupies inside a card, in logical pixels.
    pub fn thumb_size(&self) -> (f32, f32) {
        (self.card_w - 8.0, self.card_h - self.footer_h - 8.0)
    }
}

/// A palette picked for legibility on both light and dark wallpapers: mid
/// saturation, mid lightness, no two neighbours close in hue.
pub const PALETTE: [Rgb; 12] = [
    Rgb(0x3B, 0x82, 0xF6), // blue
    Rgb(0xEF, 0x6C, 0x3B), // orange
    Rgb(0x22, 0xA5, 0x7A), // green
    Rgb(0xA8, 0x55, 0xF7), // violet
    Rgb(0xE0, 0x4F, 0x6E), // rose
    Rgb(0x0E, 0xA5, 0xA8), // teal
    Rgb(0xD9, 0x77, 0x06), // amber
    Rgb(0x63, 0x66, 0xF1), // indigo
    Rgb(0x84, 0xCC, 0x16), // lime
    Rgb(0xC0, 0x26, 0xD3), // magenta
    Rgb(0x08, 0x91, 0xB2), // cyan
    Rgb(0xB4, 0x53, 0x09), // brown-orange
];

/// Picks the accent for an application key (executable name, lower-cased).
/// Deterministic across runs and cheap enough to call per frame.
pub fn accent_for(key: &str) -> Rgb {
    if key.is_empty() {
        return PALETTE[0];
    }
    // FNV-1a: stable across Rust versions, unlike std's DefaultHasher.
    let mut hash: u32 = 2_166_136_261;
    for b in key.as_bytes() {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(16_777_619);
    }
    PALETTE[(hash as usize) % PALETTE.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accent_is_stable() {
        assert_eq!(accent_for("chrome.exe"), accent_for("chrome.exe"));
    }

    #[test]
    fn different_apps_usually_differ() {
        let names = ["chrome.exe", "code.exe", "wechat.exe", "explorer.exe", "notepad.exe", "slack.exe"];
        let mut seen = std::collections::HashSet::new();
        for n in names {
            seen.insert(accent_for(n));
        }
        assert!(seen.len() >= 4, "palette collided too often: {seen:?}");
    }
}
