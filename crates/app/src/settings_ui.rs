//! The settings window: two shortcut controls, the rope-curve slider and the
//! hover behaviour. Native Win32 controls keep it small and let Windows do the
//! keyboard handling, IME and accessibility.
//!
//! The window is created on demand and destroyed when it closes; only one may
//! exist at a time. It talks back to [`App`] through the pointer handed to it
//! by `open`, so saving is a direct call, not a file round-trip.

use std::ffi::c_void;
use std::sync::atomic::{AtomicIsize, Ordering};

use shaiwin_core::layout::SAG_PERCENT_MAX;
use shaiwin_platform_win::monitor::Monitor;
use windows::core::{PCWSTR, w};
use windows::Win32::Foundation::{HINSTANCE, HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, COLOR_WINDOW, CreateFontW,
    DEFAULT_CHARSET, DEFAULT_GUI_FONT, DEFAULT_PITCH, DeleteObject, FW_NORMAL, GetStockObject,
    HBRUSH, HFONT, HGDIOBJ, OUT_DEFAULT_PRECIS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemServices::SS_LEFT;
use windows::Win32::UI::Controls::{
    BST_CHECKED, HKM_GETHOTKEY, HKM_SETHOTKEY, HOTKEYF_ALT, HOTKEYF_CONTROL, HOTKEYF_EXT, HOTKEYF_SHIFT,
    ICC_BAR_CLASSES, ICC_HOTKEY_CLASS, ICC_STANDARD_CLASSES, INITCOMMONCONTROLSEX, InitCommonControlsEx,
    TBM_SETPOS, TBM_SETRANGE, TBM_SETTICFREQ, TBS_AUTOTICKS, TBS_HORZ,
};
use windows::Win32::UI::HiDpi::AdjustWindowRectExForDpi;
use windows::Win32::UI::Input::KeyboardAndMouse::SetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    BN_CLICKED, BS_AUTOCHECKBOX, BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CreateWindowExW,
    DefWindowProcW, DestroyWindow, GWLP_USERDATA, GetClientRect, GetWindowLongPtrW, HICON, HMENU,
    ICON_BIG, ICON_SMALL, IDC_ARROW, IsDialogMessageW, LoadCursorW, MB_ICONWARNING, MB_OK, MSG,
    MessageBoxW, RegisterClassExW, SW_SHOWNORMAL, SendMessageW, SetForegroundWindow,
    SetWindowLongPtrW, SetWindowTextW, ShowWindow, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_HSCROLL,
    WM_SETFONT, WM_SETICON, WNDCLASSEXW, WINDOW_EX_STYLE, WINDOW_STYLE, WS_BORDER, WS_CAPTION,
    WS_CHILD, WS_OVERLAPPED, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
};

use crate::app::App;
use crate::i18n::t;
use crate::settings;

const CLASS: PCWSTR = w!("ShaiWin.Settings");
const SAVE_ID: i32 = 1;
const CANCEL_ID: i32 = 2;
const HANG_ID: i32 = 101;
const TOGGLE_ID: i32 = 102;
const SAG_ID: i32 = 103;
const HOVER_ID: i32 = 104;

/// `BM_GETCHECK` / `BM_SETCHECK`: stable control messages, not worth importing
/// two constants for.
const BM_GETCHECK: u32 = 0x00F0;
const BM_SETCHECK: u32 = 0x00F1;
/// `TBM_GETPOS` is `WM_USER + 0` and is not in the bindings.
const TBM_GETPOS: u32 = 0x0400;

/// The window height is computed in logical pixels; the client area is 420
/// high, the rest is the title bar.
const LOGICAL_W: f32 = 520.0;
const LOGICAL_H: f32 = 470.0;

/// Both bottom buttons are the same size.
const BUTTON_W: f32 = 96.0;
const BUTTON_H: f32 = 32.0;

/// Set while a settings window exists, for `IsDialogMessage` in the message
/// loop (Tab navigation, Enter and Escape).
static SETTINGS_HWND: AtomicIsize = AtomicIsize::new(0);

struct State {
    app: *mut App,
    /// Borrowed from [`App`], which owns and frees them: the large one for Alt-Tab
    /// and the small one for the title bar.
    window_icon: HICON,
    tray_icon: HICON,
    scale: f32,
    font: HFONT,
    hang: HWND,
    toggle: HWND,
    sag: HWND,
    sag_value: HWND,
    hover: HWND,
    always: HWND,
    /// "Drag a window up to hang it" - the whole drag gesture, on or off.
    drag_hang: HWND,
    /// The curve as it was when the window opened, for putting it back if the
    /// user drags the slider and then cancels. May be [`SAG_AUTO`], which is not
    /// a percentage at all, so the slider keeps its own display value.
    initial_sag: f32,
    shown_sag: f32,
    previewed: bool,
    saved: bool,
}

pub fn register_class(hinst: HINSTANCE, big: HICON, small: HICON) {
    unsafe {
        let controls = INITCOMMONCONTROLSEX {
            dwSize: std::mem::size_of::<INITCOMMONCONTROLSEX>() as u32,
            dwICC: ICC_HOTKEY_CLASS | ICC_BAR_CLASSES | ICC_STANDARD_CLASSES,
        };
        let _ = InitCommonControlsEx(&controls);
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: hinst,
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hIcon: big,
            hIconSm: small,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut c_void),
            lpszClassName: CLASS,
            ..Default::default()
        };
        let _ = RegisterClassExW(&class);
    }
}

/// The percentage that reproduces the shipped curve on this monitor. The curve
/// itself is a fixed 26 logical pixels, so it is not a percentage of anything.
fn sag_shown(settings: &settings::Settings, monitor: &Monitor) -> f32 {
    let layout = settings.layout_for(monitor.bounds.width() as f32, monitor.scale);
    if settings.sag_is_auto() {
        layout.sag() / layout.width.max(1.0) * 100.0
    } else {
        settings.sag_percent
    }
}

/// Opens the settings window. The caller keeps the returned handle to focus an
/// existing window instead of making a second one.
pub fn open(
    window_icon: HICON,
    tray_icon: HICON,
    settings: &settings::Settings,
    monitor: Monitor,
    app: *mut App,
) -> Option<HWND> {
    unsafe {
        let hinst: HINSTANCE = GetModuleHandleW(None).ok()?.into();
        let scale = monitor.scale.max(1.0);
        let mut frame = RECT {
            left: 0,
            top: 0,
            right: (LOGICAL_W * scale).round() as i32,
            bottom: (LOGICAL_H * scale).round() as i32,
        };
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU;
        let _ = AdjustWindowRectExForDpi(
            &mut frame,
            style,
            false,
            WINDOW_EX_STYLE(0),
            (96.0 * scale).round() as u32,
        );
        let width = frame.right - frame.left;
        let height = frame.bottom - frame.top;
        let work = monitor.work;
        let x = work.left + ((work.width() - width) / 2).max(0);
        let y = work.top + ((work.height() - height) / 2).max(0);

        let title = wstr(t("settings_title"));
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            CLASS,
            PCWSTR(title.as_ptr()),
            style,
            x,
            y,
            width,
            height,
            None,
            None,
            Some(hinst),
            None,
        )
        .ok()?;

        let ptr = Box::into_raw(Box::new(State {
            app,
            window_icon,
            tray_icon,
            scale,
            font: make_font(scale),
            hang: HWND::default(),
            toggle: HWND::default(),
            sag: HWND::default(),
            sag_value: HWND::default(),
            hover: HWND::default(),
            always: HWND::default(),
            drag_hang: HWND::default(),
            initial_sag: settings.sag_percent,
            shown_sag: sag_shown(settings, &monitor),
            previewed: false,
            saved: false,
        }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, ptr as isize);
        (*ptr).build(hwnd, settings);
        SETTINGS_HWND.store(hwnd.0 as isize, Ordering::Relaxed);
        let _ = ShowWindow(hwnd, SW_SHOWNORMAL);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some((*ptr).hang));
        Some(hwnd)
    }
}

/// Called from the message loop before Translate/Dispatch. Returns true when
/// the dialog manager handled the key (Tab, Enter, Escape, arrows).
pub fn is_dialog_message(msg: &MSG) -> bool {
    let raw = SETTINGS_HWND.load(Ordering::Relaxed);
    if raw == 0 {
        return false;
    }
    unsafe { IsDialogMessageW(HWND(raw as *mut c_void), msg).as_bool() }
}

impl State {
    unsafe fn build(&mut self, hwnd: HWND, settings: &settings::Settings) {
        unsafe {
            let mut client = RECT::default();
            let _ = GetClientRect(hwnd, &mut client);
            let s = self.scale;
            let pad = 24.0 * s;
            let row = 46.0 * s;
            let label_w = 196.0 * s;
            let field_x = pad + label_w;
            let field_w = client.right as f32 - field_x - pad;
            let full_w = client.right as f32 - pad * 2.0;
            let mut y = pad;

            self.label(hwnd, t("settings_hang"), pad, y, label_w - 12.0 * s);
            self.hang = self.hotkey(hwnd, field_x, y - 3.0 * s, field_w, 28.0 * s, HANG_ID);
            y += row;
            self.label(hwnd, t("settings_toggle"), pad, y, label_w - 12.0 * s);
            self.toggle = self.hotkey(hwnd, field_x, y - 3.0 * s, field_w, 28.0 * s, TOGGLE_ID);
            y += row + 8.0 * s;

            self.label(hwnd, t("settings_sag"), pad, y, label_w - 12.0 * s);
            self.sag_value = self.label(hwnd, "", field_x, y, field_w);
            y += 25.0 * s;
            self.sag = self.trackbar(hwnd, pad, y, full_w, 38.0 * s);
            self.set_sag(settings.sag_is_auto());
            y += 50.0 * s;

            self.label(hwnd, t("settings_sag_note"), pad, y, full_w);
            y += 26.0 * s;
            self.always = self.checkbox(hwnd, t("settings_always"), pad, y, full_w);
            if settings.always_on {
                let _ = send(self.always, BM_SETCHECK, BST_CHECKED.0 as usize, 0);
            }
            y += 30.0 * s;
            self.hover = self.checkbox(hwnd, t("settings_hover"), pad, y, full_w);
            if settings.hover_open {
                let _ = send(self.hover, BM_SETCHECK, BST_CHECKED.0 as usize, 0);
            }
            y += 26.0 * s;
            self.label(hwnd, t("settings_hover_note"), pad, y, full_w);
            y += 34.0 * s;
            self.drag_hang = self.checkbox(hwnd, t("settings_drag"), pad, y, full_w);
            if settings.drag_hang {
                let _ = send(self.drag_hang, BM_SETCHECK, BST_CHECKED.0 as usize, 0);
            }
            y += 26.0 * s;
            self.label(hwnd, t("settings_drag_note"), pad, y, full_w);
            y += 30.0 * s;
            self.label(hwnd, t("settings_hint"), pad, y, full_w);

            let button_w = BUTTON_W * s;
            let button_y = client.bottom as f32 - pad - BUTTON_H * s;
            self.button(hwnd, t("settings_save"), SAVE_ID, client.right as f32 - pad - button_w * 2.0 - 12.0 * s, button_y, true);
            self.button(hwnd, t("settings_cancel"), CANCEL_ID, client.right as f32 - pad - button_w, button_y, false);

            if !self.window_icon.is_invalid() {
                let _ = send(hwnd, WM_SETICON, ICON_BIG as usize, self.window_icon.0 as isize);
            }
            if !self.tray_icon.is_invalid() {
                let _ = send(hwnd, WM_SETICON, ICON_SMALL as usize, self.tray_icon.0 as isize);
            }

            set_hotkey(self.hang, settings.hang_mods, settings.hang_vk);
            set_hotkey(self.toggle, settings.toggle_mods, settings.toggle_vk);
        }
    }

    unsafe fn label(&self, parent: HWND, text: &str, x: f32, y: f32, width: f32) -> HWND {
        unsafe {
            let wide = wstr(text);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("STATIC"),
                PCWSTR(wide.as_ptr()),
                WS_CHILD | WS_VISIBLE | WINDOW_STYLE(SS_LEFT.0),
                x.round() as i32,
                y.round() as i32,
                width.round() as i32,
                (22.0 * self.scale).round() as i32,
                Some(parent),
                None,
                None,
                None,
            )
            .unwrap_or_default();
            self.use_font(hwnd);
            hwnd
        }
    }

    unsafe fn hotkey(&mut self, parent: HWND, x: f32, y: f32, width: f32, height: f32, id: i32) -> HWND {
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("msctls_hotkey32"),
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WS_BORDER,
                x.round() as i32,
                y.round() as i32,
                width.round() as i32,
                height.round() as i32,
                Some(parent),
                Some(HMENU(id as usize as *mut c_void)),
                None,
                None,
            )
            .unwrap_or_default();
            self.use_font(hwnd);
            hwnd
        }
    }

    unsafe fn trackbar(&mut self, parent: HWND, x: f32, y: f32, width: f32, height: f32) -> HWND {
        unsafe {
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("msctls_trackbar32"),
                PCWSTR::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(TBS_AUTOTICKS | TBS_HORZ),
                x.round() as i32,
                y.round() as i32,
                width.round() as i32,
                height.round() as i32,
                Some(parent),
                Some(HMENU(SAG_ID as usize as *mut c_void)),
                None,
                None,
            )
            .unwrap_or_default();
            let max = (SAG_PERCENT_MAX * 10.0) as isize;
            let _ = send(hwnd, TBM_SETRANGE, 1, max << 16);
            let _ = send(hwnd, TBM_SETTICFREQ, 10, 0);
            self.use_font(hwnd);
            hwnd
        }
    }

    unsafe fn checkbox(&mut self, parent: HWND, text: &str, x: f32, y: f32, width: f32) -> HWND {
        unsafe {
            let wide = wstr(text);
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                PCWSTR(wide.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(BS_AUTOCHECKBOX as u32),
                x.round() as i32,
                y.round() as i32,
                width.round() as i32,
                (24.0 * self.scale).round() as i32,
                Some(parent),
                Some(HMENU(HOVER_ID as usize as *mut c_void)),
                None,
                None,
            )
            .unwrap_or_default();
            self.use_font(hwnd);
            hwnd
        }
    }

    unsafe fn button(
        &mut self,
        parent: HWND,
        text: &str,
        id: i32,
        x: f32,
        y: f32,
        default: bool,
    ) -> HWND {
        unsafe {
            let wide = wstr(text);
            let style = if default { BS_DEFPUSHBUTTON } else { BS_PUSHBUTTON };
            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                w!("BUTTON"),
                PCWSTR(wide.as_ptr()),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | WINDOW_STYLE(style as u32),
                x.round() as i32,
                y.round() as i32,
                (BUTTON_W * self.scale).round() as i32,
                (BUTTON_H * self.scale).round() as i32,
                Some(parent),
                Some(HMENU(id as usize as *mut c_void)),
                None,
                None,
            )
            .unwrap_or_default();
            self.use_font(hwnd);
            hwnd
        }
    }

    unsafe fn use_font(&self, hwnd: HWND) {
        if !hwnd.is_invalid() && !self.font.is_invalid() {
            unsafe {
                let _ = send(hwnd, WM_SETFONT, self.font.0 as usize, 1);
            }
        }
    }

    unsafe fn set_sag(&self, shipped: bool) {
        unsafe {
            let percent = self.shown_sag.clamp(0.0, SAG_PERCENT_MAX);
            let pos = (percent * 10.0).round() as isize;
            let _ = send(self.sag, TBM_SETPOS, 1, pos);
            // The shipped curve is a fixed 26 logical pixels; the number beside it
            // is only where that lands on this monitor, so say which one it is.
            let text = if shipped {
                format!("{percent:.1}% · {}", t("sag_shipped"))
            } else {
                format!("{percent:.1}%")
            };
            let wide = wstr(&text);
            let _ = SetWindowTextW(self.sag_value, PCWSTR(wide.as_ptr()));
        }
    }

    unsafe fn update_sag(&mut self) {
        unsafe {
            let pos = send(self.sag, TBM_GETPOS, 0, 0) as f32 / 10.0;
            let text = format!("{:.1}%", pos);
            let wide = wstr(&text);
            let _ = SetWindowTextW(self.sag_value, PCWSTR(wide.as_ptr()));
            // The line changes shape while the thumb moves, so the curve is
            // judged where it is used instead of against a number.
            self.previewed = true;
            (*self.app).preview_sag(pos);
        }
    }

    unsafe fn save(&mut self) -> bool {
        unsafe {
            let (hang_mods, hang_vk) = get_hotkey(self.hang);
            let (toggle_mods, toggle_vk) = get_hotkey(self.toggle);
            let bare = |vk: u32| vk != 0 && !(0x70..=0x87).contains(&vk);
            if (hang_vk != 0 && hang_mods == 0 && bare(hang_vk))
                || (toggle_vk != 0 && toggle_mods == 0 && bare(toggle_vk))
            {
                message(self.hang, t("settings_no_mods"));
                return false;
            }
            let pos = send(self.sag, TBM_GETPOS, 0, 0) as f32 / 10.0;
            // Saving the shortcuts must not silently replace the shipped curve
            // with the percentage that happens to match it on this monitor.
            let sag = if self.previewed { pos } else { self.initial_sag };
            let hover = send(self.hover, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED.0;
            let always = send(self.always, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED.0;
            let drag_hang = send(self.drag_hang, BM_GETCHECK, 0, 0) as u32 == BST_CHECKED.0;
            match (*self.app).apply_settings(
                (hang_mods, hang_vk),
                (toggle_mods, toggle_vk),
                sag,
                hover,
                always,
                drag_hang,
            ) {
                Ok(()) => {
                    self.saved = true;
                    true
                }
                Err(error) => {
                    message(self.hang, &error);
                    false
                }
            }
        }
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    unsafe {
        let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        if ptr.is_null() {
            return DefWindowProcW(hwnd, msg, wp, lp);
        }
        let state = &mut *ptr;
        match msg {
            WM_COMMAND => {
                let id = (wp.0 & 0xFFFF) as i32;
                let notification = ((wp.0 >> 16) & 0xFFFF) as u32;
                if notification == BN_CLICKED {
                    if id == SAVE_ID {
                        if state.save() {
                            let _ = DestroyWindow(hwnd);
                        }
                        return LRESULT(0);
                    }
                    if id == CANCEL_ID {
                        let _ = DestroyWindow(hwnd);
                        return LRESULT(0);
                    }
                }
                LRESULT(0)
            }
            WM_HSCROLL => {
                state.update_sag();
                LRESULT(0)
            }
            WM_CLOSE => {
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_DESTROY => {
                SETTINGS_HWND.store(0, Ordering::Relaxed);
                // A slider drag that never reached Save was only a preview.
                if state.previewed && !state.saved {
                    (*state.app).preview_sag(state.initial_sag);
                }
                (*state.app).settings_window_closed();
                if !state.font.is_invalid() {
                    let _ = DeleteObject(HGDIOBJ(state.font.0));
                }
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                drop(Box::from_raw(ptr));
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wp, lp),
        }
    }
}

/// `SendMessageW` in these bindings takes optional parameters; every message
/// this window sends has real ones.
unsafe fn send(hwnd: HWND, msg: u32, wp: usize, lp: isize) -> isize {
    unsafe { SendMessageW(hwnd, msg, Some(WPARAM(wp)), Some(LPARAM(lp))).0 }
}

fn make_font(scale: f32) -> HFONT {
    let face = if crate::i18n::is_chinese() {
        w!("Microsoft YaHei UI")
    } else {
        w!("Segoe UI")
    };
    unsafe {
        let font = CreateFontW(
            -(13.0 * scale).round() as i32,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            DEFAULT_PITCH.0 as u32,
            face,
        );
        if font.is_invalid() {
            HFONT(GetStockObject(DEFAULT_GUI_FONT).0)
        } else {
            font
        }
    }
}

fn set_hotkey(control: HWND, mods: u32, vk: u32) {
    let mut flags = 0u32;
    if mods & settings::MOD_CONTROL != 0 {
        flags |= HOTKEYF_CONTROL;
    }
    if mods & settings::MOD_ALT != 0 {
        flags |= HOTKEYF_ALT;
    }
    if mods & settings::MOD_SHIFT != 0 {
        flags |= HOTKEYF_SHIFT;
    }
    let value = ((vk & 0xFF) | ((flags & 0xFF) << 8)) as usize;
    unsafe {
        let _ = send(control, HKM_SETHOTKEY, value, 0);
    }
}

fn get_hotkey(control: HWND) -> (u32, u32) {
    let value = unsafe { send(control, HKM_GETHOTKEY, 0, 0) as u16 };
    let vk = (value & 0xFF) as u32;
    let flags = ((value >> 8) & 0xFF) as u32 & !HOTKEYF_EXT;
    let mut mods = 0;
    if flags & HOTKEYF_CONTROL != 0 {
        mods |= settings::MOD_CONTROL;
    }
    if flags & HOTKEYF_ALT != 0 {
        mods |= settings::MOD_ALT;
    }
    if flags & HOTKEYF_SHIFT != 0 {
        mods |= settings::MOD_SHIFT;
    }
    if vk == 0 {
        (0, 0)
    } else {
        (mods, vk)
    }
}

fn message(parent: HWND, text: &str) {
    let text = wstr(text);
    let title = wstr(t("settings_title"));
    unsafe {
        let _ = MessageBoxW(
            Some(parent),
            PCWSTR(text.as_ptr()),
            PCWSTR(title.as_ptr()),
            MB_OK | MB_ICONWARNING,
        );
    }
}

fn wstr(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}
