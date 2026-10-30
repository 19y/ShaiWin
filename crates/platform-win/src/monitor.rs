//! Monitor geometry and DPI.
//!
//! Everything the overlay does is per-monitor: each display gets its own line
//! at its own DPI, and cards never straddle two screens.

use std::ffi::c_void;

use windows::core::BOOL;
use windows::Win32::Foundation::{LPARAM, POINT, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromRect, HDC, HMONITOR,
    MONITORINFO, MONITOR_DEFAULTTONEAREST, MONITOR_DEFAULTTOPRIMARY,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;

use crate::Rect;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Monitor {
    pub handle: isize,
    /// Full bounds, in virtual-screen device pixels.
    pub bounds: Rect,
    /// Bounds minus the taskbar and other appbars.
    pub work: Rect,
    /// DPI factor: 1.0, 1.25, 1.5, 1.75, 2.0...
    pub scale: f32,
    pub primary: bool,
}

impl Monitor {
    pub fn width(&self) -> i32 {
        self.bounds.width()
    }

    pub fn height(&self) -> i32 {
        self.bounds.height()
    }
}

pub fn monitors() -> Vec<Monitor> {
    let mut found: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(
            None,
            None,
            Some(collect),
            LPARAM(&mut found as *mut Vec<Monitor> as isize),
        );
    }
    if found.is_empty() {
        // A headless session (some CI machines) still deserves a sane fallback.
        found.push(Monitor {
            handle: 0,
            bounds: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1080,
            },
            work: Rect {
                left: 0,
                top: 0,
                right: 1920,
                bottom: 1040,
            },
            scale: 1.0,
            primary: true,
        });
    }
    found.sort_by_key(|m| (!m.primary, m.bounds.left));
    found
}

unsafe extern "system" fn collect(
    hmon: HMONITOR,
    _hdc: HDC,
    _rect: *mut RECT,
    lparam: LPARAM,
) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<Monitor>) };
    if let Some(monitor) = unsafe { read(hmon) } {
        list.push(monitor);
    }
    true.into()
}

/// One monitor from its handle, without enumerating every display.
unsafe fn read(hmon: HMONITOR) -> Option<Monitor> {
    unsafe {
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        if !GetMonitorInfoW(hmon, &mut info).as_bool() {
            return None;
        }
        let mut sx = 96u32;
        let mut sy = 96u32;
        let scale = match GetDpiForMonitor(hmon, MDT_EFFECTIVE_DPI, &mut sx, &mut sy) {
            Ok(()) if sx > 0 => sx as f32 / 96.0,
            _ => 1.0,
        };
        Some(Monitor {
            handle: hmon.0 as isize,
            bounds: Rect {
                left: info.rcMonitor.left,
                top: info.rcMonitor.top,
                right: info.rcMonitor.right,
                bottom: info.rcMonitor.bottom,
            },
            work: Rect {
                left: info.rcWork.left,
                top: info.rcWork.top,
                right: info.rcWork.right,
                bottom: info.rcWork.bottom,
            },
            scale,
            primary: info.dwFlags & 1 != 0,
        })
    }
}

/// The monitor containing a screen point.
pub fn at_point(x: i32, y: i32) -> Monitor {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x, y }, MONITOR_DEFAULTTONEAREST);
        // Asking the handle it named is enough: enumerating every display and
        // its DPI to find the same one again is a cost paid on every hang and
        // every line toggle.
        read(hmon).unwrap_or_else(|| monitors()[0])
    }
}

/// The monitor a window is mostly on.
pub fn for_window(hwnd: isize) -> Monitor {
    unsafe {
        let hwnd = crate::window::hwnd_of(hwnd);
        let mut rect = RECT::default();
        if windows::Win32::UI::WindowsAndMessaging::GetWindowRect(hwnd, &mut rect).is_err() {
            return primary();
        }
        let hmon = MonitorFromRect(&rect, MONITOR_DEFAULTTONEAREST);
        read(hmon).unwrap_or_else(primary)
    }
}

pub fn primary() -> Monitor {
    unsafe {
        let hmon = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        read(hmon).unwrap_or_else(|| monitors()[0])
    }
}

pub fn cursor() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

pub fn virtual_bounds() -> Rect {
    let all = monitors();
    let mut r = all[0].bounds;
    for m in &all[1..] {
        r = Rect {
            left: r.left.min(m.bounds.left),
            top: r.top.min(m.bounds.top),
            right: r.right.max(m.bounds.right),
            bottom: r.bottom.max(m.bounds.bottom),
        };
    }
    r
}

#[allow(dead_code)]
fn _assert_send(_: *mut c_void) {}
