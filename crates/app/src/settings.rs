//! User settings, stored as `key=value` lines in
//! `%LOCALAPPDATA%\ShaiWin\settings.txt`. No serde, no registry: the file is
//! meant to be readable and editable by hand.

use std::fs;
use std::path::PathBuf;

use shaiwin_core::layout::{LineLayout, SAG_PERCENT_MAX};

pub const APP_DIR: &str = "ShaiWin";

/// `sag_percent` holds this while the rope keeps the curve the program ships
/// with. No valid slider value is negative, and a hand-edited file that leaves
/// `sag=` empty means the same thing.
pub const SAG_AUTO: f32 = -1.0;

pub const MOD_ALT: u32 = 0x0001;
pub const MOD_CONTROL: u32 = 0x0002;
pub const MOD_SHIFT: u32 = 0x0004;
pub const MOD_WIN: u32 = 0x0008;
const MOD_ALL: u32 = MOD_ALT | MOD_CONTROL | MOD_SHIFT | MOD_WIN;

pub const VK_2: u32 = 0x32;
pub const VK_T: u32 = 0x54;

pub fn data_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .or_else(|| std::env::var_os("APPDATA"))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(APP_DIR)
}

#[derive(Clone, Debug)]
pub struct Settings {
    /// Shown the welcome balloon once.
    pub welcomed: bool,
    /// Respect the system's "reduce motion" intent: no flights, shorter slides.
    pub reduced_motion: bool,
    /// Lower-cased executable names that may not be hung.
    pub excluded: Vec<String>,
    /// Show the line at the top of every monitor instead of just one.
    pub all_monitors: bool,
    /// Modifiers and virtual-key code for "hang the foreground window".
    /// A virtual key of 0 means the shortcut is switched off.
    pub hang_mods: u32,
    pub hang_vk: u32,
    /// The same, for showing/hiding the line.
    pub toggle_mods: u32,
    pub toggle_vk: u32,
    /// Sag of the rope at the middle, as a percentage of the monitor width.
    /// [`SAG_AUTO`] keeps the curve the program ships with, which is not a
    /// percentage of anything: it is a fixed 26 logical pixels.
    pub sag_percent: f32,
    /// Hovering the top edge brings the line down immediately. When off — the
    /// default — the dwell opens a small "expand the line" pill instead, so a
    /// cursor that merely grazes the top of the screen never takes the screen.
    pub hover_open: bool,
    /// Explained once that taking a window down keeps it hooked.
    pub hook_taught: bool,
    /// The line comes down when ShaiWin starts and stays down until it is
    /// dismissed. Off it again, and a program in the tray looks like a program
    /// that is not running.
    pub always_on: bool,
    /// Dragging a window to the top of the screen takes part in hanging at all:
    /// the line lights and letting go hangs it. Off — the line never answers a
    /// drag, and windows go up by hotkey or tray.
    pub drag_hang: bool,
}

impl Default for Settings {
    fn default() -> Settings {
        Settings {
            welcomed: false,
            reduced_motion: false,
            excluded: Vec::new(),
            all_monitors: false,
            hang_mods: MOD_CONTROL,
            hang_vk: VK_2,
            toggle_mods: MOD_CONTROL,
            toggle_vk: VK_T,
            sag_percent: SAG_AUTO,
            hover_open: false,
            hook_taught: false,
            always_on: true,
            drag_hang: true,
        }
    }
}

impl Settings {
    /// The rope as it ships. Everything else is the user dialing it in.
    pub fn layout_for(&self, width: f32, scale: f32) -> LineLayout {
        if self.sag_is_auto() {
            LineLayout::new(width, scale)
        } else {
            LineLayout::with_sag(width, scale, self.sag_percent)
        }
    }

    pub fn sag_is_auto(&self) -> bool {
        !(0.0..=SAG_PERCENT_MAX).contains(&self.sag_percent)
    }

    fn file() -> PathBuf {
        data_dir().join("settings.txt")
    }

    pub fn load() -> Settings {
        let mut s = Settings::default();
        let Ok(text) = fs::read_to_string(Self::file()) else { return s };
        for line in text.lines() {
            let Some((key, value)) = line.split_once('=') else { continue };
            let value = value.trim();
            match key.trim() {
                "welcomed" => s.welcomed = value == "1",
                "reduced_motion" => s.reduced_motion = value == "1",
                "all_monitors" => s.all_monitors = value == "1",
                "hover_open" => s.hover_open = value == "1",
                "hook_taught" => s.hook_taught = value == "1",
                "always_on" => s.always_on = value != "0",
                "drag_hang" => s.drag_hang = value != "0",
                "excluded" => {
                    s.excluded = value
                        .split(',')
                        .map(|e| e.trim().to_lowercase())
                        .filter(|e| !e.is_empty())
                        .collect();
                }
                "hang_mods" => s.hang_mods = parse_mods(value),
                "hang_vk" => s.hang_vk = parse_vk(value),
                "toggle_mods" => s.toggle_mods = parse_mods(value),
                "toggle_vk" => s.toggle_vk = parse_vk(value),
                "sag" => s.sag_percent = parse_sag(value),
                _ => {}
            }
        }
        s
    }

    pub fn save(&self) {
        let file = Self::file();
        if let Some(dir) = file.parent() {
            let _ = fs::create_dir_all(dir);
        }
        let text = format!(
            "welcomed={}\nreduced_motion={}\nall_monitors={}\n\
             hang_mods={}\nhang_vk={}\ntoggle_mods={}\ntoggle_vk={}\n\
             sag={}\nhover_open={}\nhook_taught={}\nalways_on={}\ndrag_hang={}\nexcluded={}\n",
            u8::from(self.welcomed),
            u8::from(self.reduced_motion),
            u8::from(self.all_monitors),
            self.hang_mods,
            self.hang_vk,
            self.toggle_mods,
            self.toggle_vk,
            if self.sag_is_auto() { String::new() } else { format!("{:.1}", self.sag_percent) },
            u8::from(self.hover_open),
            u8::from(self.hook_taught),
            u8::from(self.always_on),
            u8::from(self.drag_hang),
            self.excluded.join(",")
        );
        let _ = fs::write(file, text);
    }

    pub fn is_excluded(&self, exe: &str) -> bool {
        self.excluded.iter().any(|e| e == exe)
    }

    pub fn hang_label(&self) -> String {
        hotkey_label(self.hang_mods, self.hang_vk)
    }

    pub fn toggle_label(&self) -> String {
        hotkey_label(self.toggle_mods, self.toggle_vk)
    }
}

/// A human-readable shortcut, e.g. `Ctrl+Alt+H`. `vk == 0` means unbound.
pub fn hotkey_label(mods: u32, vk: u32) -> String {
    if vk == 0 {
        return "—".to_string();
    }
    let mut parts = Vec::new();
    if mods & MOD_CONTROL != 0 {
        parts.push("Ctrl");
    }
    if mods & MOD_ALT != 0 {
        parts.push("Alt");
    }
    if mods & MOD_SHIFT != 0 {
        parts.push("Shift");
    }
    if mods & MOD_WIN != 0 {
        parts.push("Win");
    }
    let key = key_name(vk);
    parts.push(key.as_str());
    parts.join("+")
}

fn key_name(vk: u32) -> String {
    match vk {
        0x30..=0x39 => char::from_u32(vk).map(String::from).unwrap_or_else(|| format!("VK{vk:02X}")),
        0x41..=0x5A => char::from_u32(vk).map(String::from).unwrap_or_else(|| format!("VK{vk:02X}")),
        0x70..=0x87 => format!("F{}", vk - 0x6F),
        0x20 => "Space".to_string(),
        0x25 => "Left".to_string(),
        0x26 => "Up".to_string(),
        0x27 => "Right".to_string(),
        0x28 => "Down".to_string(),
        0x24 => "Home".to_string(),
        0x23 => "End".to_string(),
        0x21 => "PgUp".to_string(),
        0x22 => "PgDn".to_string(),
        0x2D => "Insert".to_string(),
        0x2E => "Delete".to_string(),
        0x09 => "Tab".to_string(),
        0x0D => "Enter".to_string(),
        0x1B => "Esc".to_string(),
        _ => format!("VK{vk:02X}"),
    }
}

/// Clamps whatever is in the file. A hand-edited `sag=99` is a typo, not a
/// request to draw the rope off the screen. An empty or negative value means
/// the shipped curve.
pub fn parse_sag(value: &str) -> f32 {
    match value.parse::<f32>() {
        Ok(v) if v >= 0.0 => v.clamp(0.0, SAG_PERCENT_MAX),
        _ => SAG_AUTO,
    }
}

fn parse_mods(value: &str) -> u32 {
    value.parse::<u32>().map(|v| v & MOD_ALL).unwrap_or(0)
}

fn parse_vk(value: &str) -> u32 {
    value.parse::<u32>().map(|v| v & 0xFF).unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_are_readable() {
        assert_eq!(hotkey_label(MOD_CONTROL | MOD_ALT, 0x48), "Ctrl+Alt+H");
        assert_eq!(hotkey_label(0, 0x70), "F1");
        assert_eq!(hotkey_label(MOD_SHIFT, 0x41), "Shift+A");
    }

    #[test]
    fn the_defaults_are_bound() {
        let s = Settings::default();
        assert_eq!(s.hang_label(), "Ctrl+2");
        assert_eq!(s.toggle_label(), "Ctrl+T");
        assert!(s.sag_is_auto());
    }

    #[test]
    fn an_empty_or_negative_sag_is_the_shipped_curve() {
        assert_eq!(parse_sag(""), SAG_AUTO);
        assert_eq!(parse_sag("-4"), SAG_AUTO);
        assert_eq!(parse_sag("nope"), SAG_AUTO);
        assert_eq!(parse_sag("99"), SAG_PERCENT_MAX);
        assert!((parse_sag("2.5") - 2.5).abs() < 0.01);
        assert!((parse_sag("0.0") - 0.0).abs() < 0.01);
    }

    #[test]
    fn auto_sag_keeps_the_shipped_rope_on_any_width() {
        let mut s = Settings::default();
        // The same settings on a laptop and an ultrawide both get 26 logical pixels.
        for (width, scale) in [(1920.0, 1.0), (2560.0, 1.0), (3440.0, 1.25)] {
            let l = s.layout_for(width, scale);
            assert!((l.sag() - 26.0 * scale).abs() < 0.01, "{width}@{scale}: sag = {}", l.sag());
        }
        s.sag_percent = 4.0;
        assert!((s.layout_for(2560.0, 1.0).sag() - 102.4).abs() < 0.01);
    }
}
