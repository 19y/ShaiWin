//! ShaiWin: hang your windows on a clothesline above the desktop.
//!
//! The Windows entry point. It does four things: makes sure only one copy runs,
//! asks Windows for per-monitor DPI awareness before any window exists, creates
//! the overlay, and runs the message loop.

#![windows_subsystem = "windows"]

mod app;
mod i18n;
mod ico;
mod journal;
mod line;
mod log;
mod render;
mod settings;
mod settings_ui;
mod tag_ui;
mod text;
mod tray;
mod worker;

use windows::core::w;
use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::UI::HiDpi::{
    SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DispatchMessageW, GetMessageW, TranslateMessage, MSG, WM_APP,
};

/// Posted to the window when a background thread has something for the UI.
pub const WM_EVENT: u32 = WM_APP + 1;

fn main() {
    unsafe {
        // Only one line per user session. A second launch just exits.
        let _mutex = CreateMutexW(None, true, w!("Local\\ShaiWin.SingleInstance"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            return;
        }
        // Before the first window: crisp cards on every monitor, at its own DPI.
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // The shell's tray and menu code wants a single-threaded apartment.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }

    log::init();

    let Some(raw) = app::App::create() else {
        crate::log!("could not create the overlay window");
        return;
    };

    let mut msg = MSG::default();
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).0 > 0 {
            // The settings window uses standard controls; the dialog manager
            // gives them Tab order, Enter and Escape.
            if settings_ui::is_dialog_message(&msg) {
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        let mut app = Box::from_raw(raw);
        app.shutdown();
    }
}
