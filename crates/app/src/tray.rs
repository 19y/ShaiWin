//! Tray icon, its menu, balloons, and the "start with Windows" entry.

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIIF_INFO,
    NIIF_WARNING, NIM_ADD, NIM_DELETE, NIM_MODIFY, NOTIFYICONDATAW,
};
use windows::Win32::UI::WindowsAndMessaging::{HICON, WM_APP};
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};
use winreg::RegKey;

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

/// Changes only the tooltip. The other flags are deliberately left out: a
/// NIM_MODIFY that carries NIF_ICON re-publishes the icon, and the shell
/// redraws the notification area every time it happens, which reads as the
/// taskbar flickering while the line is being used.
pub fn set_tip(hwnd: HWND, tip: &str) {
    unsafe {
        let mut data = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: TRAY_ID,
            uFlags: NIF_TIP | NIF_SHOWTIP,
            ..Default::default()
        };
        let mut text = [0u16; 128];
        set_text(&mut data, &mut text, tip);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

/// Shows a tray balloon. Used for errors that happen while the line is hidden.
/// `NOTIFYICONDATAW` carries the balloon text in fixed arrays, and Windows cuts
/// whatever does not fit at the end of the array - which is how a lesson in a
/// language with longer words than yours used to stop in the middle of one.
pub const BALLOON_MAX: usize = 255;
const TITLE_MAX: usize = 63;

pub fn balloon(hwnd: HWND, icon: HICON, title: &str, body: &str, warning: bool) {
    unsafe {
        let mut data = nid(hwnd, Some(icon));
        data.uFlags = NIF_INFO | NIF_ICON;
        data.dwInfoFlags = if warning { NIIF_WARNING } else { NIIF_INFO };
        let title_wide = clip(title, TITLE_MAX);
        data.szInfoTitle[..title_wide.len()].copy_from_slice(&title_wide);
        let body_wide = clip(body, BALLOON_MAX);
        data.szInfo[..body_wide.len()].copy_from_slice(&body_wide);
        let _ = Shell_NotifyIconW(NIM_MODIFY, &data);
    }
}

/// `text` as UTF-16 for one of those arrays, at most `max` units. When it does
/// not fit, the cut comes at the last whole word that does, and the reader is
/// told the sentence went on.
fn clip(text: &str, max: usize) -> Vec<u16> {
    let mut keep = text.to_string();
    if keep.chars().count() > max {
        let head: String = keep.chars().take(max.saturating_sub(1)).collect();
        match head.rfind(char::is_whitespace) {
            // A language with no spaces in it at all is not a reason to show
            // nothing: take the characters.
            Some(at) if at > max / 2 => keep = head[..at].trim_end().to_string(),
            _ => keep = head,
        }
        keep.push('…');
    }
    keep.encode_utf16().take(max).collect()
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
    let Ok(key) = hkcu.open_subkey_with_flags(RUN_KEY, KEY_READ | KEY_WRITE) else {
        return false;
    };
    if on {
        let Ok(exe) = std::env::current_exe() else {
            return false;
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(text: &str, max: usize) -> String {
        String::from_utf16(&clip(text, max)).expect("the clip is valid text")
    }

    #[test]
    fn a_short_balloon_is_left_alone() {
        assert_eq!(shown("Quit ShaiWin", 63), "Quit ShaiWin");
    }

    /// The cut is where a translator meets the API, so it has to land somewhere
    /// a person can read: at a word, with the tail of the sentence admitted.
    #[test]
    fn a_cut_balloon_ends_at_a_word() {
        let long = "one two three four five six seven eight nine ten";
        let cut = shown(long, 21);
        assert_eq!(cut, "one two three four…");
        assert!(cut.chars().count() <= 21);
        // A language that writes the whole lesson without a space still gets the
        // characters it has room for instead of an empty balloon.
        let packed = "一二三四五六七八九十";
        assert_eq!(shown(packed, 4), "一二三…");
    }
}
