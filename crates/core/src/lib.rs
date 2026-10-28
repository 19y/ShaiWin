//! The reusable heart of ShaiWin.
//!
//! Nothing in this crate touches an operating system. It owns the things that
//! must feel identical on every platform:
//!
//! * [`layout`] — where the line hangs, where each slot sits, how far the rope
//!   sags, how many cards fit.
//! * [`spring`] — the damped spring every moving value uses. Stepped in fixed
//!   substeps so the motion is frame-rate independent.
//! * [`anim`] — the arc a window's snapshot flies along when it is hung up or
//!   taken back down.
//! * [`theme`] — design sizes, colours, and the stable accent colour each app
//!   gets (this is what makes "who belongs to whom" readable at a glance).
//! * [`hit`] — plain rectangles and hit testing.
//!
//! The Windows shell lives in `shaiwin-platform-win` and `shaiwin`; a future
//! macOS shell (Swift/AppKit) consumes this crate through `shaiwin-ffi`.

#![forbid(unsafe_code)]

pub mod anim;
pub mod hit;
pub mod layout;
pub mod spring;
pub mod theme;

pub use anim::{ease_in_out_cubic, ease_out_cubic, ArcFlight};
pub use hit::Rect;
pub use layout::LineLayout;
pub use spring::Spring;
pub use theme::{Rgb, Theme};

/// The version of the reusable core, shown in the settings window and used by
/// the macOS shell to make sure the static library it links matches.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
