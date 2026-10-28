//! C ABI over the reusable core, for a future macOS AppKit/SwiftUI shell.
//!
//! The header lives in `include/shaiwin.h`. Everything here is plain `f32` and
//! opaque pointers: no Rust types cross the boundary, and the caller owns the
//! lifetime of every pointer it is given.

use shaiwin_core::{ArcFlight, LineLayout};

/// The core's version as a NUL-terminated string. The caller must not free it.
#[no_mangle]
pub extern "C" fn shaiwin_version() -> *const std::os::raw::c_char {
    static VERSION: &str = concat!(env!("CARGO_PKG_VERSION"), "\0");
    VERSION.as_ptr() as *const std::os::raw::c_char
}

/// Creates a layout for a monitor. Free with [`shaiwin_layout_free`].
#[no_mangle]
pub extern "C" fn shaiwin_layout_new(width: f32, scale: f32) -> *mut LineLayout {
    Box::into_raw(Box::new(LineLayout::new(width, scale)))
}

/// # Safety
/// `layout` must come from [`shaiwin_layout_new`] and must not be used after.
#[no_mangle]
pub unsafe extern "C" fn shaiwin_layout_free(layout: *mut LineLayout) {
    if !layout.is_null() {
        drop(unsafe { Box::from_raw(layout) });
    }
}

/// # Safety
/// `layout` must be a valid pointer from [`shaiwin_layout_new`].
#[no_mangle]
pub unsafe extern "C" fn shaiwin_layout_slot_x(layout: *const LineLayout, index: usize, count: usize) -> f32 {
    unsafe { (*layout).slot_x(index, count) }
}

/// # Safety
/// `layout` must be a valid pointer from [`shaiwin_layout_new`].
#[no_mangle]
pub unsafe extern "C" fn shaiwin_layout_rope_y(layout: *const LineLayout, x: f32) -> f32 {
    unsafe { (*layout).rope_y(x) }
}

/// # Safety
/// `layout` must be a valid pointer from [`shaiwin_layout_new`].
#[no_mangle]
pub unsafe extern "C" fn shaiwin_layout_panel_height(layout: *const LineLayout) -> f32 {
    unsafe { (*layout).panel_height() }
}

/// A flight the Swift shell can drive its own layer animation with.
#[repr(C)]
pub struct ShaiwinFlight {
    flight: ArcFlight,
}

#[no_mangle]
pub extern "C" fn shaiwin_flight_new(
    from_x: f32,
    from_y: f32,
    to_x: f32,
    to_y: f32,
    lift: f32,
    duration: f32,
) -> *mut ShaiwinFlight {
    Box::into_raw(Box::new(ShaiwinFlight {
        flight: ArcFlight::new((from_x, from_y), (to_x, to_y), lift, duration),
    }))
}

/// Advances the flight; returns 1 while it is still in the air.
///
/// # Safety
/// `flight` must come from [`shaiwin_flight_new`].
#[no_mangle]
pub unsafe extern "C" fn shaiwin_flight_step(flight: *mut ShaiwinFlight, dt: f32) -> i32 {
    i32::from(unsafe { (*flight).flight.step(dt) })
}

/// Writes the current position into `out` (two floats).
///
/// # Safety
/// `flight` must be valid and `out` must point at two writable floats.
#[no_mangle]
pub unsafe extern "C" fn shaiwin_flight_position(flight: *const ShaiwinFlight, out: *mut f32) {
    let (x, y) = unsafe { (*flight).flight.position() };
    unsafe {
        *out = x;
        *out.add(1) = y;
    }
}

/// # Safety
/// `flight` must come from [`shaiwin_flight_new`].
#[no_mangle]
pub unsafe extern "C" fn shaiwin_flight_free(flight: *mut ShaiwinFlight) {
    if !flight.is_null() {
        drop(unsafe { Box::from_raw(flight) });
    }
}
