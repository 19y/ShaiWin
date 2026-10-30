//! Everything that touches Win32 for ShaiWin.
//!
//! The app crate owns the overlay window, the tray and the animation loop; this
//! crate owns the parts that need to talk to other people's windows:
//!
//! * [`window`] — enumerating and describing other applications' windows.
//! * [`park`] — putting a window safely out of sight and bringing it back.
//! * [`capture`] — taking the still picture that becomes a card.
//! * [`icon`] — pulling an application's icon out of its executable.
//! * [`monitor`] — monitor geometry and DPI.
//!
//! Everything here is synchronous and may block for a few milliseconds (or, for
//! an unresponsive application, much longer). Callers must run capture and icon
//! work off the UI thread.

#![cfg(windows)]

pub mod capture;
mod dib;
pub mod icon;
pub mod image;
pub mod monitor;
pub mod park;
pub mod window;

pub use image::Image;

/// The six numbers a parked window needs remembered, in a form that can go in
/// the journal and come back after a crash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(&self) -> i32 {
        self.right - self.left
    }

    pub fn height(&self) -> i32 {
        self.bottom - self.top
    }

    pub fn is_empty(&self) -> bool {
        self.width() <= 0 || self.height() <= 0
    }

    pub fn center(&self) -> (i32, i32) {
        ((self.left + self.right) / 2, (self.top + self.bottom) / 2)
    }

    pub fn from_xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Rect {
            left: x,
            top: y,
            right: x + w,
            bottom: y + h,
        }
    }

    pub fn overlaps(&self, other: &Rect) -> bool {
        self.left < other.right
            && other.left < self.right
            && self.top < other.bottom
            && other.top < self.bottom
    }
}

/// A short human-readable description of the last Win32 error. Used in the
/// tray balloon when a window refuses to be moved.
pub fn last_error_text(code: u32) -> String {
    let message = match code {
        5 => "Access denied (the window belongs to an elevated or protected process)",
        87 => "Invalid parameter",
        1400 => "Invalid window handle (the window has closed)",
        0 => "No error",
        _ => "Windows error",
    };
    format!("{message} (0x{code:04X})")
}
