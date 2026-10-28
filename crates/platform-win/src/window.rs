//! Finding, describing and validating other applications' windows.
//!
//! The filter is deliberately conservative: if hanging a window could confuse
//! the user or break its owner, we do not offer it.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows::core::{BOOL, PWSTR};
use windows::Win32::Foundation::{CloseHandle, FILETIME, HWND, LPARAM, RECT};
use windows::Win32::Graphics::Dwm::{DWMWA_CLOAKED, DWMWA_EXTENDED_FRAME_BOUNDS, DwmGetWindowAttribute};
use windows::Win32::Graphics::Gdi::{MONITOR_DEFAULTTONULL, MonitorFromWindow};
use windows::Win32::Security::{GetTokenInformation, TOKEN_ELEVATION, TOKEN_QUERY, TokenElevation};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, GetProcessTimes, OpenProcess, OpenProcessToken, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GA_ROOT, GUI_INMOVESIZE, GUITHREADINFO, GW_HWNDNEXT, GWL_EXSTYLE, GA_PARENT, GetAncestor,
    GetClassNameW, GetForegroundWindow, GetGUIThreadInfo, GetShellWindow, GetWindow, GetWindowLongPtrW,
    GetWindowRect, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed,
    SW_RESTORE, SetForegroundWindow, ShowWindow, SwitchToThisWindow, WS_EX_APPWINDOW, WS_EX_TOOLWINDOW,
    WindowFromPoint,
};

use crate::Rect;

/// Classes that are never windows the user thinks of as theirs.
const IGNORED_CLASSES: &[&str] = &[
    "Progman",
    "WorkerW",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "DV2ControlHost",
    "SysShadow",
    "Windows.UI.Core.CoreWindow",
    "TaskManagerWindow",
];

fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|c| *c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

pub fn hwnd_of(raw: isize) -> HWND {
    HWND(raw as *mut core::ffi::c_void)
}

pub fn raw_of(hwnd: HWND) -> isize {
    hwnd.0 as isize
}

/// One window, described well enough to hang it, restore it and label it.
#[derive(Clone, Debug, PartialEq)]
pub struct WindowInfo {
    pub hwnd: isize,
    pub pid: u32,
    /// Window title as shown on the card.
    pub title: String,
    pub class: String,
    /// Lower-cased executable name, e.g. `chrome.exe`. This is the identity
    /// used for the accent colour and for grouping.
    pub exe: String,
    pub exe_path: String,
    pub rect: Rect,
    pub maximized: bool,
    /// Creation time of the owning process, in 100 ns units. Together with the
    /// pid this identifies the *process instance*, not just the pid, so a
    /// recycled pid after a restart cannot be mistaken for the parked window.
    pub process_start: u64,
}

impl WindowInfo {
    /// The line on the card under the title.
    pub fn app_name(&self) -> String {
        let stem = self.exe.strip_suffix(".exe").unwrap_or(&self.exe);
        let mut chars = stem.chars();
        match chars.next() {
            Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
            None => "Window".to_string(),
        }
    }
}

pub fn title(hwnd: HWND) -> String {
    let mut buf = [0u16; 512];
    let n = unsafe { GetWindowTextW(hwnd, &mut buf) };
    if n <= 0 { String::new() } else { from_wide(&buf[..n as usize]) }
}

pub fn class_name(hwnd: HWND) -> String {
    let mut buf = [0u16; 256];
    let n = unsafe { GetClassNameW(hwnd, &mut buf) };
    if n <= 0 { String::new() } else { from_wide(&buf[..n as usize]) }
}

pub fn pid_of(hwnd: HWND) -> u32 {
    let mut pid = 0u32;
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
    }
    pid
}

/// The window's process, or `None` when we are not allowed to look.
pub fn process_path(pid: u32) -> Option<String> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let handle = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(handle, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len).is_ok();
        let _ = CloseHandle(handle);
        if !ok || len == 0 {
            return None;
        }
        Some(String::from_utf16_lossy(&buf[..len as usize]))
    }
}

/// Creation time of a process. Zero when the process cannot be inspected.
pub fn process_start(pid: u32) -> u64 {
    if pid == 0 {
        return 0;
    }
    unsafe {
        let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else { return 0 };
        let mut creation = FILETIME::default();
        let mut exit = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        let ok = GetProcessTimes(handle, &mut creation, &mut exit, &mut kernel, &mut user).is_ok();
        let _ = CloseHandle(handle);
        if !ok {
            return 0;
        }
        ((creation.dwHighDateTime as u64) << 32) | creation.dwLowDateTime as u64
    }
}

/// Whether a process runs elevated. `None` when the token cannot be read.
pub fn process_elevated(pid: u32) -> Option<bool> {
    if pid == 0 {
        return None;
    }
    unsafe {
        let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut token = windows::Win32::Foundation::HANDLE::default();
        let opened = OpenProcessToken(process, TOKEN_QUERY, &mut token).is_ok();
        let _ = CloseHandle(process);
        if !opened {
            return None;
        }
        let mut elevation = TOKEN_ELEVATION::default();
        let mut returned = 0u32;
        let ok = GetTokenInformation(
            token,
            TokenElevation,
            Some(&mut elevation as *mut TOKEN_ELEVATION as *mut core::ffi::c_void),
            std::mem::size_of::<TOKEN_ELEVATION>() as u32,
            &mut returned,
        )
        .is_ok();
        let _ = CloseHandle(token);
        if ok { Some(elevation.TokenIsElevated != 0) } else { None }
    }
}

/// Whether *we* run elevated. Cached by the caller; this is a syscall.
pub fn self_elevated() -> bool {
    process_elevated(std::process::id()).unwrap_or(false)
}

/// UWP windows and windows on another virtual desktop are "cloaked": they
/// exist, but the user cannot see them, so they must never appear on the line.
pub fn is_cloaked(hwnd: HWND) -> bool {
    let mut cloaked = 0u32;
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CLOAKED,
            &mut cloaked as *mut u32 as *mut core::ffi::c_void,
            std::mem::size_of::<u32>() as u32,
        )
    };
    ok.is_ok() && cloaked != 0
}

/// The rectangle DWM actually draws, without the invisible resize border.
/// Falls back to `GetWindowRect` for windows DWM does not manage.
pub fn visible_rect(hwnd: HWND) -> Option<Rect> {
    unsafe {
        let mut frame = RECT::default();
        let got = DwmGetWindowAttribute(
            hwnd,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            &mut frame as *mut RECT as *mut core::ffi::c_void,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok();
        if got {
            let r = Rect { left: frame.left, top: frame.top, right: frame.right, bottom: frame.bottom };
            if !r.is_empty() {
                return Some(r);
            }
        }
        rect_of(hwnd)
    }
}

pub fn rect_of(hwnd: HWND) -> Option<Rect> {
    unsafe {
        let mut rect = RECT::default();
        GetWindowRect(hwnd, &mut rect).ok()?;
        Some(Rect { left: rect.left, top: rect.top, right: rect.right, bottom: rect.bottom })
    }
}

/// Why a window cannot be hung, or `None` when it can.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rejection {
    Gone,
    Ours,
    Hidden,
    NoTitle,
    ToolWindow,
    Cloaked,
    Minimized,
    /// UIPI: an unelevated process cannot move an elevated window.
    Elevated,
    TooSmall,
}

impl Rejection {
    pub fn message(self) -> &'static str {
        match self {
            Rejection::Gone => "That window has already closed.",
            Rejection::Ours => "ShaiWin cannot hang its own windows.",
            Rejection::Hidden => "That window is hidden.",
            Rejection::NoTitle => "That window has no title to show on a card.",
            Rejection::ToolWindow => "Tool windows and palettes cannot be hung.",
            Rejection::Cloaked => "That window is on another desktop or is not visible.",
            Rejection::Minimized => "Restore the window before hanging it up.",
            Rejection::Elevated => {
                "That window runs as administrator, and Windows blocks a normal app from moving it. \
                 Run ShaiWin as administrator to hang it."
            }
            Rejection::TooSmall => "That window is too small to make a useful card.",
        }
    }
}

pub fn check(hwnd: HWND, self_pid: u32) -> Result<WindowInfo, Rejection> {
    if !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return Err(Rejection::Gone);
    }
    let pid = pid_of(hwnd);
    if self_pid != 0 && pid == self_pid {
        return Err(Rejection::Ours);
    }
    if !unsafe { IsWindowVisible(hwnd).as_bool() } {
        return Err(Rejection::Hidden);
    }
    if is_cloaked(hwnd) {
        return Err(Rejection::Cloaked);
    }
    let ex = unsafe { GetWindowLongPtrW(hwnd, GWL_EXSTYLE) } as u32;
    let is_tool = ex & WS_EX_TOOLWINDOW.0 != 0;
    let is_app = ex & WS_EX_APPWINDOW.0 != 0;
    if is_tool && !is_app {
        return Err(Rejection::ToolWindow);
    }
    let class = class_name(hwnd);
    if IGNORED_CLASSES.iter().any(|c| c.eq_ignore_ascii_case(&class)) {
        return Err(Rejection::ToolWindow);
    }
    let title = title(hwnd);
    if title.trim().is_empty() {
        return Err(Rejection::NoTitle);
    }
    if unsafe { IsIconic(hwnd).as_bool() } {
        return Err(Rejection::Minimized);
    }
    let Some(rect) = visible_rect(hwnd) else { return Err(Rejection::Gone) };
    if rect.width() < 120 || rect.height() < 80 {
        return Err(Rejection::TooSmall);
    }
    if !self_elevated() && process_elevated(pid) == Some(true) {
        return Err(Rejection::Elevated);
    }
    let exe_path = process_path(pid).unwrap_or_default();
    let exe = std::path::Path::new(&exe_path)
        .file_name()
        .map(|f| f.to_string_lossy().to_lowercase())
        .unwrap_or_else(|| class.to_lowercase());
    Ok(WindowInfo {
        hwnd: raw_of(hwnd),
        pid,
        title,
        class,
        exe,
        exe_path,
        rect,
        maximized: unsafe { IsZoomed(hwnd).as_bool() },
        process_start: process_start(pid),
    })
}

/// Every window the user could reasonably want to hang, front to back.
pub fn list_hangable(self_pid: u32) -> Vec<WindowInfo> {
    let mut found: Vec<WindowInfo> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(collect), LPARAM(&mut found as *mut Vec<WindowInfo> as isize));
    }
    found.retain(|w| w.pid != self_pid);
    found
}

unsafe extern "system" fn collect(hwnd: HWND, lparam: LPARAM) -> BOOL {
    let list = unsafe { &mut *(lparam.0 as *mut Vec<WindowInfo>) };
    if let Ok(info) = check(hwnd, 0) {
        list.push(info);
    }
    true.into()
}

pub fn foreground() -> Option<HWND> {
    let hwnd = unsafe { GetForegroundWindow() };
    if hwnd.0.is_null() { None } else { Some(hwnd) }
}

/// The top-level window under a screen point, which is what the user thinks
/// they are dragging.
pub fn root_at(x: i32, y: i32) -> Option<HWND> {
    let hwnd = unsafe { WindowFromPoint(windows::Win32::Foundation::POINT { x, y }) };
    if hwnd.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(hwnd, GA_ROOT) };
    if root.0.is_null() { Some(hwnd) } else { Some(root) }
}

/// The thread that owns a window.
pub fn thread_of(hwnd: HWND) -> u32 {
    unsafe { GetWindowThreadProcessId(hwnd, None) }
}

/// The window `thread` is moving or sizing right now, if any.
///
/// A title-bar drag runs in the target's UI thread as the system's move loop,
/// and Windows reports exactly that through the thread's GUI state. Guessing
/// instead from the point under the pointer is unreliable for the whole
/// gesture: a Chromium drag image, a tooltip, the snap preview, or our own strip
/// can be the window the point hit-tests to, while the window the user is moving
/// is the one the move loop names.
///
/// The thread must be named by the caller: `GetGUIThreadInfo(0)` follows the
/// foreground window, and the dragged window is not always the foreground one
/// for the whole gesture (nor on every desktop).
pub fn moving_on(thread: u32) -> Option<HWND> {
    let mut info = GUITHREADINFO {
        cbSize: std::mem::size_of::<GUITHREADINFO>() as u32,
        ..Default::default()
    };
    unsafe {
        GetGUIThreadInfo(thread, &mut info).ok()?;
    }
    if !info.flags.contains(GUI_INMOVESIZE) || info.hwndMoveSize.0.is_null() {
        return None;
    }
    let root = unsafe { GetAncestor(info.hwndMoveSize, GA_ROOT) };
    Some(if root.0.is_null() { info.hwndMoveSize } else { root })
}

/// Restores a parked window to the foreground. Windows only lets the foreground
/// process steal focus, so we attach to whatever is in front for the call.
pub fn force_foreground(hwnd: HWND) {
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        let fg = GetForegroundWindow();
        let our_thread = GetCurrentThreadId();
        let fg_thread = if fg.0.is_null() { 0 } else { GetWindowThreadProcessId(fg, None) };
        let attached = fg_thread != 0
            && fg_thread != our_thread
            && AttachThreadInput(our_thread, fg_thread, true).as_bool();
        let _ = SetForegroundWindow(hwnd);
        if attached {
            let _ = AttachThreadInput(our_thread, fg_thread, false);
        }
    }
}

/// Hands the foreground over to another window before `avoid` disappears.
///
/// Right after a window is parked it is still the foreground window — it only
/// moved off screen. The taskbar reads a click on the button of the window that
/// is *already* in front as "minimize it", so the click does nothing the user can
/// see, and the window stays invisible on the line. Giving the foreground away
/// first turns that same click into a real activation, which the shell notices
/// and answers by bringing the window back.
///
/// Doing it needs a window that will take the focus: the next one down the
/// z-order that looks like something the user could have been using. Returns
/// `false` when there is none, so the caller can decide what happens to the
/// window that is still holding the focus.
pub fn hand_off_foreground(avoid: HWND, self_pid: u32) -> bool {
    unsafe {
        if GetForegroundWindow() != avoid {
            return false;
        }
        let mut candidate = next_window(avoid);
        for _ in 0..64 {
            if candidate.0.is_null() {
                return false;
            }
            if is_foreground_material(candidate, avoid, self_pid) {
                force_foreground(candidate);
                if GetForegroundWindow() == candidate {
                    return true;
                }
                // SetForegroundWindow is refused when this process did not
                // receive the last input event, which is exactly the case here:
                // the user clicked somebody else's window. The shell's own
                // switching call is not subject to that rule.
                SwitchToThisWindow(candidate, false);
                if GetForegroundWindow() == candidate {
                    return true;
                }
            }
            candidate = next_window(candidate);
        }
        false
    }
}

/// Whether a foreground change away from `previous` is the shell's own doing,
/// not the user asking for the window that took the focus.
///
/// Windows moves the foreground to the next window in the z-order whenever the
/// window holding it is minimized, closed or hidden. A parked window is still
/// visible - it is only off screen - so it can be that next window. Treating
/// its arrival as a request to see it is what opened a random card whenever
/// another window was minimized.
pub fn focus_handed_over(previous: isize, self_pid: u32) -> bool {
    if previous == 0 || !is_alive(previous) {
        return true;
    }
    let h = hwnd_of(previous);
    if self_pid != 0 && pid_of(h) == self_pid {
        return true;
    }
    minimized(h)
}

/// The window just below this one in the z-order. Windows reports "no next
/// window" as a failure, which for a walk like this just means the end.
fn next_window(hwnd: HWND) -> HWND {
    unsafe { GetWindow(hwnd, GW_HWNDNEXT).unwrap_or_default() }
}

/// Gives the foreground to the desktop.
///
/// The fallback when the focus lands on a window the user cannot see and there
/// is no other window to hand it to. The desktop is always there, so the focus
/// never has to stay on something invisible, and the next click on a taskbar
/// button is a real activation again.
pub fn hand_to_shell() -> bool {
    unsafe {
        let shell = GetShellWindow();
        if shell.0.is_null() {
            return false;
        }
        force_foreground(shell);
        GetForegroundWindow() == shell
    }
}

fn is_foreground_material(hwnd: HWND, avoid: HWND, self_pid: u32) -> bool {
    if hwnd == avoid || !unsafe { IsWindow(Some(hwnd)).as_bool() } {
        return false;
    }
    if self_pid != 0 && pid_of(hwnd) == self_pid {
        return false;
    }
    if !unsafe { IsWindowVisible(hwnd).as_bool() } || unsafe { IsIconic(hwnd).as_bool() } {
        return false;
    }
    // A parked window is visible and titled; it is only off screen. It can take
    // the focus, but handing it over only moves the problem to another window
    // the user cannot see.
    if !on_screen(hwnd) {
        return false;
    }
    if is_cloaked(hwnd) {
        return false;
    }
    let class = class_name(hwnd);
    if IGNORED_CLASSES.iter().any(|c| c.eq_ignore_ascii_case(&class)) {
        return false;
    }
    !title(hwnd).trim().is_empty()
}

pub fn is_alive(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { IsWindow(Some(hwnd_of(hwnd))).as_bool() }
}

/// Whether the window is minimized, not merely hidden.
pub fn minimized(hwnd: HWND) -> bool {
    unsafe { IsIconic(hwnd).as_bool() }
}

/// Whether the window is the system's own maximized state. Releasing a title-bar
/// drag at the top of the screen puts a window here, which is the honest answer
/// to "did the user mean to enlarge it".
pub fn maximized(hwnd: isize) -> bool {
    hwnd != 0 && unsafe { IsZoomed(hwnd_of(hwnd)).as_bool() }
}

/// Whether the window intersects any monitor at all. A window we parked lives
/// past the right edge of the virtual desktop, so this is false for it. If a
/// parked window's rectangle changes but it is still off screen, it must be
/// brought back before the card goes away.
pub fn on_screen(hwnd: HWND) -> bool {
    !unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONULL) }.0.is_null()
}

/// True when the window still belongs to the process instance we recorded.
/// This is what keeps a recycled HWND (or pid) from being "restored" wrongly
/// after a reboot.
pub fn still_same_window(hwnd: isize, pid: u32, process_start: u64) -> bool {
    if !is_alive(hwnd) {
        return false;
    }
    let h = hwnd_of(hwnd);
    if pid_of(h) != pid {
        return false;
    }
    if process_start == 0 {
        return true;
    }
    let now = self::process_start(pid);
    now == 0 || now == process_start
}

/// True when the window is not a child of another window (i.e. it is a real
/// top-level window, which is all we can park).
pub fn is_top_level(hwnd: isize) -> bool {
    if !is_alive(hwnd) {
        return false;
    }
    let parent = unsafe { GetAncestor(hwnd_of(hwnd), GA_PARENT) };
    parent.0.is_null() || parent == hwnd_of(hwnd)
}

/// The executable name for a window, lower-cased, or an empty string.
pub fn exe_of(hwnd: isize) -> String {
    let h = hwnd_of(hwnd);
    let pid = pid_of(h);
    process_path(pid)
        .and_then(|p| std::path::Path::new(&p).file_name().map(|f| f.to_string_lossy().to_lowercase()))
        .unwrap_or_default()
}

/// A short description for logs.
pub fn describe(hwnd: isize) -> String {
    let h = hwnd_of(hwnd);
    format!("{} [pid {}]", title(h), pid_of(h))
}

#[allow(dead_code)]
fn _wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(std::iter::once(0)).collect()
}
