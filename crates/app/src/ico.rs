//! The embedded application icon.
//!
//! The `.ico` is compiled into the binary, so the app is a single file with no
//! side-car assets. Windows wants an `HICON`, which is not the same thing as the
//! `.ico` file, so the container is parsed and the best-matching image is
//! handed to `CreateIconFromResourceEx`.

use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconFromResourceEx, DestroyIcon, GetSystemMetrics, HICON, LR_DEFAULTCOLOR, SM_CXICON,
    SM_CXSMICON,
};

const FILE: &[u8] = include_bytes!("../../../assets/shaiwin.ico");

/// The large icon size the shell asks for at the current DPI (32 px at 100%).
pub fn large_size() -> i32 {
    unsafe { GetSystemMetrics(SM_CXICON) }.max(32)
}

/// The small icon size the shell asks for at the current DPI (16 px at 100%,
/// 24 px at 150%). Handing Windows an icon that is already this size is what
/// keeps the title bar, the tray and the taskbar identical to Explorer's copy
/// instead of three different downscales of a 32 px image.
pub fn small_size() -> i32 {
    unsafe { GetSystemMetrics(SM_CXSMICON) }.max(16)
}

/// Loads the icon best suited to `size` pixels, and frees every other copy.
pub fn load(size: i32) -> Option<HICON> {
    let count = u16::from_le_bytes([*FILE.get(4)?, *FILE.get(5)?]) as usize;
    if count == 0 {
        return None;
    }
    let mut best: Option<(u32, &[u8])> = None;
    for i in 0..count {
        let base = 6 + i * 16;
        let w = *FILE.get(base)? as u32;
        let w = if w == 0 { 256 } else { w };
        let len = u32::from_le_bytes(FILE.get(base + 8..base + 12)?.try_into().ok()?) as usize;
        let offset = u32::from_le_bytes(FILE.get(base + 12..base + 16)?.try_into().ok()?) as usize;
        let data = FILE.get(offset..offset + len)?;
        let delta = w.abs_diff(size as u32);
        if best.is_none_or(|(d, _)| delta < d) {
            best = Some((delta, data));
        }
    }
    let (_, data) = best?;
    unsafe { CreateIconFromResourceEx(data, true, 0x0003_0000, 0, 0, LR_DEFAULTCOLOR).ok() }
}

pub fn release(icon: HICON) {
    unsafe {
        let _ = DestroyIcon(icon);
    }
}
