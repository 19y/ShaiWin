//! Tray icon, its menu, balloons, and the "start with Windows" entry.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO, NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY,
    NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{HICON, WM_APP};
use winreg::RegKey;
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

pub const WM_TRAY: u32 = WM_APP + 2;
pub const TRAY_ID: u32 = 1;

const RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
const RUN_VALUE: &str = "ShaiWin";

fn nid(hwnd: HWND, icon: Option<HICON>) -> NOTIFYICONDATAW {
    let mut data = NOTIFYICONDATAW {
        cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
        hWnd: hwnd,
        uID: TRAY_ID,
        uFlags: NIF_MESSAGE | NIF_ICON | NIF_TIP | NIF_SHOWTIP,
        uCallbackMessage: WM_TRAY,
        ..Default::default()
    };
    if let Some(icon) = icon {
        data.hIcon = icon;
    }
    data
}

fn set_text(data: &mut NOTIFYICONDATAW, slot: &mut [u16; 128], text: &str) {
    let wide: Vec<u16> = text.encode_utf16().take(127).collect();
    slot[..wide.len()].copy_from_slice(&wide);
    data.szTip = *slot;
}

pub fn add(hwnd: HWND, icon: HICON, tip: &str) -> bool {
    unsafe {
        let mut data = nid(hwnd, Some(icon));
        let mut text = [0u16; 128];
        set_text(&mut data, &mut text, tip);
        Shell_NotifyIconW(NIM_ADD, &data).as_bool()
    }
}

pub fn remove(hwnd: HWND) {
    unsafe {
        let data = nid(hwnd, None);
        let _ = Shell_NotifyIconW(NIM_DELETE, &data);
    }
}

pub fn set_tip(hwnd: HWND, icon: HICON, tip: &str) {
    unsafe {
        let mut data = nid(hwnd, Some(icon));
        let mut text = [0u16; 128];
        set_text(&mut data, &mut text, tip);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

/// Shows a tray balloon. Used for errors that happen while the line is hidden.
pub fn balloon(hwnd: HWND, icon: HICON, title: &str, body: &str, warning: bool) {
    unsafe {
        let mut data = nid(hwnd, Some(icon));
        data.uFlags = NIF_INFO | NIF_ICON;
        data.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
        let title_wide: Vec<u16> = title.encode_utf16().take(63).collect();
        data.szInfoTitle[..title_wide.len()].copy_from_slice(&title_wide);
        let body_wide: Vec<u16> = body.encode_utf16().take(255).collect();
        data.szInfo[..body_wide.len()].copy_from_slice(&body_wide);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

pub fn start_at_login() -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ)
        .ok()
        .and_then(|key| key.get_value::<String, _>(RUN_VALUE).ok())
        .is_some()
}

pub fn set_start_at_login(on: bool) -> bool {
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let Ok(key) = hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE) else { return false };
    if on {
        let Ok(exe) = std::env::current_exe() else { return false };
        let command = format!("\"{}\"", exe.display());
        key.set_value(RUN_VALUE, &command).is_ok()
    } else {
        let _ = key.delete_value(RUN_VALUE);
        true
    }
}

/// Plays the system's soft notification sound, if any.
pub fn beep() {
    unsafe {
        let _ = windows::Win32::System::Diagnostics::Debug::MessageBeep(
            windows::Win32::UI::WindowsAndMessaging::MB_ICONASTERISK,
        );
    }
}

/// Screen position of the mouse, for popup menus.
pub fn cursor() -> POINT {
    let mut p = POINT::default();
    unsafe {
        let _ = windows::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p);
    }
    p
}

